//! Synthetic tests call the existing ProviderService entry with the actual DB,
//! vault, settings owner and client files. No system credential store is used.

use super::*;
use crate::live::engine::DeviceStore;
use crate::mode::operation::{failpoint, RecoveryOutcome};
use crate::mode::state::{self, Mode};
use crate::secrets::testing::TestHome;
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    _home: TestHome,
    state: AppState,
    env: std::path::PathBuf,
    settings: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(crate::secrets::testing::initialize_database().unwrap());
        {
            let conn = db.conn.lock().unwrap();
            crate::database::Database::apply_upstream4_migrations_on_conn(&conn).unwrap();
        }
        for (id, key) in [("a", "old-key"), ("b", "new-key")] {
            let provider = Provider::with_id(
                id.into(),
                format!("synthetic-{id}"),
                json!({"env":{"GEMINI_API_KEY":key,"GOOGLE_GEMINI_BASE_URL":format!("https://{id}.example.invalid")},"config":{"model":{"name":format!("model-{id}")}}}),
                None,
            );
            db.save_provider("gemini", &provider).unwrap();
        }
        db.set_current_provider("gemini", "a").unwrap();
        crate::settings::set_current_provider(&AppType::Gemini, Some("a")).unwrap();
        let state = AppState::new(db).unwrap();
        let env = crate::gemini_config::get_gemini_env_path();
        let settings = crate::gemini_config::get_gemini_settings_path();
        assert!(env.starts_with(home.path()));
        assert!(settings.starts_with(home.path()));
        std::fs::create_dir_all(env.parent().unwrap()).unwrap();
        std::fs::write(
            &env,
            b"# hand-written\nGEMINI_API_KEY=old-key\nKEEP_ENV=keep\n",
        )
        .unwrap();
        std::fs::write(&settings, br#"{"custom":{"keep":true},"mcpServers":{"local":{"command":"synthetic"}},"model":{"name":"model-a"}}"#).unwrap();
        {
            let vault = state.db.secret_session().read().unwrap();
            state::update(&DeviceStore::for_device(), &vault, |live| {
                let app = live.apps.entry("gemini".into()).or_default();
                app.mode = Some(Mode::Direct);
                app.attached = false;
                Ok(())
            })
            .unwrap();
        }
        Self {
            _home: home,
            state,
            env,
            settings,
        }
    }

    fn assert_current(&self, id: &str) {
        assert_eq!(
            self.state
                .db
                .get_current_provider("gemini")
                .unwrap()
                .as_deref(),
            Some(id)
        );
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Gemini).as_deref(),
            Some(id)
        );
    }

    fn pending(&self) -> Option<state::Pending> {
        let vault = self.state.db.secret_session().read().unwrap();
        state::pending(&DeviceStore::for_device(), &vault, "gemini").unwrap()
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
fn migrated_gemini_switch_uses_upstream_patch_and_one_committed_target() {
    let fixture = Fixture::new();
    ProviderService::switch(&fixture.state, AppType::Gemini, "b").unwrap();
    fixture.assert_current("b");
    let env = std::fs::read_to_string(&fixture.env).unwrap();
    assert!(
        env.starts_with("# hand-written\n"),
        "switch must preserve unowned dotenv text"
    );
    assert!(env.contains("KEEP_ENV=keep\n"));
    assert!(env.contains("GEMINI_API_KEY=new-key"));
    let settings: Value =
        serde_json::from_slice(&std::fs::read(&fixture.settings).unwrap()).unwrap();
    assert_eq!(settings["custom"]["keep"], true);
    assert_eq!(settings["mcpServers"]["local"]["command"], "synthetic");
    assert_eq!(settings["model"]["name"], "model-b");
    let vault = fixture.state.db.secret_session().read().unwrap();
    let live = state::load(&DeviceStore::for_device(), &vault).unwrap();
    assert!(live.apps["gemini"].pending.is_none());
    assert_eq!(live.apps["gemini"].mode, Some(Mode::Direct));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn malformed_second_file_leaves_first_file_and_current_untouched() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.env).unwrap();
    std::fs::write(&fixture.settings, b"{ malformed").unwrap();
    assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
    assert_eq!(std::fs::read(&fixture.env).unwrap(), before);
    assert_eq!(std::fs::read(&fixture.settings).unwrap(), b"{ malformed");
    fixture.assert_current("a");
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn real_switch_faults_recover_the_original_operation_without_legacy_rollback() {
    for point in ["pending", "published:0", "target"] {
        let fixture = Fixture::new();
        let before_env = std::fs::read(&fixture.env).unwrap();
        let before_settings = std::fs::read(&fixture.settings).unwrap();
        {
            let _fault = Fault::at(point);
            assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
        }
        assert!(fixture.pending().is_some());
        if point == "pending" {
            assert_eq!(std::fs::read(&fixture.env).unwrap(), before_env);
            assert_eq!(std::fs::read(&fixture.settings).unwrap(), before_settings);
            fixture.assert_current("a");
        } else {
            assert!(
                std::fs::read_to_string(&fixture.env)
                    .unwrap()
                    .contains("GEMINI_API_KEY=new-key"),
                "legacy rollback must not overwrite the published file"
            );
        }
        // A fresh service call must report the pending operation instead of
        // treating the unknown outcome as permission to switch again.
        assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "a").is_err());
        let recovered = super::gemini_direct::recover_pending(&fixture.state).unwrap();
        if point == "pending" {
            assert_eq!(recovered, Some(RecoveryOutcome::Discarded));
            fixture.assert_current("a");
        } else {
            assert_eq!(recovered, Some(RecoveryOutcome::RolledForward));
            fixture.assert_current("b");
            let settings: Value =
                serde_json::from_slice(&std::fs::read(&fixture.settings).unwrap()).unwrap();
            assert_eq!(settings["model"]["name"], "model-b");
        }
        assert!(fixture.pending().is_none());
        let verified_env = std::fs::read(&fixture.env).unwrap();
        assert_eq!(
            super::gemini_direct::recover_pending(&fixture.state).unwrap(),
            None
        );
        assert_eq!(std::fs::read(&fixture.env).unwrap(), verified_env);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn external_change_retains_blocked_intent_and_never_reverts_newer_bytes() {
    let fixture = Fixture::new();
    {
        let _fault = Fault::at("published:0");
        assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
    }
    let external = b"# newer client configuration\nGEMINI_API_KEY=external-newer\n";
    std::fs::write(&fixture.env, external).unwrap();
    assert!(matches!(
        super::gemini_direct::recover_pending(&fixture.state).unwrap(),
        Some(RecoveryOutcome::VerificationRequired { .. })
    ));
    assert_eq!(std::fs::read(&fixture.env).unwrap(), external);
    fixture.assert_current("a");
    assert!(fixture.pending().is_some());
    assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "a").is_err());
    assert_eq!(std::fs::read(&fixture.env).unwrap(), external);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn missing_stage_remains_verification_required_without_committing_current() {
    let fixture = Fixture::new();
    {
        let _fault = Fault::at("published:0");
        assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
    }
    let pending = fixture.pending().unwrap();
    let staged = pending
        .files
        .iter()
        .find(|file| file.path == fixture.settings)
        .unwrap()
        .staged
        .as_ref()
        .unwrap();
    assert!(staged.starts_with(fixture._home.path()));
    std::fs::remove_file(staged).unwrap();
    assert!(matches!(
        super::gemini_direct::recover_pending(&fixture.state).unwrap(),
        Some(RecoveryOutcome::VerificationRequired { .. })
    ));
    fixture.assert_current("a");
    assert!(fixture.pending().is_some());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn checkpoint_and_unknown_mode_block_the_new_service_before_client_writes() {
    for checkpoint in [false, true] {
        let fixture = Fixture::new();
        let store = DeviceStore::for_device();
        if checkpoint {
            std::fs::write(
                store
                    .root()
                    .join(crate::secrets::owned_file::UPGRADE_CHECKPOINT_FILE),
                b"pending-even-if-corrupt",
            )
            .unwrap();
        } else {
            let vault = fixture.state.db.secret_session().read().unwrap();
            state::update(&store, &vault, |live| {
                live.apps.remove("gemini");
                Ok(())
            })
            .unwrap();
        }
        let before_env = std::fs::read(&fixture.env).unwrap();
        let before_settings = std::fs::read(&fixture.settings).unwrap();
        assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
        assert_eq!(std::fs::read(&fixture.env).unwrap(), before_env);
        assert_eq!(std::fs::read(&fixture.settings).unwrap(), before_settings);
        fixture.assert_current("a");
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    target_schema_stays_on_the_new_writer_after_startup_support_is_activated();
    println!("PASS schema20 keeps the new writer when ordinary support reaches20");
    corrupt_legacy_recovery_material_blocks_switch_and_explicit_recovery();
    println!("PASS corrupt legacy recovery material is refused before client writes");
    partial_real_target_commit_keeps_intent_until_settings_database_and_preference_agree();
    println!("PASS real target failures after settings and database commit recover");
    migrated_gemini_switch_uses_upstream_patch_and_one_committed_target();
    println!("PASS actual Gemini switch preserves fields and commits one current");
    malformed_second_file_leaves_first_file_and_current_untouched();
    println!("PASS malformed input has no partial write");
    real_switch_faults_recover_the_original_operation_without_legacy_rollback();
    println!("PASS pending/first-publication/target faults recover explicitly");
    external_change_retains_blocked_intent_and_never_reverts_newer_bytes();
    println!("PASS external change retains blocked intent");
    missing_stage_remains_verification_required_without_committing_current();
    println!("PASS missing stage does not commit current");
    checkpoint_and_unknown_mode_block_the_new_service_before_client_writes();
    println!("PASS checkpoint and unknown mode block client writes");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn partial_real_target_commit_keeps_intent_until_settings_database_and_preference_agree() {
    for fail_database in [true, false] {
        let fixture = Fixture::new();
        crate::proxy::auto_strategy::set_model_pref(
            &fixture.state.db,
            "gemini",
            Some("old-filter"),
        )
        .unwrap();
        let sql = if fail_database {
            "CREATE TRIGGER synthetic_target_failure BEFORE UPDATE OF is_current ON providers WHEN NEW.app_type='gemini' AND NEW.id='b' AND NEW.is_current=1 BEGIN SELECT RAISE(ABORT, 'synthetic current failure'); END".to_owned()
        } else {
            format!("CREATE TRIGGER synthetic_target_failure BEFORE INSERT ON settings WHEN NEW.key='{}gemini' BEGIN SELECT RAISE(ABORT, 'synthetic preference failure'); END", crate::proxy::auto_strategy::SETTING_MODEL_PREFIX)
        };
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch(&sql)
            .unwrap();
        assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
        // These are real persisted owners. A callback failure is a partial
        // outcome, not permission for the old rollback wrapper to erase intent.
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Gemini).as_deref(),
            Some("b")
        );
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider("gemini")
                .unwrap()
                .as_deref(),
            Some(if fail_database { "a" } else { "b" })
        );
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref(&fixture.state.db, "gemini").as_deref(),
            Some("old-filter")
        );
        assert!(fixture.pending().unwrap().published);
        assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "a").is_err());
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER synthetic_target_failure")
            .unwrap();
        assert_eq!(
            super::gemini_direct::recover_pending(&fixture.state).unwrap(),
            Some(RecoveryOutcome::RolledForward)
        );
        fixture.assert_current("b");
        assert!(crate::proxy::auto_strategy::get_model_pref(&fixture.state.db, "gemini").is_none());
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn corrupt_legacy_recovery_material_blocks_switch_and_explicit_recovery() {
    for pending in [false, true] {
        let fixture = Fixture::new();
        if pending {
            let _fault = Fault::at("published:0");
            assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
        }
        futures::executor::block_on(fixture.state.db.save_live_backup("gemini", "{}")).unwrap();
        fixture.state.db.conn.lock().unwrap().execute(
            "UPDATE proxy_live_backup SET original_config='corrupt-envelope' WHERE app_type='gemini'", [],
        ).unwrap();
        let env = std::fs::read(&fixture.env).unwrap();
        let settings = std::fs::read(&fixture.settings).unwrap();
        if pending {
            assert!(super::gemini_direct::recover_pending(&fixture.state).is_err());
            assert!(fixture.pending().is_some());
        } else {
            assert!(ProviderService::switch(&fixture.state, AppType::Gemini, "b").is_err());
            assert!(fixture.pending().is_none());
        }
        assert_eq!(std::fs::read(&fixture.env).unwrap(), env);
        assert_eq!(std::fs::read(&fixture.settings).unwrap(), settings);
        fixture.assert_current("a");
    }
}

#[cfg_attr(test, test)]
fn target_schema_stays_on_the_new_writer_after_startup_support_is_activated() {
    assert!(super::gemini_direct::uses_upstream4_version(20, 20).unwrap());
    assert!(super::gemini_direct::uses_upstream4_version(20, 17).unwrap());
    assert!(!super::gemini_direct::uses_upstream4_version(17, 17).unwrap());
    assert!(super::gemini_direct::uses_upstream4_version(21, 20).is_err());
}
