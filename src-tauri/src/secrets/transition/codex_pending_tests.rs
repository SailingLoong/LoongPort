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
    nested_home_does_not_reuse_failed_settings_owner();
    println!("PASS isolated homes restore prior settings owner on success and failure");
    malformed_or_unknown_mode_blocks_new_generation_before_mutation();
    println!("PASS malformed and unknown mode still block new vault generation");
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
    crate::settings::unlock_settings_for_test(reopened.clone()).unwrap();
    let vault = reopened.read().unwrap();
    assert_ne!(vault.metadata().key_id, old_key);
    assert!(state::pending(&store, &vault, "codex").unwrap().is_some());
    assert!(!root.join(INTENT).exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn nested_home_does_not_reuse_failed_settings_owner() {
    for blocked in [false, true] {
        let outer = TestHome::new().unwrap();
        let db = initialize_database().unwrap();
        if blocked {
            db.secret_session().set_blocked(true);
        } else {
            std::fs::write(
                crate::settings::settings_path(),
                b"invalid settings fixture",
            )
            .unwrap();
        }
        assert!(crate::settings::reload_settings().is_err());
        // Failed construction must restore both the environment and exact owner.
        let bad_home = crate::secrets::testing::tempdir().unwrap();
        let bad_root = bad_home.path().join(crate::APP_DIR_NAME);
        std::fs::create_dir_all(&bad_root).unwrap();
        std::fs::write(bad_root.join("settings.json"), b"invalid settings fixture").unwrap();
        let selected = std::env::var_os("CC_SWITCH_TEST_HOME");
        assert!(TestHome::from_directory(bad_home).is_err());
        assert_eq!(std::env::var_os("CC_SWITCH_TEST_HOME"), selected);
        assert!(crate::settings::reload_settings().is_err());
        {
            let inner = TestHome::new().unwrap();
            assert_ne!(inner.path(), outer.path());
            // A selected new fixture must bootstrap its own path, not reload a
            // still-existing corrupt file or blocked session from the outer home.
            crate::settings::reload_settings()
                .expect("new home must not reuse prior failed settings owner");
            let fresh = initialize_database().unwrap();
            assert!(fresh.secret_session().read().is_ok());
        }
        assert!(
            crate::settings::reload_settings().is_err(),
            "leaving nested fixture preserves the outer owner's actual failure"
        );
        if blocked {
            db.secret_session().set_blocked(false);
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn malformed_or_unknown_mode_blocks_new_generation_before_mutation() {
    for plaintext in [
        b"invalid mode fixture".as_slice(),
        br#"{"version":1,"unexpected":true}"#.as_slice(),
    ] {
        let _home = TestHome::new().unwrap();
        let db = initialize_database().unwrap();
        db.secret_session().complete_migration().unwrap();
        let store = DeviceStore::for_device();
        let file = DeviceFile::registered(crate::secrets::owned_file::DEVICE_STATE_FILE).unwrap();
        let vault = db.secret_session().read().unwrap();
        let before = vault.metadata().clone();
        let ciphertext = file.encode(&vault, plaintext).unwrap();
        write_durable(&store.state_path(), &ciphertext).unwrap();
        drop(vault);
        let result = rotate(
            &db,
            &MemoryKeyStore::default(),
            "synthetic recovery password",
            false,
        );
        assert!(
            matches!(result, Err(AppError::Config(ref code)) if code == "mode.verification_required")
        );
        assert_eq!(db.secret_session().read().unwrap().metadata(), &before);
        assert_eq!(std::fs::read(store.state_path()).unwrap(), ciphertext);
        assert!(!db.secret_session().root().join(INTENT).exists());
    }
}
