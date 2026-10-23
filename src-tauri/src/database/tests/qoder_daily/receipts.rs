use crate::database::dao::qoder_campaign_operations as receipts;
fn create_unknown(f: &Fixture, db: &Database) -> daily::Operation {
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(db, "synthetic-provider", &target(), &g).unwrap();
    let _ = f;
    db.qoder_operations(&identity("synthetic-provider"))
        .unwrap()
        .remove(0)
}
#[test]
fn restore_helper_preserves_only_local_operation_facts() {
    let live = Fixture::new();
    let db = live.open();
    create_unknown(&live, &db);
    let incoming = Fixture::new();
    let other = incoming.open();
    create_unknown(&incoming, &other);
    let before = receipts::read_rows(&db.conn.lock().unwrap()).unwrap();
    assert_ne!(
        before,
        receipts::read_rows(&other.conn.lock().unwrap()).unwrap()
    );
    receipts::preserve_local(&db.conn.lock().unwrap(), &other.conn.lock().unwrap()).unwrap();
    assert_eq!(
        before,
        receipts::read_rows(&other.conn.lock().unwrap()).unwrap()
    );
}
#[test]
fn empty_local_owner_does_not_adopt_foreign_receipts() {
    let live = Fixture::new();
    let db = live.open();
    let incoming = Fixture::new();
    let other = incoming.open();
    create_unknown(&incoming, &other);
    receipts::preserve_local(&db.conn.lock().unwrap(), &other.conn.lock().unwrap()).unwrap();
    assert!(receipts::read_rows(&other.conn.lock().unwrap())
        .unwrap()
        .is_empty());
}
#[test]
fn installed_v26_missing_restore_target_table_is_not_repaired() {
    let live = Fixture::new();
    let db = live.open();
    let other = Connection::open_in_memory().unwrap();
    other.execute_batch("CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER);INSERT INTO loongport_schema_version VALUES(1,26)").unwrap();
    assert!(receipts::preserve_local(&db.conn.lock().unwrap(), &other).is_err());
    assert!(other
        .prepare("SELECT * FROM qoder_campaign_operations")
        .is_err());
}
#[test]
fn weakened_unique_constraint_or_trigger_is_rejected() {
    let f = Fixture::new();
    let db = f.open();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("CREATE INDEX unauthorized_index ON qoder_campaign_operations(subject)")
        .unwrap();
    assert!(receipts::read_rows(&conn).is_err());
    let f = Fixture::new();
    let db = f.open();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("CREATE TRIGGER qoder_mutation AFTER INSERT ON qoder_campaign_operations BEGIN DELETE FROM qoder_campaign_operations;END").unwrap();
    assert!(receipts::read_rows(&conn).is_err());
}
#[test]
fn expiry_pending_and_receipt_rollback_together_on_provider_write_failure() {
    let f = Fixture::new();
    let db = f.open();
    create_unknown(&f, &db);
    let before = state(&f, &db);
    let rows = receipts::read_rows(&db.conn.lock().unwrap()).unwrap();
    db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_pending BEFORE UPDATE OF meta ON providers BEGIN SELECT RAISE(ABORT,'fixture write failure');END").unwrap();
    assert!(db
        .retire_qoder_operations(&identity("synthetic-provider"), 200)
        .is_err());
    assert_eq!(state(&f, &db), before);
    assert_eq!(receipts::read_rows(&db.conn.lock().unwrap()).unwrap(), rows);
}
#[test]
fn old_receipt_expiry_never_clears_newer_pending_attempt() {
    let f = Fixture::new();
    let db = f.open();
    let old = create_unknown(&f, &db);
    db.finish_qoder_operation(&old, Outcome::ClaimedDeltaUnknown, None)
        .unwrap();
    let p = f.load(&db);
    let generation = state(&f, &db).generation;
    let new = daily::Operation {
        id: "newer-attempt".into(),
        generation: generation.clone(),
        binding: BoundCampaign::from_operation(
            identity(&p.id),
            "new-campaign".into(),
            "new-key".into(),
            200,
            300,
        )
        .unwrap(),
        outcome: Outcome::Unknown,
    };
    db.save_qoder_operation(&p, &new).unwrap();
    db.retire_qoder_operations(&identity(&p.id), 200).unwrap();
    let owned = state(&f, &db);
    assert_eq!(owned.generation, generation);
    assert_eq!(owned.pending.unwrap().id, new.id);
    let rows = db.qoder_operations(&identity(&p.id)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, new.id);
}
#[test]
fn statement_failure_rolls_back_both_intent_and_provider_reference() {
    let f = Fixture::new();
    let db = f.open();
    db.save_provider("codex", &f.load(&db)).unwrap();
    db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_reference BEFORE UPDATE OF meta ON providers BEGIN SELECT RAISE(ABORT,'fixture failure');END").unwrap();
    let g = Gateway::new();
    assert!(daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).is_err());
    assert!(db
        .qoder_operations(&identity("synthetic-provider"))
        .unwrap()
        .is_empty());
    assert!(state(&f, &db).pending.is_none());
    assert_eq!(g.posts.load(Ordering::SeqCst), 0);
}
#[test]
fn date_parser_rejects_local_style_bad_weekday_or_cached_observations() {
    let c = VerifiedContext::synthetic(identity("synthetic-provider"), "generation".into(), true);
    for (date, age) in [
        ("1970-01-01T00:02:30Z", None),
        ("Fri, 01 Jan 1970 00:02:30 GMT", None),
        ("Thu, 01 Jan 1970 00:02:30 +0800", None),
        ("Thu, 01 Jan 1970 00:02:30 GMT", Some("3")),
    ] {
        assert!(daily::clock::synthetic(date, age, &c).is_none());
    }
    let observation = daily::clock::synthetic("Thu, 01 Jan 1970 00:02:30 GMT", None, &c).unwrap();
    let other = VerifiedContext::synthetic(
        identity("synthetic-provider"),
        "new-generation".into(),
        true,
    );
    assert!(observation.for_context(&other).is_none());
    assert_eq!(observation.for_context(&c), Some(150));
}
#[test]
fn actual_auth_precedence_and_all_inline_paths_rotate_only_selected_material() {
    let candidates = [
        serde_json::json!({"auth":{"OPENAI_API_KEY":"candidate-auth"}}),
        serde_json::json!({"apiKey":"candidate-root"}),
        serde_json::json!({"config":{"api_key":"candidate-object"}}),
        serde_json::json!({"config":"experimental_bearer_token='candidate-experimental'"}),
        serde_json::json!({"config":"[models]\ndefault='fixture'\n[model.fixture]\nmodel='m'\nbase_url='https://example.invalid'\nname='f'\napi_key='candidate-grok-inline'\napi_backend='openai'\ncontext_window=1000\n"}),
    ];
    let f = Fixture::new();
    let db = f.open();
    db.save_provider("codex", &f.load(&db)).unwrap();
    for config in candidates {
        let previous = state(&f, &db).generation;
        db.update_provider_settings_config("codex", "synthetic-provider", &config)
            .unwrap();
        assert_ne!(state(&f, &db).generation, previous);
        let current = state(&f, &db).generation;
        let mut cosmetic = config.clone();
        cosmetic["model"] = serde_json::json!("new-model");
        db.update_provider_settings_config("codex", "synthetic-provider", &cosmetic)
            .unwrap();
        assert_eq!(state(&f, &db).generation, current);
    }
}

#[test]
fn restore_cannot_resurrect_backend_generation_or_incoming_pending() {
    let f = Fixture::new();
    let db = f.open();
    let old = create_unknown(&f, &db);
    let old_state = state(&f, &db);
    let mut b = f.load(&db);
    b.settings_config["env"]["OPENAI_API_KEY"] = serde_json::json!("synthetic-B");
    db.save_provider("codex", &b).unwrap();
    let current = state(&f, &db);
    assert_ne!(current.generation, old_state.generation);
    let incoming = Fixture::new();
    let target = incoming.open();
    create_unknown(&incoming, &target);
    let live_vault = f.session.read().unwrap();
    let target_vault = incoming.session.read().unwrap();
    receipts::preserve_local(&db.conn.lock().unwrap(), &target.conn.lock().unwrap()).unwrap();
    Database::preserve_qoder_states_on_restore(
        &db.conn.lock().unwrap(),
        &target.conn.lock().unwrap(),
        &live_vault,
        &target_vault,
    )
    .unwrap();
    let restored = state(&incoming, &target);
    assert_ne!(restored.generation, current.generation);
    assert_ne!(restored.generation, old_state.generation);
    assert_eq!(restored.pending.unwrap().id, old.id);
}
#[test]
fn restore_same_candidate_keeps_current_state_not_foreign_metadata() {
    let f = Fixture::new();
    let db = f.open();
    create_unknown(&f, &db);
    let current = state(&f, &db);
    let incoming = Fixture::new();
    let target = incoming.open();
    create_unknown(&incoming, &target);
    let live_vault = f.session.read().unwrap();
    let target_vault = incoming.session.read().unwrap();
    receipts::preserve_local(&db.conn.lock().unwrap(), &target.conn.lock().unwrap()).unwrap();
    Database::preserve_qoder_states_on_restore(
        &db.conn.lock().unwrap(),
        &target.conn.lock().unwrap(),
        &live_vault,
        &target_vault,
    )
    .unwrap();
    let restored = state(&incoming, &target);
    assert_eq!(restored.generation, current.generation);
    assert_eq!(restored.pending, current.pending);
    assert!(!restored.automatic.enabled);
    assert!(restored.automatic.revision > current.automatic.revision);
}
#[test]
fn existing_mode_saved_row_digest_accepts_backend_generation_normalization() {
    let f = Fixture::new();
    let db = f.open();
    let target = f.load(&db);
    let expected = digest(&target);
    f.save(&db, &target, &expected).unwrap();
    assert_eq!(digest(&f.load(&db)), digest(&target));
    let before = f.load(&db);
    let expected = digest(&before);
    let mut target = before.clone();
    target.settings_config["env"]["OPENAI_API_KEY"] =
        serde_json::json!("synthetic-mode-key-change");
    f.save(&db, &target, &expected).unwrap();
    assert_eq!(digest(&f.load(&db)), digest(&target));
}
#[test]
fn monotonic_elapsed_time_never_reuses_old_date_at_window_edge() {
    let c = VerifiedContext::synthetic(identity("synthetic-provider"), "generation".into(), true);
    let observed = daily::clock::synthetic("Thu, 01 Jan 1970 00:02:30 GMT", None, &c).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(observed.for_context(&c).unwrap() >= 151);
}
#[test]
fn capacity_exhaustion_never_commits_an_unreadable_ledger_or_posts() {
    let f = Fixture::new();
    let db = f.open();
    {
        let mut c = db.conn.lock().unwrap();
        let tx = c.transaction().unwrap();
        for n in 0..4096 {
            tx.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result) VALUES(?1,?2,'international','campaign','key',100,200,'generation','claimed')",params![format!("attempt-{n}"),format!("subject-{n}")]).unwrap();
        }
        tx.commit().unwrap();
    }
    let g = Gateway::new();
    assert!(daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).is_err());
    assert_eq!(g.posts.load(Ordering::SeqCst), 0);
    assert_eq!(
        receipts::read_rows(&db.conn.lock().unwrap()).unwrap().len(),
        4096
    );
}
#[test]
fn oversized_binding_is_rejected_before_durable_commit() {
    let f = Fixture::new();
    let db = f.open();
    db.save_provider("codex", &f.load(&db)).unwrap();
    let p = f.load(&db);
    let operation = daily::Operation {
        id: "oversized-attempt".into(),
        generation: state(&f, &db).generation,
        binding: BoundCampaign::from_operation(
            identity(&p.id),
            "campaign".into(),
            "k".repeat(513),
            100,
            200,
        )
        .unwrap(),
        outcome: Outcome::Unknown,
    };
    assert!(db.save_qoder_operation(&p, &operation).is_err());
    assert!(receipts::read_rows(&db.conn.lock().unwrap())
        .unwrap()
        .is_empty());
}
#[test]
fn expiry_then_extended_same_campaign_never_posts_twice() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.now.store(200, Ordering::SeqCst);
    g.end.store(300, Ordering::SeqCst);
    let _ = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g);
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn expired_unknown_get_then_extended_old_campaign_never_reposts() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.now.store(200, Ordering::SeqCst);
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.end.store(300, Ordering::SeqCst);
    let _ = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g);
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn rolling_receipt_replaces_one_window_and_rejects_old_start_resurrection() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.start.store(200, Ordering::SeqCst);
    g.end.store(300, Ordering::SeqCst);
    g.now.store(250, Ordering::SeqCst);
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Observed {
            action: Action::Post,
            added_credits: Some(100),
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 2);
    let rows = db
        .qoder_operations(&identity("synthetic-provider"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].binding.start_at(), 200);
    assert_eq!(state(&f, &db).pending.unwrap().id, rows[0].id);
    g.start.store(100, Ordering::SeqCst);
    g.end.store(400, Ordering::SeqCst);
    g.now.store(260, Ordering::SeqCst);
    assert!(daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).is_err());
    assert_eq!(g.posts.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .binding
            .start_at(),
        200
    );
}
#[test]
fn unknown_cross_window_requires_get_then_separate_explicit_prepare() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.start.store(200, Ordering::SeqCst);
    g.end.store(300, Ordering::SeqCst);
    g.now.store(250, Ordering::SeqCst);
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Observed {
            action: Action::Get,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Observed {
            action: Action::Post,
            outcome: Outcome::Unknown,
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 2);
    let rows = db
        .qoder_operations(&identity("synthetic-provider"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].binding.start_at(), 200);
}
#[test]
fn unresolved_get_cannot_discard_unknown_for_new_window() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    g.lose_get = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.start.store(200, Ordering::SeqCst);
    g.end.store(300, Ordering::SeqCst);
    g.now.store(250, Ordering::SeqCst);
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .binding
            .start_at(),
        100
    );
}
#[test]
fn failed_window_replacement_restores_old_receipt_and_pending_atomically() {
    let f = Fixture::new();
    let db = f.open();
    let old = create_unknown(&f, &db);
    db.finish_qoder_operation(&old, Outcome::ClaimedDeltaUnknown, None)
        .unwrap();
    let before = state(&f, &db);
    let p = f.load(&db);
    let new = daily::Operation {
        id: "next-window".into(),
        generation: before.generation.clone(),
        binding: BoundCampaign::from_operation(
            identity(&p.id),
            "next-campaign".into(),
            "key".into(),
            200,
            300,
        )
        .unwrap(),
        outcome: Outcome::Unknown,
    };
    db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_replacement BEFORE UPDATE OF meta ON providers BEGIN SELECT RAISE(ABORT,'fixture failure');END").unwrap();
    assert!(db.save_qoder_operation(&p, &new).is_err());
    assert_eq!(state(&f, &db), before);
    assert_eq!(db.qoder_operations(&identity(&p.id)).unwrap()[0].id, old.id);
}
#[test]
fn stale_old_window_response_cannot_overwrite_latest_receipt() {
    let f = Fixture::new();
    let db = f.open();
    let old = create_unknown(&f, &db);
    db.finish_qoder_operation(&old, Outcome::ClaimedDeltaUnknown, None)
        .unwrap();
    let p = f.load(&db);
    let new = daily::Operation {
        id: "next-window".into(),
        generation: state(&f, &db).generation,
        binding: BoundCampaign::from_operation(
            identity(&p.id),
            "next-campaign".into(),
            "key".into(),
            200,
            300,
        )
        .unwrap(),
        outcome: Outcome::Unknown,
    };
    db.save_qoder_operation(&p, &new).unwrap();
    assert!(db
        .finish_qoder_operation(&old, Outcome::NewlyClaimed, Some(350))
        .is_err());
    assert_eq!(db.qoder_operations(&identity(&p.id)).unwrap()[0].id, new.id);
    assert_eq!(state(&f, &db).pending.unwrap().id, new.id);
}
#[test]
fn unknown_inserted_during_listing_is_get_only_not_already_claimed() {
    struct Race {
        other: Database,
        inner: Gateway,
    }
    impl CampaignAdapter for Race {
        fn verify(&self, p: &Provider, g: &str) -> Result<VerifiedContext, Dependency> {
            self.inner.verify(p, g)
        }
        fn list(&self, c: &VerifiedContext) -> Result<Listing, error::AppError> {
            let p = self
                .other
                .get_provider_by_id("synthetic-provider", "codex")?
                .unwrap();
            let generation = p
                .meta
                .as_ref()
                .unwrap()
                .qoder_campaign_state
                .as_ref()
                .unwrap()
                .generation
                .clone();
            let op = daily::Operation {
                id: "racing-unknown".into(),
                generation,
                binding: BoundCampaign::from_operation(
                    identity(&p.id),
                    target().campaign_id,
                    target().campaign_key,
                    100,
                    200,
                )
                .unwrap(),
                outcome: Outcome::Unknown,
            };
            self.other.save_qoder_operation(&p, &op)?;
            self.inner.list(c)
        }
        fn post(
            &self,
            c: &VerifiedContext,
            b: &BoundCampaign,
        ) -> Result<Response, error::AppError> {
            self.inner.post(c, b)
        }
        fn get_reward(
            &self,
            c: &VerifiedContext,
            b: &BoundCampaign,
        ) -> Result<Response, error::AppError> {
            self.inner.get_reward(c, b)
        }
    }
    let f = Fixture::new();
    let db = f.open();
    let mut inner = Gateway::new();
    inner.lose_get = true;
    let g = Race {
        other: f.open(),
        inner,
    };
    let result = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        result,
        ClaimResult::Observed {
            action: Action::Get,
            outcome: Outcome::Unknown,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(g.inner.posts.load(Ordering::SeqCst), 0);
    assert_eq!(g.inner.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn real_credential_reset_walker_clears_provider_values_but_keeps_latest_receipt() {
    let f = Fixture::new();
    let db = f.open();
    create_unknown(&f, &db);
    let target = VaultContext::generate().unwrap();
    let conn = db.conn.lock().unwrap();
    let before = receipts::read_rows(&conn).unwrap();
    assert!(secrets::inventory::reset_database(&conn, &target).unwrap() > 0);
    assert_eq!(receipts::read_rows(&conn).unwrap(), before);
    for column in ["settings_config", "meta"] {
        let sealed: String = conn
            .query_row(
                &format!("SELECT {column} FROM providers WHERE id='synthetic-provider'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            secrets::inventory::open_db(
                &target,
                "providers",
                column,
                &["synthetic-provider", "codex"],
                &sealed
            )
            .unwrap(),
            "{}"
        );
    }
}
#[test]
fn restore_wrong_current_or_target_vault_cannot_mutate_live_or_target() {
    let f = Fixture::new();
    let db = f.open();
    create_unknown(&f, &db);
    let incoming = Fixture::new();
    let target = incoming.open();
    create_unknown(&incoming, &target);
    let current_vault = f.session.read().unwrap();
    let target_vault = incoming.session.read().unwrap();
    let wrong = VaultContext::generate().unwrap();
    receipts::preserve_local(&db.conn.lock().unwrap(), &target.conn.lock().unwrap()).unwrap();
    let before = state(&incoming, &target);
    let live = state(&f, &db);
    assert!(Database::preserve_qoder_states_on_restore(
        &db.conn.lock().unwrap(),
        &target.conn.lock().unwrap(),
        &wrong,
        &target_vault
    )
    .is_err());
    assert!(Database::preserve_qoder_states_on_restore(
        &db.conn.lock().unwrap(),
        &target.conn.lock().unwrap(),
        &current_vault,
        &wrong
    )
    .is_err());
    assert_eq!(state(&incoming, &target), before);
    assert_eq!(state(&f, &db), live);
}

#[test]
fn cross_window_get_response_names_actual_old_attempt_and_window() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    let old = db
        .qoder_operations(&identity("synthetic-provider"))
        .unwrap()
        .remove(0);
    g.start.store(200, Ordering::SeqCst);
    g.end.store(300, Ordering::SeqCst);
    g.now.store(250, Ordering::SeqCst);
    let ClaimResult::Observed {
        operation,
        action: Action::Get,
        ..
    } = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap()
    else {
        panic!("must reconcile old attempt")
    };
    assert_eq!(operation.attempt_id, old.id);
    assert_eq!(operation.start_at, 100);
    assert_eq!(operation.end_at, 200);
    assert_eq!(operation.campaign_id, old.binding.target().campaign_id);
}
