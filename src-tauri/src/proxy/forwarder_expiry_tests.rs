//! Real admission/attempt consumer. Only entitlement facts are synthetic;
//! invalid synthetic transport prevents all socket and real-account access.
use super::*;
use crate::{database::Database, provider::ProviderMeta, relay::entitlement_expiry::*};
use serde_json::json;
use std::collections::HashMap;
struct Fixture {
    db: Arc<Database>,
    forwarder: RequestForwarder,
    providers: Vec<Provider>,
}
impl Fixture {
    async fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let db = Arc::new(Database::memory().unwrap());
        let mut providers = Vec::new();
        for (id, account) in [("expiry-a", 1), ("expiry-b", 2)] {
            let mut p = Provider::with_id(
                id.into(),
                id.into(),
                json!({"env":{
                "ANTHROPIC_BASE_URL":"synthetic://expiry", "ANTHROPIC_AUTH_TOKEN":"synthetic-token",
                "ANTHROPIC_MODEL":"final-model"}}),
                Some("https://expiry.example".into()),
            );
            p.meta = Some(ProviderMeta {
                loongport_account_id: Some(account),
                ..Default::default()
            });
            db.save_provider("claude", &p).unwrap();
            providers.push(p);
        }
        db.set_current_provider("claude", "expiry-a").unwrap();
        db.set_setting(
            super::super::auto_strategy::SETTING_STRATEGY,
            "known_expiry_first",
        )
        .unwrap();
        let router = Arc::new(ProviderRouter::new(db.clone()));
        let now = chrono::Utc::now().timestamp_millis();
        for (id, at) in [("expiry-a", now + 60_000), ("expiry-b", now + 30_000)] {
            router.expiry_test_evidence.lock().unwrap().insert(
                id.into(),
                vec![Evidence {
                    source: "synthetic authoritative bucket".into(),
                    trusted: true,
                    observed_at_ms: Some(now),
                    complete: false,
                    applicable_models: Some(vec!["final-model".into()]),
                    remaining: Some(42.0),
                    unit: Some("synthetic:credits".into()),
                    expiry: Expiry::ConsumableEndsAtMs(at),
                    loss_events: vec![],
                    remaining_precision: AmountPrecision::Exact,
                    concurrent_limits: None,
                }],
            );
        }
        let forwarder = RequestForwarder::new(
            ProxyDiagnosticId::new(),
            router,
            5,
            Arc::new(RwLock::new(ProxyStatus::default())),
            Arc::new(RwLock::new(HashMap::from([(
                "claude".into(),
                ("expiry-a".into(), "expiry-a".into()),
            )]))),
            Arc::new(GeminiShadowStore::new()),
            Arc::new(CodexChatHistoryStore::default()),
            Arc::new(CodexToolCarrierStore::default()),
            Arc::new(FailoverSwitchManager::new(db.clone())),
            #[cfg(feature = "gui")]
            None,
            std::sync::Weak::new(),
            "expiry-a".into(),
            "synthetic-session".into(),
            false,
            5,
            5,
            RectifierConfig::default(),
            OptimizerConfig::default(),
            CopilotOptimizerConfig::default(),
            0,
            Arc::new(super::super::model_alignment::ModelAlignmentAlerts::new()),
            true,
        );
        Self {
            db,
            forwarder,
            providers,
        }
    }
    async fn first_attempt(&self) -> String {
        let result=self.forwarder.forward_with_retry(&AppType::Claude,http::Method::POST,"/v1/messages",
            json!({"model":"client-alias","max_tokens":8,"messages":[{"role":"user","content":"history"}],
                "tools":[{"name":"probe","input_schema":{"type":"object"}}]}),http::HeaderMap::new(),Extensions::new(),self.providers.clone()).await;
        match result {
            Err(e) => e.provider.expect("selected attempt").id,
            Ok(_) => panic!("synthetic scheme cannot send"),
        }
    }
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_explicit_policy_sends_b_first_with_partial_other_buckets() {
    let f = Fixture::new().await;
    assert_eq!(f.first_attempt().await, "expiry-b");
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_default_and_fixed_remain_a() {
    let mut f = Fixture::new().await;
    f.db.set_setting(super::super::auto_strategy::SETTING_STRATEGY, "cheapest")
        .unwrap();
    assert_eq!(f.first_attempt().await, "expiry-a");
    f.db.set_setting(
        super::super::auto_strategy::SETTING_STRATEGY,
        "known_expiry_first",
    )
    .unwrap();
    f.forwarder.automatic_failover = false;
    assert_eq!(f.first_attempt().await, "expiry-a");
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_manual_order_and_unknown_facts_remain_a() {
    let f = Fixture::new().await;
    f.db.set_setting("easy_mode_mode_claude", "manual").unwrap();
    assert_eq!(f.first_attempt().await, "expiry-a");
    f.db.set_setting("easy_mode_mode_claude", "auto").unwrap();
    f.forwarder
        .router
        .expiry_test_evidence
        .lock()
        .unwrap()
        .clear();
    assert_eq!(f.first_attempt().await, "expiry-a");
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_inapplicable_untrusted_and_expired_b_do_not_gain_priority() {
    for reason in ["model", "conflict", "expired"] {
        let f = Fixture::new().await;
        {
            let mut facts = f.forwarder.router.expiry_test_evidence.lock().unwrap();
            let b = &mut facts.get_mut("expiry-b").unwrap()[0];
            match reason {
                "model" => b.applicable_models = Some(vec!["client-alias".into()]),
                "conflict" => b.trusted = false,
                _ => b.expiry = Expiry::ConsumableEndsAtMs(1),
            }
        }
        assert_eq!(f.first_attempt().await, "expiry-a", "{reason}");
    }
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_busy_b_does_not_resurrect_capacity() {
    let f = Fixture::new().await;
    let _busy = f
        .forwarder
        .router
        .acquire_account_capacity(&f.providers[1])
        .await
        .unwrap();
    assert!(_busy.is_some(), "fixture must hold a real account lane");
    assert_eq!(f.first_attempt().await, "expiry-a");
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_never_adds_a_candidate_outside_authorized_list() {
    let mut f = Fixture::new().await;
    f.providers.truncate(1);
    assert_eq!(f.first_attempt().await, "expiry-a");
}

/// No Evidence injection: original encrypted account -> native fields -> owner
/// projection -> real forwarder -> original WorkBuddy dispatch, with only HTTP
/// replaced by the guarded in-process synthetic transport.
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_native_owner_fields_reach_b_dispatch_without_changing_current() {
    use crate::{
        provider::{AuthBinding, AuthBindingSource},
        workbuddy::inference::FixtureTransport,
    };
    use tauri::Manager;
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let mut f = Fixture::new().await;
    f.db.set_setting(super::super::auto_strategy::SETTING_STRATEGY, "cheapest")
        .unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage(crate::store::AppState::new(f.db.clone()).unwrap());
    crate::commands::set_auto_mode_strategy(
        app.state::<crate::store::AppState>(),
        "known_expiry_first".into(),
    )
    .await
    .unwrap();
    f.forwarder
        .router
        .expiry_test_evidence
        .lock()
        .unwrap()
        .clear();
    let ids = FixtureTransport::seed_accounts(
        f.db.secret_session(),
        &["synthetic-expiry-a", "synthetic-expiry-b"],
    )
    .await
    .unwrap();
    let transport = FixtureTransport::in_process(f.db.secret_session()).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    for (i, p) in f.providers.iter_mut().enumerate() {
        p.website_url = None;
        p.meta = Some(ProviderMeta {
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("workbuddy".into()),
                account_id: Some(ids[i].clone()),
            }),
            ..Default::default()
        });
        f.db.save_provider("claude", p).unwrap();
        transport.cache_expiry_fixture(f.db.secret_session(),&ids[i],[
            json!({"code":0,"data":{"Packages":[{"PackageCode":"pack","CapacityUnit":"credits"}]}}),
            json!({"code":0,"data":{"Accounts":[{"ResourceId":"resource","PackageCode":"pack","CycleCapacityRemainPrecise":"42","DeductionEndTime":now+if i==0 {60_000} else {30_000}}],"TotalCount":200}}),
            json!({"agents":[{"name":"cli","models":["final-model"]}],"models":[{"id":"final-model","credits":"x0.03"}],"modelPromotions":[]})
        ],now).await.unwrap();
    }
    let mut extensions = Extensions::new();
    extensions.insert(transport.clone());
    let result=f.forwarder.forward_with_retry(&AppType::Claude,http::Method::POST,"/v1/messages",
        json!({"model":"client-alias","stream":true,"max_tokens":16,
            "messages":[{"role":"user","content":"earlier history"},{"role":"assistant","content":"earlier answer"},{"role":"user","content":"continue"}],
            "tools":[{"name":"probe","description":"synthetic","input_schema":{"type":"object","properties":{}}}]}),
        http::HeaderMap::new(),extensions,f.providers.clone()).await.unwrap_or_else(|e|panic!("{}",e.error));
    assert_eq!(result.provider.id, "expiry-b");
    assert_eq!(result.outbound_model.as_deref(), Some("final-model"));
    let captured = transport.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, "Bearer fixture-access-synthetic-expiry-b");
    assert_eq!(captured[0].1["model"], "final-model");
    assert_eq!(captured[0].1["messages"][0]["content"], "earlier history");
    assert_eq!(captured[0].1["messages"][1]["content"], "earlier answer");
    assert_eq!(captured[0].1["tools"][0]["function"]["name"], "probe");
    assert_eq!(
        f.db.get_current_provider("claude").unwrap().as_deref(),
        Some("expiry-a")
    );
    assert_eq!(
        f.forwarder.current_providers.read().await["claude"].0,
        "expiry-a"
    );
    assert_eq!(f.forwarder.status.read().await.failover_count, 0);
    assert_eq!(
        f.forwarder.failover_manager.join_spawned_for_test().await,
        0
    );
    FixtureTransport::retire_fixture_grant(f.db.secret_session(), "synthetic-expiry-b")
        .await
        .unwrap();
    assert!(
        result.response.bytes_with_limit(64 * 1024).await.is_err(),
        "the actual selected B owner must still fence unread response bytes"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn expiry_settings_command_roundtrip_drives_consumer_and_preserves_manual_binding() {
    use crate::commands::{get_auto_mode_status, set_auto_mode_strategy};
    use crate::store::AppState;
    use tauri::Manager;
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let f = Fixture::new().await;
    f.db.set_setting(super::super::auto_strategy::SETTING_STRATEGY, "cheapest")
        .unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage(AppState::new(f.db.clone()).unwrap());
    // Seed after startup migration, which retires legacy manual state.
    f.db.set_setting(
        "easy_mode_manual_order_claude",
        "[\"expiry-a\",\"expiry-b\"]",
    )
    .unwrap();
    assert_eq!(
        get_auto_mode_status(app.state(), "claude".into())
            .await
            .unwrap()
            .strategy,
        "cheapest"
    );
    set_auto_mode_strategy(app.state(), "known_expiry_first".into())
        .await
        .unwrap();
    assert_eq!(
        get_auto_mode_status(app.state(), "codex".into())
            .await
            .unwrap()
            .strategy,
        "known_expiry_first"
    );
    assert_eq!(f.first_attempt().await, "expiry-b");
    f.db.set_setting("easy_mode_mode_claude", "manual").unwrap();
    assert_eq!(f.first_attempt().await, "expiry-a");
    set_auto_mode_strategy(app.state(), "cheapest".into())
        .await
        .unwrap();
    assert_eq!(
        get_auto_mode_status(app.state(), "claude".into())
            .await
            .unwrap()
            .strategy,
        "cheapest"
    );
    assert_eq!(
        f.db.get_setting("easy_mode_mode_claude")
            .unwrap()
            .as_deref(),
        Some("manual")
    );
    assert_eq!(
        f.db.get_setting("easy_mode_manual_order_claude")
            .unwrap()
            .as_deref(),
        Some("[\"expiry-a\",\"expiry-b\"]")
    );
    f.db.set_setting("easy_mode_mode_claude", "auto").unwrap();
    assert_eq!(f.first_attempt().await, "expiry-a");
    assert_eq!(
        f.db.get_current_provider("claude").unwrap().as_deref(),
        Some("expiry-a")
    );
    assert!(
        !get_auto_mode_status(app.state(), "claude".into())
            .await
            .unwrap()
            .enabled
    );
}

#[tokio::test]
#[serial_test::serial]
async fn expiry_settings_command_rejects_retired_or_unknown_values_without_overwriting() {
    use crate::commands::{get_auto_mode_status, set_auto_mode_strategy};
    use tauri::Manager;
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage(crate::store::AppState::new(Arc::new(Database::memory().unwrap())).unwrap());
    assert_eq!(
        get_auto_mode_status(app.state(), "claude".into())
            .await
            .unwrap()
            .strategy,
        "cheapest"
    );
    set_auto_mode_strategy(app.state(), "known_expiry_first".into())
        .await
        .unwrap();
    for invalid in ["fastest", "", "unknown", "KNOWN_EXPIRY_FIRST"] {
        assert!(set_auto_mode_strategy(app.state(), invalid.into())
            .await
            .is_err());
        assert_eq!(
            get_auto_mode_status(app.state(), "claude".into())
                .await
                .unwrap()
                .strategy,
            "known_expiry_first"
        );
    }
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_open_breaker_is_not_revived_by_earlier_credits() {
    let f = Fixture::new().await;
    for _ in 0..10 {
        f.forwarder
            .router
            .record_result(
                "expiry-b",
                "claude",
                false,
                false,
                Some("synthetic failure".into()),
            )
            .await
            .unwrap();
    }
    assert_eq!(f.first_attempt().await, "expiry-a");
}
#[tokio::test]
#[serial_test::serial]
async fn expiry_consumer_revoked_caller_cancels_reordered_capacity_wait() {
    let f = Fixture::new().await;
    let grant =
        f.db.issue_client_credential(
            "synthetic-expiry",
            "claude",
            &["client-alias".into(), "final-model".into()],
        )
        .unwrap();
    let key =
        f.db.resolve_client_credential(&grant.id, "synthetic-expiry", "claude")
            .unwrap();
    let caller =
        f.db.authenticate_client(&key, "claude", Some("client-alias"))
            .unwrap();
    let _a = f
        .forwarder
        .router
        .acquire_account_capacity(&f.providers[0])
        .await
        .unwrap();
    let _b = f
        .forwarder
        .router
        .acquire_account_capacity(&f.providers[1])
        .await
        .unwrap();
    assert!(_a.is_some() && _b.is_some());
    let mut extensions = Extensions::new();
    extensions.insert(caller);
    let mut waiting=Box::pin(f.forwarder.forward_with_retry(&AppType::Claude,http::Method::POST,"/v1/messages",
        json!({"model":"client-alias","max_tokens":8,"messages":[{"role":"user","content":"synthetic"}]}),
        http::HeaderMap::new(),extensions,f.providers.clone()));
    assert!(futures::poll!(&mut waiting).is_pending());
    f.db.revoke_client_credential(&grant.id).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_millis(300), waiting)
        .await
        .expect("revocation must end reordered wait");
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::ClientAccessDenied,
            ..
        })
    ));
    let attempts: i64 =
        f.db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM provider_attempt_outcomes",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(attempts, 0);
}

#[tokio::test]
#[serial_test::serial]
async fn expiry_settings_status_does_not_disguise_a_database_read_error_as_default_order() {
    use crate::commands::get_auto_mode_status;
    use tauri::Manager;
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let db = Arc::new(Database::memory().unwrap());
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage(crate::store::AppState::new(db.clone()).unwrap());
    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES ('auto_mode_strategy', X'01')",
            [],
        )
        .unwrap();
    assert!(get_auto_mode_status(app.state(), "claude".into())
        .await
        .is_err());
    // Runtime keeps its conservative fallback; the user-visible read must fail.
    assert!(!super::super::auto_strategy::known_expiry_first(
        &db, "claude"
    ));
}
