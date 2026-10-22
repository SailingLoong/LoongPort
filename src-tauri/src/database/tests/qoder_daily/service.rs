use crate::services::provider::qoder_daily::{
    self as daily,
    contract::{BoundCampaign, Identity, Outcome, Region, Target},
    Action, CampaignAdapter, ClaimResult, Dependency, Listing, Response, VerifiedContext,
};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

#[test]
fn automatic_policy_is_explicitly_off_for_new_and_legacy_state() {
    let fresh = daily::State::fresh();
    let encoded = serde_json::to_value(&fresh).unwrap();
    assert_eq!(encoded["automatic"]["enabled"], serde_json::json!(false));
    let old: daily::State = serde_json::from_value(serde_json::json!({
        "generation": "synthetic-old-generation", "pending": null
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(old).unwrap()["automatic"]["enabled"],
        serde_json::json!(false)
    );
}
#[derive(Default)]
struct ListingRendezvous {
    arrived: Mutex<usize>,
    ready: std::sync::Condvar,
}
impl ListingRendezvous {
    fn wait(&self) {
        let mut arrived = self.arrived.lock().unwrap();
        *arrived += 1;
        self.ready.notify_all();
        let (arrived, _) = self
            .ready
            .wait_timeout_while(arrived, std::time::Duration::from_secs(5), |count| {
                *count < 2
            })
            .unwrap();
        assert_eq!(
            *arrived, 2,
            "both independent connections must reach listing"
        );
    }
}
struct Gateway {
    posts: AtomicUsize,
    gets: AtomicUsize,
    device: bool,
    lose_post: bool,
    lose_get: bool,
    now: AtomicI64,
    start: AtomicI64,
    end: AtomicI64,
    clock: bool,
    http_status: u16,
    http_url: &'static str,
    duplicate_age: bool,
    barrier: Option<Arc<ListingRendezvous>>,
}
impl Gateway {
    fn new() -> Self {
        Self {
            posts: AtomicUsize::new(0),
            gets: AtomicUsize::new(0),
            device: true,
            lose_post: false,
            lose_get: false,
            now: AtomicI64::new(150),
            start: AtomicI64::new(100),
            end: AtomicI64::new(200),
            clock: true,
            http_status: 200,
            http_url: "https://openapi.qoder.sh/api/v1/campaigns",
            duplicate_age: false,
            barrier: None,
        }
    }
}
fn identity(id: &str) -> Identity {
    Identity {
        provider_id: id.into(),
        uid: "synthetic-official-subject".into(),
        region: Region::International,
    }
}
fn target() -> Target {
    Target {
        campaign_id: "synthetic-daily-1".into(),
        campaign_key: "explicit-supported-target".into(),
    }
}
impl CampaignAdapter for Gateway {
    fn verify(&self, p: &Provider, generation: &str) -> Result<VerifiedContext, Dependency> {
        Ok(VerifiedContext::synthetic(
            identity(&p.id),
            generation.into(),
            self.device,
        ))
    }
    fn list(&self, _: &VerifiedContext) -> Result<Listing, error::AppError> {
        if let Some(b) = &self.barrier {
            b.wait();
        }
        let date = chrono::DateTime::from_timestamp(self.now.load(Ordering::SeqCst), 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        Ok(Listing{identity:identity("synthetic-provider"),region:Region::International,http:fixture_http(self.metadata_body(&date,serde_json::json!({"uid":"synthetic-official-subject","campaigns":[{"campaignId":"synthetic-daily-1","campaignKey":"explicit-supported-target","actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMABLE","startAt":self.start.load(Ordering::SeqCst),"endAt":self.end.load(Ordering::SeqCst),"benefit":{"kind":"CREDITS","amount":100}}]}).to_string()))})
    }
    fn post(
        &self,
        _: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, error::AppError> {
        self.posts.fetch_add(1, Ordering::SeqCst);
        if self.lose_post {
            return Err(error::AppError::Config("synthetic response loss".into()));
        }
        Ok(response(binding, false))
    }
    fn get_reward(
        &self,
        _: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, error::AppError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.lose_get {
            return Err(error::AppError::Config("synthetic GET loss".into()));
        }
        let mut r = response(binding, true);
        let date = chrono::DateTime::from_timestamp(self.now.load(Ordering::SeqCst), 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        r.http = fixture_http(self.metadata_body(&date, receipt_body(binding, true).to_string()));
        Ok(r)
    }
}
fn receipt_body(binding: &BoundCampaign, replayed: bool) -> serde_json::Value {
    serde_json::json!({"status":"CLAIMED","campaignId":binding.target().campaign_id,"campaignKey":binding.target().campaign_key,"grantId":"synthetic-grant","claimedAt":chrono::DateTime::from_timestamp(binding.start_at()+50,0).unwrap().to_rfc3339(),"replayed":replayed})
}
fn response(binding: &BoundCampaign, replayed: bool) -> Response {
    Response {
        identity: binding.identity().clone(),
        http: fixture_http(http_response_body(
            "https://openapi.qoder.sh/api/v1/campaigns",
            200,
            &[],
            receipt_body(binding, replayed).to_string(),
        )),
    }
}

fn state(f: &Fixture, db: &Database) -> daily::State {
    f.load(db)
        .meta
        .unwrap()
        .qoder_campaign_state
        .expect("owned Qoder generation must exist")
}
#[test]
fn explicit_service_posts_once_and_persists_result() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    let result = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        result,
        ClaimResult::Observed {
            action: Action::Post,
            outcome: Outcome::NewlyClaimed,
            added_credits: Some(100),
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
    assert!(state(&f, &db).pending.is_some());
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .outcome,
        Outcome::NewlyClaimed
    );
}
#[test]
fn lost_post_response_reopens_with_get_only() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    let first = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        first,
        ClaimResult::Observed {
            action: Action::Post,
            outcome: Outcome::Unknown,
            ..
        }
    ));
    drop(db);
    let result =
        daily::prepare_with_adapter(&f.open(), "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        result,
        ClaimResult::Observed {
            action: Action::Get,
            outcome: Outcome::ClaimedDeltaUnknown,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
    assert_eq!(g.gets.load(Ordering::SeqCst), 1);
}
#[test]
fn missing_device_is_explicit_and_no_post() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.device = false;
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Unavailable(Dependency::OfficialDeviceUnavailable)
    );
    assert_eq!(g.posts.load(Ordering::SeqCst), 0);
}
#[test]
fn production_entry_does_not_infer_pat_family_from_nonempty_settings() {
    let f = Fixture::new();
    let db = f.open();
    assert_eq!(
        daily::prepare_claim(&db, "synthetic-provider", Some(&target()), None).unwrap(),
        ClaimResult::Unavailable(Dependency::AuthFamilyUnverified)
    );
}
#[test]
fn normal_save_owns_generation_and_model_edits_do_not_rotate_it() {
    let f = Fixture::new();
    let db = f.open();
    let p = f.load(&db);
    db.save_provider("codex", &p).unwrap();
    let initial = state(&f, &db);
    let mut model = f.load(&db);
    model.settings_config["model"] = serde_json::json!("another-model");
    db.save_provider("codex", &model).unwrap();
    assert_eq!(state(&f, &db).generation, initial.generation);
    let mut changed = f.load(&db);
    changed.settings_config["env"]["OPENAI_API_KEY"] = serde_json::json!("synthetic-new-candidate");
    db.save_provider("codex", &changed).unwrap();
    assert_ne!(state(&f, &db).generation, initial.generation);
    db.save_provider("codex", &p).unwrap();
    assert_ne!(state(&f, &db).generation, initial.generation);
}
#[test]
fn direct_settings_path_cannot_skip_generation_invalidation() {
    let f = Fixture::new();
    let db = f.open();
    let p = f.load(&db);
    db.save_provider("codex", &p).unwrap();
    let before = state(&f, &db);
    db.update_provider_settings_config(
        "codex",
        &p.id,
        &provider("synthetic-generation-2", None).settings_config,
    )
    .unwrap();
    assert_ne!(state(&f, &db).generation, before.generation);
}

#[test]
fn concurrent_two_connection_clicks_have_one_post_winner() {
    let f = Fixture::new();
    let db = f.open();
    db.save_provider("codex", &f.load(&db)).unwrap();
    drop(db);
    let mut g = Gateway::new();
    g.lose_post = true;
    g.lose_get = true;
    g.barrier = Some(Arc::new(ListingRendezvous::default()));
    std::thread::scope(|scope| {
        let a = scope
            .spawn(|| daily::prepare_with_adapter(&f.open(), "synthetic-provider", &target(), &g));
        let b = scope
            .spawn(|| daily::prepare_with_adapter(&f.open(), "synthetic-provider", &target(), &g));
        let results = [
            a.join()
                .unwrap()
                .expect("first independent prepare returns a result"),
            b.join()
                .unwrap()
                .expect("second independent prepare returns a result"),
        ];
        assert!(
            results.iter().all(|result| matches!(
                result,
                ClaimResult::Observed {
                    outcome: Outcome::Unknown,
                    added_credits: None,
                    ..
                }
            )),
            "both uncertain responses must preserve unknown: {results:?}"
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    ClaimResult::Observed {
                        action: Action::Post,
                        ..
                    }
                ))
                .count(),
            1
        );
    });
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.open()
            .qoder_operations(&identity("synthetic-provider"))
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn delete_recreate_same_subject_keeps_unknown_and_only_gets() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    let old = state(&f, &db).generation;
    db.delete_provider("codex", "synthetic-provider").unwrap();
    assert!(db
        .get_provider_by_id("synthetic-provider", "codex")
        .unwrap()
        .is_none());
    db.save_provider("codex", &provider("replacement", None))
        .unwrap();
    assert_ne!(state(&f, &db).generation, old);
    let r = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert!(matches!(
        r,
        ClaimResult::Observed {
            action: Action::Get,
            added_credits: None,
            ..
        }
    ));
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn terminal_campaign_window_drift_is_rejected() {
    let f = Fixture::new();
    let db = f.open();
    let g = Gateway::new();
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.end.store(201, Ordering::SeqCst);
    assert!(daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).is_err());
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn missing_date_never_expires_unknown_or_reposts() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    g.clock = false;
    g.now.store(999, Ordering::SeqCst);
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()
            .len(),
        1
    );
    assert!(state(&f, &db).pending.is_some());
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn missing_listing_date_blocks_initial_post() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.clock = false;
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Unavailable(Dependency::ClockUnverified)
    );
    assert_eq!(g.posts.load(Ordering::SeqCst), 0);
}
#[test]
fn authenticated_get_at_expiry_keeps_receipt_and_detaches_exact_pending() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    let mut p = f.load(&db);
    p.settings_config["env"]["OPENAI_API_KEY"] = serde_json::json!("nonsecret-changed");
    db.save_provider("codex", &p).unwrap();
    let generation = state(&f, &db).generation;
    g.now.store(200, Ordering::SeqCst);
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(state(&f, &db).generation, generation);
    assert!(state(&f, &db).pending.is_none());
    assert_eq!(g.posts.load(Ordering::SeqCst), 1);
}
#[test]
fn forged_frontend_state_cannot_rollback_generation_or_pending() {
    let f = Fixture::new();
    let db = f.open();
    let mut g = Gateway::new();
    g.lose_post = true;
    daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap();
    let expected = state(&f, &db);
    let mut p = f.load(&db);
    p.meta.as_mut().unwrap().qoder_campaign_state = Some(daily::State {
        generation: "forged".into(),
        pending: None,
        automatic: Default::default(),
    });
    db.save_provider("codex", &p).unwrap();
    assert_eq!(state(&f, &db), expected);
}
#[test]
fn stale_aba_intent_cannot_obtain_post_permission() {
    let f = Fixture::new();
    let db = f.open();
    db.save_provider("codex", &f.load(&db)).unwrap();
    let a = f.load(&db);
    let original = state(&f, &db);
    let binding = BoundCampaign::from_operation(
        identity(&a.id),
        target().campaign_id,
        target().campaign_key,
        100,
        200,
    )
    .unwrap();
    let operation = daily::Operation {
        id: "stale-attempt".into(),
        generation: original.generation,
        binding,
        outcome: Outcome::Unknown,
    };
    db.update_provider_settings_config("codex", &a.id, &provider("b", None).settings_config)
        .unwrap();
    db.update_provider_settings_config("codex", &a.id, &a.settings_config)
        .unwrap();
    assert!(db.save_qoder_operation(&a, &operation).is_err());
    assert!(db.qoder_operations(&identity(&a.id)).unwrap().is_empty());
}
#[test]
fn normal_identical_cas_remains_idempotent_ok() {
    let f = Fixture::new();
    let db = f.open();
    db.save_provider("codex", &f.load(&db)).unwrap();
    let before = f.load(&db);
    let expected = digest(&before);
    let mut after = before.clone();
    after.name = "Changed name".into();
    assert!(f.save(&db, &after, &expected).is_ok());
    assert!(f.save(&db, &after, &expected).is_ok());
}
fn grok_external() -> serde_json::Value {
    serde_json::json!({"config":"[models]\ndefault='fixture'\n[model.fixture]\nmodel='model'\nbase_url='https://example.invalid'\nname='fixture'\nenv_key='LOONGPORT_QODER_TEST_MUST_NOT_READ_ENV'\napi_backend='openai'\ncontext_window=1000\n"})
}
#[test]
fn process_env_credential_source_is_fail_closed_without_reading_environment() {
    let f = Fixture::new();
    let db = f.open();
    db.update_provider_settings_config("codex", "synthetic-provider", &grok_external())
        .unwrap();
    let g = Gateway::new();
    assert_eq!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Unavailable(Dependency::ExternalCredentialSourceUntracked)
    );
    assert_eq!(g.posts.load(Ordering::SeqCst), 0);
}
#[test]
fn higher_priority_inline_candidate_bypasses_unused_external_env_declaration() {
    let f = Fixture::new();
    let db = f.open();
    let mut settings = grok_external();
    settings["env"] = serde_json::json!({"OPENAI_API_KEY":"synthetic-inline"});
    db.update_provider_settings_config("codex", "synthetic-provider", &settings)
        .unwrap();
    let g = Gateway::new();
    assert!(matches!(
        daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g).unwrap(),
        ClaimResult::Observed {
            action: Action::Post,
            ..
        }
    ));
}

impl Gateway {
    fn metadata_body(&self, date: &str, body: String) -> reqwest::Response {
        let mut headers = vec![];
        if self.clock {
            headers.push(("date", date));
        }
        if self.duplicate_age {
            headers.push(("age", "0"));
            headers.push(("age", "4"));
        }
        http_response_body(self.http_url, self.http_status, &headers, body)
    }
}
#[test]
fn service_consumes_http_metadata_and_rejects_bad_listing_status_or_origin() {
    for (status, url, duplicate_age) in [
        (500, "https://openapi.qoder.sh/api/v1/campaigns", false),
        (200, "https://foreign.invalid/api/v1/campaigns", false),
        (200, "https://openapi.qoder.sh/api/v1/campaigns", true),
    ] {
        let f = Fixture::new();
        let db = f.open();
        let mut g = Gateway::new();
        g.http_status = status;
        g.http_url = url;
        g.duplicate_age = duplicate_age;
        let result = daily::prepare_with_adapter(&db, "synthetic-provider", &target(), &g);
        assert!(!matches!(
            result,
            Ok(ClaimResult::Observed {
                action: Action::Post,
                ..
            })
        ));
        assert_eq!(g.posts.load(Ordering::SeqCst), 0);
        assert!(db
            .qoder_operations(&identity("synthetic-provider"))
            .unwrap()
            .is_empty());
    }
}
struct MixedWindows(Gateway, bool);
impl CampaignAdapter for MixedWindows {
    fn verify(&self, p: &Provider, g: &str) -> Result<VerifiedContext, Dependency> {
        self.0.verify(p, g)
    }
    fn list(&self, c: &VerifiedContext) -> Result<Listing, error::AppError> {
        if self.0.now.load(Ordering::SeqCst) == 150 {
            return self.0.list(c);
        }
        let mut body = serde_json::json!({"uid":"synthetic-official-subject","campaigns":[{"campaignId":"synthetic-daily-1","campaignKey":"explicit-supported-target","actionType":"VIEW_DETAILS","claimStatus":"CLAIMED","startAt":100,"endAt":200},{"campaignId":"synthetic-next-window","campaignKey":"next-opaque-key","actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMABLE","startAt":200,"endAt":300,"benefit":{"kind":"CREDITS","amount":100}}]});
        if self.1 {
            let mut extra = body["campaigns"][1].clone();
            extra["campaignId"] = serde_json::json!("ambiguous-extra");
            body["campaigns"].as_array_mut().unwrap().push(extra);
        }
        Ok(Listing {
            identity: identity("synthetic-provider"),
            region: Region::International,
            http: fixture_http(
                self.0
                    .metadata_body("Thu, 01 Jan 1970 00:04:10 GMT", body.to_string()),
            ),
        })
    }
    fn post(&self, c: &VerifiedContext, b: &BoundCampaign) -> Result<Response, error::AppError> {
        self.0.post(c, b)
    }
    fn get_reward(
        &self,
        c: &VerifiedContext,
        b: &BoundCampaign,
    ) -> Result<Response, error::AppError> {
        self.0.get_reward(c, b)
    }
}
#[test]
fn ui_automatic_selection_does_not_stop_at_an_old_claimed_listing_row() {
    let f = Fixture::new();
    let db = f.open();
    let g = MixedWindows(Gateway::new(), false);
    daily::prepare_selected(&db, "synthetic-provider", None, &g).unwrap();
    g.0.now.store(250, Ordering::SeqCst);
    let second = daily::prepare_selected(&db, "synthetic-provider", None, &g).unwrap();
    assert!(
        matches!(
            second,
            ClaimResult::Observed {
                action: Action::Post,
                outcome: Outcome::NewlyClaimed,
                ..
            }
        ),
        "old row must not shadow unique current window: {second:?}"
    );
    assert_eq!(g.0.posts.load(Ordering::SeqCst), 2);
    assert_eq!(
        db.qoder_operations(&identity("synthetic-provider"))
            .unwrap()[0]
            .binding
            .target()
            .campaign_id,
        "synthetic-next-window"
    );
}

#[test]
fn old_terminal_row_cannot_hide_ambiguous_new_candidates() {
    let f = Fixture::new();
    let db = f.open();
    let g = MixedWindows(Gateway::new(), true);
    daily::prepare_selected(&db, "synthetic-provider", None, &g).unwrap();
    g.0.now.store(250, Ordering::SeqCst);
    assert_eq!(
        daily::prepare_selected(&db, "synthetic-provider", None, &g).unwrap(),
        ClaimResult::Unavailable(Dependency::NoEligibleCampaign)
    );
    assert_eq!(g.0.posts.load(Ordering::SeqCst), 1);
}
