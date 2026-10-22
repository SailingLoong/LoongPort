use daily::automatic::{self, Admission, Lease, Request as AutomaticRequest};

fn automatic_request(f: &Fixture, db: &Database, enabled: bool) -> AutomaticRequest {
    let current = state(f, db);
    AutomaticRequest {
        provider_id: "synthetic-provider".into(),
        credential_generation: current.generation,
        expected_revision: current.automatic.revision,
        policy_version: automatic::POLICY_VERSION,
        enabled,
        source: enabled.then_some(daily::http::CredentialSource::InternationalQoderPat),
    }
}
fn automatic_enable(f: &Fixture, db: &Database, gateway: &dyn CampaignAdapter) -> Lease {
    let view = automatic::set_with_adapter(db, automatic_request(f, db, true), gateway).unwrap();
    assert!(view.enabled);
    Lease::from_provider(&f.load(db), db.secrets.lifecycle_epoch()).unwrap()
}

struct NoAutomaticNetwork;
impl CampaignAdapter for NoAutomaticNetwork {
    fn verify(&self, _: &Provider, _: &str) -> Result<VerifiedContext, Dependency> {
        panic!("unauthorized verification")
    }
    fn list(&self, _: &VerifiedContext) -> Result<Listing, error::AppError> {
        panic!("unauthorized listing")
    }
    fn post(&self, _: &VerifiedContext, _: &BoundCampaign) -> Result<Response, error::AppError> {
        panic!("unauthorized claim")
    }
    fn get_reward(
        &self,
        _: &VerifiedContext,
        _: &BoundCampaign,
    ) -> Result<Response, error::AppError> {
        panic!("unauthorized recovery")
    }
}

#[test]
fn automatic_requires_explicit_source_policy_and_current_revision_without_network() {
    let f = Fixture::new();
    let db = f.open();
    for change in 0..4 {
        let mut request = automatic_request(&f, &db, true);
        match change {
            0 => request.source = None,
            1 => request.policy_version += 1,
            2 => request.expected_revision += 1,
            _ => request.credential_generation.push_str("-stale"),
        }
        assert!(automatic::set_with_adapter(&db, request, &NoAutomaticNetwork).is_err());
    }
    assert!(Lease::from_provider(&f.load(&db), 0).is_err());
    automatic::tick(&db).unwrap(); // Default-off: no production adapter is created.
}

#[test]
fn automatic_enrollment_only_binds_identity_and_disable_requires_no_network() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    let lease = automatic_enable(&f, &db, &g);
    assert_eq!(lease.subject, "synthetic-official-subject");
    assert_eq!(g.posts.load(Ordering::SeqCst), 0);
    assert_eq!(g.gets.load(Ordering::SeqCst), 0);
    assert!(db
        .qoder_operations(&identity("synthetic-provider"))
        .unwrap()
        .is_empty());
    let out =
        automatic::set_with_adapter(&db, automatic_request(&f, &db, false), &NoAutomaticNetwork)
            .unwrap();
    assert!(!out.enabled);
    assert!(out.revision > lease.revision);
    assert!(automatic::run_with_adapter(&db, lease, &NoAutomaticNetwork).is_err());
}

#[test]
fn duplicate_subject_cannot_gain_two_automatic_owners() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    automatic_enable(&f, &db, &g);
    let mut duplicate = f.load(&db);
    duplicate.id = "duplicate-provider".into();
    db.save_provider("codex", &duplicate).unwrap();
    let duplicate = db
        .get_provider_by_id("duplicate-provider", "codex")
        .unwrap()
        .unwrap();
    let s = duplicate
        .meta
        .as_ref()
        .unwrap()
        .qoder_campaign_state
        .as_ref()
        .unwrap();
    assert!(!s.automatic.enabled); // Generic insertion ignores copied policy.
    let request = AutomaticRequest {
        provider_id: duplicate.id,
        credential_generation: s.generation.clone(),
        expected_revision: s.automatic.revision,
        policy_version: automatic::POLICY_VERSION,
        enabled: true,
        source: Some(daily::http::CredentialSource::InternationalQoderPat),
    };
    assert!(automatic::set_with_adapter(&db, request, &g).is_err());
    assert_eq!(
        db.get_all_providers("codex")
            .unwrap()
            .values()
            .filter(|p| automatic::active(p).is_some())
            .count(),
        1
    );
}

#[test]
fn automatic_credential_change_and_aba_revoke_old_consent() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    let lease = automatic_enable(&f, &db, &g);
    let original = f.load(&db).settings_config;
    let mut provider = f.load(&db);
    let expected = digest(&provider);
    provider.settings_config =
        serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-replacement"}});
    f.save(&db, &provider, &expected).unwrap();
    assert!(!state(&f, &db).automatic.enabled);
    let mut provider = f.load(&db);
    let expected = digest(&provider);
    provider.settings_config = original;
    f.save(&db, &provider, &expected).unwrap();
    assert!(!state(&f, &db).automatic.enabled);
    assert!(automatic::run_with_adapter(&db, lease, &NoAutomaticNetwork).is_err());
}

#[test]
fn automatic_lost_post_survives_restart_opt_out_and_next_window_without_repost() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    g.lose_get = true;
    let lease = automatic_enable(&f, &db, &g);
    let first = automatic::run_with_adapter(&db, lease, &g).unwrap();
    assert!(matches!(
        first,
        ClaimResult::Observed {
            action: Action::Post,
            outcome: Outcome::Unknown,
            ..
        }
    ));
    automatic::set_with_adapter(&db, automatic_request(&f, &db, false), &NoAutomaticNetwork)
        .unwrap();
    drop(db);
    let db = f.open();
    g.now.store(250, Ordering::SeqCst);
    g.start.store(200, Ordering::SeqCst);
    g.end.store(300, Ordering::SeqCst);
    let lease = automatic_enable(&f, &db, &g);
    for _ in 0..2 {
        assert!(matches!(
            automatic::run_with_adapter(&db, lease.clone(), &g).unwrap(),
            ClaimResult::Observed {
                action: Action::Get,
                outcome: Outcome::Unknown,
                ..
            }
        ));
    }
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
    assert_eq!(g.gets.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .binding
            .start_at(),
        100
    );
}

#[test]
fn automatic_delete_recreate_is_off_and_keeps_unknown_receipt() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    g.lose_get = true;
    let lease = automatic_enable(&f, &db, &g);
    automatic::run_with_adapter(&db, lease.clone(), &g).unwrap();
    let saved = f.load(&db);
    db.delete_provider("codex", &saved.id).unwrap();
    db.save_provider("codex", &saved).unwrap();
    assert!(!state(&f, &db).automatic.enabled);
    assert!(automatic::run_with_adapter(&db, lease, &NoAutomaticNetwork).is_err());
    let lease = automatic_enable(&f, &db, &g);
    assert!(matches!(
        automatic::run_with_adapter(&db, lease, &g).unwrap(),
        ClaimResult::Observed {
            action: Action::Get,
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}

struct RevokeAtDispatch<'a> {
    f: &'a Fixture,
    db: &'a Database,
    inner: Gateway,
    action: u8,
}
impl CampaignAdapter for RevokeAtDispatch<'_> {
    fn verify(&self, p: &Provider, generation: &str) -> Result<VerifiedContext, Dependency> {
        self.inner.verify(p, generation)
    }
    fn list(&self, context: &VerifiedContext) -> Result<Listing, error::AppError> {
        self.inner.list(context)
    }
    fn post(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, error::AppError> {
        self.inner.post(context, binding)
    }
    fn get_reward(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, error::AppError> {
        self.inner.get_reward(context, binding)
    }
    fn post_admitted(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
        observation: &daily::clock::Observation,
        admission: &Admission<'_>,
    ) -> Result<Response, error::AppError> {
        match self.action {
            0 => {
                automatic::set_with_adapter(
                    self.db,
                    automatic_request(self.f, self.db, false),
                    &NoAutomaticNetwork,
                )
                .unwrap();
            }
            1 => {
                self.db
                    .delete_provider("codex", "synthetic-provider")
                    .unwrap();
            }
            2 => {
                self.db.secrets.set_blocked(true);
                self.db.secrets.set_blocked(false);
            }
            _ => {
                let mut p = self.f.load(self.db);
                let old = digest(&p);
                p.settings_config =
                    serde_json::json!({"env":{"OPENAI_API_KEY":"changed-before-dispatch"}});
                self.f.save(self.db, &p, &old).unwrap();
            }
        }
        admission.dispatch_claim(context, binding, observation, || {
            self.inner.post(context, binding)
        })
    }
}

#[test]
fn automatic_final_dispatch_blocks_opt_out_delete_lock_and_credential_change() {
    for action in 0..4 {
        let f = Fixture::new();
        let db = f.open();
        let g = RevokeAtDispatch {
            f: &f,
            db: &db,
            inner: Gateway::new(),
            action,
        };
        let lease = automatic_enable(&f, &db, &g);
        let out = automatic::run_with_adapter(&db, lease, &g).unwrap();
        assert!(matches!(
            out,
            ClaimResult::Observed {
                outcome: Outcome::Unknown,
                ..
            }
        ));
        assert_eq!(g.inner.posts.load(Ordering::SeqCst), 0);
        assert_eq!(
            db.qoder_operations(&identity("synthetic-provider"))
                .unwrap()[0]
                .outcome,
            Outcome::Unknown
        );
    }
}

#[test]
fn automatic_stale_claim_snapshot_cannot_restore_consent_after_opt_out() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    automatic_enable(&f, &db, &g);
    let old = f.load(&db);
    automatic::set_with_adapter(&db, automatic_request(&f, &db, false), &NoAutomaticNetwork)
        .unwrap();
    let op = daily::Operation {
        id: uuid::Uuid::new_v4().to_string(),
        generation: old
            .meta
            .as_ref()
            .unwrap()
            .qoder_campaign_state
            .as_ref()
            .unwrap()
            .generation
            .clone(),
        binding: contract_binding(&identity("synthetic-provider")),
        outcome: Outcome::Unknown,
    };
    assert!(db.save_qoder_operation(&old, &op).is_err());
    assert!(!state(&f, &db).automatic.enabled);
}
fn contract_binding(identity: &Identity) -> BoundCampaign {
    BoundCampaign::from_operation(
        identity.clone(),
        target().campaign_id,
        target().campaign_key,
        100,
        200,
    )
    .unwrap()
}

#[test]
fn automatic_send_admission_covers_future_creation_and_releases_before_wait() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    let lease = automatic_enable(&f, &db, &g);
    let admission = Admission {
        db: &db,
        lease: lease.clone(),
    };
    let context = g.verify(&f.load(&db), &lease.generation).unwrap();
    let binding = contract_binding(&identity("synthetic-provider"));
    let observation =
        daily::clock::synthetic("Thu, 01 Jan 1970 00:02:30 GMT", None, &context).unwrap();
    let polls = AtomicUsize::new(0);
    crate::rt::block_on(admission.send(&context, &binding, &observation, || {
        std::future::poll_fn(|cx| {
            if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            } else {
                // This would deadlock if SQLite/vault guards survived the first poll.
                automatic::set_with_adapter(
                    &db,
                    automatic_request(&f, &db, false),
                    &NoAutomaticNetwork,
                )
                .unwrap();
                std::task::Poll::Ready(Ok(http_response("https://openapi.qoder.sh", 200, &[])))
            }
        })
    }))
    .unwrap();
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    let created = AtomicUsize::new(0);
    assert!(
        crate::rt::block_on(admission.send(&context, &binding, &observation, || {
            created.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(http_response("https://openapi.qoder.sh", 200, &[])))
        }))
        .is_err()
    );
    assert_eq!(created.load(Ordering::SeqCst), 0);
}

#[test]
fn automatic_restore_revokes_consent_and_preserves_the_original_unknown_receipt() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    automatic_enable(&f, &db, &g);
    let operation = create_unknown(&f, &db);
    let before = state(&f, &db);
    let incoming = Fixture::new();
    let target = incoming.open();
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
    let after = state(&incoming, &target);
    assert_eq!(after.generation, before.generation);
    assert!(!after.automatic.enabled);
    assert!(after.automatic.revision > before.automatic.revision);
    assert_eq!(
        target
            .qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .id,
        operation.id
    );
}

#[test]
fn automatic_and_manual_two_connection_race_share_one_post_winner() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    let lease = automatic_enable(&f, &db, &g);
    drop(db);
    g.lose_post = true;
    g.lose_get = true;
    g.barrier = Some(Arc::new(ListingRendezvous::default()));
    std::thread::scope(|scope| {
        let manual = scope
            .spawn(|| daily::prepare_with_adapter(&f.open(), "synthetic-provider", &target(), &g));
        let automatic = scope.spawn(|| automatic::run_with_adapter(&f.open(), lease, &g));
        let outcomes = [
            manual.join().unwrap().unwrap(),
            automatic.join().unwrap().unwrap(),
        ];
        assert!(outcomes.iter().all(|outcome| matches!(
            outcome,
            ClaimResult::Observed {
                outcome: Outcome::Unknown,
                ..
            }
        )));
    });
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}

#[test]
fn automatic_pending_crosses_the_official_ten_oclock_window_without_repost() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    let boundary = chrono::DateTime::parse_from_rfc3339("2026-10-22T10:00:00+08:00")
        .unwrap()
        .timestamp();
    g.start.store(boundary - 86_400, Ordering::SeqCst);
    g.end.store(boundary, Ordering::SeqCst);
    g.now.store(boundary - 60, Ordering::SeqCst);
    g.lose_post = true;
    g.lose_get = true;
    let lease = automatic_enable(&f, &db, &g);
    automatic::run_with_adapter(&db, lease.clone(), &g).unwrap();
    g.start.store(boundary, Ordering::SeqCst);
    g.end.store(boundary + 86_400, Ordering::SeqCst);
    g.now.store(boundary + 60, Ordering::SeqCst);
    assert!(matches!(
        automatic::run_with_adapter(&db, lease, &g).unwrap(),
        ClaimResult::Observed {
            action: Action::Get,
            outcome: Outcome::Unknown,
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}

fn automatic_dispatch_after_lock_delay(stale_date: bool) {
    use std::time::Duration;
    let f = Fixture::new();
    let db = f.open();
    let gateway = Gateway::new();
    let lease = automatic_enable(&f, &db, &gateway);
    let context = gateway.verify(&f.load(&db), &lease.generation).unwrap();
    let operation = create_unknown(&f, &db);
    let observation = daily::clock::synthetic(
        if stale_date {
            "Thu, 01 Jan 1970 00:02:30 GMT"
        } else {
            "Thu, 01 Jan 1970 00:03:17 GMT"
        },
        None,
        &context,
    )
    .unwrap();
    let observation = daily::clock::synthetic_elapsed(
        observation,
        Duration::from_secs(if stale_date { 29 } else { 0 }),
    );
    assert!(observation.window_open(
        &context,
        operation.binding.start_at(),
        operation.binding.end_at(),
    ));
    let created = AtomicUsize::new(0);
    let admission = Admission { db: &db, lease };
    std::thread::scope(|scope| {
        let (held, wait) = std::sync::mpsc::channel();
        let db_ref = &db;
        scope.spawn(move || {
            let _guard = db_ref.conn.lock().unwrap();
            held.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(1_200));
        });
        wait.recv().unwrap();
        let result =
            crate::rt::block_on(
                admission.send(&context, &operation.binding, &observation, || {
                    created.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(Ok(http_response("https://openapi.qoder.sh", 200, &[])))
                }),
            );
        assert!(result.is_err(), "expired observation must refuse dispatch");
    });
    assert_eq!(created.load(Ordering::SeqCst), 0);
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .outcome,
        Outcome::Unknown,
    );
}

#[test]
fn automatic_final_dispatch_rechecks_campaign_deadline_after_lock_wait() {
    automatic_dispatch_after_lock_delay(false);
}

#[test]
fn automatic_final_dispatch_rechecks_date_freshness_after_lock_wait() {
    automatic_dispatch_after_lock_delay(true);
}
