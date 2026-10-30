//! Real bounded fixture IO -> original selected owner -> registered encrypted account store.
use super::{model::*, native::NativeContext, native_source::*, store};
use crate::{
    app_store::isolated_scope::{capture_selection_at, SelectionSnapshot},
    secrets::{session::SecretSession, storage::StorageIdentity, VaultContext},
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
struct Fixture {
    root: tempfile::TempDir,
    session: Arc<SecretSession>,
    selection: SelectionSnapshot,
    control: PathBuf,
    auth: PathBuf,
    context: NativeContext,
}
impl Fixture {
    fn new() -> Self {
        let root = crate::secrets::testing::tempdir().unwrap();
        let data = root.path().join("data");
        let device = root.path().join("device");
        let c = root.path().join("control");
        let source = root.path().join("observed-source");
        for p in [&data, &device, &c, &source] {
            std::fs::create_dir(p).unwrap();
        }
        let control = c.join("app_paths.json");
        let selection = capture_selection_at(&control, &data, &device).unwrap();
        let session = SecretSession::from_context(data, VaultContext::generate().unwrap());
        let auth = source.join("workbuddy-desktop.info");
        let f = Self {
            root,
            session,
            selection,
            control,
            auth,
            context: NativeContext {
                product: "workbuddy".into(),
                region: "cn".into(),
                realm: "www.workbuddy.cn".into(),
                source: "fixture-observation-only-not-authentication".into(),
            },
        };
        f.write_auth("synthetic-image-one");
        f
    }
    fn write_auth(&self, image: &str) {
        std::fs::write(&self.auth,serde_json::to_vec(&json!({"account":{"uid":"synthetic-observed","type":"personal"},"auth":{"domain":"www.workbuddy.cn","accessToken":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":image}}},"accounts":[],"allAccounts":[]})).unwrap()).unwrap();
    }
    fn binding(&self) -> StorageIdentity {
        store::binding(&self.session).unwrap()
    }
    fn catalog<'a>(&'a self, b: &'a StorageIdentity) -> Catalog<'a> {
        Catalog {
            session: &self.session,
            identity: b,
            epoch: self.session.lifecycle_epoch(),
            selection: &self.selection,
        }
    }
    fn files(&self) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &std::path::Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
            for e in std::fs::read_dir(root).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out)
                } else {
                    out.insert(p.clone(), std::fs::read(p).unwrap());
                }
            }
        }
        let mut out = Default::default();
        walk(self.session.root(), &mut out);
        out
    }
}
#[tokio::test]
async fn observed_source_persists_and_reopens_only_as_original_owner_data() {
    let f = Fixture::new();
    let b = f.binding();
    let observed = Observation::read(&f.auth, &f.context).expect("bounded observation");
    let original = std::fs::read(&f.auth).unwrap();
    let receipt = f
        .catalog(&b)
        .save(&observed, None, "Synthetic observed")
        .await
        .expect("production observer must reach original owner/store");
    assert_eq!(receipt.generation, 0);
    assert_eq!(receipt.realm, "www.workbuddy.cn");
    let row = f.catalog(&b).load(&receipt).await.unwrap();
    assert_eq!(row.account.uid, "synthetic-observed");
    assert!(!row.account.is_api());
    assert!(row.account.api_token().is_err());
    assert!(!row.automatic_enabled());
    assert!(!row.inference_allowed());
    let wire = serde_json::to_value(&row).unwrap();
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(wire["account"]["native"]["snapshot"].as_str().unwrap())
            .unwrap(),
        original
    );
    assert_eq!(std::fs::read(&f.auth).unwrap(), original);
    assert!(
        !String::from_utf8_lossy(&std::fs::read(f.session.root().join(store::FILE)).unwrap())
            .contains("synthetic-image-one")
    );
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
    assert!(!f.control.exists());
}
#[tokio::test]
async fn recapture_uses_original_generation_and_rejects_duplicate_or_stale_receipts() {
    let f = Fixture::new();
    let b = f.binding();
    let c = f.catalog(&b);
    let first = Observation::read(&f.auth, &f.context).unwrap();
    let a = c.save(&first, None, "First").await.unwrap();
    let before = f.files();
    assert!(c.save(&first, None, "Duplicate").await.is_err());
    assert_eq!(f.files(), before);
    f.write_auth("synthetic-image-two");
    let next = Observation::read(&f.auth, &f.context).unwrap();
    let second = c.save(&next, Some(&a), "Second").await.unwrap();
    assert_eq!(second.id, a.id);
    assert_eq!(second.generation, 1);
    assert!(c.load(&a).await.is_err());
    let before = f.files();
    assert!(c.save(&next, Some(&a), "Stale").await.is_err());
    assert_eq!(f.files(), before);
    let rows = store::load(&f.session, &b).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account.label, "Second");
    store::save(&f.session, &b, vec![]).unwrap();
    assert!(c.load(&second).await.is_err());
}
#[tokio::test]
async fn observation_change_or_logout_never_writes_catalog() {
    for case in ["auth", "marker", "parent"] {
        let f = Fixture::new();
        let b = f.binding();
        let obs = Observation::read(&f.auth, &f.context).unwrap();
        match case {
            "auth" => f.write_auth("changed"),
            "marker" => std::fs::write(
                f.auth.with_file_name("workbuddy-desktop.info.logged-out"),
                b"logout",
            )
            .unwrap(),
            _ => {
                let p = f.auth.parent().unwrap();
                std::fs::rename(p, p.with_file_name("old-source")).unwrap();
                std::fs::create_dir(p).unwrap();
                f.write_auth("synthetic-image-one");
            }
        };
        let before = f.files();
        assert!(f.catalog(&b).save(&obs, None, "Changed").await.is_err());
        assert_eq!(f.files(), before);
    }
    let f = Fixture::new();
    std::fs::write(
        f.auth.with_file_name("workbuddy-desktop.info.logged-out"),
        b"logout",
    )
    .unwrap();
    let before = f.files();
    assert!(Observation::read(&f.auth, &f.context).is_err());
    assert_eq!(f.files(), before);
}
#[tokio::test]
async fn owner_epoch_selector_context_and_wrong_key_refuse_without_write() {
    for case in ["epoch", "blocked", "selector", "key", "context"] {
        let f = Fixture::new();
        let b = f.binding();
        let obs = Observation::read(&f.auth, &f.context).unwrap();
        let mut c = f.catalog(&b);
        let foreign = Fixture::new();
        let foreign_b = foreign.binding();
        match case {
            "epoch" => f.session.invalidate_database_leases(),
            "blocked" => f.session.set_blocked(true),
            "selector" => {
                std::fs::write(&f.control, b"corrupt selection").unwrap();
            }
            "key" => c.identity = &foreign_b,
            _ => {
                let mut bad = f.context.clone();
                bad.realm = "www.codebuddy.cn".into();
                assert!(Observation::read(&f.auth, &bad).is_err());
                continue;
            }
        };
        let before = f.files();
        assert!(c.save(&obs, None, "Denied").await.is_err());
        assert_eq!(f.files(), before);
    }
}
#[tokio::test]
async fn native_storage_binding_does_not_merge_or_lock_out_legacy_api_duplicates() {
    let f = Fixture::new();
    let b = f.binding();
    let make = |label: &str| {
        SavedAccount::new(Account {
            uid: "synthetic-observed".into(),
            domain: Domain::WorkBuddy,
            label: label.into(),
            credential: "synthetic-api-secret".into(),
        })
    };
    store::save(&f.session, &b, vec![make("First"), make("Second")]).unwrap();
    let old = serde_json::to_value(store::load(&f.session, &b).unwrap()).unwrap();
    let before = f.files();
    let obs = Observation::read(&f.auth, &f.context).unwrap();
    assert!(f
        .catalog(&b)
        .save(&obs, None, "Untrusted association")
        .await
        .is_err());
    assert_eq!(f.files(), before);
    assert_eq!(
        serde_json::to_value(store::load(&f.session, &b).unwrap()).unwrap(),
        old
    );
}
#[tokio::test]
async fn native_and_lifecycle_recovery_intents_block_catalog_without_cleanup() {
    use super::native_owner;
    for kind in [
        "native-pending",
        "native-corrupt",
        "transition",
        "reset",
        "bootstrap",
        "rewrap",
    ] {
        let f = Fixture::new();
        let b = f.binding();
        let obs = Observation::read(&f.auth, &f.context).unwrap();
        let name = match kind {
            "transition" => crate::secrets::transition::INTENT,
            "reset" => "unused-reset-name",
            "bootstrap" => ".vault-bootstrap",
            "rewrap" => ".vault-rewrap",
            _ => crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE,
        };
        let bytes = if kind == "native-pending" {
            let v = f.session.read().unwrap();
            native_owner::testing::phase_receipts(f.session.root(), &v, f.auth.parent().unwrap()).0
        } else {
            b"synthetic-unreadable-intent".to_vec()
        };
        let path = if kind == "reset" {
            f.root.path().join(".data-vault-reset")
        } else {
            f.session.root().join(name)
        };
        std::fs::write(&path, &bytes).unwrap();
        let before = f.files();
        assert!(
            f.catalog(&b).save(&obs, None, "Blocked").await.is_err(),
            "accepted {kind}"
        );
        assert_eq!(f.files(), before);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn waiting_for_original_owner_rechecks_source_epoch_selector_and_phase() {
    use std::future::Future;
    for case in ["source", "epoch", "selector", "native", "bootstrap"] {
        let f = Fixture::new();
        let b = f.binding();
        let obs = Observation::read(&f.auth, &f.context).unwrap();
        let catalog = f.catalog(&b);
        let guard = crate::services::sync_protocol::sync_mutex().lock().await;
        let mut operation = std::pin::pin!(catalog.save(&obs, None, "Queued"));
        let waker = std::task::Waker::noop();
        assert!(operation
            .as_mut()
            .poll(&mut std::task::Context::from_waker(waker))
            .is_pending());
        match case {
            "source" => f.write_auth("changed-while-waiting"),
            "epoch" => f.session.invalidate_database_leases(),
            "selector" => std::fs::write(&f.control, b"invalid-selector").unwrap(),
            "native" => {
                let v = f.session.read().unwrap();
                let pending = super::native_owner::testing::phase_receipts(
                    f.session.root(),
                    &v,
                    f.auth.parent().unwrap(),
                )
                .0;
                std::fs::write(
                    f.session
                        .root()
                        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE),
                    pending,
                )
                .unwrap();
            }
            _ => std::fs::write(
                f.session.root().join(".vault-bootstrap"),
                b"pending-bootstrap",
            )
            .unwrap(),
        }
        let before = f.files();
        drop(guard);
        assert!(operation.await.is_err(), "accepted queued {case}");
        assert_eq!(f.files(), before);
    }
}

#[tokio::test]
async fn original_owner_rechecks_recovery_at_account_commit_boundary() {
    let f = Fixture::new();
    let b = f.binding();
    let obs = Observation::read(&f.auth, &f.context).unwrap();
    let receipt = f.catalog(&b).save(&obs, None, "Stored").await.unwrap();
    let before = std::fs::read(f.session.root().join(store::FILE)).unwrap();
    let result = super::native_owner::with_selected_owner(
        &f.session,
        &b,
        f.session.lifecycle_epoch(),
        &f.selection,
        |owner| {
            let rows = owner.load_accounts()?;
            owner.save_accounts(rows, || {
                std::fs::write(f.session.root().join(".vault-rewrap"), b"competing-rewrap")
                    .unwrap();
                Ok(())
            })
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(f.session.root().join(store::FILE)).unwrap(),
        before
    );
    assert!(f.catalog(&b).load(&receipt).await.is_err());
    assert_eq!(
        std::fs::read(f.session.root().join(".vault-rewrap")).unwrap(),
        b"competing-rewrap"
    );
}

#[tokio::test]
async fn original_native_clear_receipts_and_vault_reopen_preserve_stored_data() {
    let f = Fixture::new();
    let b = f.binding();
    let obs = Observation::read(&f.auth, &f.context).unwrap();
    let c = f.catalog(&b);
    let receipt = c.save(&obs, None, "Saved").await.unwrap();
    let old = serde_json::to_value(c.load(&receipt).await.unwrap()).unwrap();
    let (pending, restored, committed) = {
        let v = f.session.read().unwrap();
        super::native_owner::testing::phase_receipts(f.session.root(), &v, f.auth.parent().unwrap())
    };
    let path = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    std::fs::write(&path, &pending).unwrap();
    let before = f.files();
    assert!(c.load(&receipt).await.is_err());
    assert_eq!(f.files(), before);
    for bytes in [restored, committed] {
        std::fs::write(&path, &bytes).unwrap();
        let before = f.files();
        assert_eq!(
            serde_json::to_value(c.load(&receipt).await.unwrap()).unwrap(),
            old
        );
        assert_eq!(f.files(), before);
    }
    // New session from the same original key/context, not a raw plaintext or API-token fallback.
    let storage = {
        let v = f.session.read().unwrap();
        match &*v {
            crate::secrets::storage::StorageContext::Encrypted(v) => {
                crate::secrets::VaultContext::from_key(v.metadata().clone(), v.export_key())
                    .unwrap()
            }
            _ => unreachable!(),
        }
    };
    let reopened = SecretSession::from_context(f.session.root().into(), storage);
    let identity = store::binding(&reopened).unwrap();
    let catalog = Catalog {
        session: &reopened,
        identity: &identity,
        epoch: reopened.lifecycle_epoch(),
        selection: &f.selection,
    };
    assert_eq!(
        serde_json::to_value(catalog.load(&receipt).await.unwrap()).unwrap(),
        old
    );
}

#[tokio::test]
async fn native_context_change_and_generation_overflow_refuse_without_write() {
    let f = Fixture::new();
    let b = f.binding();
    let obs = Observation::read(&f.auth, &f.context).unwrap();
    let c = f.catalog(&b);
    let receipt = c.save(&obs, None, "Stored").await.unwrap();
    let mut other = f.context.clone();
    other.source = "other-observation-scope-not-authentication".into();
    let changed = Observation::read(&f.auth, &other).unwrap();
    let before = f.files();
    assert!(c
        .save(&changed, Some(&receipt), "Wrong scope")
        .await
        .is_err());
    assert_eq!(f.files(), before);
    let mut rows = store::load(&f.session, &b).unwrap();
    rows[0].generation = u64::MAX;
    store::save(&f.session, &b, rows).unwrap();
    let before = f.files();
    let mut overflow_receipt = receipt.clone();
    overflow_receipt.generation = u64::MAX;
    assert!(c
        .save(&obs, Some(&overflow_receipt), "Overflow")
        .await
        .is_err());
    assert_eq!(f.files(), before);
}

#[tokio::test]
async fn observed_uid_never_attaches_to_api_or_hybrid_even_with_matching_generation() {
    for hybrid in [false, true] {
        let f = Fixture::new();
        let b = f.binding();
        let c = f.catalog(&b);
        let obs = Observation::read(&f.auth, &f.context).unwrap();
        let receipt = c.save(&obs, None, "Original opaque data").await.unwrap();
        let mut value = serde_json::to_value(store::load(&f.session, &b).unwrap()).unwrap();
        value[0]["account"]["token"] = json!("synthetic-api-secret");
        if hybrid {
            value[0]["account"]["credential_version"] = json!(1);
        } else {
            value[0]["account"]
                .as_object_mut()
                .unwrap()
                .remove("native");
        }
        let rows = serde_json::from_value(value).unwrap();
        store::save(&f.session, &b, rows).unwrap();
        let before = f.files();
        assert!(c
            .save(&obs, Some(&receipt), "Unproven binding")
            .await
            .is_err());
        assert_eq!(f.files(), before);
        assert!(c.load(&receipt).await.is_err());
    }
}

#[tokio::test]
async fn old_data_receipt_cannot_read_a_recreated_identity_with_different_image() {
    let f = Fixture::new();
    let b = f.binding();
    let c = f.catalog(&b);
    let one = Observation::read(&f.auth, &f.context).unwrap();
    let old = c.save(&one, None, "Old data").await.unwrap();
    store::save(&f.session, &b, vec![]).unwrap();
    f.write_auth("different-recreated-image");
    let next = Observation::read(&f.auth, &f.context).unwrap();
    let fresh = c.save(&next, None, "New data").await.unwrap();
    assert_eq!(fresh.id, old.id);
    let before = f.files();
    assert!(
        c.load(&old).await.is_err(),
        "a generation number alone must not select different stored bytes after deletion"
    );
    assert_eq!(f.files(), before);
    assert!(c.save(&next, Some(&old), "Stale overwrite").await.is_err());
    assert_eq!(f.files(), before);
}

#[tokio::test]
async fn same_image_recreation_rejects_old_receipt_read() {
    let f = Fixture::new();
    let b = f.binding();
    let c = f.catalog(&b);
    let observed = Observation::read(&f.auth, &f.context).unwrap();
    let old = c.save(&observed, None, "Original lifetime").await.unwrap();
    let original_auth = std::fs::read(&f.auth).unwrap();
    store::save(&f.session, &b, vec![]).unwrap();
    // Keep the exact same file, bytes, realm/context and selector/session epoch.
    let fresh_observation = Observation::read(&f.auth, &f.context).unwrap();
    let fresh = c
        .save(&fresh_observation, None, "Recreated lifetime")
        .await
        .unwrap();
    assert_eq!(old.id, fresh.id);
    assert_eq!(old.generation, fresh.generation);
    assert_eq!(std::fs::read(&f.auth).unwrap(), original_auth);
    let before = f.files();
    assert!(
        c.load(&old).await.is_err(),
        "receipt from deleted lifetime must not read an identical recreated account"
    );
    assert_eq!(f.files(), before);
    let row = c
        .load(&fresh)
        .await
        .expect("new lifetime receipt remains usable");
    assert_eq!(row.account.label, "Recreated lifetime");
    let wire = serde_json::to_value(&row).unwrap();
    assert_eq!(wire["account"]["native"]["version"], 2);
    assert!(wire["account"]["native"]["incarnation"].is_string());
}

#[tokio::test]
async fn same_image_recreation_rejects_old_receipt_replacement() {
    let f = Fixture::new();
    let b = f.binding();
    let c = f.catalog(&b);
    let observed = Observation::read(&f.auth, &f.context).unwrap();
    let old = c.save(&observed, None, "Original lifetime").await.unwrap();
    store::save(&f.session, &b, vec![]).unwrap();
    let fresh_observation = Observation::read(&f.auth, &f.context).unwrap();
    let fresh = c
        .save(&fresh_observation, None, "Recreated lifetime")
        .await
        .unwrap();
    assert_eq!(old.id, fresh.id);
    assert_eq!(old.generation, fresh.generation);
    let before = f.files();
    assert!(
        c.save(&fresh_observation, Some(&old), "Stale overwrite")
            .await
            .is_err(),
        "receipt from deleted lifetime must not replace an identical recreated account"
    );
    assert_eq!(f.files(), before);
    let before_wire = serde_json::to_value(c.load(&fresh).await.unwrap()).unwrap();
    let next = c
        .save(&fresh_observation, Some(&fresh), "Current lifetime update")
        .await
        .expect("new lifetime receipt may recapture");
    assert_eq!(next.generation, fresh.generation + 1);
    let updated = serde_json::to_value(c.load(&next).await.unwrap()).unwrap();
    assert_eq!(
        updated["account"]["native"]["incarnation"],
        before_wire["account"]["native"]["incarnation"]
    );
    assert_eq!(updated["account"]["label"], "Current lifetime update");
    assert!(c.load(&old).await.is_err());
}

#[tokio::test]
async fn native_v2_recapture_preserves_lifetime_and_reopen() {
    let f = Fixture::new();
    let b = f.binding();
    let c = f.catalog(&b);
    let obs = Observation::read(&f.auth, &f.context).unwrap();
    let first = c.save(&obs, None, "First").await.unwrap();
    let row = c.load(&first).await.unwrap();
    let wire = serde_json::to_value(&row).unwrap();
    assert_eq!(wire["account"]["native"]["version"], 2);
    let incarnation = wire["account"]["native"]["incarnation"]
        .as_str()
        .expect("native v2 lifecycle required")
        .to_owned();
    let id = uuid::Uuid::parse_str(&incarnation).unwrap();
    assert_eq!(id.get_version_num(), 4);
    assert_eq!(id.hyphenated().to_string(), incarnation);
    f.write_auth("recaptured-image");
    let again = Observation::read(&f.auth, &f.context).unwrap();
    let second = c.save(&again, Some(&first), "Second").await.unwrap();
    assert_eq!(second.generation, first.generation + 1);
    let row = c.load(&second).await.unwrap();
    let next = serde_json::to_value(&row).unwrap();
    assert_eq!(next["account"]["native"]["incarnation"], incarnation);
    let reopened = {
        let v = f.session.read().unwrap();
        match &*v {
            crate::secrets::storage::StorageContext::Encrypted(v) => {
                crate::secrets::VaultContext::from_key(v.metadata().clone(), v.export_key())
                    .unwrap()
            }
            _ => unreachable!(),
        }
    };
    let session = SecretSession::from_context(f.session.root().into(), reopened);
    let binding = store::binding(&session).unwrap();
    let rc = Catalog {
        session: &session,
        identity: &binding,
        epoch: session.lifecycle_epoch(),
        selection: &f.selection,
    };
    assert_eq!(
        serde_json::to_value(rc.load(&second).await.unwrap()).unwrap(),
        next
    );
    assert!(c.load(&first).await.is_err());
}

#[tokio::test]
async fn native_v2_rejects_invalid_lifetime_without_overwriting_storage() {
    for case in [
        "missing",
        "null",
        "upper",
        "compact",
        "v1uuid",
        "variant",
        "unknown-version",
        "v1-mixed",
    ] {
        let f = Fixture::new();
        let b = f.binding();
        let c = f.catalog(&b);
        let obs = Observation::read(&f.auth, &f.context).unwrap();
        let receipt = c.save(&obs, None, "Original").await.unwrap();
        let row = c.load(&receipt).await.unwrap();
        let mut value = serde_json::to_value(&row).unwrap();
        let native = &mut value["account"]["native"];
        native["version"] = json!(2);
        native["incarnation"] = json!("11111111-1111-4111-8111-111111111111");
        match case {
            "missing" => {
                native.as_object_mut().unwrap().remove("incarnation");
            }
            "null" => native["incarnation"] = serde_json::Value::Null,
            "upper" => native["incarnation"] = json!("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"),
            "compact" => native["incarnation"] = json!("11111111111141118111111111111111"),
            "v1uuid" => native["incarnation"] = json!("11111111-1111-1111-8111-111111111111"),
            "variant" => native["incarnation"] = json!("11111111-1111-4111-7111-111111111111"),
            "unknown-version" => native["version"] = json!(99),
            _ => native["version"] = json!(1),
        };
        let owned = crate::secrets::owned_file::OwnedFile::registered(store::FILE).unwrap();
        let raw = {
            let v = f.session.read().unwrap();
            owned
                .encode_storage(
                    &v,
                    &serde_json::to_vec(&json!({"accounts":[value]})).unwrap(),
                )
                .unwrap()
        };
        std::fs::write(f.session.root().join(store::FILE), &raw).unwrap();
        let before = f.files();
        assert!(store::load(&f.session, &b).is_err(), "accepted {case}");
        assert!(store::save(&f.session, &b, vec![]).is_err());
        assert!(c.load(&receipt).await.is_err());
        assert_eq!(f.files(), before);
    }
}

#[tokio::test]
async fn native_v2_legacy_v1_stays_lossless_but_never_issues_new_receipt() {
    let f = Fixture::new();
    let b = f.binding();
    let c = f.catalog(&b);
    let obs = Observation::read(&f.auth, &f.context).unwrap();
    let receipt = c.save(&obs, None, "Before legacy restore").await.unwrap();
    let mut value = serde_json::to_value(c.load(&receipt).await.unwrap()).unwrap();
    value["account"]["native"]["version"] = json!(1);
    value["account"]["native"]
        .as_object_mut()
        .unwrap()
        .remove("incarnation");
    let row: SavedAccount = serde_json::from_value(value.clone()).unwrap();
    store::save(&f.session, &b, vec![row]).unwrap();
    let before = f.files();
    assert!(c.load(&receipt).await.is_err());
    assert!(c.save(&obs, Some(&receipt), "No upgrade").await.is_err());
    assert!(c.save(&obs, None, "No UID merge").await.is_err());
    assert_eq!(f.files(), before);
    let loaded = store::load(&f.session, &b).unwrap();
    assert_eq!(serde_json::to_value(&loaded[0]).unwrap(), value);
    store::save(&f.session, &b, loaded).unwrap();
    assert_eq!(
        serde_json::to_value(&store::load(&f.session, &b).unwrap()[0]).unwrap(),
        value
    );
    store::save(&f.session, &b, vec![]).unwrap();
    assert!(store::load(&f.session, &b).unwrap().is_empty());
}

#[test]
fn native_v2_preserves_legacy_v1_hybrid_and_api_fingerprints() {
    let mut native = super::native_account_tests::native_row_fixture();
    native
        .account
        .replace_api_preserving_native(Account {
            uid: native.account.uid.clone(),
            domain: native.account.domain,
            label: "API".into(),
            credential: "synthetic-api-secret".into(),
        })
        .unwrap();
    assert_eq!(
        hex::encode(native.account.http_fingerprint().unwrap()),
        "5a916fae0daab5c39b06b51d7848233917f2e2c238e2edd604522a5e31b1d1b0"
    );
    let api = Account {
        uid: "legacy".into(),
        domain: Domain::WorkBuddy,
        label: "API".into(),
        credential: "synthetic-api-secret".into(),
    };
    assert_eq!(
        hex::encode(api.http_fingerprint().unwrap()),
        "89b4c7f3bf2b8cc4c9ddb7d67c4ca16c20594aa98b37ddbe078917fde1649eb4"
    );
}
