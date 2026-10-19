//! Original vendor binding -> reviewed routing revision -> actual ProxyServer.
//! Only upstream HTTP is synthetic. This covers existing apps, not empty-app bootstrap.
use super::*;
use crate::{
    app_config::AppType,
    proxy::{
        application_routing,
        opencode_vendor_fixture::{FixtureOwner, FixtureTransport},
    },
    services::{application_selection as selection, ProviderService},
};
use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;

#[derive(Clone, Copy)]
enum Behavior {
    Complete,
    FirstUnavailable,
}
#[derive(Clone)]
struct Capture {
    headers: HeaderMap,
    body: Value,
}
struct Fixture {
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<Mutex<Vec<Capture>>>,
    upstream: tokio::task::JoinHandle<()>,
    owner: FixtureOwner,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl Fixture {
    async fn new(behavior: Behavior) -> Self {
        Self::with_owner(behavior, FixtureOwner::new()).await
    }
    async fn with_owner(behavior: Behavior, owner: FixtureOwner) -> Self {
        let state = owner.state();
        assert!(state.db.get_current_provider("codex").unwrap().is_none());
        assert!(application_routing::chain_ids(&state.db, "codex")
            .unwrap()
            .is_empty());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captures = calls.clone();
        let first_key = owner.synthetic_key(0);
        let app = axum::Router::new().route("/zen/v1/responses", axum::routing::post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let calls = captures.clone();
            async move {
                let first = headers.get("authorization").and_then(|h| h.to_str().ok()) == Some(format!("Bearer {first_key}").as_str());
                calls.lock().unwrap().push(Capture { headers, body: body.clone() });
                if first && matches!(behavior, Behavior::FirstUnavailable) {
                    return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({"error":{"type":"server_error","message":"synthetic unavailable"}}))).into_response();
                }
                axum::Json(json!({"id":"resp_synthetic","object":"response","status":"completed","model":body["model"],"output":[{"id":"msg_synthetic","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"synthetic vendor answer","annotations":[]}]}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = owner.transport(listener.local_addr().unwrap()).unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut config = state.db.get_proxy_config().await.unwrap();
        config.listen_address = "127.0.0.1".into();
        config.listen_port = 0;
        state.db.update_proxy_config(config).await.unwrap();
        // Existing-app setup uses the original direct selection and ENTER owners.
        // Any native files and synthetic credential projection remain inside TestHome.
        // The original synchronous Codex writer drives its own runtime. Match
        // the command's blocking boundary instead of nesting it in this Tokio task.
        let switch_state = state.clone();
        let first_id = owner.ids()[0].clone();
        tokio::task::spawn_blocking(move || {
            ProviderService::switch(&switch_state, AppType::Codex, &first_id).unwrap();
        })
        .await
        .unwrap();
        state
            .proxy_service
            .set_takeover_for_app("codex", true)
            .await
            .unwrap();
        let mut config = state.db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        config.max_retries = 2;
        config.non_streaming_timeout = 5;
        config.streaming_first_byte_timeout = 3;
        state.db.update_proxy_config_for_app(config).await.unwrap();
        let proxy = ProxyServer::new(
            ProxyConfig::default(),
            state.db.clone(),
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
        );
        let f = Self {
            proxy,
            transport,
            calls,
            upstream,
            owner,
        };
        let count = f.owner.state().db.get_all_providers("codex").unwrap().len();
        let positions = (0..count).collect::<Vec<_>>();
        f.apply(&positions, Some(0), "notRecorded").await;
        f
    }
    async fn apply(&self, positions: &[usize], selected: Option<usize>, before_status: &str) {
        let state = self.owner.state();
        let ids = positions
            .iter()
            .map(|&i| self.owner.ids()[i].clone())
            .collect::<Vec<_>>();
        let change = selection::ApplicationRoutingChange {
            order: Some(selection::RoutingOrder {
                profile_name: "default".into(),
                provider_ids: ids.clone(),
            }),
            selection: selected.map(|i| selection::TierSelection {
                provider_id: self.owner.ids()[i].clone(),
                model: None,
            }),
            catalog_decision: None,
            catalog_preview_only: false,
        };
        let request = selection::prepare_routing_request(
            state,
            &AppType::Codex,
            &change,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        // The original owner retains only the last receipt: another completed
        // request means unknown; only the first request is notRecorded.
        assert_eq!(
            selection::query_routing_request(state, &AppType::Codex, request.clone())
                .unwrap()
                .status,
            before_status
        );
        let apply_state = state.clone();
        let apply_request = request.clone();
        tokio::task::spawn_blocking(move || {
            // Keep the original app lock held around the original revision CAS
            // and synchronous mode writer; only the execution boundary changes.
            let _lock =
                futures::executor::block_on(apply_state.proxy_service.lock_switch_for_app("codex"));
            let (result, _) = selection::apply_requested_routing_locked(
                &apply_state,
                &AppType::Codex,
                &change,
                Some(false),
                &apply_request,
                false,
                |action| {
                    Ok((
                        action()?,
                        crate::relay::chatgpt_app::AroundOutcome::default(),
                    ))
                },
            )
            .unwrap();
            assert_eq!(result["status"], "switched");
        })
        .await
        .unwrap();
        assert_eq!(
            selection::query_routing_request(state, &AppType::Codex, request)
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            application_routing::chain_ids(&state.db, "codex").unwrap(),
            ids
        );
    }
    async fn request(&self, transport: Option<FixtureTransport>) -> axum::response::Response {
        let mut router = self.proxy.build_router();
        if let Some(transport) = transport {
            router = router.layer(axum::Extension(transport));
        }
        let body = json!({"model":self.owner.model(),"input":"synthetic prompt","stream":false});
        tokio::time::timeout(
            Duration::from_secs(8),
            router.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer untrusted-client-key")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }
    fn inventory_has_no_keys(&self) {
        let rows = self.owner.state().db.get_all_providers("codex").unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows.values() {
            let serialized = serde_json::to_string(row).unwrap();
            for i in 0..2 {
                assert!(!serialized.contains(self.owner.synthetic_key(i)));
            }
        }
    }
    fn vendor_snapshot(&self) -> Vec<(i64, String, String, String)> {
        let conn = self.owner.state().db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id,vendor_id,account_id,api_key FROM loongport_vendor ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }
    async fn stop(&self) {
        self.owner
            .state()
            .proxy_service
            .set_takeover_for_app("codex", false)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn existing_vendor_keys_follow_explicit_revision_selection_without_provisioning() {
    let f = Fixture::new(Behavior::Complete).await;
    let before = f.vendor_snapshot();
    for index in 0..2 {
        f.apply(&[0, 1], Some(index), "unknown").await;
        let response = f.request(Some(f.transport.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("synthetic vendor answer"));
        let captures = f.calls.lock().unwrap();
        assert_eq!(captures.len(), index + 1);
        assert_eq!(
            captures[index].headers["authorization"],
            format!("Bearer {}", f.owner.synthetic_key(index))
        );
        assert_eq!(captures[index].body["model"], f.owner.model());
        assert_ne!(
            captures[index].headers["authorization"],
            "Bearer untrusted-client-key"
        );
    }
    assert_eq!(f.vendor_snapshot(), before);
    f.inventory_has_no_keys();
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn retryable_http_failure_uses_only_the_next_applied_vendor_account() {
    let f = Fixture::new(Behavior::FirstUnavailable).await;
    let before = f.vendor_snapshot();
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("synthetic vendor answer"));
    {
        let captures = f.calls.lock().unwrap();
        assert_eq!(captures.len(), 2);
        for i in 0..2 {
            assert_eq!(
                captures[i].headers["authorization"],
                format!("Bearer {}", f.owner.synthetic_key(i))
            );
        }
    }
    assert_eq!(f.vendor_snapshot(), before);
    f.inventory_has_no_keys();
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn saved_but_unapplied_vendor_account_is_not_a_failover_candidate() {
    let f = Fixture::new(Behavior::FirstUnavailable).await;
    f.apply(&[0], Some(0), "unknown").await;
    let response = f.request(Some(f.transport.clone())).await;
    assert!(!response.status().is_success());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.inventory_has_no_keys();
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn missing_fixture_and_changed_session_epoch_fail_before_upstream_http() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(None).await;
    assert!(!response.status().is_success());
    assert!(f.calls.lock().unwrap().is_empty());
    let session = f.owner.state().db.secret_session();
    session.set_blocked(true);
    session.set_blocked(false);
    let response = f.request(Some(f.transport.clone())).await;
    assert!(!response.status().is_success());
    assert!(f.calls.lock().unwrap().is_empty());
    f.stop().await;
}

/// Expected RED against the current owner: an absent priority falls back to all rows.
/// Kept separate from the explicitly initialized-chain router tests above.
#[test]
#[serial_test::serial]
fn inactive_binding_preserves_the_preinsert_chain_when_priority_is_missing() {
    let owner = FixtureOwner::with_uninitialized_chain();
    assert!(owner
        .state()
        .db
        .get_current_provider("codex")
        .unwrap()
        .is_none());
    assert!(
        application_routing::chain_ids(&owner.state().db, "codex")
            .unwrap()
            .is_empty(),
        "binding an inactive source must not silently make it a fallback candidate"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn binding_missing_priority_invalidates_an_older_routing_revision_without_replay() {
    let f = Fixture::with_owner(Behavior::Complete, FixtureOwner::with_one_binding()).await;
    let state = f.owner.state();
    // Model a restored modern library whose routing priority is absent. This is
    // a test precondition, not a production mutation or a fake startup guarantee.
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM settings WHERE key='application_priority_codex'",
            [],
        )
        .unwrap();
    let change = selection::ApplicationRoutingChange {
        order: Some(selection::RoutingOrder {
            profile_name: "default".into(),
            provider_ids: vec![f.owner.ids()[0].clone()],
        }),
        selection: None,
        catalog_decision: None,
        catalog_preview_only: false,
    };
    let request = selection::prepare_routing_request(
        state,
        &AppType::Codex,
        &change,
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    assert!(crate::services::vendor_source_binding::bind_vendor_source(
        &state.db,
        &f.owner.ids()[1],
        &AppType::Codex
    )
    .unwrap());
    {
        let _lock = state.proxy_service.lock_switch_for_app("codex").await;
        let (result, selected) = selection::apply_requested_routing_locked(
            state,
            &AppType::Codex,
            &change,
            Some(false),
            &request,
            false,
            |_action| panic!("stale routing request must not execute external effects"),
        )
        .unwrap();
        assert_eq!(result["status"], "requestResolved");
        assert_eq!(result["result"]["status"], "unknown");
        assert!(selected.is_none());
    }
    assert_eq!(
        application_routing::chain_ids(&state.db, "codex").unwrap(),
        vec![f.owner.ids()[0].clone()]
    );
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.inventory_has_no_keys();
    f.stop().await;
}

#[cfg(all(test, feature = "gui"))]
#[path = "vendor_response_fence_tests.rs"]
mod vendor_response_fence_tests;
