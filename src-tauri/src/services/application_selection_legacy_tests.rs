//! Synthetic source17 fixtures exercise the original legacy owner, never a user's files.
use super::*;

struct LegacyFixture {
    _home: crate::secrets::testing::TestHome,
    state: AppState,
}
impl LegacyFixture {
    fn new() -> Self {
        let home = crate::secrets::testing::TestHome::new().unwrap();
        let root = crate::config::get_app_config_dir();
        assert!(root.starts_with(home.path()));
        let store = crate::secrets::testing::MemoryKeyStore::default();
        let session = crate::SecretSession::open(&root, &store, None).unwrap();
        let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
        crate::Database::create_tables_on_conn(&conn).unwrap();
        crate::Database::apply_upgrade_source_migrations_on_conn(&conn).unwrap();
        crate::database::loongport_schema::apply(&conn).unwrap();
        crate::database::vault::stamp(&conn, session.read().unwrap().encrypted().unwrap()).unwrap();
        session.complete_migration().unwrap();
        crate::settings::unlock_settings_for_test(session.clone()).unwrap();
        let db = std::sync::Arc::new(crate::Database::from_connection(conn, session));
        let mut settings = crate::settings::get_settings();
        settings.claude_config_dir =
            Some(home.path().join(".claude").to_string_lossy().into_owned());
        crate::settings::update_settings(settings).unwrap();
        let provider = crate::provider::Provider::with_id(
            "custom".into(),
            "Synthetic".into(),
            serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://relay.example","ANTHROPIC_AUTH_TOKEN":"fixture-key","ANTHROPIC_MODEL":"model-one"},"modelCatalog":{"models":[{"model":"model-one"},{"model":"model-two"}]}}),
            None,
        );
        db.save_provider("claude", &provider).unwrap();
        db.set_current_provider("claude", "custom").unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some("custom")).unwrap();
        crate::services::provider::write_standalone_live_snapshot(&AppType::Claude, &provider)
            .unwrap();
        crate::proxy::auto_strategy::set_model_pref(&db, "claude", Some("model-one")).unwrap();
        Self {
            _home: home,
            state: AppState::new(db).unwrap(),
        }
    }
    fn change(&self) -> ApplicationRoutingChange {
        ApplicationRoutingChange {
            order: None,
            selection: Some(TierSelection {
                provider_id: "custom".into(),
                model: Some("model-two".into()),
            }),
            catalog_decision: None,
            catalog_preview_only: false,
        }
    }
    fn review(&self, change: &ApplicationRoutingChange) -> LegacyRoutingReview {
        match prepare_routing_strategy(&self.state, &AppType::Claude, change, "synthetic-request")
            .unwrap()
        {
            PreparedRoutingRequest::Legacy { review } => review,
            _ => panic!("source17 must use the original legacy owner"),
        }
    }
    fn live(&self) -> Vec<u8> {
        std::fs::read(crate::config::get_claude_settings_path()).unwrap()
    }
}

#[test]
#[serial_test::serial]
fn legacy_preparation_and_original_owner_succeed_without_native_receipt() {
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    let view =
        crate::mode::current::read_application_view(&f.state.proxy_service, &AppType::Claude)
            .unwrap();
    assert_eq!(view.write_protocol, Some("legacy"));
    assert!(view.can_write);
    assert!(view.mode.is_none());
    apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review).unwrap();
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-two")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&f.live()).unwrap()["env"]["ANTHROPIC_MODEL"],
        "model-two"
    );
}

#[test]
#[serial_test::serial]
fn legacy_same_schema_stale_provider_review_cannot_write() {
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let mut provider = f
        .state
        .db
        .get_provider_by_id("custom", "claude")
        .unwrap()
        .unwrap();
    provider.settings_config["env"]["ANTHROPIC_BASE_URL"] =
        serde_json::json!("https://changed.example");
    f.state.db.save_provider("claude", &provider).unwrap();
    let before = f.live();
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    assert!(apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review).is_err());
    assert_eq!(before, f.live());
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
}

#[test]
#[serial_test::serial]
fn legacy_review_rejects_changed_draft_before_write() {
    let f = LegacyFixture::new();
    let mut change = f.change();
    let review = f.review(&change);
    change.selection.as_mut().unwrap().model = Some("model-one".into());
    let before = f.live();
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    assert!(apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review).is_err());
    assert_eq!(before, f.live());
}

#[test]
#[serial_test::serial]
fn legacy_to_native_schema_change_cannot_fall_through_to_modern_write() {
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    // Actual synthetic migration, not a version-flag flip or a user's database.
    crate::Database::apply_schema_migrations_on_conn(&f.state.db.conn.lock().unwrap()).unwrap();
    assert!(crate::mode::operation::uses_upstream4_schema(&f.state.db).unwrap());
    let before = f.live();
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    assert!(apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review).is_err());
    assert_eq!(before, f.live());
}

#[test]
#[serial_test::serial]
fn pending_checkpoint_blocks_legacy_preparation_read_and_apply() {
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let device = crate::live::engine::DeviceStore::for_device();
    std::fs::create_dir_all(device.root()).unwrap();
    std::fs::write(
        device
            .root()
            .join(crate::secrets::upgrade::checkpoint::FILE),
        b"synthetic-pending-checkpoint",
    )
    .unwrap();
    let before = f.live();
    assert!(prepare_routing_strategy(&f.state, &AppType::Claude, &change, "second").is_err());
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    let view =
        crate::mode::current::read_application_view(&f.state.proxy_service, &AppType::Claude)
            .unwrap();
    assert!(!view.can_write);
    assert_eq!(view.write_protocol, None);
    assert!(apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review).is_err());
    assert_eq!(before, f.live());
}

#[test]
#[serial_test::serial]
fn cancelling_a_legacy_review_is_read_only() {
    let f = LegacyFixture::new();
    let before = f.live();
    let change = f.change();
    let _review = f.review(&change);
    // Discarding a review invokes no backend mutation and creates no receipt.
    assert_eq!(before, f.live());
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
}

#[test]
#[serial_test::serial]
fn legacy_same_bytes_at_a_different_native_target_rejects_old_review() {
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let before = f.live();
    let next = f._home.path().join("other-claude");
    std::fs::create_dir_all(&next).unwrap();
    let mut settings = crate::settings::get_settings();
    settings.claude_config_dir = Some(next.to_string_lossy().into_owned());
    crate::settings::update_settings(settings).unwrap();
    std::fs::write(crate::config::get_claude_settings_path(), &before).unwrap();
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    assert!(apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review).is_err());
    assert_eq!(before, f.live());
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn legacy_rechecks_reviewed_native_bytes_after_synthetic_quit() {
    use crate::relay::chatgpt_app::{self, QuitOutcome};
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    verify_legacy_routing_review(&f.state, &AppType::Claude, &change, &review).unwrap();
    let mut flushed: serde_json::Value = serde_json::from_slice(&f.live()).unwrap();
    flushed["syntheticQuitFlush"] = serde_json::json!(true);
    let flushed = serde_json::to_vec_pretty(&flushed).unwrap();
    let relaunched = std::cell::Cell::new(false);
    let result = chatgpt_app::around_with(
        false,
        || {
            std::fs::write(crate::config::get_claude_settings_path(), &flushed).unwrap();
            QuitOutcome::Quit
        },
        || {
            relaunched.set(true);
            Ok(())
        },
        || apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review),
    );
    assert!(result.is_err());
    assert!(
        relaunched.get(),
        "original around seam restores the synthetic app"
    );
    assert_eq!(
        flushed,
        f.live(),
        "a stale review must preserve the newer external write"
    );
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
}

#[test]
#[serial_test::serial]
fn legacy_original_transaction_restores_provider_pointers_model_and_native_on_commit_failure() {
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let before_live = f.live();
    let before_provider = f
        .state
        .db
        .get_provider_by_id("custom", "claude")
        .unwrap()
        .unwrap();
    let before_current = f.state.db.get_current_provider("claude").unwrap();
    let before_local = crate::settings::get_current_provider_ready(&AppType::Claude).unwrap();
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    verify_legacy_routing_review(&f.state, &AppType::Claude, &change, &review).unwrap();
    let error = select_legacy_locked_with_commit(
        &f.state,
        &AppType::Claude,
        change.selection.as_ref().unwrap(),
        || Err(AppError::Config("synthetic commit rejected".into())),
    )
    .unwrap_err();
    assert!(error.to_string().contains("synthetic commit rejected"));
    assert_eq!(before_live, f.live());
    assert_eq!(
        before_provider.settings_config,
        f.state
            .db
            .get_provider_by_id("custom", "claude")
            .unwrap()
            .unwrap()
            .settings_config
    );
    assert_eq!(
        before_current,
        f.state.db.get_current_provider("claude").unwrap()
    );
    assert_eq!(
        before_local,
        crate::settings::get_current_provider_ready(&AppType::Claude).unwrap()
    );
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
}

#[test]
#[serial_test::serial]
fn legacy_cross_provider_commit_failure_restores_both_rows_pointers_model_and_native() {
    let f = LegacyFixture::new();
    let incoming = crate::provider::Provider::with_id(
        "incoming".into(),
        "Synthetic incoming".into(),
        serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://incoming.example","ANTHROPIC_AUTH_TOKEN":"incoming-fixture-key","ANTHROPIC_MODEL":"incoming-default"},"modelCatalog":{"models":[{"model":"incoming-default"},{"model":"incoming-selected"}]}}),
        None,
    );
    f.state.db.save_provider("claude", &incoming).unwrap();
    let mut change = f.change();
    change.selection = Some(TierSelection {
        provider_id: "incoming".into(),
        model: Some("incoming-selected".into()),
    });
    let review = f.review(&change);
    let row = |id: &str| {
        serde_json::to_value(
            f.state
                .db
                .get_provider_by_id(id, "claude")
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    };
    let before_outgoing = row("custom");
    let before_incoming = row("incoming");
    let before_live = f.live();
    assert_eq!(
        f.state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("custom")
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Claude)
            .unwrap()
            .as_deref(),
        Some("custom")
    );
    let read_journal = || {
        let vault = f.state.db.secret_session().read().unwrap();
        crate::mode::state::load_app(
            &crate::live::engine::DeviceStore::for_device(),
            &vault,
            "claude",
        )
        .unwrap()
    };
    let before_journal = serde_json::to_value(read_journal()).unwrap();
    let reached_commit = std::cell::Cell::new(false);
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    verify_legacy_routing_review(&f.state, &AppType::Claude, &change, &review).unwrap();
    let error = select_legacy_locked_with_commit(
        &f.state,
        &AppType::Claude,
        change.selection.as_ref().unwrap(),
        || {
            // Prove the failure happens after an actual A -> B switch, not before
            // mutation or after editing only A's model.
            assert_eq!(
                f.state
                    .db
                    .get_current_provider("claude")
                    .unwrap()
                    .as_deref(),
                Some("incoming")
            );
            assert_eq!(
                crate::settings::get_current_provider_ready(&AppType::Claude)
                    .unwrap()
                    .as_deref(),
                Some("incoming")
            );
            assert_eq!(
                crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
                    .unwrap()
                    .as_deref(),
                Some("incoming-selected")
            );
            assert_eq!(
                row("incoming")["settingsConfig"]["env"]["ANTHROPIC_MODEL"],
                "incoming-selected"
            );
            let live: serde_json::Value = serde_json::from_slice(&f.live()).unwrap();
            assert_eq!(
                live["env"]["ANTHROPIC_BASE_URL"],
                "https://incoming.example"
            );
            assert_eq!(live["env"]["ANTHROPIC_MODEL"], "incoming-selected");
            assert_ne!(before_live, f.live());
            reached_commit.set(true);
            Err(AppError::Config(
                "synthetic cross-provider commit rejected".into(),
            ))
        },
    )
    .unwrap_err();
    assert!(reached_commit.get());
    assert!(error
        .to_string()
        .contains("synthetic cross-provider commit rejected"));
    assert_eq!(f.live(), before_live);
    assert_eq!(row("custom"), before_outgoing);
    assert_eq!(row("incoming"), before_incoming);
    assert_eq!(
        f.state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("custom")
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Claude)
            .unwrap()
            .as_deref(),
        Some("custom")
    );
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
    let journal = read_journal();
    if let Some(entry) = journal.apps.get("claude") {
        assert!(entry.pending.is_none());
        assert!(entry.last_save.is_none());
    }
    assert_eq!(serde_json::to_value(journal).unwrap(), before_journal);
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn legacy_user_declined_quit_never_invokes_configuration_action() {
    use crate::relay::chatgpt_app::{self, QuitOutcome};
    let f = LegacyFixture::new();
    let change = f.change();
    let review = f.review(&change);
    let before = f.live();
    let called = std::cell::Cell::new(false);
    let _lock = futures::executor::block_on(f.state.proxy_service.lock_switch_for_app("claude"));
    verify_legacy_routing_review(&f.state, &AppType::Claude, &change, &review).unwrap();
    let result = chatgpt_app::around_with(
        false,
        || QuitOutcome::UserDeclined,
        || panic!("declined quit must not relaunch"),
        || {
            called.set(true);
            apply_legacy_routing_locked(&f.state, &AppType::Claude, &change, &review)
        },
    );
    assert!(result.is_err());
    assert!(!called.get());
    assert_eq!(before, f.live());
    assert_eq!(
        crate::proxy::auto_strategy::get_model_pref_checked(&f.state.db, "claude")
            .unwrap()
            .as_deref(),
        Some("model-one")
    );
}
