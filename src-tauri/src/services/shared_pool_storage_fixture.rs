//! Synthetic original storage owners and isolated restore consumers; no real account data.
use super::*;
use crate::{
    database::Database,
    secrets::{storage::PlaintextIdentity, testing::TestHome},
};
use std::sync::Arc;

pub(crate) fn fresh_plaintext_database() -> Database {
    let home = std::path::PathBuf::from(
        std::env::var_os("CC_SWITCH_TEST_HOME").expect("isolated synthetic home is required"),
    );
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    assert!(home.starts_with(&temporary) && home != temporary);
    assert_eq!(home.canonicalize().unwrap(), home);
    let root = crate::config::get_app_config_dir();
    assert!(root.starts_with(&home));
    for name in [
        crate::config::DB_FILE_NAME,
        "vault.json",
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    ] {
        assert!(
            !root.join(name).exists(),
            "fixture requires a fresh original owner"
        );
    }
    let context = StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    });
    let session = SecretSession::from_storage_context(root, context);
    let db = Database::init_with_secrets(session).unwrap();
    Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap()).unwrap();
    db
}

fn identity_fences() {
    let _home = TestHome::new().unwrap();
    let encrypted = crate::secrets::VaultContext::generate().unwrap();
    let metadata = encrypted.metadata();
    let plaintext = PlaintextIdentity {
        format_version: 1,
        store_id: metadata.vault_id.clone(),
        generation_id: metadata.key_id.clone(),
        revision: metadata.revision,
    };
    let storage = StorageContext::Plaintext(plaintext.clone());
    let root = crate::config::get_app_config_dir();
    let session = SecretSession::from_storage_context(root.clone(), storage.clone());
    let guard = NativeReadGuard::acquire(&session, &storage, false, false, false).unwrap();
    guard.lock.check_context(&session, &storage).unwrap();
    assert!(guard.lock.check_context(&session, &StorageContext::Encrypted(encrypted)).is_err(),
        "same store/generation/revision strings do not make plaintext and encrypted modes interchangeable");
    let mut generation = plaintext.clone();
    generation.generation_id = uuid::Uuid::new_v4().to_string();
    assert!(guard
        .lock
        .check_context(&session, &StorageContext::Plaintext(generation))
        .is_err());
    let mut revision = plaintext;
    revision.revision += 1;
    assert!(guard
        .lock
        .check_context(&session, &StorageContext::Plaintext(revision))
        .is_err());
    let other_session = SecretSession::from_storage_context(root, storage.clone());
    assert!(guard.lock.check_context(&other_session, &storage).is_err());
    session.set_blocked(true);
    session.set_blocked(false);
    assert!(
        guard.check().is_err(),
        "unblock cannot revive a captured native proof"
    );
}

fn vendor_credential_retirement(case: &str) {
    use crate::{
        app_config::AppType,
        provider::{Provider, ProviderMeta},
        services::vendor_response_fence::{Expected, Fence},
        vendor::{self, Vendor},
    };
    let _home = TestHome::new().unwrap();
    let db = Arc::new(fresh_plaintext_database());
    let account = "synthetic-plaintext-opencode-workspace";
    let key = "SYNTHETIC_PLAINTEXT_OPENCODE_KEY";
    let source = vendor::provision::provider_id_for("opencode", account);
    {
        let storage = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(701,'opencode',?1)",
            [account],
        )
        .unwrap();
        vendor::creds::set_api_key(&conn, &storage, 701, key).unwrap();
    }
    let bundle = vendor::provision::plan_rows_for(Vendor::OpenCode, account, "")
        .into_iter()
        .find(|b| b.plan.id_segment == "opencode")
        .unwrap();
    let settings = bundle
        .rows
        .into_iter()
        .find(|(app, _)| *app == AppType::Claude)
        .unwrap()
        .1;
    let mut origin = Provider::with_id(
        source.clone(),
        "Synthetic original owner".into(),
        settings,
        None,
    );
    origin.meta = Some(ProviderMeta {
        loongport_vendor_account: Some(account.into()),
        ..Default::default()
    });
    db.save_provider("claude", &origin).unwrap();
    crate::services::vendor_source_binding::bind_vendor_source(&db, &source, &AppType::Codex)
        .unwrap();
    let provider = db.get_provider_by_id(&source, "codex").unwrap().unwrap();
    let old = {
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
    let fence = Fence::acquire_with_expected(db.clone(), AppType::Codex, &provider, &old)
        .unwrap()
        .unwrap();
    if case.ends_with("_aba") {
        {
            let storage = db.secrets.read().unwrap();
            vendor::creds::set_api_key(&db.conn.lock().unwrap(), &storage, 701, "").unwrap();
        }
        assert!(Fence::acquire_with_expected(db.clone(), AppType::Codex, &provider, &old).is_err());
        assert!(fence.check().is_err());
    }
    {
        let storage = db.secrets.read().unwrap();
        vendor::creds::set_api_key(&db.conn.lock().unwrap(), &storage, 701, key).unwrap();
    }
    assert!(Fence::acquire(db.clone(), AppType::Codex, &provider)
        .unwrap()
        .is_some());
    if case.starts_with("opencode_response_") {
        assert!(
            fence.check().is_err(),
            "plaintext original vendor key reissue must retire final-response proof"
        );
    } else {
        assert!(
            Fence::acquire_with_expected(db, AppType::Codex, &provider, &old).is_err(),
            "plaintext original vendor key reissue must retire captured request proof"
        );
    }
}

#[path = "shared_pool_restore_proofs.rs"]
mod restore_proofs;
pub(crate) use restore_proofs::seed_restore_proofs;

pub(crate) async fn verify(case: &str) {
    if case == "restore_stream_control" {
        return crate::proxy::opencode_shared_fixture::verify("personal_stream_same_owner_control")
            .await;
    }
    if case == "restore_sql_stream" {
        return crate::proxy::opencode_shared_fixture::verify("personal_stream_sql_restore").await;
    }
    if let Some(point) = case.strip_prefix("restore_joint_") {
        return crate::secrets::transition::content_tests::verify_pool_restore(point).await;
    }
    if let Some(case) = case.strip_prefix("revision_") {
        return crate::database::credential_revision::verify_fixture(case);
    }
    if let Some(case) = case.strip_prefix("journal_") {
        let case = case.to_owned();
        return tokio::task::spawn_blocking(move || {
            crate::mode::controller::verify_credential_write_fixture(&case)
        })
        .await
        .unwrap();
    }
    if let Some(case) = case.strip_prefix("openrouter_") {
        return crate::services::openrouter_source::verify_fixture(&format!("plaintext_{case}"));
    }
    if case.starts_with("opencode_") {
        return vendor_credential_retirement(case);
    }
    match case {
        "identity" => return identity_fences(),
        "openrouter" => {
            return crate::proxy::openrouter_shared_fixture::verify("plaintext_two_apps").await
        }
        #[cfg(unix)]
        "zcode" => return crate::proxy::zcode_shared_fixture::verify("plaintext_two_apps").await,
        "minimax" | "workbuddy" => {}
        _ => panic!("unknown shared-storage fixture: {case}"),
    }
    let _home = TestHome::new().unwrap();
    crate::settings::reload_settings().unwrap();
    let db = Arc::new(fresh_plaintext_database());
    crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
    {
        let storage = db.secrets.read().unwrap();
        for app in ["claude", "codex"] {
            crate::mode::state::update_app(
                &crate::live::engine::DeviceStore::for_device(),
                &storage,
                app,
                |entry| {
                    entry.mode = Some(crate::mode::state::Mode::Direct);
                    Ok(())
                },
            )
            .unwrap();
        }
    }
    let state = crate::store::AppState::new(db).unwrap();
    match case {
        "minimax" => crate::minimax_code::shared_flow_fixture::verify(&state, "ordered").await,
        "workbuddy" => crate::workbuddy::shared_routing_fixture::verify(&state, "ordered").await,
        _ => unreachable!(),
    }
}
