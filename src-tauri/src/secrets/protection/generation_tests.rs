use super::*;
use crate::{
    app_store::isolated_scope,
    config_file_io,
    secrets::{
        protection, session,
        testing::{MemoryKeyStore, TestHome},
        VaultContext,
    },
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
const KEY: &str = " 代际 🔑\r\n aBc ";
const CANARY: &str = "synthetic-native-generation-secret";
struct Fixture {
    _home: TestHome,
    root: PathBuf,
    store: PathBuf,
    db: crate::Database,
    cache: MemoryKeyStore,
}
impl Fixture {
    fn new(encrypted: bool) -> Self {
        let home = TestHome::new().unwrap();
        let root = crate::config::get_app_config_dir();
        let cache = MemoryKeyStore::default();
        if encrypted {
            config_file_io::ensure_private_directory(&root).unwrap();
            let vault = VaultContext::create_generation_with_user_text(
                uuid::Uuid::new_v4().to_string(),
                1,
                KEY,
            )
            .unwrap();
            session::write_metadata(&root, &session::completed_metadata(&vault, true).unwrap())
                .unwrap();
            let record = ProtectionRecord::encrypted(&vault, None).unwrap();
            config_file_io::write_durable(
                &root.join(protection::FILE),
                &serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
            crate::secrets::key_store::save_user_text_verified(&cache, &vault, KEY).unwrap();
        } else {
            protection::FreshScope::capture(&root, &root, &[])
                .unwrap()
                .publish_initial_plaintext()
                .unwrap();
        }
        let session = session::SecretSession::open_protected(&root, &root, &cache, None).unwrap();
        let db = crate::Database::init_with_secrets(session).unwrap();
        {
            let context = db.secret_session().read().unwrap();
            let conn = db.conn.lock().unwrap();
            let value = crate::secrets::inventory::seal_db(
                &context,
                "providers",
                "settings_config",
                &["generation", "codex"],
                CANARY,
            )
            .unwrap();
            let meta = crate::secrets::inventory::seal_db(
                &context,
                "providers",
                "meta",
                &["generation", "codex"],
                "{}",
            )
            .unwrap();
            conn.execute("INSERT INTO providers(id,app_type,name,settings_config,meta) VALUES('generation','codex','Synthetic',?1,?2)",rusqlite::params![value,meta]).unwrap();
            conn.execute_batch("CREATE TABLE future_generation_data(id INTEGER PRIMARY KEY, value BLOB);INSERT INTO future_generation_data VALUES(1,X'00FF12');").unwrap();
            let owned =
                crate::secrets::files::OwnedFile::registered(Path::new("workbuddy_accounts.json"))
                    .unwrap();
            config_file_io::write_durable(
                &root.join("workbuddy_accounts.json"),
                &owned.encode_storage(&context, CANARY.as_bytes()).unwrap(),
            )
            .unwrap();
        }
        config_file_io::write_durable(
            &root.join("unknown-user-file.bin"),
            b"untouched unknown user bytes",
        )
        .unwrap();
        let store = crate::config::get_home_dir().join("control/app_paths.json");
        config_file_io::ensure_private_directory(store.parent().unwrap()).unwrap();
        config_file_io::write_durable(&store, b"{\"futureField\":18446744073709551615}").unwrap();
        Self {
            _home: home,
            root,
            store,
            db,
            cache,
        }
    }
    fn selection(&self) -> SelectionSnapshot {
        isolated_scope::capture_selection_at(&self.store, &self.root, &self.root).unwrap()
    }
}
fn result_root(outcome: PublicationOutcome) -> PathBuf {
    match outcome {
        PublicationOutcome::RestartRequired(scope) => scope.root().to_path_buf(),
        other => panic!("expected completed publication: {other:?}"),
    }
}
fn verify_target(root: &Path, cache: &dyn KeyStore, plain: bool) {
    let session = session::SecretSession::open_protected(root, root, cache, None).unwrap();
    let context = session.read().unwrap();
    assert_eq!(context.is_plaintext(), plain);
    let db = crate::database::inspection::capture(&root.join(crate::config::DB_FILE_NAME))
        .unwrap()
        .unwrap();
    let raw: String = db
        .image
        .query_row(
            "SELECT settings_config FROM providers WHERE id='generation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        crate::secrets::inventory::open_db(
            &context,
            "providers",
            "settings_config",
            &["generation", "codex"],
            &raw
        )
        .unwrap(),
        CANARY
    );
    let blob: Vec<u8> = db
        .image
        .query_row("SELECT value FROM future_generation_data", [], |r| r.get(0))
        .unwrap();
    assert_eq!(blob, [0, 255, 18]);
    let owned =
        crate::secrets::files::OwnedFile::registered(Path::new("workbuddy_accounts.json")).unwrap();
    assert_eq!(
        &*owned
            .decode_storage(
                &context,
                &std::fs::read(root.join("workbuddy_accounts.json")).unwrap()
            )
            .unwrap(),
        CANARY.as_bytes()
    );
    assert_eq!(
        std::fs::read(root.join("unknown-user-file.bin")).unwrap(),
        b"untouched unknown user bytes"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_plain_to_text_publishes_verified_successor_and_retires_source() {
    let f = Fixture::new(false);
    let root = result_root(
        convert_at(
            &f.db,
            &f.root,
            f.selection(),
            GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
            &f.cache,
        )
        .unwrap(),
    );
    assert!(f.db.secret_session().read().is_err());
    assert!(f.db.conn.lock().is_err());
    verify_target(&root, &f.cache, false);
    assert!(!f.root.join(crate::config::DB_FILE_NAME).exists());
    assert!(!f.root.join("workbuddy_accounts.json").exists());
    assert_eq!(
        std::fs::read(f.root.join("unknown-user-file.bin")).unwrap(),
        b"untouched unknown user bytes"
    );
    assert!(
        !String::from_utf8_lossy(&std::fs::read(root.join("vault.json")).unwrap()).contains(KEY)
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_text_to_plain_preserves_old_encrypted_files_and_cache() {
    let f = Fixture::new(true);
    let old_metadata = std::fs::read(f.root.join("vault.json")).unwrap();
    let old_owned = std::fs::read(f.root.join("workbuddy_accounts.json")).unwrap();
    let saved = session::read_metadata(&f.root).unwrap();
    let old_cache = f
        .cache
        .load(&saved.metadata.vault_id, &saved.metadata.key_id)
        .unwrap()
        .unwrap();
    let root = result_root(
        convert_at(
            &f.db,
            &f.root,
            f.selection(),
            GenerationMode::Plaintext,
            &f.cache,
        )
        .unwrap(),
    );
    verify_target(&root, &f.cache, true);
    assert_eq!(
        std::fs::read(f.root.join("vault.json")).unwrap(),
        old_metadata
    );
    assert_eq!(
        std::fs::read(f.root.join("workbuddy_accounts.json")).unwrap(),
        old_owned
    );
    assert_eq!(
        f.cache
            .load(&saved.metadata.vault_id, &saved.metadata.key_id)
            .unwrap()
            .unwrap(),
        old_cache
    );
    assert!(!root.join("vault.json").exists());
}
struct Denied(AtomicUsize);
impl KeyStore for Denied {
    fn load(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, crate::secrets::key_store::KeyStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(crate::secrets::key_store::KeyStoreError::Unavailable)
    }
    fn save(
        &self,
        _: &str,
        _: &str,
        _: &[u8],
    ) -> Result<(), crate::secrets::key_store::KeyStoreError> {
        Err(crate::secrets::key_store::KeyStoreError::Unavailable)
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), crate::secrets::key_store::KeyStoreError> {
        panic!("no old cache deletion")
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_new_key_cache_failure_never_publishes_or_retires_old_runtime() {
    let f = Fixture::new(false);
    let control = std::fs::read(&f.store).unwrap();
    let owned = std::fs::read(f.root.join("workbuddy_accounts.json")).unwrap();
    let cache = Denied(AtomicUsize::new(0));
    assert!(convert_at(
        &f.db,
        &f.root,
        f.selection(),
        GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
        &cache
    )
    .is_err());
    assert!(cache.0.load(Ordering::SeqCst) > 0);
    assert_eq!(std::fs::read(&f.store).unwrap(), control);
    assert_eq!(
        std::fs::read(f.root.join("workbuddy_accounts.json")).unwrap(),
        owned
    );
    assert!(f.db.secret_session().read().is_ok());
    assert!(f.db.conn.lock().is_ok());
}
#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "plain_to_text" => native_plain_to_text_publishes_verified_successor_and_retires_source(),
        "text_to_plain" => native_text_to_plain_preserves_old_encrypted_files_and_cache(),
        "cache_failure" => native_new_key_cache_failure_never_publishes_or_retires_old_runtime(),
        "large_record" => large_authenticated_pending_record_reaches_original_inspection(),
        "before_selector" => retirement_before_selector_never_reopens_old_runtime(),
        "selected_cleanup" => {
            selected_cleanup_requires_same_authenticated_key_and_durable_completion()
        }
        "partial_cleanup" => partial_cleanup_resumes_same_target_not_a_third_configuration(),
        "cleanup_source_changed" => cleanup_refuses_changed_source_without_more_deletion(),
        "startup_cleanup" => selected_cleanup_restarts_through_original_startup_and_scope_paths(),
        "pending_cache" => pending_cache_failure_reuses_authenticated_candidate(),
        "pending_changes" => pending_full_inventory_changes_are_not_refreshed_into_authority(),
        "empty_input" => empty_input_never_reserves_a_pending_generation(),
        "dead_process_resume" => native_dead_process_before_selector_resumes_same_pending(),
        "runtime_admission" => runtime_admission_requires_actual_durable_choice_without_os_access(),
        "cleanup_recreated" => cleanup_recreated_owned_file_never_claims_complete(),
        "cleanup_new_owned" => cleanup_new_registered_owned_file_never_claims_complete(),
        "cleanup_new_backup" => cleanup_new_owned_database_backup_never_claims_complete(),
        "separate_dead_process" => {
            native_separate_dead_process_resumes_same_opaque_preserving_candidate()
        }
        _ => panic!("unknown native generation case"),
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn large_authenticated_pending_record_reaches_original_inspection() {
    let f = Fixture::new(true);
    let context = f.db.secret_session().read().unwrap();
    let vault = context.encrypted().unwrap();
    let record = ProtectionRecord::encrypted(vault, Some(&"x".repeat(18000))).unwrap();
    let bytes = serde_json::to_vec(&record).unwrap();
    assert!(bytes.len() > 16 * 1024);
    io::write_durable(&f.root.join(protection::FILE), &bytes).unwrap();
    assert!(
        crate::secrets::upgrade::inspect(&f.root, &crate::live::engine::DeviceStore::at(&f.root))
            .is_ok(),
        "passive startup must use original protection record bound, not a shadow16KiB limit"
    );
    assert!(
        session::SecretSession::open_protected(&f.root, &f.root, &f.cache, None)
            .err()
            .unwrap()
            .to_string()
            .contains("protection.cleanup_required")
    );
}

fn interrupted(f: &Fixture, boundary: GenerationBoundary) -> PathBuf {
    let before = std::fs::read(&f.store).unwrap();
    let result = convert_with_hook(
        &f.db,
        &f.root,
        f.selection(),
        GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
        &f.cache,
        &mut |at, _| {
            if at == boundary {
                Err(AppError::Config("test.synthetic_generation_stop".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    assert!(matches!(result, PublicationOutcome::PossiblyPublished(_)));
    assert!(f.db.secret_session().read().is_err());
    assert!(f.db.conn.lock().is_err());
    if boundary == GenerationBoundary::RuntimeRetired {
        assert_eq!(std::fs::read(&f.store).unwrap(), before);
        return f.root.clone();
    }
    let selected = isolated_scope::capture_selection_at(&f.store, &f.root, &f.root).unwrap();
    let target = selected.source_data_root().to_path_buf();
    assert_ne!(target, f.root);
    assert!(protection::read(&target)
        .unwrap()
        .unwrap()
        .cleanup
        .is_some());
    assert!(
        session::SecretSession::open_protected(&target, &target, &f.cache, None)
            .err()
            .unwrap()
            .to_string()
            .contains("protection.cleanup_required")
    );
    target
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retirement_before_selector_never_reopens_old_runtime() {
    let f = Fixture::new(false);
    interrupted(&f, GenerationBoundary::RuntimeRetired);
    assert!(f.root.join("workbuddy_accounts.json").exists());
    assert!(convert_at(
        &f.db,
        &f.root,
        f.selection(),
        GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
        &f.cache
    )
    .is_err());
    assert_eq!(
        std::fs::read_dir(f.store.parent().unwrap().join("isolated-configs"))
            .unwrap()
            .count(),
        1
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn selected_cleanup_requires_same_authenticated_key_and_durable_completion() {
    let f = Fixture::new(false);
    let target = interrupted(&f, GenerationBoundary::SelectorPublished);
    let selected = isolated_scope::capture_selection_at(&f.store, &f.root, &f.root).unwrap();
    let before = std::fs::read(target.join(protection::FILE)).unwrap();
    let denied = Denied(AtomicUsize::new(0));
    assert!(recover_selected_at(
        &target,
        &selected,
        &denied,
        Some(Zeroizing::new(vec![9; 32]))
    )
    .is_err());
    assert_eq!(
        std::fs::read(target.join(protection::FILE)).unwrap(),
        before
    );
    assert!(f.root.join("workbuddy_accounts.json").exists());
    let metadata = session::read_metadata(&target).unwrap();
    let vault = VaultContext::from_user_text(metadata.metadata, KEY).unwrap();
    recover_selected_at(&target, &selected, &denied, Some(vault.export_key())).unwrap();
    assert_eq!(denied.0.load(Ordering::SeqCst), 0);
    assert!(!f.root.join("workbuddy_accounts.json").exists());
    assert!(!f.root.join(crate::config::DB_FILE_NAME).exists());
    assert!(protection::read(&target)
        .unwrap()
        .unwrap()
        .cleanup
        .is_none());
    assert!(session::SecretSession::open_protected(
        &target,
        &target,
        &denied,
        Some(vault.export_key())
    )
    .is_ok());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn partial_cleanup_resumes_same_target_not_a_third_configuration() {
    let f = Fixture::new(false);
    let target = interrupted(&f, GenerationBoundary::CleanupFileRemoved);
    assert!(!f.root.join("workbuddy_accounts.json").exists());
    let selected = isolated_scope::capture_selection_at(&f.store, &f.root, &f.root).unwrap();
    recover_selected_at(&target, &selected, &f.cache, None).unwrap();
    assert!(protection::read(&target)
        .unwrap()
        .unwrap()
        .cleanup
        .is_none());
    verify_target(&target, &f.cache, false);
    assert_eq!(
        std::fs::read_dir(f.store.parent().unwrap().join("isolated-configs"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(f.root.join("unknown-user-file.bin")).unwrap(),
        b"untouched unknown user bytes"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn cleanup_refuses_changed_source_without_more_deletion() {
    let f = Fixture::new(false);
    let target = interrupted(&f, GenerationBoundary::SelectorPublished);
    let selected = isolated_scope::capture_selection_at(&f.store, &f.root, &f.root).unwrap();
    io::write_durable(
        &f.root.join("workbuddy_accounts.json"),
        b"synthetic changed after publication",
    )
    .unwrap();
    let before = std::fs::read(target.join(protection::FILE)).unwrap();
    assert!(recover_selected_at(&target, &selected, &f.cache, None).is_err());
    assert!(f.root.join(crate::config::DB_FILE_NAME).exists());
    assert_eq!(
        std::fs::read(target.join(protection::FILE)).unwrap(),
        before
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn selected_cleanup_restarts_through_original_startup_and_scope_paths() {
    if std::env::var_os("LOONGPORT_TEST_GENERATION_CLEANUP_CHILD").is_some() {
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        crate::app_store::initialize_active_scope().unwrap();
        let root = crate::config::get_app_config_dir();
        assert_eq!(crate::config::get_device_config_dir(), root);
        assert!(protection::read(&root).unwrap().unwrap().cleanup.is_some());
        let vault =
            VaultContext::from_user_text(session::read_metadata(&root).unwrap().metadata, KEY)
                .unwrap();
        let denied = Denied(AtomicUsize::new(0));
        let inspection = crate::secrets::upgrade::inspect(
            &root,
            &crate::live::engine::DeviceStore::for_device(),
        )
        .unwrap();
        let coordinator =
            crate::secrets::startup::StartupCoordinator::new(root.clone(), inspection);
        coordinator
            .initialize_with_internal_key(None, Some(vault.export_key()), &denied, |session| {
                let (state, _, _) =
                    crate::initialize_runtime_core(session).map_err(|e| e.to_string())?;
                assert_eq!(state.db.secret_session().root(), root);
                assert!(state.db.secret_session().read().is_ok());
                Ok(())
            })
            .unwrap();
        assert!(protection::read(&root).unwrap().unwrap().cleanup.is_none());
        assert_eq!(denied.0.load(Ordering::SeqCst), 0);
        return;
    }
    let mut f = Fixture::new(false);
    let original_store = f.store.clone();
    f.store = crate::app_store::tauri_store_path().unwrap();
    std::fs::create_dir_all(f.store.parent().unwrap()).unwrap();
    io::ensure_private_directory(f.store.parent().unwrap()).unwrap();
    io::write_durable(&f.store, &std::fs::read(original_store).unwrap()).unwrap();
    let target = interrupted(&f, GenerationBoundary::CleanupFileRemoved);
    let name = if cfg!(test) {
        "secrets::protection::generation::tests::selected_cleanup_restarts_through_original_startup_and_scope_paths"
    } else {
        "native_generation_startup_cleanup"
    };
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--test-threads=1", "--nocapture"])
        .env("LOONGPORT_TEST_GENERATION_CLEANUP_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(protection::read(&target)
        .unwrap()
        .unwrap()
        .cleanup
        .is_none());
    assert!(!f.root.join(crate::config::DB_FILE_NAME).exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn pending_cache_failure_reuses_authenticated_candidate() {
    let f = Fixture::new(false);
    let denied = Denied(AtomicUsize::new(0));
    let selection = f.selection();
    assert!(convert_at(
        &f.db,
        &f.root,
        selection.clone(),
        GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
        &denied
    )
    .is_err());
    let pending = isolated_scope::inspect_pending_generation_at(&selection)
        .unwrap()
        .unwrap();
    assert!(convert_at(
        &f.db,
        &f.root,
        selection.clone(),
        GenerationMode::Encrypted(Zeroizing::new("wrong same-pending input".into())),
        &f.cache
    )
    .is_err());
    assert!(f.db.secret_session().read().is_ok());
    let target = result_root(
        convert_at(
            &f.db,
            &f.root,
            selection,
            GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
            &f.cache,
        )
        .unwrap(),
    );
    assert_eq!(target, pending.root());
    assert_eq!(
        std::fs::read_dir(f.store.parent().unwrap().join("isolated-configs"))
            .unwrap()
            .count(),
        1
    );
    verify_target(&target, &f.cache, false);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn pending_full_inventory_changes_are_not_refreshed_into_authority() {
    for target_change in [false, true] {
        let f = Fixture::new(false);
        let selection = f.selection();
        let denied = Denied(AtomicUsize::new(0));
        assert!(convert_at(
            &f.db,
            &f.root,
            selection.clone(),
            GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
            &denied
        )
        .is_err());
        let pending = isolated_scope::inspect_pending_generation_at(&selection)
            .unwrap()
            .unwrap();
        if target_change {
            io::write_durable(
                &pending.root().join("unknown-user-file.bin"),
                b"changed target unknown bytes",
            )
            .unwrap();
        } else {
            f.db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE future_generation_data SET value=X'AA' WHERE id=1",
                    [],
                )
                .unwrap();
        }
        let original = std::fs::read(&f.store).unwrap();
        assert!(convert_at(
            &f.db,
            &f.root,
            selection,
            GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
            &f.cache
        )
        .is_err());
        assert_eq!(std::fs::read(&f.store).unwrap(), original);
        assert!(f.db.secret_session().read().is_ok());
        assert!(f.root.join("workbuddy_accounts.json").exists());
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn empty_input_never_reserves_a_pending_generation() {
    let f = Fixture::new(false);
    assert!(convert_at(
        &f.db,
        &f.root,
        f.selection(),
        GenerationMode::Encrypted(Zeroizing::new(String::new())),
        &f.cache
    )
    .is_err());
    assert!(
        isolated_scope::inspect_pending_generation_at(&f.selection())
            .unwrap()
            .is_none()
    );
    assert!(f.db.secret_session().read().is_ok());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_dead_process_before_selector_resumes_same_pending() {
    if let Some(phase) = std::env::var_os("LOONGPORT_TEST_PENDING_PROCESS") {
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        crate::app_store::initialize_active_scope().unwrap();
        let root = crate::config::get_app_config_dir();
        let cache = MemoryKeyStore::default();
        let session = session::SecretSession::open_protected(&root, &root, &cache, None).unwrap();
        let db = crate::Database::init_with_secrets(session).unwrap();
        let selection = capture_current_selection().unwrap();
        if phase == "crash" {
            let _ = convert_with_hook(
                &db,
                &root,
                selection,
                GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
                &cache,
                &mut |boundary, _| {
                    if boundary == GenerationBoundary::RuntimeRetired {
                        std::process::exit(77)
                    }
                    Ok(())
                },
            );
            panic!("did not reach native crash boundary")
        }
        let pending = isolated_scope::inspect_pending_generation_at(&selection)
            .unwrap()
            .unwrap();
        let target = result_root(
            convert_at(
                &db,
                &root,
                selection,
                GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
                &cache,
            )
            .unwrap(),
        );
        assert_eq!(target, pending.root());
        verify_target(&target, &cache, false);
        return;
    }
    let mut f = Fixture::new(false);
    let old_store = f.store.clone();
    f.store = crate::app_store::tauri_store_path().unwrap();
    std::fs::create_dir_all(f.store.parent().unwrap()).unwrap();
    io::write_durable(&f.store, &std::fs::read(old_store).unwrap()).unwrap();
    let Fixture {
        _home,
        root,
        store,
        db,
        cache,
    } = f;
    drop(db);
    drop(cache);
    let name = if cfg!(test) {
        "secrets::protection::generation::tests::native_dead_process_before_selector_resumes_same_pending"
    } else {
        "native_generation_dead_process_resume"
    };
    let run = |phase| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env("LOONGPORT_TEST_PENDING_PROCESS", phase)
            .output()
            .unwrap()
    };
    let crashed = run("crash");
    assert_eq!(
        crashed.status.code(),
        Some(77),
        "{}",
        String::from_utf8_lossy(&crashed.stderr)
    );
    let selection = isolated_scope::capture_selection_at(&store, &root, &root).unwrap();
    assert_eq!(selection.source_data_root(), root);
    let candidate = isolated_scope::inspect_pending_generation_at(&selection)
        .unwrap()
        .unwrap();
    let resumed = run("resume");
    assert!(
        resumed.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    let selected = isolated_scope::capture_selection_at(&store, &root, &root).unwrap();
    assert_eq!(selected.source_data_root(), candidate.root());
    assert!(!root.join(crate::config::DB_FILE_NAME).exists());
    assert_eq!(
        std::fs::read_dir(store.parent().unwrap().join("isolated-configs"))
            .unwrap()
            .count(),
        1
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn runtime_admission_requires_actual_durable_choice_without_os_access() {
    for encrypted in [false, true] {
        let f = Fixture::new(encrypted);
        let context = f.db.secret_session().read().unwrap();
        protection::validate_runtime_storage(&f.root, &context).unwrap();
        let record = std::fs::read(f.root.join(protection::FILE)).unwrap();
        std::fs::remove_file(f.root.join(protection::FILE)).unwrap();
        assert!(protection::validate_runtime_storage(&f.root, &context).is_err());
        io::write_durable(&f.root.join(protection::FILE), &record).unwrap();
        if encrypted {
            let pending = ProtectionRecord::encrypted(
                context.encrypted().unwrap(),
                Some("synthetic pending cleanup"),
            )
            .unwrap();
            write_record(&f.root, &pending).unwrap();
            assert!(protection::validate_runtime_storage(&f.root, &context).is_err());
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn cleanup_recreated_owned_file_never_claims_complete() {
    cleanup_residue_case("recreated");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn cleanup_new_registered_owned_file_never_claims_complete() {
    cleanup_residue_case("owned");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn cleanup_new_owned_database_backup_never_claims_complete() {
    cleanup_residue_case("backup");
}
fn cleanup_residue_case(kind: &str) {
    let f = Fixture::new(false);
    let backup = inspection::capture(&f.root.join(crate::config::DB_FILE_NAME))
        .unwrap()
        .unwrap()
        .image
        .serialize(rusqlite::MAIN_DB)
        .unwrap()
        .to_vec();
    let residue = match kind {
        "recreated" => f.root.join("workbuddy_accounts.json"),
        "owned" => f.root.join("minimax_code_accounts.json"),
        "backup" => f.root.join("backups/new-during-cleanup.db"),
        _ => unreachable!(),
    };
    let mut wrote = false;
    let result = convert_with_hook(
        &f.db,
        &f.root,
        f.selection(),
        GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
        &f.cache,
        &mut |boundary, path| {
            if boundary == GenerationBoundary::CleanupFileRemoved
                && path
                    .file_name()
                    .is_some_and(|name| name == "workbuddy_accounts.json")
                && !wrote
            {
                wrote = true;
                io::ensure_private_directory(residue.parent().unwrap())?;
                io::write_durable(
                    &residue,
                    if kind == "backup" {
                        &backup
                    } else {
                        b"synthetic new plaintext must not be silently removed"
                    },
                )?;
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(wrote);
    assert!(residue.exists());
    assert!(
        matches!(result, PublicationOutcome::PossiblyPublished(_)),
        "new source residue must retain pending protection: {result:?}"
    );
    let selection = f.selection();
    let target = selection.source_data_root();
    assert_ne!(target, f.root);
    assert!(protection::read(target).unwrap().unwrap().cleanup.is_some());
    assert!(session::SecretSession::open_protected(target, target, &f.cache, None).is_err());
    assert!(f.db.secret_session().read().is_err());
    assert_eq!(
        std::fs::read(&residue).unwrap(),
        if kind == "backup" {
            backup
        } else {
            b"synthetic new plaintext must not be silently removed".to_vec()
        }
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_separate_dead_process_resumes_same_opaque_preserving_candidate() {
    if let Some(phase) = std::env::var_os("LOONGPORT_TEST_SEPARATE_PROCESS") {
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        crate::app_store::initialize_active_scope().unwrap();
        let root = crate::config::get_app_config_dir();
        let cache = MemoryKeyStore::default();
        let co = crate::secrets::startup::StartupCoordinator::new(
            root.clone(),
            crate::secrets::upgrade::inspect(
                &root,
                &crate::live::engine::DeviceStore::for_device(),
            )
            .unwrap(),
        );
        let selection = capture_current_selection().unwrap();
        if phase == "crash" {
            let _ = create_separate_with_hook(
                &co,
                selection,
                GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
                &cache,
                &mut |boundary, _| {
                    if boundary == GenerationBoundary::RuntimeRetired {
                        std::process::exit(77)
                    }
                    Ok(())
                },
            );
            panic!("did not reach separate native crash boundary")
        }
        let pending = isolated_scope::inspect_pending_generation_at(&selection)
            .unwrap()
            .unwrap();
        let target = result_root(
            create_separate_at(
                &co,
                selection,
                GenerationMode::Encrypted(Zeroizing::new(KEY.into())),
                &cache,
            )
            .unwrap(),
        );
        assert_eq!(target, pending.root());
        let context =
            session::SecretSession::open_protected(&target, &target, &cache, None).unwrap();
        assert!(!context.read().unwrap().is_plaintext());
        let image = inspection::capture(&target.join(crate::config::DB_FILE_NAME))
            .unwrap()
            .unwrap();
        assert_eq!(
            image
                .image
                .query_row("SELECT count(*) FROM providers", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        return;
    }
    let Fixture {
        _home,
        root,
        store,
        db,
        cache,
    } = Fixture::new(true);
    drop(db);
    drop(cache);
    let control = crate::app_store::tauri_store_path().unwrap();
    std::fs::create_dir_all(control.parent().unwrap()).unwrap();
    io::ensure_private_directory(control.parent().unwrap()).unwrap();
    io::write_durable(&control, &std::fs::read(&store).unwrap()).unwrap();
    let before = target_tree(&root).unwrap();
    let record = std::fs::read(root.join(protection::FILE)).unwrap();
    let name = if cfg!(test) {
        "secrets::protection::generation::tests::native_separate_dead_process_resumes_same_opaque_preserving_candidate"
    } else {
        "separate_configuration_dead_process"
    };
    let child = |phase: &str| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env("LOONGPORT_TEST_SEPARATE_PROCESS", phase)
            .output()
            .unwrap()
    };
    let crashed = child("crash");
    assert_eq!(
        crashed.status.code(),
        Some(77),
        "{}",
        String::from_utf8_lossy(&crashed.stderr)
    );
    let selection = isolated_scope::capture_selection_at(&control, &root, &root).unwrap();
    let pending = isolated_scope::inspect_pending_generation_at(&selection)
        .unwrap()
        .unwrap();
    let pending_root = pending.root().to_path_buf();
    let resumed = child("resume");
    assert!(
        resumed.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(
        isolated_scope::capture_selection_at(&control, &root, &root)
            .unwrap()
            .source_data_root(),
        pending_root
    );
    assert_eq!(
        std::fs::read_dir(control.parent().unwrap().join("isolated-configs"))
            .unwrap()
            .count(),
        1
    );
    assert!(target_tree(&root).unwrap() == before);
    assert_eq!(std::fs::read(root.join(protection::FILE)).unwrap(), record);
}
