//! Actual Claude/Grok ProviderService flows under the existing synthetic home,
//! database and in-memory credential-store fixtures. No native accounts are used.
use super::*;
use crate::live::engine::{DeviceStore, LiveFile};
use crate::mode::operation::{self, failpoint, RecoveryOutcome};
use crate::mode::state::{self, Mode};
use crate::secrets::testing::TestHome;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

struct Fixture {
    state: AppState,
    app: AppType,
    file: PathBuf,
    _home: TestHome,
}

fn grok_row(id: &str) -> String {
    format!("[models]\ndefault = \"{id}\"\n\n[model.{id}]\nmodel = \"model-{id}\"\nname = \"synthetic-{id}\"\nbase_url = \"https://{id}.example.invalid/v1\"\napi_key = \"key-{id}\"\napi_backend = \"responses\"\ncontext_window = 200000\n")
}

impl Fixture {
    fn new(app: AppType) -> Self {
        Self::with_schema(app, true)
    }
    fn with_schema(app: AppType, modern: bool) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(crate::secrets::testing::initialize_database().unwrap());
        if modern {
            crate::database::Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap())
                .unwrap();
        }
        for id in ["a", "b"] {
            let settings = match app {
                AppType::Claude => {
                    let mut settings = json!({"env":{"ANTHROPIC_AUTH_TOKEN":format!("key-{id}"),"ANTHROPIC_BASE_URL":format!("https://{id}.example.invalid"),"ANTHROPIC_MODEL":format!("model-{id}")},"rowOnly":{"mustNotBeProjected":true}});
                    if id == "a" {
                        settings["env"]["CLAUDE_CODE_DISABLE_ARTIFACT"] = json!("1");
                    }
                    settings
                }
                AppType::GrokBuild => json!({"config":grok_row(id)}),
                _ => unreachable!(),
            };
            db.save_provider(
                app.as_str(),
                &Provider::with_id(id.into(), format!("synthetic-{id}"), settings, None),
            )
            .unwrap();
        }
        db.set_current_provider(app.as_str(), "a").unwrap();
        crate::settings::set_current_provider(&app, Some("a")).unwrap();
        let state = AppState::new(db).unwrap();
        let (file, bytes) = match app {
            AppType::Claude => (crate::config::get_claude_settings_path(), serde_json::to_vec_pretty(&json!({"env":{"ANTHROPIC_AUTH_TOKEN":"key-a","ANTHROPIC_BASE_URL":"https://a.example.invalid","ANTHROPIC_MODEL":"model-a","CLAUDE_CODE_DISABLE_ARTIFACT":"1","USER_ENV":"keep"},"userOnly":{"keep":true},"mcpServers":{"local":{"command":"synthetic"}},"hooks":{"Stop":[]}})).unwrap()),
            AppType::GrokBuild => (crate::grok_config::get_grok_config_path(), format!("# hand-written\n[ui]\ntheme = \"dark\"\n\n{}\n[model.mine]\nmodel = \"user-model\"\n\n[mcp_servers.local]\ncommand = \"synthetic\"\n", grok_row("a")).into_bytes()),
            _ => unreachable!(),
        };
        assert!(file.starts_with(home.path()));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        let vault = state.db.secret_session().read().unwrap();
        state::update(&DeviceStore::for_device(), &vault, |live| {
            live.apps.entry(app.as_str().into()).or_default().mode = Some(Mode::Direct);
            Ok(())
        })
        .unwrap();
        drop(vault);
        Self {
            _home: home,
            state,
            app,
            file,
        }
    }
    fn recover(&self) -> Result<Option<RecoveryOutcome>, AppError> {
        operation::recover_pending(&self.state, &self.app, &[LiveFile::private(&self.file)])
    }
    fn pending(&self) -> Option<state::Pending> {
        state::pending(
            &DeviceStore::for_device(),
            &self.state.db.secret_session().read().unwrap(),
            self.app.as_str(),
        )
        .unwrap()
    }
    fn assert_current(&self, id: &str) {
        assert_eq!(
            crate::settings::get_current_provider(&self.app).as_deref(),
            Some(id)
        );
        assert_eq!(
            self.state
                .db
                .get_current_provider(self.app.as_str())
                .unwrap()
                .as_deref(),
            Some(id)
        );
    }
    fn assert_target(&self) {
        let text = std::fs::read_to_string(&self.file).unwrap();
        match self.app {
            AppType::Claude => {
                let live: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(live["env"]["ANTHROPIC_AUTH_TOKEN"], "key-b");
                assert_eq!(live["env"]["ANTHROPIC_MODEL"], "model-b");
                assert!(live["env"].get("CLAUDE_CODE_DISABLE_ARTIFACT").is_none());
                assert_eq!(live["env"]["USER_ENV"], "keep");
                assert_eq!(live["userOnly"]["keep"], true);
                assert_eq!(live["mcpServers"]["local"]["command"], "synthetic");
                assert!(live.get("rowOnly").is_none());
                assert_eq!(live["hooks"]["Stop"], json!([]));
            }
            AppType::GrokBuild => {
                let live: toml_edit::DocumentMut = text.parse().unwrap();
                assert_eq!(live["models"]["default"].as_str(), Some("b"));
                assert!(live["model"].get("a").is_none());
                assert_eq!(live["model"]["mine"]["model"].as_str(), Some("user-model"));
                assert_eq!(
                    live["mcp_servers"]["local"]["command"].as_str(),
                    Some("synthetic")
                );
                assert!(text.starts_with("# hand-written\n[ui]\ntheme = \"dark\"\n"));
                let written = state::written(
                    &DeviceStore::for_device(),
                    &self.state.db.secret_session().read().unwrap(),
                    self.app.as_str(),
                )
                .unwrap()
                .unwrap();
                assert_eq!(written.tables, vec!["b"]);
            }
            _ => unreachable!(),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&self.file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

struct Fault;
impl Fault {
    fn at(point: &str) -> Self {
        failpoint::crash_at(Some(point));
        Self
    }
}
impl Drop for Fault {
    fn drop(&mut self) {
        failpoint::crash_at(None);
        failpoint::on_before_publish(None);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn migrated_switches_preserve_unowned_fields_and_commit_current() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        let fixture = Fixture::new(app.clone());
        let previous = fixture
            .state
            .db
            .get_provider_by_id("a", app.as_str())
            .unwrap()
            .unwrap()
            .settings_config;
        ProviderService::switch(&fixture.state, app.clone(), "b").unwrap();
        fixture.assert_current("b");
        fixture.assert_target();
        assert_eq!(
            fixture
                .state
                .db
                .get_provider_by_id("a", app.as_str())
                .unwrap()
                .unwrap()
                .settings_config,
            previous,
            "no live backfill"
        );
        assert!(fixture.pending().is_none());
        assert!(DeviceStore::for_device().first_write_backup_dir().exists());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn migrated_switches_honor_pending_fault_before_any_client_write() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        let fixture = Fixture::new(app.clone());
        let before = std::fs::read(&fixture.file).unwrap();
        {
            let _fault = Fault::at("pending");
            assert!(ProviderService::switch(&fixture.state, app, "b").is_err());
        }
        assert_eq!(std::fs::read(&fixture.file).unwrap(), before);
        fixture.assert_current("a");
        assert!(fixture.pending().is_some());
    }
}

#[cfg(feature = "test-hooks")]
pub(super) fn verify() -> Result<(), AppError> {
    failed_settings_reload_blocks_adopted_switches_before_publication();
    println!("PASS Claude/Grok retained settings failure blocks before publication");
    migrated_switches_honor_pending_fault_before_any_client_write();
    println!("PASS Claude/Grok pending intent precedes client write");
    migrated_switches_preserve_unowned_fields_and_commit_current();
    println!("PASS Claude/Grok preserve unowned fields and commit current/written");
    switched_faults_require_explicit_recovery_and_never_snapshot_rollback();
    println!("PASS Claude/Grok pending/first-publication/target recover explicitly");
    malformed_and_external_changes_preserve_bytes_and_current_barrier();
    println!("PASS Claude/Grok malformed input and external changes remain safe");
    missing_staging_retains_original_intent();
    println!("PASS Claude/Grok missing staging retains verification barrier");
    partial_target_owner_failure_recovers_settings_database_and_written();
    println!("PASS Claude/Grok real target failure recovers current/preference/written");
    unknown_mode_checkpoint_and_corrupt_legacy_backup_block_before_writes();
    println!("PASS Claude/Grok shared admission rejects unknown/blocked/corrupt state");
    official_defaults_and_grok_retirement_use_the_existing_owners();
    println!("PASS native official defaults and Grok written-table retirement");
    blocked_selection_and_source17_keep_their_existing_paths();
    println!("PASS blocked selection and genuine source17 legacy paths");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn switched_faults_require_explicit_recovery_and_never_snapshot_rollback() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        for point in ["pending", "published:0", "target"] {
            let fixture = Fixture::new(app.clone());
            let before = std::fs::read(&fixture.file).unwrap();
            {
                let _fault = Fault::at(point);
                assert!(ProviderService::switch(&fixture.state, app.clone(), "b").is_err());
            }
            assert!(fixture.pending().is_some());
            if point == "pending" {
                assert_eq!(std::fs::read(&fixture.file).unwrap(), before);
                fixture.assert_current("a");
            } else {
                assert!(std::fs::read_to_string(&fixture.file)
                    .unwrap()
                    .contains("key-b"));
            }
            assert!(ProviderService::switch(&fixture.state, app.clone(), "a").is_err());
            let result = fixture.recover().unwrap();
            if point == "pending" {
                assert_eq!(result, Some(RecoveryOutcome::Discarded));
                fixture.assert_current("a");
            } else {
                assert_eq!(result, Some(RecoveryOutcome::RolledForward));
                fixture.assert_current("b");
                fixture.assert_target();
            }
            assert!(fixture.pending().is_none());
            assert_eq!(fixture.recover().unwrap(), None);
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn malformed_and_external_changes_preserve_bytes_and_current_barrier() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        {
            let fixture = Fixture::new(app.clone());
            let broken = b"{ malformed\n[malformed";
            std::fs::write(&fixture.file, broken).unwrap();
            assert!(ProviderService::switch(&fixture.state, app.clone(), "b").is_err());
            assert_eq!(std::fs::read(&fixture.file).unwrap(), broken);
            fixture.assert_current("a");
            assert!(fixture.pending().is_none());
        }
        let fixture = Fixture::new(app.clone());
        {
            let _fault = Fault::at("published:0");
            assert!(ProviderService::switch(&fixture.state, app.clone(), "b").is_err());
        }
        let external = b"external newer client bytes";
        std::fs::write(&fixture.file, external).unwrap();
        assert!(matches!(
            fixture.recover().unwrap(),
            Some(RecoveryOutcome::VerificationRequired { .. })
        ));
        assert!(fixture.pending().is_some());
        fixture.assert_current("a");
        assert!(ProviderService::switch(&fixture.state, app, "a").is_err());
        assert_eq!(std::fs::read(&fixture.file).unwrap(), external);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn missing_staging_retains_original_intent() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        let fixture = Fixture::new(app.clone());
        let before = std::fs::read(&fixture.file).unwrap();
        {
            let _fault = Fault::at("marked");
            assert!(ProviderService::switch(&fixture.state, app, "b").is_err());
        }
        let pending = fixture.pending().unwrap();
        assert!(pending.published);
        std::fs::remove_file(pending.files[0].staged.as_ref().unwrap()).unwrap();
        assert!(matches!(
            fixture.recover().unwrap(),
            Some(RecoveryOutcome::VerificationRequired { .. })
        ));
        assert_eq!(std::fs::read(&fixture.file).unwrap(), before);
        fixture.assert_current("a");
        assert!(fixture.pending().is_some());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn partial_target_owner_failure_recovers_settings_database_and_written() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        let fixture = Fixture::new(app.clone());
        crate::proxy::auto_strategy::set_model_pref(
            &fixture.state.db,
            app.as_str(),
            Some("old-filter"),
        )
        .unwrap();
        fixture.state.db.conn.lock().unwrap().execute_batch(&format!("CREATE TRIGGER synthetic_current_failure BEFORE UPDATE OF is_current ON providers WHEN NEW.app_type='{}' AND NEW.id='b' AND NEW.is_current=1 BEGIN SELECT RAISE(ABORT, 'synthetic current failure'); END", app.as_str())).unwrap();
        assert!(ProviderService::switch(&fixture.state, app.clone(), "b").is_err());
        assert_eq!(
            crate::settings::get_current_provider(&app).as_deref(),
            Some("b")
        );
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider(app.as_str())
                .unwrap()
                .as_deref(),
            Some("a")
        );
        assert!(fixture.pending().unwrap().published);
        assert!(ProviderService::switch(&fixture.state, app.clone(), "a").is_err());
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER synthetic_current_failure")
            .unwrap();
        assert_eq!(
            fixture.recover().unwrap(),
            Some(RecoveryOutcome::RolledForward)
        );
        fixture.assert_current("b");
        fixture.assert_target();
        assert!(
            crate::proxy::auto_strategy::get_model_pref(&fixture.state.db, app.as_str()).is_none()
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn unknown_mode_checkpoint_and_corrupt_legacy_backup_block_before_writes() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        for reason in [
            "unknown",
            "checkpoint",
            "legacy",
            "future-schema",
            "self-schema",
        ] {
            let fixture = Fixture::new(app.clone());
            let store = DeviceStore::for_device();
            match reason {
                "unknown" => {
                    let vault = fixture.state.db.secret_session().read().unwrap();
                    state::update(&store, &vault, |live| {
                        live.apps.remove(app.as_str());
                        Ok(())
                    })
                    .unwrap();
                }
                "checkpoint" => std::fs::write(
                    store
                        .root()
                        .join(crate::secrets::owned_file::UPGRADE_CHECKPOINT_FILE),
                    b"unverified checkpoint",
                )
                .unwrap(),
                "legacy" => {
                    futures::executor::block_on(
                        fixture.state.db.save_live_backup(app.as_str(), "{}"),
                    )
                    .unwrap();
                    fixture.state.db.conn.lock().unwrap().execute("UPDATE proxy_live_backup SET original_config='corrupt-envelope' WHERE app_type=?1", [app.as_str()]).unwrap();
                }
                "future-schema" => fixture
                    .state
                    .db
                    .conn
                    .lock()
                    .unwrap()
                    .pragma_update(None, "user_version", 21)
                    .unwrap(),
                "self-schema" => {
                    fixture
                        .state
                        .db
                        .conn
                        .lock()
                        .unwrap()
                        .execute(
                            "UPDATE loongport_schema_version SET version=25 WHERE id=1",
                            [],
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = std::fs::read(&fixture.file).unwrap();
            assert!(
                ProviderService::switch(&fixture.state, app.clone(), "b").is_err(),
                "{} {reason}",
                app.as_str()
            );
            assert!(fixture.recover().is_err(), "{} {reason}", app.as_str());
            assert_eq!(std::fs::read(&fixture.file).unwrap(), before);
            fixture.assert_current("a");
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn official_defaults_and_grok_retirement_use_the_existing_owners() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        let fixture = Fixture::new(app.clone());
        // A stale device selection must resolve the existing database default
        // without rewriting settings as a read side effect.
        crate::settings::set_current_provider(&app, Some("missing-row")).unwrap();
        ProviderService::switch(&fixture.state, app.clone(), "b").unwrap();
        fixture.assert_target();
        if app == AppType::GrokBuild {
            let text = std::fs::read_to_string(&fixture.file)
                .unwrap()
                .replace("default = \"b\"", "default = \"builtin-client-choice\"");
            std::fs::write(&fixture.file, text).unwrap();
            ProviderService::switch(&fixture.state, app.clone(), "a").unwrap();
            let text = std::fs::read_to_string(&fixture.file).unwrap();
            let doc: toml_edit::DocumentMut = text.parse().unwrap();
            assert!(
                doc["model"].get("b").is_none(),
                "retire written table even after client changes default"
            );
            assert_eq!(doc["model"]["mine"]["model"].as_str(), Some("user-model"));
        }
        let mut official = Provider::with_id(
            "official".into(),
            "synthetic official".into(),
            json!({}),
            None,
        );
        official.category = Some("official".into());
        fixture
            .state
            .db
            .save_provider(app.as_str(), &official)
            .unwrap();
        ProviderService::switch(&fixture.state, app.clone(), "official").unwrap();
        fixture.assert_current("official");
        let text = std::fs::read_to_string(&fixture.file).unwrap();
        if app == AppType::Claude {
            let live: Value = serde_json::from_str(&text).unwrap();
            assert!(live["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
            assert_eq!(live["env"]["USER_ENV"], "keep");
        } else {
            let live: toml_edit::DocumentMut = text.parse().unwrap();
            assert!(live
                .get("models")
                .and_then(|models| models.get("default"))
                .is_none());
            assert!(live["model"].get("a").is_none());
            assert_eq!(live["model"]["mine"]["model"].as_str(), Some("user-model"));
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn blocked_selection_and_source17_keep_their_existing_paths() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        {
            let fixture = Fixture::new(app.clone());
            let before = std::fs::read(&fixture.file).unwrap();
            crate::proxy::application_routing::set_tier_blocked(
                &fixture.state.db,
                app.as_str(),
                "b",
                true,
            )
            .unwrap();
            assert!(ProviderService::switch(&fixture.state, app.clone(), "b").is_err());
            assert_eq!(std::fs::read(&fixture.file).unwrap(), before);
            fixture.assert_current("a");
        }
        let fixture = Fixture::with_schema(app.clone(), false);
        assert_eq!(
            crate::database::Database::get_user_version(&fixture.state.db.conn.lock().unwrap())
                .unwrap(),
            17
        );
        {
            let _fault = Fault::at("pending");
            ProviderService::switch(&fixture.state, app, "b").unwrap();
        }
        fixture.assert_current("b");
        assert!(fixture.pending().is_none());
        assert!(!DeviceStore::for_device().first_write_backup_dir().exists());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn failed_settings_reload_blocks_adopted_switches_before_publication() {
    for app in [AppType::Claude, AppType::GrokBuild] {
        let fixture = Fixture::new(app.clone());
        let settings_path = crate::settings::settings_path();
        let verified_settings = std::fs::read(&settings_path).unwrap();
        let file_before = std::fs::read(&fixture.file).unwrap();
        let journal_before = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        std::fs::write(&settings_path, b"corrupt settings envelope").unwrap();
        assert!(crate::settings::reload_settings().is_err());
        assert!(ProviderService::switch(&fixture.state, app, "b").is_err());
        assert_eq!(
            std::fs::read(&fixture.file).unwrap(),
            file_before,
            "known-unavailable settings must block before client publication"
        );
        fixture.assert_current("a");
        assert!(fixture.pending().is_none());
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal_before
        );
        assert!(!DeviceStore::for_device().first_write_backup_dir().exists());
        std::fs::write(&settings_path, verified_settings).unwrap();
        crate::settings::reload_settings().unwrap();
    }
}
