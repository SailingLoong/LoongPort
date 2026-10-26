//! Test-hook-only original native and vendor capabilities captured before real restore.
use super::*;
use crate::{
    app_config::AppType,
    minimax_code::{
        catalog::Region,
        inference::{FixtureTransport, Lease},
    },
    provider::{Provider, ProviderMeta},
    services::{
        service_source_identity::{MiniMaxStamp, NativeStamp},
        vendor_response_fence::{Expected, Fence},
    },
    vendor::{self, Vendor},
};

const NATIVE_ID: &str = "synthetic-real-restore-native";
const ACCOUNT: &str = "synthetic-real-restore-workspace";
const ROW: i64 = 799;

pub(crate) struct RestoreProofs {
    db: Arc<Database>,
    native: MiniMaxStamp,
    provider: Provider,
    expected: Expected,
    fence: Fence,
    key_and_revision: (String, String),
    native_file: Vec<u8>,
    storage: crate::secrets::storage::StorageIdentity,
}

fn native_stamp(db: &Database) -> MiniMaxStamp {
    let storage = db.secrets.read().unwrap();
    let guard = NativeReadGuard::acquire(&db.secrets, &storage, true, false, false).unwrap();
    let fact = guard.account("minimax_code", NATIVE_ID).unwrap();
    let NativeStamp::MiniMax(stamp) = fact.stamp else {
        unreachable!()
    };
    stamp
}
fn key_and_revision(db: &Database) -> (String, String) {
    db.conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT api_key,api_key_revision FROM loongport_vendor WHERE id=?1",
            [ROW],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

pub(crate) async fn seed_restore_proofs(db: Arc<Database>) -> RestoreProofs {
    FixtureTransport::seed_accounts(db.secret_session(), &[(NATIVE_ID, Region::Cn)])
        .await
        .unwrap();
    let source = vendor::provision::provider_id_for("opencode", ACCOUNT);
    {
        let storage = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(?1,'opencode',?2)",
            rusqlite::params![ROW, ACCOUNT],
        )
        .unwrap();
        vendor::creds::set_api_key(&conn, &storage, ROW, "SYNTHETIC_RESTORE_ONLY_KEY").unwrap();
    }
    let settings = vendor::provision::plan_rows_for(Vendor::OpenCode, ACCOUNT, "")
        .into_iter()
        .find(|bundle| bundle.plan.id_segment == "opencode")
        .unwrap()
        .rows
        .into_iter()
        .find(|(app, _)| *app == AppType::Claude)
        .unwrap()
        .1;
    let mut origin = Provider::with_id(
        source.clone(),
        "Synthetic restore owner".into(),
        settings,
        None,
    );
    origin.meta = Some(ProviderMeta {
        loongport_vendor_account: Some(ACCOUNT.into()),
        ..Default::default()
    });
    db.save_provider("claude", &origin).unwrap();
    crate::services::vendor_source_binding::bind_vendor_source(&db, &source, &AppType::Codex)
        .unwrap();
    let provider = db.get_provider_by_id(&source, "codex").unwrap().unwrap();
    let expected = {
        let storage = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        Expected::capture_on(
            &conn,
            &storage,
            db.secrets.lifecycle_epoch(),
            &AppType::Codex,
            &provider,
        )
        .unwrap()
        .unwrap()
    };
    let fence = Fence::acquire_with_expected(db.clone(), AppType::Codex, &provider, &expected)
        .unwrap()
        .unwrap();
    fence.check().unwrap();
    let native = native_stamp(&db);
    Lease::acquire(db.secrets.clone(), NATIVE_ID, "synthetic-before-restore")
        .await
        .unwrap()
        .verify_routing_stamp(&native)
        .unwrap();
    let key_and_revision = key_and_revision(&db);
    let native_file =
        std::fs::read(db.secrets.root().join(crate::minimax_code::catalog::FILE)).unwrap();
    let storage = db.secrets.read().unwrap().identity();
    RestoreProofs {
        db,
        native,
        provider,
        expected,
        fence,
        key_and_revision,
        native_file,
        storage,
    }
}
impl RestoreProofs {
    pub(crate) async fn assert_current(&self) {
        Fence::acquire_with_expected(
            self.db.clone(),
            AppType::Codex,
            &self.provider,
            &self.expected,
        )
        .unwrap()
        .unwrap()
        .check()
        .unwrap();
        self.fence.check().unwrap();
        Lease::acquire(
            self.db.secrets.clone(),
            NATIVE_ID,
            "synthetic-current-proof",
        )
        .await
        .unwrap()
        .verify_routing_stamp(&self.native)
        .unwrap();
    }
    pub(crate) async fn assert_retired(&self) {
        assert!(
            Fence::acquire_with_expected(
                self.db.clone(),
                AppType::Codex,
                &self.provider,
                &self.expected
            )
            .is_err(),
            "old vendor request proof must not revive after real restore"
        );
        assert!(
            self.fence.check().is_err(),
            "old vendor response fence must not revive after real restore"
        );
        if let Ok(lease) = Lease::acquire(
            self.db.secrets.clone(),
            NATIVE_ID,
            "synthetic-after-restore",
        )
        .await
        {
            assert!(
                lease.verify_routing_stamp(&self.native).is_err(),
                "old native selection proof must not revive after real restore"
            );
        }
    }
    pub(crate) fn assert_same_credentials(&self, db: &Database) {
        assert!(
            key_and_revision(db) == self.key_and_revision,
            "real restore preserves identical original synthetic key and revision"
        );
        assert!(
            std::fs::read(db.secrets.root().join(crate::minimax_code::catalog::FILE)).unwrap()
                == self.native_file,
            "SQL/content restore must not rewrite the native account owner"
        );
        assert!(
            db.secrets.read().unwrap().identity() == self.storage,
            "restore must retain exact storage identity"
        );
    }
    pub(crate) async fn assert_fresh(&self, db: Arc<Database>) {
        self.assert_same_credentials(&db);
        let provider = db
            .get_provider_by_id(&self.provider.id, "codex")
            .unwrap()
            .unwrap();
        let expected = {
            let storage = db.secrets.read().unwrap();
            let conn = db.conn.lock().unwrap();
            Expected::capture_on(
                &conn,
                &storage,
                db.secrets.lifecycle_epoch(),
                &AppType::Codex,
                &provider,
            )
            .unwrap()
            .unwrap()
        };
        Fence::acquire_with_expected(db.clone(), AppType::Codex, &provider, &expected)
            .unwrap()
            .unwrap()
            .check()
            .unwrap();
        let stamp = native_stamp(&db);
        Lease::acquire(db.secrets.clone(), NATIVE_ID, "synthetic-new-restore-proof")
            .await
            .unwrap()
            .verify_routing_stamp(&stamp)
            .unwrap();
    }
}
