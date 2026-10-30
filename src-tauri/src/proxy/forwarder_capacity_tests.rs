//! Consumer regressions through the real forwarder and a loopback HTTP upstream.
use super::*;
use crate::database::Database;
use crate::provider::ProviderMeta;
use axum::{extract::State, http::Uri, response::IntoResponse, Json};
use serde_json::json;
use serial_test::serial;
use std::{collections::HashMap, sync::Mutex, time::Duration};

#[derive(Clone)]
struct Upstream(Arc<Mutex<Vec<(String, Value)>>>);
struct Fixture {
    db: Arc<Database>,
    forwarder: RequestForwarder,
    providers: Vec<Provider>,
    upstream: Upstream,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
impl Fixture {
    async fn new() -> Self {
        let upstream = Upstream(Arc::new(Mutex::new(Vec::new())));
        let app = axum::Router::new()
            .fallback(
                |State(state): State<Upstream>, uri: Uri, Json(body): Json<Value>| async move {
                    state.0.lock().unwrap().push((uri.to_string(), body));
                    Json(
                        json!({"id":"synthetic-response","type":"message","role":"assistant",
                    "model":"synthetic-model","content":[{"type":"text","text":"ok"}],
                    "stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),
                    )
                    .into_response()
                },
            )
            .with_state(upstream.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self::build(upstream, format!("http://{address}"), Some(task)).await
    }
    async fn without_transport() -> Self {
        // Queue-only tests never release occupied slots while the request lives.
        // An invalid transport scheme also prevents accidental network traffic.
        Self::build(
            Upstream(Arc::new(Mutex::new(Vec::new()))),
            "synthetic://capacity".into(),
            None,
        )
        .await
    }
    async fn build(
        upstream: Upstream,
        base: String,
        task: Option<tokio::task::JoinHandle<()>>,
    ) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let db = Arc::new(Database::memory().unwrap());
        let mut providers = Vec::new();
        for (id, account) in [("capacity-a", 1), ("capacity-b", 2)] {
            let mut provider = Provider::with_id(
                id.into(),
                id.into(),
                json!({"env":{
                    "ANTHROPIC_BASE_URL":format!("{base}/{id}"),
                    "ANTHROPIC_AUTH_TOKEN":"synthetic-token"
                }}),
                Some("https://capacity.example".into()),
            );
            provider.meta = Some(ProviderMeta {
                loongport_account_id: Some(account),
                ..Default::default()
            });
            db.save_provider("claude", &provider).unwrap();
            providers.push(provider);
        }
        db.set_current_provider("claude", "capacity-a").unwrap();
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();
        let router = Arc::new(ProviderRouter::new(db.clone()));
        let current = Arc::new(RwLock::new(HashMap::from([(
            "claude".into(),
            ("capacity-a".into(), "capacity-a".into()),
        )])));
        let forwarder = RequestForwarder::new(
            ProxyDiagnosticId::new(),
            router,
            5,
            Arc::new(RwLock::new(ProxyStatus::default())),
            current,
            Arc::new(GeminiShadowStore::new()),
            Arc::new(CodexChatHistoryStore::default()),
            Arc::new(CodexToolCarrierStore::default()),
            Arc::new(FailoverSwitchManager::new(db.clone())),
            #[cfg(feature = "gui")]
            None,
            std::sync::Weak::new(),
            "capacity-a".into(),
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
            upstream,
            task,
        }
    }
    async fn send(
        &self,
        providers: Vec<Provider>,
        extensions: Extensions,
    ) -> Result<ForwardResult, ForwardError> {
        self.forwarder.forward_with_retry(&AppType::Claude, http::Method::POST, "/v1/messages",
            json!({"model":"synthetic-model","max_tokens":8,"messages":[{"role":"user","content":"hello"}],
                "tools":[{"name":"probe","description":"synthetic","input_schema":{"type":"object","properties":{}}}]}),
            http::HeaderMap::new(), extensions, providers).await
    }
    fn hits(&self) -> Vec<(String, Value)> {
        self.upstream.0.lock().unwrap().clone()
    }
}

#[tokio::test]
#[serial]
async fn capacity_consumer_idle_control_uses_real_transport() {
    let fixture = Fixture::new().await;
    let result = fixture
        .send(vec![fixture.providers[0].clone()], Extensions::new())
        .await
        .unwrap_or_else(|e| panic!("control transport failed: {}", e.error));
    assert_eq!(result.provider.id, "capacity-a");
    let hits = fixture.hits();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, "/capacity-a/v1/messages");
    assert_eq!(hits[0].1["model"], "synthetic-model");
    assert_eq!(hits[0].1["tools"][0]["name"], "probe");
}

#[tokio::test]
#[serial]
async fn capacity_consumer_busy_a_sends_idle_b_without_spending_retry_or_switching_current() {
    let fixture = Fixture::new().await;
    let busy_a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        fixture.send(fixture.providers.clone(), Extensions::new()),
    )
    .await
    .expect("B must actually send before A's held capacity is released")
    .unwrap_or_else(|e| panic!("Busy must not consume max_attempts=1: {}", e.error));
    assert_eq!(result.provider.id, "capacity-b");
    let hits = fixture.hits();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, "/capacity-b/v1/messages");
    assert_eq!(hits[0].1["model"], "synthetic-model");
    assert_eq!(hits[0].1["tools"][0]["name"], "probe");
    assert_eq!(
        fixture
            .forwarder
            .failover_manager
            .join_spawned_for_test()
            .await,
        0,
        "capacity-only success must not launch the original persistent switch worker"
    );
    assert_eq!(
        super::super::application_routing::current_provider_id_checked(&fixture.db, "claude")
            .unwrap()
            .as_deref(),
        Some("capacity-a")
    );
    assert_eq!(
        fixture.forwarder.current_providers.read().await["claude"].0,
        "capacity-a"
    );
    assert_eq!(fixture.forwarder.status.read().await.failover_count, 0);
    let stats = fixture
        .forwarder
        .router
        .get_circuit_breaker_stats("capacity-a", "claude")
        .await;
    assert!(stats.is_none_or(|s| s.failed_requests == 0 && s.total_requests == 0));
    let attempts: i64 = fixture
        .db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM provider_attempt_outcomes WHERE provider_id='capacity-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 0);
    assert!(
        matches!(
            fixture
                .forwarder
                .router
                .try_acquire_account_capacity(&fixture.providers[1])
                .unwrap(),
            super::super::account_capacity::CapacityAdmission::Busy
        ),
        "unread response must retain B capacity"
    );
    drop(result);
    assert!(
        matches!(
            fixture
                .forwarder
                .router
                .try_acquire_account_capacity(&fixture.providers[1])
                .unwrap(),
            super::super::account_capacity::CapacityAdmission::Acquired(Some(_))
        ),
        "dropping the response releases B"
    );
    drop(busy_a);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_all_busy_uses_request_deadline_before_transport() {
    let mut fixture = Fixture::without_transport().await;
    fixture.forwarder.non_streaming_timeout = Duration::from_millis(20);
    let _a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let _b = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[1])
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        fixture.send(fixture.providers.clone(), Extensions::new()),
    )
    .await
    .expect("forward_with_retry must bound its all-busy wait with the request deadline");
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::Timeout(_),
            ..
        })
    ));
    assert_no_attempts(&fixture);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_revoked_caller_exits_without_waiting_for_slot() {
    let fixture = Fixture::without_transport().await;
    let grant = fixture
        .db
        .issue_client_credential("synthetic", "claude", &["synthetic-model".into()])
        .unwrap();
    let key = fixture
        .db
        .resolve_client_credential(&grant.id, "synthetic", "claude")
        .unwrap();
    let caller = fixture
        .db
        .authenticate_client(&key, "claude", Some("synthetic-model"))
        .unwrap();
    let _a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let _b = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[1])
        .await
        .unwrap();
    let mut extensions = Extensions::new();
    extensions.insert(caller);
    let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), extensions));
    assert!(futures::poll!(&mut waiting).is_pending());
    fixture.db.revoke_client_credential(&grant.id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(300), waiting)
        .await
        .expect("caller revocation must terminate the capacity wait before a slot is released");
    assert!(matches!(
        result,
        Err(ForwardError {
            error: ProxyError::ClientAccessDenied,
            ..
        })
    ));
    assert_no_attempts(&fixture);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_cancel_releases_waiter_without_transport() {
    let fixture = Fixture::without_transport().await;
    let a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let b = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[1])
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
    assert!(futures::poll!(&mut waiting).is_pending());
    drop(waiting);
    assert_no_attempts(&fixture);
    drop(a);
    drop(b);
    for provider in &fixture.providers {
        let mut next = Box::pin(fixture.forwarder.router.acquire_account_capacity(provider));
        assert!(
            futures::poll!(&mut next).is_ready(),
            "cancelled consumer must not retain capacity"
        );
    }
}

fn assert_no_attempts(fixture: &Fixture) {
    let count: i64 = fixture
        .db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM provider_attempt_outcomes", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0, "capacity waiting is not a provider attempt");
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_same_account_rows_cannot_create_an_extra_slot() {
    let mut fixture = Fixture::without_transport().await;
    fixture.providers[1]
        .meta
        .as_mut()
        .unwrap()
        .loongport_account_id = Some(1);
    fixture
        .db
        .save_provider("claude", &fixture.providers[1])
        .unwrap();
    let busy = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
    assert!(
        futures::poll!(&mut waiting).is_pending(),
        "second row shares the same real account capacity"
    );
    drop(waiting);
    drop(busy);
    assert_no_attempts(&fixture);
    let mut next = Box::pin(
        fixture
            .forwarder
            .router
            .acquire_account_capacity(&fixture.providers[1]),
    );
    assert!(futures::poll!(&mut next).is_ready());
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_disabled_timeout_preserves_manual_selection() {
    for automatic in [false, true] {
        let mut fixture = Fixture::without_transport().await;
        fixture.forwarder.automatic_failover = automatic;
        fixture.forwarder.non_streaming_timeout = if automatic {
            Duration::ZERO
        } else {
            Duration::from_millis(10)
        };
        let _a = fixture
            .forwarder
            .router
            .acquire_account_capacity(&fixture.providers[0])
            .await
            .unwrap();
        // In manual mode B deliberately remains idle. Explicit A must still wait.
        let _b = if automatic {
            fixture
                .forwarder
                .router
                .acquire_account_capacity(&fixture.providers[1])
                .await
                .unwrap()
        } else {
            None
        };
        let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
        assert!(futures::poll!(&mut waiting).is_pending());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            futures::poll!(&mut waiting).is_pending(),
            "disabled timeout must stay disabled"
        );
        drop(waiting);
        assert_no_attempts(&fixture);
    }
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_honors_time_already_spent_in_request_context() {
    let mut fixture = Fixture::without_transport().await;
    fixture.forwarder.non_streaming_timeout = Duration::from_millis(10);
    fixture.forwarder.request_started_at = Some(std::time::Instant::now() - Duration::from_secs(1));
    let _a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let _b = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[1])
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
    assert!(
        matches!(
            futures::poll!(&mut waiting),
            std::task::Poll::Ready(Err(ForwardError {
                error: ProxyError::Timeout(_),
                ..
            }))
        ),
        "admission must not create a new budget after handler work has spent it"
    );
    assert_no_attempts(&fixture);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_does_not_take_a_half_open_probe() {
    use super::super::circuit_breaker::CircuitBreakerConfig;
    let mut fixture = Fixture::without_transport().await;
    fixture.forwarder.current_provider_id_at_start = "other-current".into();
    let router = &fixture.forwarder.router;
    router.allow_provider_request("capacity-a", "claude").await;
    router
        .update_all_configs(CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await;
    router
        .record_result(
            "capacity-a",
            "claude",
            false,
            false,
            Some("synthetic failure".into()),
        )
        .await
        .unwrap();
    let _a = router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let _b = router
        .acquire_account_capacity(&fixture.providers[1])
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
    assert!(futures::poll!(&mut waiting).is_pending());
    let probe = router.allow_provider_request("capacity-a", "claude").await;
    assert!(
        probe.allowed && probe.used_half_open_permit,
        "queued request must leave the probe available"
    );
    router
        .release_permit_neutral("capacity-a", "claude", probe.used_half_open_permit)
        .await;
    drop(waiting);
    assert_no_attempts(&fixture);
}

// These tests exercise real admission / bookkeeping without an HTTP success fixture.
#[tokio::test]
#[serial]
async fn capacity_consumer_logic_busy_skips_to_b_with_one_attempt() {
    let fixture = Fixture::without_transport().await;
    let _busy_a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_millis(300),
        fixture.send(fixture.providers.clone(), Extensions::new()),
    )
    .await
    .expect("busy A must yield to available B without waiting for A");
    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("invalid synthetic URI cannot send"),
    };
    assert_eq!(
        error.provider.as_ref().map(|p| p.id.as_str()),
        Some("capacity-b")
    );
    let attempts: Vec<(String, i64)> = fixture
        .db
        .conn
        .lock()
        .unwrap()
        .prepare("SELECT provider_id,count(*) FROM provider_attempt_outcomes GROUP BY provider_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        attempts,
        vec![("capacity-b".into(), 1)],
        "Busy is not a send opportunity or retry"
    );
    let a = fixture
        .forwarder
        .router
        .get_circuit_breaker_stats("capacity-a", "claude")
        .await;
    assert!(a.is_none_or(|s| s.failed_requests == 0 && s.total_requests == 0));
}

#[tokio::test]
#[serial]
async fn capacity_consumer_logic_busy_success_keeps_current_and_counts_success() {
    let fixture = Fixture::without_transport().await;
    fixture.forwarder.status.write().await.total_requests = 1;
    fixture
        .forwarder
        .record_provider_success(
            "claude",
            "capacity-a",
            &fixture.providers[1],
            false,
            ProviderSelectionReason::BusyAlternative,
        )
        .await;
    assert_eq!(
        fixture
            .forwarder
            .failover_manager
            .join_spawned_for_test()
            .await,
        0,
        "capacity borrowing must not invoke the persistent switch worker"
    );
    assert_eq!(
        fixture.forwarder.current_providers.read().await["claude"].0,
        "capacity-a"
    );
    assert_eq!(
        fixture
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("capacity-a")
    );
    let status = fixture.forwarder.status.read().await;
    assert_eq!(status.failover_count, 0);
    assert_eq!(status.success_requests, 1);
    assert_eq!(status.success_rate, 100.0);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_logic_selected_config_change_while_waiting_is_terminal() {
    let fixture = Fixture::without_transport().await;
    let busy = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.send(vec![fixture.providers[0].clone()], Extensions::new()));
    assert!(futures::poll!(&mut waiting).is_pending());
    let mut changed = fixture.providers[0].clone();
    changed.meta.as_mut().unwrap().loongport_account_id = Some(99);
    fixture.db.save_provider("claude", &changed).unwrap();
    drop(busy);
    let result = waiting.await;
    assert!(
        matches!(result, Err(ForwardError { error:ProxyError::ConfigError(ref code), .. }) if code=="account_capacity.provider_changed")
    );
    assert_no_attempts(&fixture);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_logic_ordered_success_retains_existing_failover_behavior() {
    let fixture = Fixture::without_transport().await;
    fixture
        .forwarder
        .record_provider_success(
            "claude",
            "capacity-a",
            &fixture.providers[1],
            false,
            ProviderSelectionReason::Ordered,
        )
        .await;
    assert_eq!(
        fixture
            .forwarder
            .failover_manager
            .join_spawned_for_test()
            .await,
        1
    );
    assert_eq!(
        fixture.forwarder.current_providers.read().await["claude"].0,
        "capacity-b"
    );
    assert_eq!(fixture.forwarder.status.read().await.failover_count, 1);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_logic_unselected_row_removal_does_not_poison_a() {
    let fixture = Fixture::without_transport().await;
    fixture.db.delete_provider("claude", "capacity-b").unwrap();
    let result = fixture
        .send(fixture.providers.clone(), Extensions::new())
        .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("synthetic URI cannot send"),
    };
    assert_eq!(
        error.provider.as_ref().map(|p| p.id.as_str()),
        Some("capacity-a")
    );
    assert!(
        !matches!(error.error, ProxyError::ConfigError(ref code) if code=="account_capacity.provider_changed")
    );
    let attempts: i64 = fixture
        .db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM provider_attempt_outcomes WHERE provider_id='capacity-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 1);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_selected_row_removed_exits_before_slot_release() {
    let fixture = Fixture::without_transport().await;
    let _a = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[0])
        .await
        .unwrap();
    let _b = fixture
        .forwarder
        .router
        .acquire_account_capacity(&fixture.providers[1])
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
    assert!(futures::poll!(&mut waiting).is_pending());
    fixture.db.delete_provider("claude", "capacity-a").unwrap();
    let result = tokio::time::timeout(Duration::from_millis(300), waiting)
        .await
        .expect("selected row removal must cancel queued admission");
    assert!(
        matches!(result,Err(ForwardError {error:ProxyError::ConfigError(ref code),..}) if code=="account_capacity.provider_changed")
    );
    assert_no_attempts(&fixture);
}

#[tokio::test]
#[serial]
async fn capacity_consumer_queue_original_request_owner_revocation_is_terminal() {
    for remove_owner in [false, true] {
        let mut fixture = Fixture::without_transport().await;
        let service = crate::services::ProxyService::new(
            fixture.db.clone(),
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
        )
        .unwrap();
        fixture.forwarder.request_identity = Some(service.request_identity("claude").unwrap());
        fixture.forwarder.service_owner = Arc::downgrade(&service);
        let _a = fixture
            .forwarder
            .router
            .acquire_account_capacity(&fixture.providers[0])
            .await
            .unwrap();
        let _b = fixture
            .forwarder
            .router
            .acquire_account_capacity(&fixture.providers[1])
            .await
            .unwrap();
        let mut waiting = Box::pin(fixture.send(fixture.providers.clone(), Extensions::new()));
        assert!(futures::poll!(&mut waiting).is_pending());
        if remove_owner {
            drop(service);
        } else {
            service.invalidate_app_requests("claude").unwrap();
        }
        let result = tokio::time::timeout(Duration::from_millis(300), waiting)
            .await
            .expect("original app owner revocation must terminate the queue");
        let expected = if remove_owner {
            "proxy.request_owner_unavailable"
        } else {
            "proxy.request_identity_changed"
        };
        assert!(
            matches!(result,Err(ForwardError {error:ProxyError::ConfigError(ref code),..}) if code==expected)
        );
        assert_no_attempts(&fixture);
    }
}

#[tokio::test]
#[serial]
async fn capacity_consumer_logic_response_stream_retains_capacity_until_eof_error_or_drop() {
    use super::super::account_capacity::CapacityAdmission;
    for ending in ["eof", "error", "unread"] {
        let fixture = Fixture::without_transport().await;
        let guard = ActiveConnectionGuard::acquire(fixture.forwarder.status.clone()).await;
        guard
            .account_capacity
            .hold(
                fixture
                    .forwarder
                    .router
                    .acquire_account_capacity(&fixture.providers[1])
                    .await
                    .unwrap(),
            )
            .unwrap();
        let mut chunks = vec![Ok(Bytes::from_static(b"data: synthetic\n\n"))];
        if ending == "error" {
            chunks.push(Err(std::io::Error::other("synthetic stream failure")));
        }
        let mut stream = Box::pin(
            super::super::response_processor::create_logged_passthrough_stream(
                futures::stream::iter(chunks),
                "CapacityTest",
                None,
                super::super::handler_context::StreamingTimeoutConfig {
                    first_byte_timeout: 0,
                    idle_timeout: 0,
                },
                Some(guard),
                None,
                crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
                ProxyDiagnosticId::new(),
            ),
        );
        assert!(matches!(
            fixture
                .forwarder
                .router
                .try_acquire_account_capacity(&fixture.providers[1])
                .unwrap(),
            CapacityAdmission::Busy
        ));
        if ending != "unread" {
            assert!(stream.next().await.unwrap().is_ok());
            assert!(matches!(
                fixture
                    .forwarder
                    .router
                    .try_acquire_account_capacity(&fixture.providers[1])
                    .unwrap(),
                CapacityAdmission::Busy
            ));
            if ending == "error" {
                assert!(stream.next().await.unwrap().is_err());
            } else {
                assert!(stream.next().await.is_none());
            }
        }
        drop(stream);
        assert!(matches!(
            fixture
                .forwarder
                .router
                .try_acquire_account_capacity(&fixture.providers[1])
                .unwrap(),
            CapacityAdmission::Acquired(Some(_))
        ));
    }
}

#[tokio::test]
#[serial]
async fn capacity_consumer_review_available_candidate_keeps_original_attempt_timeout_contract() {
    for unscoped in [false, true] {
        let mut fixture = Fixture::without_transport().await;
        if unscoped {
            fixture.providers[0].meta = None;
            fixture
                .db
                .save_provider("claude", &fixture.providers[0])
                .unwrap();
        }
        // Prior request work or an earlier provider consumed this interval.
        // There is no capacity queue: the original per-attempt timeout still applies.
        fixture.forwarder.request_started_at =
            Some(std::time::Instant::now() - Duration::from_secs(1));
        fixture.forwarder.non_streaming_timeout = Duration::from_millis(10);
        let result = fixture
            .send(vec![fixture.providers[0].clone()], Extensions::new())
            .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("synthetic URI cannot send"),
        };
        assert!(
            !matches!(error.error, ProxyError::Timeout(ref code) if code=="account_capacity.deadline_exceeded"),
            "available candidates must retain the original per-attempt timeout contract"
        );
        let attempts: i64 = fixture
            .db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM provider_attempt_outcomes", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(attempts, 1);
    }
}

#[tokio::test]
#[serial]
async fn capacity_consumer_review_middle_busy_must_not_keep_a_failed_current() {
    for current_is_busy in [false, true] {
        let fixture = Fixture::without_transport().await;
        let mut c = fixture.providers[1].clone();
        c.id = "capacity-c".into();
        c.name = "capacity-c".into();
        c.meta.as_mut().unwrap().loongport_account_id = Some(3);
        fixture.db.save_provider("claude", &c).unwrap();
        let busy = if current_is_busy {
            &fixture.providers[0]
        } else {
            &fixture.providers[1]
        };
        let failed = if current_is_busy {
            &fixture.providers[1]
        } else {
            &fixture.providers[0]
        };
        let _held = fixture
            .forwarder
            .router
            .acquire_account_capacity(busy)
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .forwarder
                .router
                .try_acquire_account_capacity(busy)
                .unwrap(),
            super::super::account_capacity::CapacityAdmission::Busy
        ));
        fixture
            .forwarder
            .router
            .record_result(
                &failed.id,
                "claude",
                false,
                false,
                Some("synthetic retryable failure".into()),
            )
            .await
            .unwrap();
        let deferred = std::collections::VecDeque::from([busy]);
        let reason = fixture
            .forwarder
            .capacity_selection_reason(false, &deferred);
        fixture
            .forwarder
            .record_provider_success("claude", "capacity-a", &c, false, reason)
            .await;
        let expected_switches = if current_is_busy { 0 } else { 1 };
        assert_eq!(
            fixture
                .forwarder
                .failover_manager
                .join_spawned_for_test()
                .await,
            expected_switches,
            "only a still-busy original current makes the alternative temporary"
        );
        assert_eq!(
            fixture.forwarder.status.read().await.failover_count,
            expected_switches as u64
        );
        assert_eq!(
            fixture.forwarder.current_providers.read().await["claude"].0,
            if current_is_busy {
                "capacity-a"
            } else {
                "capacity-c"
            }
        );
    }
}

// Opt-in test marker: emitted just before the existing capacity wait.
#[derive(Clone)]
pub(super) struct QueueReached(pub Arc<tokio::sync::Notify>);

#[derive(Clone)]
pub(super) struct GateReached(pub Arc<tokio::sync::Notify>);

async fn expect_shared_fatal_while_queued_skips_candidate(noncurrent: bool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Clone)]
    struct Race {
        hits: Upstream,
        count: Arc<AtomicUsize>,
        first_arrived: Arc<Notify>,
        finish_first: Arc<Notify>,
        second_arrived: Arc<Notify>,
        finish_second: Arc<Notify>,
    }
    let upstream = Upstream(Arc::new(Mutex::new(Vec::new())));
    let race = Race {
        hits: upstream.clone(),
        count: Arc::new(AtomicUsize::new(0)),
        first_arrived: Arc::new(Notify::new()),
        finish_first: Arc::new(Notify::new()),
        second_arrived: Arc::new(Notify::new()),
        finish_second: Arc::new(Notify::new()),
    };
    let app = axum::Router::new().fallback(
        |State(s): State<Race>, uri: Uri, Json(body): Json<Value>| async move {
            s.hits.0.lock().unwrap().push((uri.to_string(), body));
            if s.count.fetch_add(1, Ordering::SeqCst) == 0 {
                s.first_arrived.notify_one();
                s.finish_first.notified().await;
                (axum::http::StatusCode::PAYMENT_REQUIRED, Json(json!({"error":{"type":"insufficient_quota","message":"synthetic"}}))).into_response()
            } else {
                s.second_arrived.notify_one();
                s.finish_second.notified().await;
                Json(json!({"id":"synthetic-response","type":"message","role":"assistant","model":"synthetic-model","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}})).into_response()
            }
        }).with_state(race.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut f = Fixture::build(upstream, format!("http://{address}"), Some(task)).await;
    f.providers[1].meta.as_mut().unwrap().loongport_account_id = Some(1);
    f.db.save_provider("claude", &f.providers[1]).unwrap();
    f.forwarder.legacy_multi_candidate = true;
    let snapshot = f
        .forwarder
        .router
        .select_providers_with_mode("claude", true)
        .await
        .unwrap();
    assert!(snapshot.iter().any(|p| p.id == "capacity-a"));
    assert!(snapshot.iter().any(|p| p.id == "capacity-b"));
    let a = f.providers[0].clone();
    let b = f.providers[1].clone();
    let mut c = b.clone();
    c.id = "capacity-c".into();
    c.name = "capacity-c".into();
    c.meta.as_mut().unwrap().loongport_account_id = Some(2);
    c.settings_config["env"]["ANTHROPIC_BASE_URL"] = json!(format!("http://{address}/capacity-c"));
    f.db.save_provider("claude", &c).unwrap();
    let held_c = f
        .forwarder
        .router
        .acquire_account_capacity(&c)
        .await
        .unwrap();
    let second_candidates = if noncurrent {
        vec![b.clone(), a.clone(), c.clone()]
    } else {
        vec![a.clone(), b.clone(), c.clone()]
    };
    let queue = Arc::new(Notify::new());
    let mut ext = Extensions::new();
    ext.insert(QueueReached(queue.clone()));
    let first = f.send(vec![a.clone()], Extensions::new());
    let observer = async {
        tokio::time::timeout(Duration::from_secs(2), race.first_arrived.notified())
            .await
            .unwrap();
        let mut second = Box::pin(f.send(second_candidates, ext));
        tokio::select! {
            _ = queue.notified() => {},
            result = &mut second => panic!("second completed before queue: {}", result.is_ok()),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("queue not reached"),
        }
        assert_eq!(race.count.load(Ordering::SeqCst), 1);
        race.finish_first.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .forwarder
                .router
                .fatal_current_open("claude", &b.id, &b)
                .await
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first request must commit fatal");
        drop(held_c);
        tokio::select! {
            _ = race.second_arrived.notified() => {},
            result = &mut second => panic!("second stopped before fallback transport: {}", result.is_ok()),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("fallback transport not reached"),
        }
        assert_eq!(
            f.hits()[1].0,
            "/capacity-c/v1/messages",
            "a candidate sharing the committed fatal owner must not send"
        );
        race.finish_second.notify_one();
        second
            .await
            .unwrap_or_else(|_| panic!("second request failed"))
    };
    let (first_result, second_result) = tokio::join!(first, observer);
    assert!(first_result.is_err());
    assert_eq!(second_result.provider.id, "capacity-c");
    assert_eq!(f.hits().len(), 2);
    assert!(
        matches!(
            f.forwarder.router.try_acquire_account_capacity(&b).unwrap(),
            super::super::account_capacity::CapacityAdmission::Acquired(Some(_))
        ),
        "the skipped shared-account candidate must release its capacity"
    );
}

#[tokio::test]
#[serial]
async fn red_snapshot_noncurrent_shared_fatal_while_queued_skips_to_fallback() {
    expect_shared_fatal_while_queued_skips_candidate(true).await;
}

#[tokio::test]
#[serial]
async fn red_snapshot_current_shared_fatal_while_queued_skips_to_fallback() {
    expect_shared_fatal_while_queued_skips_candidate(false).await;
}

#[tokio::test]
#[serial]
async fn characterize_snapshot_permitted_inflight_request_completes_after_fatal() {
    use tokio::sync::Notify;
    #[derive(Clone)]
    struct Inflight {
        hits: Upstream,
        arrived: Arc<Notify>,
        finish: Arc<Notify>,
    }
    let upstream = Upstream(Arc::new(Mutex::new(Vec::new())));
    let state = Inflight {
        hits: upstream.clone(),
        arrived: Arc::new(Notify::new()),
        finish: Arc::new(Notify::new()),
    };
    let app = axum::Router::new().fallback(|State(s): State<Inflight>, uri: Uri, Json(body): Json<Value>| async move {
        s.hits.0.lock().unwrap().push((uri.to_string(), body));
        s.arrived.notify_one();
        s.finish.notified().await;
        Json(json!({"id":"synthetic-response","type":"message","role":"assistant","model":"synthetic-model","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}))
    }).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut f = Fixture::build(upstream, format!("http://{address}"), Some(task)).await;
    f.providers[1].meta.as_mut().unwrap().loongport_account_id = Some(1);
    f.db.save_provider("claude", &f.providers[1]).unwrap();
    f.forwarder.legacy_multi_candidate = true;
    let a = f.providers[0].clone();
    let b = f.providers[1].clone();
    let request = f.send(vec![b.clone(), a.clone()], Extensions::new());
    let observer = async {
        tokio::time::timeout(Duration::from_secs(2), state.arrived.notified())
            .await
            .unwrap();
        // Model the completion callback of another previously permitted request.
        f.forwarder
            .router
            .record_fatal_result(&a.id, "claude", false, Some("synthetic 402".into()))
            .await
            .unwrap();
        assert!(
            f.forwarder
                .router
                .fatal_current_open("claude", &b.id, &b)
                .await
        );
        state.finish.notify_one();
    };
    let (result, ()) = tokio::join!(request, observer);
    let result = result.unwrap_or_else(|_| panic!("already-sent request failed"));
    assert_eq!(result.provider.id, "capacity-b");
    assert_eq!(f.hits().len(), 1);
}

#[tokio::test]
#[serial]
async fn send_gate_cancellation_while_waiting_for_breaker_releases_capacity() {
    use super::super::account_capacity::CapacityAdmission;
    let mut f = Fixture::without_transport().await;
    f.forwarder.legacy_multi_candidate = true;
    let b = f.providers[1].clone();
    let breaker = f.forwarder.router.test_breaker("claude:capacity-b").await;
    let locked = breaker.test_lock_transition().await;
    let marker = Arc::new(tokio::sync::Notify::new());
    let mut extensions = Extensions::new();
    extensions.insert(GateReached(marker.clone()));
    let mut request = Box::pin(f.send(vec![b.clone(), f.providers[0].clone()], extensions));
    tokio::select! {
        _ = marker.notified() => {},
        result = &mut request => panic!("request completed before the gate: {}", result.is_ok()),
        _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("gate was not reached"),
    }
    assert!(matches!(
        f.forwarder.router.try_acquire_account_capacity(&b).unwrap(),
        CapacityAdmission::Busy
    ));
    assert!(
        futures::poll!(&mut request).is_pending(),
        "request must be waiting on the real transition lock"
    );
    drop(request);
    assert!(matches!(
        f.forwarder.router.try_acquire_account_capacity(&b).unwrap(),
        CapacityAdmission::Acquired(Some(_))
    ));
    drop(locked);
    assert_no_attempts(&f);
}

#[tokio::test]
#[serial]
async fn send_gate_all_fatal_queued_candidates_release_capacity() {
    use super::super::account_capacity::CapacityAdmission;
    let mut f = Fixture::without_transport().await;
    f.forwarder.legacy_multi_candidate = true;
    let a = f.providers[0].clone();
    let b = f.providers[1].clone();
    let held_a = f
        .forwarder
        .router
        .acquire_account_capacity(&a)
        .await
        .unwrap();
    let held_b = f
        .forwarder
        .router
        .acquire_account_capacity(&b)
        .await
        .unwrap();
    let marker = Arc::new(tokio::sync::Notify::new());
    let mut extensions = Extensions::new();
    extensions.insert(QueueReached(marker.clone()));
    let mut request = Box::pin(f.send(vec![b.clone(), a.clone()], extensions));
    tokio::select! {
        _ = marker.notified() => {},
        result = &mut request => panic!("request completed before queue: {}", result.is_ok()),
        _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("queue was not reached"),
    }
    f.forwarder
        .router
        .record_fatal_result(&a.id, "claude", false, Some("synthetic".into()))
        .await
        .unwrap();
    f.forwarder
        .router
        .record_fatal_result(&b.id, "claude", false, Some("synthetic".into()))
        .await
        .unwrap();
    drop(held_a);
    drop(held_b);
    let result = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .expect("all-fatal request must terminate");
    assert!(result.is_err());
    for p in [&a, &b] {
        assert!(matches!(
            f.forwarder.router.try_acquire_account_capacity(p).unwrap(),
            CapacityAdmission::Acquired(Some(_))
        ));
    }
    assert!(f.hits().is_empty());
}

#[tokio::test]
#[serial]
async fn send_gate_group_route_preserves_existing_account_admission() {
    use super::super::provider_router::{GroupMemberSnapshot, GroupRouteSnapshot};
    let mut f = Fixture::new().await;
    f.providers[1].meta.as_mut().unwrap().loongport_account_id = Some(1);
    f.db.save_provider("claude", &f.providers[1]).unwrap();
    let a = f.providers[0].clone();
    let b = f.providers[1].clone();
    f.forwarder.legacy_multi_candidate = true;
    f.forwarder.group_route = Some(GroupRouteSnapshot {
        target_key: "synthetic-group".into(),
        automatic: true,
        revision: 1,
        members: [&b, &a]
            .into_iter()
            .map(|p| GroupMemberSnapshot {
                provider_id: p.id.clone(),
                model: "synthetic-model".into(),
                proof: None,
            })
            .collect(),
    });
    f.forwarder
        .router
        .record_fatal_result(&a.id, "claude", false, Some("synthetic".into()))
        .await
        .unwrap();
    assert!(
        f.forwarder
            .router
            .fatal_current_open("claude", &b.id, &b)
            .await
    );
    let result = f
        .send(vec![b, a], Extensions::new())
        .await
        .unwrap_or_else(|_| panic!("existing group admission should allow the sibling tier"));
    assert_eq!(result.provider.id, "capacity-b");
    assert_eq!(f.hits().len(), 1);
}
