//! Actual generation transaction exercised against private synthetic mode staging.
use super::*;
use crate::live::engine::{lock_app, DeviceStore, LiveFile};
use crate::live::patch::WholeFile;
use crate::mode::{operation, state};
use crate::secrets::testing::{initialize_database, MemoryKeyStore, TestHome};

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn pending_encrypted_stash_blocks_new_generation_before_mutation() {
    let _home = TestHome::new().unwrap();
    crate::settings::reload_settings().unwrap();
    let db = initialize_database().unwrap();
    db.secret_session().complete_migration().unwrap();
    let store = DeviceStore::for_device();
    let stash = DeviceFile::registered("codex-login-stash.json").unwrap();
    let config = crate::codex_config::get_codex_config_path();
    let staged;
    {
        let vault = db.secret_session().read().unwrap();
        let guard = lock_app("codex");
        let ciphertext = stash
            .encode(&vault, br#"{"logins":{"synthetic":"credential"}}"#)
            .unwrap();
        let config_patch = WholeFile::Write(b"model = \"synthetic\"\n".to_vec());
        let stash_patch = WholeFile::Write(ciphertext);
        operation::failpoint::crash_at(Some("published:0"));
        let result = operation::run(
            &store,
            &vault,
            &guard,
            state::op::APPLY,
            &[
                operation::FileChange {
                    file: LiveFile::private(&config),
                    patch: &config_patch,
                },
                operation::FileChange {
                    file: LiveFile::private(store.path_for(&stash)),
                    patch: &stash_patch,
                },
            ],
            state::PendingTarget::default(),
            &|_| Ok(()),
        );
        operation::failpoint::crash_at(None);
        assert!(result.is_err());
        let pending = state::pending(&store, &vault, "codex").unwrap().unwrap();
        assert!(pending.published);
        staged = pending.files[1].staged.clone().unwrap();
        assert!(stash
            .decode(&vault, &std::fs::read(&staged).unwrap())
            .is_ok());
    }
    let before = db.secret_session().read().unwrap().metadata().clone();
    let stage_bytes = std::fs::read(&staged).unwrap();
    let state_bytes = std::fs::read(store.state_path()).unwrap();
    let result = rotate(
        &db,
        &MemoryKeyStore::default(),
        "synthetic recovery password",
        false,
    );
    assert!(
        matches!(result, Err(AppError::Config(ref code)) if code == "mode.verification_required"),
        "pending encrypted staging must block a new key generation: {result:?}"
    );
    assert_eq!(db.secret_session().read().unwrap().metadata(), &before);
    assert_eq!(std::fs::read(&staged).unwrap(), stage_bytes);
    assert_eq!(std::fs::read(store.state_path()).unwrap(), state_bytes);
    assert!(!db.secret_session().root().join(INTENT).exists());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    pending_encrypted_stash_blocks_new_generation_before_mutation();
    println!("PASS pending encrypted Codex staging blocks new vault generation");
    completed_device_mode_rotates_with_separate_data_root();
    println!("PASS completed mode rotates with separate data/device roots");
    authenticated_started_vault_transition_with_mode_pending_still_recovers();
    println!("PASS existing authenticated vault transition still recovers with mode pending");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn completed_device_mode_rotates_with_separate_data_root() {
    let home = TestHome::new().unwrap();
    crate::settings::reload_settings().unwrap();
    let keys = MemoryKeyStore::default();
    let data = home.path().join("separate-data");
    let session = crate::secrets::session::SecretSession::open(&data, &keys, None).unwrap();
    crate::settings::unlock_settings_for_test(session.clone()).unwrap();
    let db = Database::init_with_secrets(session).unwrap();
    db.secret_session().complete_migration().unwrap();
    let store = DeviceStore::for_device();
    assert_ne!(store.root(), data);
    {
        let vault = db.secret_session().read().unwrap();
        state::update(&store, &vault, |live| {
            live.apps.entry("codex".into()).or_default().mode = Some(state::Mode::Direct);
            Ok(())
        })
        .unwrap();
    }
    rotate(&db, &keys, "synthetic rotation password", false).unwrap();
    assert!(
        state::pending(&store, &db.secret_session().read().unwrap(), "codex")
            .unwrap()
            .is_none()
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn authenticated_started_vault_transition_with_mode_pending_still_recovers() {
    let _home = TestHome::new().unwrap();
    crate::settings::reload_settings().unwrap();
    let db = initialize_database().unwrap();
    db.secret_session().complete_migration().unwrap();
    let store = DeviceStore::for_device();
    let keys = MemoryKeyStore::default();
    let old_key = db
        .secret_session()
        .read()
        .unwrap()
        .metadata()
        .key_id
        .clone();
    // Model a generation transaction captured by the older owner, before the
    // new-installation gate existed. Its authenticated inventory includes the
    // pending state; resuming that exact transition must remain supported.
    let result = install_with_hook(
        &db,
        &keys,
        |current| {
            let mut live = state::LiveState::default();
            live.apps.entry("codex".into()).or_default().pending = Some(state::Pending {
                op: state::op::APPLY.into(),
                files: vec![],
                target: Default::default(),
                published: true,
                extra: Default::default(),
            });
            let file = DeviceFile::registered(crate::secrets::owned_file::DEVICE_STATE_FILE)?;
            crate::config_file_io::ensure_private_directory(store.root())?;
            write_durable(
                &store.path_for(&file),
                &file.encode(current, &serde_json::to_vec(&live).unwrap())?,
            )?;
            current
                .rotate_key()
                .and_then(|next| next.with_password("synthetic recovery password"))
                .map_err(inventory::secret_error)
        },
        false,
        |source, current, next| {
            let mut memory = Connection::open_in_memory().map_err(db_error)?;
            vault::copy(source, &mut memory)?;
            inventory::transform_database(&memory, Some(current), next)?;
            vault::stamp(&memory, next)?;
            Ok(memory)
        },
        Replacements {
            skills: None,
            settings: None,
        },
        &mut |point| {
            if point == Checkpoint::Intent {
                Err(invalid())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert!(db.secret_session().root().join(INTENT).exists());
    let root = db.secret_session().root().to_path_buf();
    drop(db); // A restarted process no longer owns the old WAL connection.
    let reopened = crate::secrets::session::SecretSession::open_existing(
        &root,
        &keys,
        Some("synthetic recovery password"),
    )
    .unwrap();
    let vault = reopened.read().unwrap();
    assert_ne!(vault.metadata().key_id, old_key);
    assert!(state::pending(&store, &vault, "codex").unwrap().is_some());
    assert!(!root.join(INTENT).exists());
}
