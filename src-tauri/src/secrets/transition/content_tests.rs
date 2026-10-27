//! The existing remote restore service must retain the selected local storage mode.
use super::*;
use crate::secrets::{
    key_store::KeyStoreError,
    protection::{ProtectionRecord, ProtectionStorage},
    session::{completed_metadata, SecretSession},
    storage::{PlaintextIdentity, StorageContext},
    testing::TestHome,
};
use crate::services::sync_protocol::{self, DownloadedSnapshot, RemoteLayout};
use std::sync::Arc;

const SOURCE_TEXT: &str = "  source 文本\n🔑 ";
const TARGET_TEXT: &str = "target different 文本";
struct NoStore;
impl KeyStore for NoStore {
    fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        panic!("same-storage restore must not read OS credentials")
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
        panic!("same-storage restore must not replace OS credentials")
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        panic!("same-storage restore must not revoke OS credentials")
    }
}
fn text(text: &str) -> StorageContext {
    VaultContext::create_generation_with_user_text(uuid::Uuid::new_v4().to_string(), 1, text)
        .unwrap()
        .into()
}
fn plain() -> StorageContext {
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    })
}
fn database(root: &Path, context: &StorageContext) -> Database {
    let connection =
        crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), context)
            .unwrap();
    let record = match context {
        StorageContext::Encrypted(vault) => {
            write_metadata(root, &completed_metadata(vault, false).unwrap()).unwrap();
            ProtectionRecord::encrypted(vault, None).unwrap()
        }
        StorageContext::Plaintext(identity) => ProtectionRecord {
            format_version: 1,
            choice_complete: true,
            storage: ProtectionStorage::Plaintext {
                identity: identity.clone(),
            },
            commitment: None,
            cleanup: None,
        },
    };
    write_durable(
        &root.join(crate::secrets::protection::FILE),
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    Database::from_connection(
        connection,
        SecretSession::from_storage_context(root.to_path_buf(), context.clone()),
    )
}
fn tree(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    if !root.exists() {
        return out;
    }
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(tree(&path));
        } else {
            out.insert(path.clone(), std::fs::read(&path).unwrap());
        }
    }
    out
}
struct Fixture {
    _home: TestHome,
    current: StorageContext,
    db: Arc<Database>,
    snapshot: DownloadedSnapshot,
    metadata: std::collections::BTreeMap<PathBuf, Vec<u8>>,
}
impl Fixture {
    fn new(current: StorageContext) -> Self {
        Self::new_with_source(current, text(SOURCE_TEXT))
    }
    fn new_with_source(current: StorageContext, source_context: StorageContext) -> Self {
        Self::new_with_source_and_skill(current, source_context, b"remote skill body")
    }
    fn new_with_source_and_skill(
        current: StorageContext,
        source_context: StorageContext,
        skill_body: &[u8],
    ) -> Self {
        let home = TestHome::new().unwrap();
        let root = crate::config::get_app_config_dir();
        let source = database(&home.path().join("authenticated-source"), &source_context);
        source
            .set_setting("common_config_claude", "remote-joint-canary")
            .unwrap();
        let skills = root.join("skills");
        std::fs::create_dir_all(skills.join("remote-skill")).unwrap();
        std::fs::write(skills.join("remote-skill/SKILL.md"), skill_body).unwrap();
        crate::settings::unlock_settings_for_test(source.secrets.clone()).unwrap();
        let local = sync_protocol::build_local_snapshot(&source).unwrap();
        let snapshot = DownloadedSnapshot {
            manifest: serde_json::from_slice(&local.manifest_bytes).unwrap(),
            manifest_hash: local.manifest_hash,
            etag: None,
            db_sql: local.db_sql,
            skills_zip: local.skills_zip,
            layout: RemoteLayout::Current,
            source_path: "synthetic-local-source".into(),
        };
        std::fs::remove_dir_all(&skills).unwrap();
        std::fs::create_dir_all(skills.join("old-skill")).unwrap();
        std::fs::write(skills.join("old-skill/SKILL.md"), b"old skill body").unwrap();
        let db = Arc::new(database(&root, &current));
        db.set_setting("common_config_claude", "local-before-canary")
            .unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO session_log_sync(file_path,last_modified,last_line_offset,last_synced_at) VALUES('preserved-local-session',11,22,33)", []).unwrap();
        crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
        let metadata = ["vault.json", crate::secrets::protection::FILE]
            .into_iter()
            .filter_map(|name| {
                let path = root.join(name);
                std::fs::read(&path).ok().map(|bytes| (path, bytes))
            })
            .collect();
        Self {
            _home: home,
            current,
            db,
            snapshot,
            metadata,
        }
    }
    fn restore(&self, input: &str) -> Result<(), AppError> {
        sync_protocol::restore_from_sync(
            &self.db,
            &self.snapshot,
            &self.snapshot.manifest.snapshot_id,
            input,
            &NoStore,
        )
    }
    fn assert_installed(&self) {
        assert_eq!(
            self.db.secrets.read().unwrap().identity(),
            self.current.identity()
        );
        assert_eq!(
            self.db
                .get_setting("common_config_claude")
                .unwrap()
                .as_deref(),
            Some("remote-joint-canary")
        );
        assert_eq!(self.db.conn.lock().unwrap().query_row("SELECT last_line_offset FROM session_log_sync WHERE file_path='preserved-local-session'", [], |row| row.get::<_, i64>(0)).unwrap(), 22);
        for (path, bytes) in &self.metadata {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
        assert_eq!(
            std::fs::read(self.db.secrets.root().join("skills/remote-skill/SKILL.md")).unwrap(),
            b"remote skill body"
        );
        assert!(!self.db.secrets.root().join("skills/old-skill").exists());
        assert!(!self.db.secrets.root().join(INTENT).exists());
        if self.current.is_plaintext() {
            assert!(!self.db.secrets.root().join("vault.json").exists());
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn remote_content_into_plaintext_keeps_plaintext_identity() {
    let fixture = Fixture::new(plain());
    fixture.restore(SOURCE_TEXT).unwrap();
    fixture.assert_installed();
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn remote_content_into_text_keeps_current_user_key() {
    let fixture = Fixture::new(text(TARGET_TEXT));
    fixture.restore(SOURCE_TEXT).unwrap();
    fixture.assert_installed();
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn wrong_source_text_never_changes_receiver_or_skills() {
    let fixture = Fixture::new(text(TARGET_TEXT));
    let before = tree(fixture._home.path());
    assert!(fixture.restore("wrong source text").is_err());
    assert_eq!(tree(fixture._home.path()), before);
    assert!(fixture.db.secrets.ensure_available().is_ok());
}
#[cfg(feature = "test-hooks")]
pub(crate) fn run(case: &str) {
    match case {
        "plaintext" => remote_content_into_plaintext_keeps_plaintext_identity(),
        "text" => remote_content_into_text_keeps_current_user_key(),
        "wrong_source" => wrong_source_text_never_changes_receiver_or_skills(),
        #[cfg(feature = "gui")]
        "recover_intent" => interrupted_intent_recovers_through_original_registered_command(),
        #[cfg(feature = "gui")]
        "recover_database" => interrupted_database_recovers_through_original_registered_command(),
        #[cfg(feature = "gui")]
        "recover_skills" => interrupted_skills_recovers_through_original_registered_command(),
        #[cfg(feature = "gui")]
        "metadata_change" => recovery_rejects_target_metadata_change(),
        #[cfg(feature = "gui")]
        "database_change" => recovery_rejects_database_change(),
        #[cfg(feature = "gui")]
        "skills_change" => recovery_rejects_skills_change(),
        #[cfg(feature = "gui")]
        "stage_tamper" => recovery_rejects_stage_tampering(),
        #[cfg(feature = "gui")]
        "home_change" => recovery_does_not_retarget_unified_skills_to_changed_home(),
        #[cfg(feature = "gui")]
        "real_process_exit" => actual_process_exit_reenters_original_recovery_command(),
        _ => panic!("unknown joint storage test"),
    }
}

fn prepared_install(
    db: &Database,
    snapshot: &DownloadedSnapshot,
    location: crate::services::skill::SkillStorageLocation,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let auth = VaultContext::from_recovery_input(
        snapshot.manifest.vault_metadata().unwrap().clone(),
        SOURCE_TEXT,
    )
    .unwrap();
    let incoming = {
        let current = db.secrets.read().unwrap();
        db.prepare_sync_snapshot_for_storage(
            std::str::from_utf8(&snapshot.db_sql).unwrap(),
            auth.metadata(),
            &auth,
            &current,
            &current,
        )
        .unwrap()
    };
    let prepared =
        crate::services::webdav_sync::archive::stage_skills_zip(&snapshot.skills_zip).unwrap();
    let _skills = crate::services::skill::skill_state_write_guard();
    content::install_with_hook(
        db,
        incoming,
        &auth,
        SkillsReplacement {
            source: prepared.source.clone(),
            location,
        },
        hook,
    )
}
fn interrupt(fixture: &Fixture, point: Checkpoint) {
    let result = prepared_install(
        &fixture.db,
        &fixture.snapshot,
        crate::services::skill::SkillStorageLocation::LoongPort,
        &mut |actual| {
            if actual == point {
                Err(AppError::Config("synthetic interruption".into()))
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert!(fixture.db.secrets.read().is_err());
    assert!(pending_record(fixture.db.secrets.root()).unwrap().is_some());
}
#[cfg(feature = "gui")]
fn recovery_app(root: &Path) -> tauri::App<tauri::test::MockRuntime> {
    recovery_app_with_store(root, Arc::new(NoStore))
}
#[cfg(feature = "gui")]
fn recovery_app_with_store(
    root: &Path,
    store: Arc<dyn KeyStore>,
) -> tauri::App<tauri::test::MockRuntime> {
    use crate::secrets::startup::{
        self, credential_protection::CommandDependencies, StartupCoordinator,
    };
    let inspected =
        crate::secrets::upgrade::inspect(root, &crate::live::engine::DeviceStore::for_device())
            .unwrap();
    assert!(inspected.is_recovery_required());
    tauri::test::mock_builder()
        .manage(StartupCoordinator::new(root.to_path_buf(), inspected))
        .manage(CommandDependencies::<tauri::test::MockRuntime>::recovery_test_store(store))
        .invoke_handler(tauri::generate_handler![
            startup::get_startup_recovery,
            startup::recover_startup_operation
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}
#[cfg(feature = "gui")]
fn invoke(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    tauri::test::get_ipc_response(
        window,
        tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: crate::integration_test_support::tauri_test_origin(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    )
    .map(|response| response.deserialize::<serde_json::Value>().unwrap())
}
#[cfg(feature = "gui")]
fn recover_registered(root: &Path, input: &str) -> Result<serde_json::Value, serde_json::Value> {
    let app = recovery_app(root);
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let status = invoke(&window, "get_startup_recovery", serde_json::json!({})).unwrap();
    assert_eq!(status["status"], "pending");
    assert_eq!(status["canRecover"], true);
    invoke(
        &window,
        "recover_startup_operation",
        serde_json::json!({"token":status["token"], "password":input}),
    )
}
#[cfg(feature = "gui")]
fn recovered_at(point: Checkpoint) {
    for current in [plain(), text(TARGET_TEXT)] {
        let fixture = Fixture::new(current);
        interrupt(&fixture, point);
        let before = tree(fixture._home.path());
        assert!(recover_registered(fixture.db.secrets.root(), "wrong archive text").is_err());
        assert_eq!(tree(fixture._home.path()), before);
        let result = recover_registered(fixture.db.secrets.root(), SOURCE_TEXT).unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(result["restartRequired"], true);
        // The original live session stays retired; a restart opens the settled context.
        assert!(fixture.db.secrets.read().is_err());
        let connection = crate::database::storage::prepare(
            &fixture.db.secrets.root().join(crate::config::DB_FILE_NAME),
            &fixture.current,
        )
        .unwrap();
        let fresh = Database::from_connection(
            connection,
            SecretSession::from_storage_context(
                fixture.db.secrets.root().to_path_buf(),
                fixture.current.clone(),
            ),
        );
        assert_eq!(
            fresh
                .get_setting("common_config_claude")
                .unwrap()
                .as_deref(),
            Some("remote-joint-canary")
        );
        for (path, bytes) in &fixture.metadata {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
        assert_eq!(
            std::fs::read(fresh.secrets.root().join("skills/remote-skill/SKILL.md")).unwrap(),
            b"remote skill body"
        );
        assert!(!fresh.secrets.root().join(INTENT).exists());
    }
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn interrupted_intent_recovers_through_original_registered_command() {
    recovered_at(Checkpoint::Intent);
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn interrupted_database_recovers_through_original_registered_command() {
    recovered_at(Checkpoint::Database);
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn interrupted_skills_recovers_through_original_registered_command() {
    recovered_at(Checkpoint::Metadata);
}

#[cfg(feature = "gui")]
fn changed_source(kind: &str) {
    let fixture = Fixture::new(plain());
    interrupt(&fixture, Checkpoint::Intent);
    let root = fixture.db.secrets.root();
    match kind {
        "metadata" => {
            let path = root.join(crate::secrets::protection::FILE);
            let mut record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            record["choiceComplete"] = false.into();
            std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        "database" => {
            // Simulate an independent writer, not the correctly retired original session.
            let external = Database::from_connection(
                Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap(),
                SecretSession::from_storage_context(root.to_path_buf(), fixture.current.clone()),
            );
            external
                .set_setting("late-local-change", "untouched")
                .unwrap();
        }
        "skills" => {
            std::fs::write(
                root.join("skills/old-skill/SKILL.md"),
                b"new local user edit",
            )
            .unwrap();
        }
        "stage" => {
            let intent: serde_json::Value =
                serde_json::from_slice(&pending_record(root).unwrap().unwrap()).unwrap();
            let path = stage_file(root, intent["id"].as_str().unwrap(), 0).unwrap();
            std::fs::write(path, b"tampered encrypted staging").unwrap();
        }
        _ => unreachable!(),
    }
    let before = tree(fixture._home.path());
    assert!(recover_registered(root, SOURCE_TEXT).is_err());
    assert_eq!(tree(fixture._home.path()), before);
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn recovery_rejects_target_metadata_change() {
    changed_source("metadata");
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn recovery_rejects_database_change() {
    changed_source("database");
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn recovery_rejects_skills_change() {
    changed_source("skills");
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn recovery_rejects_stage_tampering() {
    changed_source("stage");
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn recovery_does_not_retarget_unified_skills_to_changed_home() {
    let fixture = Fixture::new(plain());
    let result = prepared_install(
        &fixture.db,
        &fixture.snapshot,
        crate::services::skill::SkillStorageLocation::Unified,
        &mut |point| {
            if point == Checkpoint::Intent {
                Err(AppError::Config("synthetic interruption".into()))
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    let old = tree(fixture._home.path());
    let changed_home = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("CC_SWITCH_TEST_HOME").unwrap();
    std::env::set_var("CC_SWITCH_TEST_HOME", changed_home.path());
    let result = recover_registered(fixture.db.secrets.root(), SOURCE_TEXT);
    std::env::set_var("CC_SWITCH_TEST_HOME", previous);
    assert!(
        result.is_err(),
        "a pending restore must not choose a different Unified skills root on restart"
    );
    assert!(tree(changed_home.path()).is_empty());
    assert_eq!(tree(fixture._home.path()), old);
}
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn actual_process_exit_reenters_original_recovery_command() {
    process_exit_reentry(Checkpoint::Database);
}
#[cfg(all(test, feature = "gui"))]
#[test]
#[serial_test::serial]
fn process_exit_before_intent_allows_authenticated_new_content_operation() {
    process_exit_reentry(Checkpoint::Staged);
}
#[cfg(all(test, feature = "gui"))]
#[test]
#[serial_test::serial]
fn process_exit_after_intent_clear_allows_authenticated_new_content_operation() {
    process_exit_reentry(Checkpoint::Keys);
}
#[cfg(feature = "gui")]
fn process_exit_reentry(exit: Checkpoint) {
    if std::env::var_os("LOONGPORT_JOINT_RESTORE_CHILD").is_some() {
        let root = crate::config::get_app_config_dir();
        let (record, _) = crate::secrets::protection::read_with_bytes(&root)
            .unwrap()
            .unwrap();
        let local = crate::secrets::session::read_metadata_with_bytes(&root).unwrap();
        let key = local.as_ref().map(|(saved, _)| {
            VaultContext::from_user_text(saved.metadata.clone(), TARGET_TEXT)
                .unwrap()
                .export_key()
        });
        let current = crate::secrets::protection::open_context(
            &record,
            local.as_ref().map(|(saved, _)| &saved.metadata),
            key,
            |_, _| panic!("child must not access the system credential cache"),
        )
        .unwrap();
        let connection =
            crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &current)
                .unwrap();
        let db = Database::from_connection(
            connection,
            SecretSession::from_storage_context(root.clone(), current),
        );
        crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
        let input = crate::config::get_home_dir().join("joint-input");
        let snapshot = DownloadedSnapshot {
            manifest: serde_json::from_slice(&std::fs::read(input.join("manifest.json")).unwrap())
                .unwrap(),
            manifest_hash: String::new(),
            etag: None,
            db_sql: std::fs::read(input.join("db.sql")).unwrap(),
            skills_zip: std::fs::read(input.join("skills.zip")).unwrap(),
            layout: RemoteLayout::Current,
            source_path: "synthetic-local-source".into(),
        };
        let result = prepared_install(
            &db,
            &snapshot,
            crate::services::skill::SkillStorageLocation::LoongPort,
            &mut |point| {
                let exit = match std::env::var("LP_CONTENT_EXIT_BOUNDARY").as_deref() {
                    Ok("staged") => Checkpoint::Staged,
                    Ok("cleared") => Checkpoint::Keys,
                    _ => Checkpoint::Database,
                };
                if point == exit {
                    std::process::exit(77);
                }
                Ok(())
            },
        );
        panic!("child did not exit at publication: {result:?}");
    }
    for current in [plain(), text(TARGET_TEXT)] {
        let Fixture {
            _home,
            current,
            db,
            snapshot,
            metadata,
        } = Fixture::new(current);
        let root = db.secrets.root().to_path_buf();
        let input = _home.path().join("joint-input");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(
            input.join("manifest.json"),
            serde_json::to_vec(&snapshot.manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(input.join("db.sql"), &snapshot.db_sql).unwrap();
        std::fs::write(input.join("skills.zip"), &snapshot.skills_zip).unwrap();
        drop(db);
        let exact = if cfg!(test) {
            "secrets::transition::content_tests::actual_process_exit_reenters_original_recovery_command"
        } else {
            "joint_storage_real_process_exit"
        };
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", exact, "--nocapture"])
            .env("LOONGPORT_JOINT_RESTORE_CHILD", "1")
            .env(
                "LP_CONTENT_EXIT_BOUNDARY",
                match exit {
                    Checkpoint::Staged => "staged",
                    Checkpoint::Keys => "cleared",
                    _ => "database",
                },
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if exit == Checkpoint::Database {
            let status = recover_registered(&root, SOURCE_TEXT).unwrap();
            assert_eq!(status["restartRequired"], true);
        } else {
            assert!(pending_record(&root).unwrap().is_none());
        }
        let connection =
            crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &current)
                .unwrap();
        let db = Database::from_connection(
            connection,
            SecretSession::from_storage_context(root.clone(), current),
        );
        if exit != Checkpoint::Database {
            crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
            prepared_install(
                &db,
                &snapshot,
                crate::services::skill::SkillStorageLocation::LoongPort,
                &mut |_| Ok(()),
            )
            .unwrap();
        }
        let store = crate::app_store::tauri_store_path().unwrap();
        assert!(crate::app_store::isolated_scope::lock_selection_at(&store).is_ok());
        assert_eq!(
            db.get_setting("common_config_claude").unwrap().as_deref(),
            Some("remote-joint-canary")
        );
        assert_eq!(
            std::fs::read(root.join("skills/remote-skill/SKILL.md")).unwrap(),
            b"remote skill body"
        );
        for (path, bytes) in metadata {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        assert!(!root.join(INTENT).exists());
    }
}

#[path = "remote_plaintext_tests.rs"]
pub(crate) mod remote_plaintext_tests;

#[path = "remote_plaintext_transport_tests.rs"]
pub(crate) mod remote_plaintext_transport;

#[cfg(feature = "gui")]
#[path = "remote_plaintext_recovery_tests.rs"]
pub(crate) mod remote_plaintext_recovery;

#[cfg(all(feature = "gui", feature = "test-hooks"))]
#[path = "pool_restore_consumer_tests.rs"]
mod pool_restore_consumer_tests;

#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub(crate) async fn verify_pool_restore(case: &str) {
    pool_restore_consumer_tests::verify(case).await;
}

#[cfg(test)]
mod native_content_admission {
    use crate::app_store::isolated_scope::{
        capture_selection_at, lock_content_source_at, SelectionSnapshot,
    };
    use crate::secrets::{
        session::SecretSession,
        storage::{PlaintextIdentity, StorageContext},
        VaultContext,
    };
    use crate::workbuddy::native_owner::testing;
    use std::path::{Path, PathBuf};
    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        selection: SelectionSnapshot,
        storage: StorageContext,
    }
    impl Fixture {
        fn new(encrypted: bool) -> Self {
            let t = crate::secrets::testing::tempdir().unwrap();
            let root = t.path().join("data");
            std::fs::create_dir(&root).unwrap();
            let store = t.path().join("control/app_paths.json");
            let selection = capture_selection_at(&store, &root, &root).unwrap();
            let storage = if encrypted {
                StorageContext::Encrypted(VaultContext::generate().unwrap())
            } else {
                StorageContext::Plaintext(PlaintextIdentity {
                    format_version: 1,
                    store_id: uuid::Uuid::new_v4().to_string(),
                    generation_id: uuid::Uuid::new_v4().to_string(),
                    revision: 1,
                })
            };
            Self {
                _temp: t,
                root,
                selection,
                storage,
            }
        }
        fn journal(&self) -> PathBuf {
            self.root
                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        }
        fn receipts(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            let client = crate::secrets::testing::tempdir().unwrap();
            testing::receipts(&self.root, &self.storage, client.path())
        }
    }
    fn check(
        root: &Path,
        storage: &StorageContext,
        guard: &crate::app_store::isolated_scope::SelectedSourceGuard,
    ) -> Result<(), crate::error::AppError> {
        crate::secrets::transition::content::verify_native_content_admission(root, storage, guard)
    }
    #[test]
    fn original_session_blocking_advances_only_the_owned_epoch_and_foreign_changes_refuse() {
        use crate::secrets::transition::content::{block_content_session, verify_content_epoch};
        let root = crate::secrets::testing::tempdir().unwrap();
        let storage = StorageContext::Plaintext(crate::secrets::storage::PlaintextIdentity {
            format_version: 1,
            store_id: "11111111-1111-4111-8111-111111111111".into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        });
        let session = SecretSession::from_storage_context(root.path().to_path_buf(), storage);
        let _old = session.write().unwrap();
        let epoch = session.lifecycle_epoch();
        verify_content_epoch(&session, epoch).unwrap();
        let blocked = block_content_session(&session, epoch).unwrap();
        assert_eq!(blocked, epoch + 1);
        verify_content_epoch(&session, blocked).unwrap();
        assert!(verify_content_epoch(&session, epoch).is_err());
        session.set_blocked(true);
        assert!(verify_content_epoch(&session, blocked).is_err());
        let foreign = session.lifecycle_epoch();
        assert!(block_content_session(&session, epoch).is_err());
        assert_eq!(session.lifecycle_epoch(), foreign);
    }
    #[test]
    fn prepared_blocks_with_original_old_context_and_preserves_receipt() {
        for encrypted in [false, true] {
            let f = Fixture::new(encrypted);
            let (prepared, _, _) = f.receipts();
            std::fs::write(f.journal(), &prepared).unwrap();
            let guard = lock_content_source_at(&f.selection, &[&f.root.join("skills")]).unwrap();
            assert!(check(&f.root, &f.storage, &guard).is_err());
            assert_eq!(std::fs::read(f.journal()).unwrap(), prepared);
        }
    }
    #[test]
    fn absent_and_terminal_allow_under_held_session_write_without_touching_receipt() {
        for encrypted in [false, true] {
            let f = Fixture::new(encrypted);
            let guard = lock_content_source_at(&f.selection, &[]).unwrap();
            let session = SecretSession::from_storage_context(f.root.clone(), f.storage.clone());
            let old = session.write().unwrap();
            check(&f.root, &old, &guard).unwrap();
            let (_, restored, committed) = f.receipts();
            for bytes in [restored, committed] {
                std::fs::write(f.journal(), &bytes).unwrap();
                check(&f.root, &old, &guard).unwrap();
                assert_eq!(std::fs::read(f.journal()).unwrap(), bytes);
            }
        }
    }
    #[test]
    fn wrong_key_unknown_or_corrupt_is_not_absent() {
        let f = Fixture::new(true);
        let (_, _, committed) = f.receipts();
        let guard = lock_content_source_at(&f.selection, &[]).unwrap();
        let foreign = StorageContext::Encrypted(VaultContext::generate().unwrap());
        assert!(check(&f.root, &foreign, &guard).is_err());
        assert_eq!(std::fs::read(f.journal()).unwrap(), committed);
        std::fs::write(f.journal(), b"unknown/corrupt synthetic receipt").unwrap();
        assert!(check(&f.root, &f.storage, &guard).is_err());
        assert_eq!(
            std::fs::read(f.journal()).unwrap(),
            b"unknown/corrupt synthetic receipt"
        );
    }
    #[test]
    fn phase_and_identity_changes_are_rechecked_on_each_admission() {
        let f = Fixture::new(false);
        let (prepared, _, _) = f.receipts();
        let guard = lock_content_source_at(&f.selection, &[]).unwrap();
        check(&f.root, &f.storage, &guard).unwrap();
        std::fs::write(f.journal(), &prepared).unwrap();
        assert!(check(&f.root, &f.storage, &guard).is_err());
        let other = f._temp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        assert!(check(&other, &f.storage, &guard).is_err());
    }
}

#[cfg(test)]
mod native_content_consumer {
    use super::*;
    use crate::app_store::isolated_scope;
    use crate::workbuddy::native_owner::testing;
    fn journal(f: &Fixture) -> PathBuf {
        f.db.secrets
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
    }
    fn receipts(f: &Fixture) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let client = crate::secrets::testing::tempdir().unwrap();
        let old = f.db.secrets.write().unwrap();
        testing::receipts(f.db.secrets.root(), &old, client.path())
    }
    fn digest(f: &Fixture) -> String {
        Database::content_digest(&f.db.conn.lock().unwrap()).unwrap()
    }
    fn install(
        f: &Fixture,
        hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        prepared_install(
            &f.db,
            &f.snapshot,
            crate::services::skill::SkillStorageLocation::LoongPort,
            hook,
        )
    }
    #[test]
    #[serial_test::serial]
    fn prepared_or_corrupt_native_receipt_blocks_original_content_before_stage() {
        for encrypted in [false, true] {
            let f = Fixture::new(if encrypted {
                text(TARGET_TEXT)
            } else {
                plain()
            });
            let (prepared, _, _) = receipts(&f);
            let before = digest(&f);
            let skills = tree(&f.db.secrets.root().join("skills"));
            for bytes in [prepared, b"corrupt synthetic receipt".to_vec()] {
                std::fs::write(journal(&f), &bytes).unwrap();
                let mut called = false;
                assert!(install(&f, &mut |_| {
                    called = true;
                    Ok(())
                })
                .is_err());
                assert!(!called);
                assert_eq!(digest(&f), before);
                assert_eq!(tree(&f.db.secrets.root().join("skills")), skills);
                assert_eq!(std::fs::read(journal(&f)).unwrap(), bytes);
                assert!(pending_record(f.db.secrets.root()).unwrap().is_none());
                assert!(f.db.secrets.ensure_available().is_ok());
                assert!(std::fs::read_dir(f.db.secrets.root()).unwrap().all(|e| !e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".vault-transition-")));
            }
        }
    }
    #[test]
    #[serial_test::serial]
    fn terminal_native_receipts_survive_actual_plain_and_encrypted_restore() {
        for encrypted in [false, true] {
            for restored in [false, true] {
                let f = Fixture::new(if encrypted {
                    text(TARGET_TEXT)
                } else {
                    plain()
                });
                let (_, r, c) = receipts(&f);
                let bytes = if restored { r } else { c };
                std::fs::write(journal(&f), &bytes).unwrap();
                install(&f, &mut |_| Ok(())).unwrap();
                f.assert_installed();
                assert_eq!(std::fs::read(journal(&f)).unwrap(), bytes);
            }
        }
    }
    #[test]
    #[serial_test::serial]
    fn original_no_native_restore_holds_selector_lock_through_actual_finish() {
        let f = Fixture::new(plain());
        let store = crate::app_store::tauri_store_path().unwrap();
        assert!(!journal(&f).exists());
        let mut phases = Vec::new();
        let idle_epoch = f.db.secrets.lifecycle_epoch();
        install(&f, &mut |phase| {
            let expected = if phase == Checkpoint::Staged {
                idle_epoch
            } else {
                idle_epoch + 1
            };
            assert_eq!(f.db.secrets.lifecycle_epoch(), expected);
            assert!(isolated_scope::lock_selection_at(&store).is_err());
            let test = format!(
                "{}::child_selector_is_busy",
                module_path!().split_once("::").unwrap().1
            );
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", &test])
                .env("LP_CONTENT_CONSUMER_STORE", &store)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            phases.push(phase);
            Ok(())
        })
        .unwrap();
        assert!(phases.contains(&Checkpoint::Staged));
        assert!(phases.contains(&Checkpoint::Database));
        assert!(phases.contains(&Checkpoint::Metadata));
        assert_eq!(f.db.secrets.lifecycle_epoch(), idle_epoch + 1);
        assert!(f.db.secrets.ensure_available().is_ok());
        f.assert_installed();
        assert!(!journal(&f).exists());
        assert!(isolated_scope::lock_selection_at(&store).is_ok());
    }
    #[test]
    #[ignore]
    fn child_selector_is_busy() {
        let path = std::env::var_os("LP_CONTENT_CONSUMER_STORE")
            .expect("parent fixture sets selector path");
        assert!(isolated_scope::lock_selection_at(Path::new(&path)).is_err());
    }
    #[test]
    #[serial_test::serial]
    fn selector_change_after_staging_blocks_intent_and_preserves_original_database() {
        let f = Fixture::new(plain());
        let before = digest(&f);
        let skills = tree(&f.db.secrets.root().join("skills"));
        let store = crate::app_store::tauri_store_path().unwrap();
        assert!(install(&f, &mut |phase| {
            if phase == Checkpoint::Staged {
                std::fs::write(&store, b"{\"syntheticForeignSelector\":true}").unwrap();
            }
            Ok(())
        })
        .is_err());
        assert_eq!(digest(&f), before);
        assert_eq!(tree(&f.db.secrets.root().join("skills")), skills);
        assert!(pending_record(f.db.secrets.root()).unwrap().is_none());
        assert!(f.db.secrets.ensure_available().is_ok());
        assert_eq!(
            std::fs::read(&store).unwrap(),
            b"{\"syntheticForeignSelector\":true}"
        );
    }
    #[test]
    #[serial_test::serial]
    fn native_phase_change_after_staging_is_not_only_checked_at_constructor() {
        let f = Fixture::new(plain());
        let (prepared, _, _) = receipts(&f);
        let before = digest(&f);
        assert!(install(&f, &mut |phase| {
            if phase == Checkpoint::Staged {
                std::fs::write(journal(&f), &prepared).unwrap();
            }
            Ok(())
        })
        .is_err());
        assert_eq!(digest(&f), before);
        assert_eq!(std::fs::read(journal(&f)).unwrap(), prepared);
        assert!(pending_record(f.db.secrets.root()).unwrap().is_none());
        assert!(f.db.secrets.ensure_available().is_ok());
    }
    #[test]
    #[serial_test::serial]
    fn foreign_epoch_changes_at_real_staging_or_intent_hook_block_database_publication() {
        for boundary in [Checkpoint::Staged, Checkpoint::Intent] {
            let f = Fixture::new(plain());
            let before = digest(&f);
            assert!(install(&f, &mut |point| {
                if point == boundary {
                    f.db.secrets.set_blocked(true);
                }
                Ok(())
            })
            .is_err());
            assert_eq!(digest(&f), before);
            assert_eq!(
                pending_record(f.db.secrets.root()).unwrap().is_some(),
                boundary == Checkpoint::Intent
            );
        }
    }
    fn leave_selector_lock_from_exited_process(f: &Fixture) -> (PathBuf, Vec<u8>) {
        let store = crate::app_store::tauri_store_path().unwrap();
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "app_store::isolated_scope::tests::content_source_owner::child_leave_owned_selector_lock"])
            .env("LP_CONTENT_DEAD_LOCK", &store).output().unwrap();
        assert_eq!(out.status.code(), Some(77));
        let lock = store.parent().unwrap().join("app_paths.selection.lock");
        let owner = std::fs::read_dir(lock)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = std::fs::read(&owner).unwrap();
        assert_eq!(f.db.secrets.root(), crate::config::get_app_config_dir());
        (owner, before)
    }
    #[test]
    #[serial_test::serial]
    fn every_original_pending_owner_blocks_before_dead_selector_lock_reclamation() {
        for kind in [
            "transition",
            "reset",
            "bootstrap",
            "rewrap",
            "mode",
            "zcode",
            "zcode-key",
            "upgrade",
            "generation",
            "native",
        ] {
            let f = Fixture::new(plain());
            let root = f.db.secrets.root();
            let device = crate::live::engine::DeviceStore::for_device();
            let (owner, owner_before) = leave_selector_lock_from_exited_process(&f);
            let before = digest(&f);
            let store = crate::app_store::tauri_store_path().unwrap();
            let path = match kind {
                "transition" => root.join(INTENT),
                "reset" => root.parent().unwrap().join(format!(
                    ".{}-vault-reset",
                    root.file_name().unwrap().to_str().unwrap()
                )),
                "bootstrap" => root.join(".vault-bootstrap"),
                "rewrap" => root.join(".vault-rewrap"),
                "mode" => device
                    .root()
                    .join(crate::secrets::owned_file::DEVICE_STATE_FILE),
                "zcode" => root.join(crate::secrets::owned_file::JOURNAL_FILE),
                "zcode-key" => root.join(crate::secrets::owned_file::KEY_INTENT_FILE),
                "upgrade" => device
                    .root()
                    .join(crate::secrets::owned_file::UPGRADE_CHECKPOINT_FILE),
                "generation" => store
                    .parent()
                    .unwrap()
                    .join("isolated-configs/unknown-candidate/sentinel"),
                "native" => journal(&f),
                _ => unreachable!(),
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"unknown pending synthetic artifact").unwrap();
            assert!(
                install(&f, &mut |_| panic!("pending must refuse before staging")).is_err(),
                "{kind}"
            );
            assert_eq!(digest(&f), before);
            assert_eq!(std::fs::read(&owner).unwrap(), owner_before, "{kind}");
            assert_eq!(
                std::fs::read(&path).unwrap(),
                b"unknown pending synthetic artifact"
            );
        }
    }
}
