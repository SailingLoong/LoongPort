use crate::{Database, Provider};
use serde_json::json;
use std::sync::{Arc, Barrier};

fn provider(id: &str, name: &str, key: &str, model: &str) -> Provider {
    Provider::with_id(
        id.into(),
        name.into(),
        json!({"env":{"ANTHROPIC_AUTH_TOKEN":key,"ANTHROPIC_MODEL":model}}),
        Some("https://relay.example".into()),
    )
}

#[test]
fn selected_prepare_inserts_only_local_record_and_preserves_application_routing() {
    let db = Database::memory().unwrap();
    let mut selected = provider(
        "loongport-0011223344556677",
        "Selected",
        "fixture-new-key",
        "selected-model",
    );
    selected.available_models = Some(vec!["selected-model".into(), "other-model".into()]);
    db.set_setting("application_priority_claude", r#"["existing-managed"]"#)
        .unwrap();
    db.set_setting("application_blocked_claude", r#"["blocked-managed"]"#)
        .unwrap();
    let before_current = db.get_current_provider("claude").unwrap();
    assert!(db
        .prepare_provider_if_absent("claude", &selected, Some(0.7), None)
        .unwrap());
    let stored = db
        .get_provider_by_id(&selected.id, "claude")
        .unwrap()
        .unwrap();
    assert_eq!(stored.settings_config, selected.settings_config);
    assert_eq!(stored.available_models, selected.available_models);
    assert_eq!(
        db.get_tier_rate_multiplier("claude", &selected.id).unwrap(),
        Some(0.7)
    );
    assert_eq!(db.get_current_provider("claude").unwrap(), before_current);
    assert_eq!(
        db.get_setting("application_priority_claude")
            .unwrap()
            .as_deref(),
        Some(r#"["existing-managed"]"#)
    );
    assert_eq!(
        db.get_setting("application_blocked_claude")
            .unwrap()
            .as_deref(),
        Some(r#"["blocked-managed"]"#)
    );
    assert!(db
        .get_provider_by_id(&selected.id, "codex")
        .unwrap()
        .is_none());
}

#[test]
fn selected_prepare_does_not_join_an_uninitialized_effective_chain() {
    let db = Database::memory().unwrap();
    let original = provider(
        "ordinary-existing",
        "Existing",
        "fixture-old-key",
        "old-model",
    );
    db.save_provider("claude", &original).unwrap();
    assert!(db
        .get_setting("application_priority_claude")
        .unwrap()
        .is_none());
    let before = crate::proxy::application_routing::chain_ids(&db, "claude").unwrap();
    assert_eq!(before, vec!["ordinary-existing"]);
    let selected = provider(
        "loongport-0011223344556677",
        "Selected",
        "fixture-new-key",
        "new-model",
    );
    assert!(db
        .prepare_provider_if_absent("claude", &selected, Some(0.7), None)
        .unwrap());
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&db, "claude").unwrap(),
        before
    );
    // A later routing initialization must not turn preparation into consent.
    crate::proxy::application_routing::migrate(&db, "claude").unwrap();
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&db, "claude").unwrap(),
        before
    );
}

#[test]
fn selected_prepare_reuses_existing_provider_without_mutating_any_effective_field() {
    let db = Database::memory().unwrap();
    let existing = provider(
        "loongport-0011223344556677",
        "Original edited",
        "fixture-old-key",
        "user-model",
    );
    db.save_provider("claude", &existing).unwrap();
    db.set_current_provider("claude", &existing.id).unwrap();
    db.set_user_edited("claude", &existing.id, true).unwrap();
    db.set_tier_rate_multiplier("claude", &existing.id, Some(0.5))
        .unwrap();
    db.set_available_models(
        "claude",
        &existing.id,
        &["user-model".into(), "legacy-model".into()],
    )
    .unwrap();
    db.set_setting(
        "application_priority_claude",
        r#"["loongport-0011223344556677"]"#,
    )
    .unwrap();
    let before = db
        .get_provider_by_id(&existing.id, "claude")
        .unwrap()
        .unwrap();
    let replacement = provider(
        &existing.id,
        "Default renamed",
        "fixture-replacement-key",
        "default-model",
    );
    assert!(!db
        .prepare_provider_if_absent("claude", &replacement, Some(9.0), None)
        .unwrap());
    let after = db
        .get_provider_by_id(&existing.id, "claude")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    // available_models intentionally does not serialize; compare separately.
    assert_eq!(after.available_models, before.available_models);
    assert!(db.get_user_edited("claude", &existing.id).unwrap());
    assert_eq!(
        db.get_tier_rate_multiplier("claude", &existing.id).unwrap(),
        Some(0.5)
    );
    assert_eq!(
        db.get_current_provider("claude").unwrap().as_deref(),
        Some(existing.id.as_str())
    );
    assert_eq!(
        db.get_setting("application_priority_claude")
            .unwrap()
            .as_deref(),
        Some(r#"["loongport-0011223344556677"]"#)
    );
}

#[test]
fn concurrent_selected_prepares_across_connections_keep_first_committed_provider() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("selected-prepare.sqlite");
    let session = crate::secrets::session::SecretSession::ephemeral().unwrap();
    let first =
        Database::from_connection(rusqlite::Connection::open(&path).unwrap(), session.clone());
    first.create_tables().unwrap();
    {
        let context = session.read().unwrap();
        let conn = first.conn.lock().unwrap();
        crate::database::loongport_schema::apply(&conn).unwrap();
        crate::database::storage::stamp(&conn, &context).unwrap();
    }
    let second = Database::from_connection(rusqlite::Connection::open(&path).unwrap(), session);
    for db in [&first, &second] {
        db.conn
            .lock()
            .unwrap()
            .busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = [
        (first, "First", "fixture-first-key", "model-first"),
        (second, "Second", "fixture-second-key", "model-second"),
    ]
    .into_iter()
    .map(|(db, name, key, model)| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let next = provider("loongport-0011223344556677", name, key, model);
            barrier.wait();
            let inserted = db
                .prepare_provider_if_absent("claude", &next, None, None)
                .unwrap();
            (inserted, next, db)
        })
    })
    .collect();
    barrier.wait();
    let receipts: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        receipts.iter().filter(|(inserted, _, _)| *inserted).count(),
        1
    );
    let winner = &receipts
        .iter()
        .find(|(inserted, _, _)| *inserted)
        .unwrap()
        .1;
    let db = &receipts[0].2;
    let stored = db
        .get_provider_by_id(&winner.id, "claude")
        .unwrap()
        .unwrap();
    assert_eq!(stored.name, winner.name);
    assert_eq!(stored.settings_config, winner.settings_config);
    assert_eq!(db.get_all_providers("claude").unwrap().len(), 1);
}

#[test]
fn explicit_maintenance_keeps_ordinary_save_semantics() {
    let db = Database::memory().unwrap();
    let before = provider("ordinary", "Before", "fixture-old-key", "before-model");
    db.save_provider("claude", &before).unwrap();
    db.set_current_provider("claude", &before.id).unwrap();
    let changed = provider("ordinary", "After", "fixture-new-key", "after-model");
    db.save_provider("claude", &changed).unwrap();
    assert_eq!(
        db.get_provider_by_id("ordinary", "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        changed.settings_config
    );
    assert_eq!(
        db.get_current_provider("claude").unwrap().as_deref(),
        Some("ordinary")
    );
}

#[test]
fn selected_initial_metadata_failure_rolls_back_provider_and_chain() {
    let db = Database::memory().unwrap();
    db.conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_initial_rate BEFORE UPDATE OF tier_rate_multiplier ON providers
         BEGIN SELECT RAISE(ABORT, 'fixture initial rate failure'); END;",
        )
        .unwrap();
    let selected = provider(
        "loongport-0011223344556677",
        "Selected",
        "fixture-key",
        "model",
    );
    let error = db
        .prepare_provider_if_absent("claude", &selected, Some(0.7), None)
        .unwrap_err();
    assert!(error.to_string().contains("fixture initial rate failure"));
    assert!(db
        .get_provider_by_id(&selected.id, "claude")
        .unwrap()
        .is_none());
    assert!(db
        .get_setting("application_priority_claude")
        .unwrap()
        .is_none());
}
