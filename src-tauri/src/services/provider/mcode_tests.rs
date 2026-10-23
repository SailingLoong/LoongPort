//! Full-crate regressions. Every native path is an isolated fixture, never a real app.
use super::mcode;
use crate::{database::Database, mcode_config, provider::Provider, store::AppState};
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    state: AppState,
    _home: crate::secrets::testing::TestHome,
    previous: Option<std::ffi::OsString>,
}
impl Fixture {
    fn new() -> Self {
        let home = crate::secrets::testing::TestHome::new().unwrap();
        let previous = std::env::var_os("MINIMAX_DATA_DIR");
        std::env::set_var("MINIMAX_DATA_DIR", home.path().join("native"));
        let db = Arc::new(Database::memory().unwrap());
        Self {
            state: AppState::new(db).unwrap(),
            _home: home,
            previous,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("MINIMAX_DATA_DIR", value),
            None => std::env::remove_var("MINIMAX_DATA_DIR"),
        }
    }
}
fn provider() -> Provider {
    Provider::with_id(
        "custom".into(),
        "Custom".into(),
        json!({
            "kind":"custom", "options":{"baseURL":"https://example.test","apiKey":"fixture-secret"}, "models":{"model":{}}
        }),
        None,
    )
}
#[test]
#[serial_test::serial]
fn list_and_editor_are_read_only_until_explicit_import() {
    let fixture = Fixture::new();
    let state = &fixture.state;
    std::fs::create_dir_all(mcode_config::data_dir()).unwrap();
    let config = provider().settings_config;
    let document = json!({"custom_provider":{"custom":config}});
    let bytes = serde_yaml::to_string(&document).unwrap();
    std::fs::write(mcode_config::config_path(), &bytes).unwrap();
    assert!(mcode::list(state).unwrap().is_empty());
    assert!(state.db.get_all_providers("mcode").unwrap().is_empty());
    assert_eq!(mcode::import_from_live(state).unwrap(), 1);
    let row = state
        .db
        .get_provider_by_id("custom", "mcode")
        .unwrap()
        .unwrap();
    assert!(row.settings_config.get("name").is_none());
    let edit = mcode::edit_settings(state, "custom").unwrap();
    assert_eq!(edit["settingsConfig"], config);
    assert_eq!(
        edit["mcodeConfigRevision"],
        mcode_config::current_revision().unwrap()
    );
    assert!(edit["modeState"].is_null());
    assert_eq!(
        std::fs::read_to_string(mcode_config::config_path()).unwrap(),
        bytes
    );
    let reread = state
        .db
        .get_provider_by_id("custom", "mcode")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(row).unwrap(),
        serde_json::to_value(reread).unwrap()
    );
}
#[test]
#[serial_test::serial]
fn edits_reject_stale_revision_and_backup_restore_is_reviewed() {
    let fixture = Fixture::new();
    let state = &fixture.state;
    mcode::add(state, provider(), true).unwrap();
    let original = std::fs::read(mcode_config::config_path()).unwrap();
    let mut edited = mcode::list(state).unwrap().shift_remove("custom").unwrap();
    edited.name = "Edited".into();
    mcode::update(state, None, edited.clone()).unwrap();
    assert!(mcode::update(state, None, edited).is_err());
    let backup = mcode::latest_backup(state).unwrap().unwrap();
    let encrypted = std::fs::read(
        state
            .db
            .secrets
            .root()
            .join("backups/mcode")
            .join(&backup.id),
    )
    .unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("fixture-secret"));
    mcode::restore_backup(state, &backup.id, &backup.revision).unwrap();
    assert_eq!(
        std::fs::read(mcode_config::config_path()).unwrap(),
        original
    );
    assert!(mcode::restore_backup(state, &backup.id, &backup.revision).is_err());
}
#[test]
#[serial_test::serial]
fn invalid_late_import_does_not_partially_save_earlier_rows() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(mcode_config::data_dir()).unwrap();
    let config = provider().settings_config;
    let text = serde_yaml::to_string(&json!({"custom_provider":{
        "a-valid":config.clone(),"loongport-0123456789abcdef":config
    }}))
    .unwrap();
    std::fs::write(mcode_config::config_path(), text).unwrap();
    assert!(mcode::import_from_live(&fixture.state).is_err());
    assert!(fixture
        .state
        .db
        .get_all_providers("mcode")
        .unwrap()
        .is_empty());
}

#[test]
#[serial_test::serial]
fn explicitly_removed_editor_options_are_not_restored_on_save() {
    let fixture = Fixture::new();
    let state = &fixture.state;
    let mut initial = provider();
    initial.settings_config["opaque_native_extension"] = json!({"keep": true});
    initial.settings_config["options"]["extra"] = json!("delete me");
    initial.settings_config["models"]["model"] = json!({
        "limit": {"context": 1024}, "options": {"reasoningEffort":"high"}, "extra": false
    });
    mcode::add(state, initial, true).unwrap();
    let mut edited = mcode::list(state).unwrap().shift_remove("custom").unwrap();
    edited
        .settings_config
        .as_object_mut()
        .unwrap()
        .remove("opaque_native_extension");
    edited.settings_config["options"]
        .as_object_mut()
        .unwrap()
        .remove("extra");
    edited.settings_config["models"]["model"] = json!({});
    mcode::update(state, None, edited).unwrap();
    let native = &mcode_config::get_providers().unwrap()["custom"];
    assert!(native["options"].get("extra").is_none());
    assert_eq!(native["models"]["model"], json!({}));
    assert_eq!(native["opaque_native_extension"], json!({"keep":true}));
}

#[test]
#[serial_test::serial]
fn invalid_reserved_option_types_never_reach_native_or_catalog() {
    let fixture = Fixture::new();
    let state = &fixture.state;
    mcode::add(state, provider(), true).unwrap();
    let before = std::fs::read(mcode_config::config_path()).unwrap();
    let saved =
        serde_json::to_value(state.db.get_provider_by_id("custom", "mcode").unwrap()).unwrap();
    for key in ["apiKey", "baseURL"] {
        for value in [json!(123), json!(false), json!(null), json!({})] {
            let mut edited = mcode::list(state).unwrap().shift_remove("custom").unwrap();
            edited.settings_config["options"][key] = value;
            assert!(mcode::update(state, None, edited).is_err());
            assert_eq!(std::fs::read(mcode_config::config_path()).unwrap(), before);
            assert_eq!(
                serde_json::to_value(state.db.get_provider_by_id("custom", "mcode").unwrap())
                    .unwrap(),
                saved
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn rollback_receipt_and_unauthenticated_file_do_not_hide_matching_backup() {
    let fixture = Fixture::new();
    let state = &fixture.state;
    mcode::add(state, provider(), true).unwrap();
    let original = std::fs::read(mcode_config::config_path()).unwrap();
    let mut edited = mcode::list(state).unwrap().shift_remove("custom").unwrap();
    edited.name = "Second".into();
    mcode::update(state, None, edited).unwrap();
    let current = std::fs::read(mcode_config::config_path()).unwrap();
    let matching = mcode::latest_backup(state).unwrap().unwrap();
    // Real SQLite rejects the catalog commit only after the native transaction
    // created its encrypted receipt and wrote the candidate config.
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_mcode_insert BEFORE INSERT ON providers
         WHEN NEW.app_type='mcode' BEGIN SELECT RAISE(FAIL, 'fixture write failure'); END;
         CREATE TRIGGER fail_mcode_update BEFORE UPDATE ON providers
         WHEN NEW.app_type='mcode' BEGIN SELECT RAISE(FAIL, 'fixture write failure'); END;",
        )
        .unwrap();
    let mut rejected = mcode::list(state).unwrap().shift_remove("custom").unwrap();
    rejected.name = "Rejected third".into();
    assert!(mcode::update(state, None, rejected).is_err());
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_mcode_insert; DROP TRIGGER fail_mcode_update;")
        .unwrap();
    assert_eq!(std::fs::read(mcode_config::config_path()).unwrap(), current);
    let backup_dir = state.db.secrets.root().join("backups/mcode");
    assert_eq!(std::fs::read_dir(&backup_dir).unwrap().count(), 3);
    let offered = mcode::latest_backup(state).unwrap().unwrap();
    assert_eq!(offered.id, matching.id);
    // A later registry-shaped but unauthenticated file must never be offered,
    // and must not conceal the older authenticated receipt either.
    std::fs::write(
        backup_dir.join("mcode_999999999999999999999999999999_1.json"),
        b"not an authenticated backup",
    )
    .unwrap();
    let offered = mcode::latest_backup(state).unwrap().unwrap();
    assert_eq!(offered.id, matching.id);
    mcode::restore_backup(state, &offered.id, &offered.revision).unwrap();
    assert_eq!(
        std::fs::read(mcode_config::config_path()).unwrap(),
        original
    );
}
