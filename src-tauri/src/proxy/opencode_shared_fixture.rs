//! Shared routing composition over the original sealed OpenCode vendor fixture.
//! Only upstream replies are synthetic; no alternate credential/account owner.
use crate::{
    app_config::AppType,
    proxy::{
        opencode_vendor_fixture::{FixtureOwner, FixtureTransport},
        server::ProxyServer,
        ProxyConfig,
    },
    services::{routing_groups as groups, shared_routing as routing, ProviderService},
};
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;

struct Capture {
    headers: HeaderMap,
    body: Value,
    path: String,
}
struct Fixture {
    owner: FixtureOwner,
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<Mutex<Vec<Capture>>>,
    upstream: tokio::task::JoinHandle<()>,
    release: Arc<tokio::sync::Notify>,
    tail_waiting: Arc<tokio::sync::Notify>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl Fixture {
    async fn new(
        personal: bool,
        fail_first: bool,
        streaming: bool,
        epoch_tail: bool,
        reseal_before_b: bool,
        plaintext: bool,
    ) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let owner = if plaintext {
            assert!(personal);
            FixtureOwner::personal_go_plaintext()
        } else if personal {
            FixtureOwner::personal_go()
        } else {
            FixtureOwner::new()
        };
        crate::settings::unlock_settings_for_test(owner.state().db.secrets.clone()).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captures = calls.clone();
        let first_key = owner.synthetic_key(0);
        let second_key = owner.synthetic_key(1);
        let original_db = owner.state().db.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let upstream_release = release.clone();
        let tail_waiting = Arc::new(tokio::sync::Notify::new());
        let upstream_waiting = tail_waiting.clone();
        let upstream_router = axum::Router::new().fallback(axum::routing::post(move |uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let captures = captures.clone();
            let release = upstream_release.clone();
            let waiting = upstream_waiting.clone();
            let db = original_db.clone();
            async move {
                let first = headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(format!("Bearer {first_key}").as_str()) || headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(first_key);
                captures.lock().unwrap().push(Capture { headers, body: body.clone(), path: uri.path().into() });
                if first && reseal_before_b {
                    // Reissue through the original owner with the same bytes. The
                    // existing fence explicitly retires a snapshot on fresh nonce;
                    // the sealed fixture's fixed key remains valid for a NEW request.
                    let vault = db.secrets.read().unwrap();
                    crate::vendor::creds::set_api_key(&db.conn.lock().unwrap(), &vault, 702, second_key).unwrap();
                }
                if first && fail_first { return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({"error":{"type":"server_error","message":"synthetic unavailable"}}))).into_response(); }
                if streaming {
                    let stream = async_stream::stream! {
                        yield Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.2\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"shared-visible-prefix.\"},\"finish_reason\":null}]}\n\n"));
                        waiting.notify_one();
                        release.notified().await;
                        if epoch_tail {
                            yield Ok(axum::body::Bytes::from_static(b"data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.2\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"forbidden-after-epoch.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"));
                        } else {
                            yield Err(std::io::Error::other("synthetic connection interrupted after visible output"));
                        }
                    };
                    return axum::http::Response::builder().status(200).header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                let value = match uri.path() {
                    "/zen/v1/responses" => json!({"id":"resp_shared","object":"response","status":"completed","model":body["model"],"output":[{"id":"msg_shared","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"shared original vendor answer","annotations":[]}]}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}),
                    "/inference/go/openai/v1/chat/completions" => json!({"id":"chatcmpl_shared","object":"chat.completion","model":body["model"],"choices":[{"index":0,"message":{"role":"assistant","content":"shared original vendor answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}),
                    "/inference/go/anthropic/v1/messages" => json!({"id":"msg_shared","type":"message","role":"assistant","model":body["model"],"content":[{"type":"text","text":"shared original vendor answer"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":2}}),
                    _ => return StatusCode::NOT_FOUND.into_response(),
                };
                axum::Json(value).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = owner.transport(listener.local_addr().unwrap()).unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, upstream_router).await.unwrap();
        });
        let state = owner.state();
        let mut config = state.db.get_proxy_config().await.unwrap();
        config.listen_address = "127.0.0.1".into();
        config.listen_port = 0;
        state.db.update_proxy_config(config).await.unwrap();
        let proxy = ProxyServer::new(
            ProxyConfig::default(),
            state.db.clone(),
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
        );
        Self {
            owner,
            proxy,
            transport,
            calls,
            upstream,
            release,
            tail_waiting,
        }
    }
    async fn connect(&self, app: &AppType, selected: usize) {
        let state = self.owner.state();
        let switching = state.clone();
        let id = self.owner.ids()[selected].clone();
        let switching_app = app.clone();
        tokio::task::spawn_blocking(move || {
            ProviderService::switch(&switching, switching_app, &id).unwrap()
        })
        .await
        .unwrap();
        state
            .proxy_service
            .set_takeover_for_app(app.as_str(), true)
            .await
            .unwrap();
        let mut config = state
            .db
            .get_proxy_config_for_app(app.as_str())
            .await
            .unwrap();
        config.max_retries = 0;
        config.non_streaming_timeout = 5;
        state.db.update_proxy_config_for_app(config).await.unwrap();
    }
    async fn bind(&self, app: AppType, target: groups::RouteTarget, selected: usize) {
        self.connect(&app, selected).await;
        let state = self.owner.state();
        let prepared =
            tokio::task::block_in_place(|| routing::prepare_target(state, &app, target.clone()))
                .unwrap();
        let request = prepared.request.clone();
        assert_eq!(
            tokio::task::block_in_place(|| routing::apply_target(state, &app, prepared))
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            tokio::task::block_in_place(|| routing::query_target(state, &app, request))
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            groups::binding(&state.db.conn.lock().unwrap(), app.as_str())
                .unwrap()
                .unwrap()
                .target,
            target
        );
    }
    async fn request_raw(
        &self,
        app: &AppType,
        session: bool,
        streaming: bool,
    ) -> axum::response::Response {
        let mut body = if *app == AppType::Claude {
            json!({"model":"caller-model-must-not-win","messages":[{"role":"user","content":"synthetic prompt"}],"max_tokens":32,"stream":false})
        } else {
            json!({"model":"caller-model-must-not-win","input":"synthetic prompt","stream":false})
        };
        body["stream"] = json!(streaming);
        if session {
            body["prompt_cache_key"] = json!("ses_shared_original_123456");
            body["metadata"] = json!({"user_id":"{\"session_id\":\"ses_shared_original_123456\"}"});
        }
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(if *app == AppType::Claude {
                "/v1/messages"
            } else {
                "/v1/responses"
            })
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(8),
            self.proxy
                .build_router()
                .layer(axum::Extension(self.transport.clone()))
                .oneshot(request),
        )
        .await
        .unwrap()
        .unwrap();
        response
    }
    async fn request(&self, app: &AppType, session: bool) -> (StatusCode, String) {
        let response = self.request_raw(app, session, false).await;
        let status = response.status();
        let bytes = tokio::time::timeout(
            Duration::from_secs(6),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }
}

pub(crate) async fn verify(case: &str) {
    if case == "free_config" {
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Hermes,
            AppType::OpenClaw,
        ] {
            assert!(
                crate::vendor::opencode::Plan::Free
                    .config_for(&app)
                    .is_none(),
                "do not provision external free-model routes"
            );
        }
        let bundles = crate::vendor::provision::plan_rows_for(
            crate::vendor::Vendor::OpenCode,
            "synthetic-workspace",
            "synthetic-one-key",
        );
        let free = bundles
            .iter()
            .find(|bundle| bundle.plan.id_segment == "opencode-free")
            .unwrap();
        assert_eq!(free.rows.len(), 1);
        assert_eq!(free.rows[0].0, AppType::OpenCode);
        assert_eq!(free.rows[0].1["freeOnly"], true);
        assert_eq!(
            free.rows[0].1["modelCatalog"]["models"]
                .as_array()
                .unwrap()
                .len(),
            crate::vendor::opencode::FREE_CHAT_MODELS.len()
        );
        assert_eq!(
            crate::relay::provider_config::extract_api_key(&free.rows[0].1, &AppType::OpenCode)
                .as_deref(),
            Some("synthetic-one-key")
        );
        assert!(
            crate::vendor::opencode::Plan::Free
                .config_for(&AppType::OpenCode)
                .is_some(),
            "native official OpenCode direct configuration is not proxy eligibility"
        );
        for plan in [
            crate::vendor::opencode::Plan::Zen,
            crate::vendor::opencode::Plan::Go,
        ] {
            assert!(plan.config_for(&AppType::Codex).is_some());
        }
        return;
    }
    if case.starts_with("policy_") {
        super::free_models::verify_external_policy(case);
        return;
    }
    let personal = case.starts_with("personal");
    let fail_first = case.ends_with("ordered")
        || case.ends_with("manual_failure")
        || case == "personal_snapshot_reseal";
    let streaming = matches!(
        case,
        "personal_stream"
            | "personal_stream_epoch"
            | "personal_stream_sql_restore"
            | "personal_stream_same_owner_control"
    );
    let f = Fixture::new(
        personal,
        fail_first,
        streaming,
        matches!(
            case,
            "personal_stream_epoch"
                | "personal_stream_sql_restore"
                | "personal_stream_same_owner_control"
        ),
        case == "personal_snapshot_reseal",
        matches!(
            case,
            "personal_stream_sql_restore" | "personal_stream_same_owner_control"
        ),
    )
    .await;
    let state = f.owner.state();
    if matches!(
        case,
        "runtime_free_fallback"
            | "runtime_unknown_override"
            | "personal_runtime_free_fallback"
            | "personal_runtime_beam_go"
    ) {
        let app = AppType::Codex;
        let mut provider = state
            .db
            .get_provider_by_id(&f.owner.ids()[0], "codex")
            .unwrap()
            .unwrap();
        provider
            .meta
            .as_mut()
            .unwrap()
            .local_proxy_request_overrides = Some(crate::provider::LocalProxyRequestOverrides {
            headers: Default::default(),
            body: Some(if case == "personal_runtime_beam_go" {
                json!({"model":"beam-free"})
            } else if case == "runtime_unknown_override" {
                json!({"model":"synthetic-unclassified-model"})
            } else {
                json!({"models":["glm-5.3-flash-free"]})
            }),
        });
        state.db.save_provider("codex", &provider).unwrap();
        f.connect(&app, 0).await;
        let before = serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap();
        let (status, text) = f.request(&app, true).await;
        if case == "personal_runtime_beam_go" {
            assert_eq!(status, StatusCode::OK, "{text}");
            let calls = f.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].body["model"], "beam-free");
            assert_eq!(calls[0].path, "/inference/go/openai/v1/chat/completions");
            assert_eq!(
                calls[0].headers["authorization"],
                format!("Bearer {}", f.owner.synthetic_key(0))
            );
            assert_eq!(
                calls[0].headers["x-opencode-session"],
                "ses_shared_original_123456"
            );
        } else {
            assert!(
                f.calls.lock().unwrap().is_empty(),
                "final external policy must reject before any synthetic vendor POST"
            );
            assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
            assert!(text.contains("opencode.external_"));
        }
        assert_eq!(
            serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap(),
            before,
            "classification must not rewrite models, providers or priorities"
        );
        assert!(groups::binding(&state.db.conn.lock().unwrap(), "codex")
            .unwrap()
            .is_none());
        state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "free_legacy_inventory" {
        let provider_id = crate::vendor::provision::provider_id_for(
            "opencode-free",
            "synthetic-opencode-workspace-a",
        );
        // Authentic pre-contraction Free shape, not a call to the now-disabled generator.
        let settings = crate::relay::provider_config::settings_config_with_roles_and_models(
            &AppType::Codex,
            f.owner.synthetic_key(0),
            "Existing Free",
            crate::vendor::opencode::FREE_API_ORIGIN,
            crate::vendor::opencode::FREE_CHAT_MODELS[0],
            None,
            None,
            crate::vendor::opencode::Plan::Free.style(),
        )
        .unwrap();
        let mut row = crate::provider::Provider::with_id(
            provider_id.clone(),
            "Existing Free".into(),
            settings,
            None,
        );
        row.meta = Some(crate::provider::ProviderMeta {
            loongport_vendor_account: Some("synthetic-opencode-workspace-a".into()),
            ..Default::default()
        });
        state.db.save_provider("codex", &row).unwrap();
        let before = serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap();
        let accounts = crate::commands::list_vendor_accounts(state, &AppType::Codex).unwrap();
        let plan = accounts
            .accounts
            .iter()
            .find(|row| row.id == 701)
            .unwrap()
            .plans
            .iter()
            .find(|plan| plan.plan_id == "opencode-free")
            .unwrap();
        assert!(
            !plan.can_switch,
            "existing external Free config must not remain selectable"
        );
        assert_eq!(
            serde_json::to_value(plan).unwrap()["switchBlockedReason"],
            "opencodeFreeExternalUnavailable"
        );
        assert!(
            plan.can_edit_config,
            "saved configuration remains editable rather than deleted"
        );
        let overview = crate::commands::application_overview(state, &AppType::Codex).unwrap();
        assert!(
            !overview
                .configurations
                .iter()
                .find(|row| row.provider_id == provider_id)
                .unwrap()
                .can_select
        );
        assert_eq!(
            serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap(),
            before
        );
        return;
    }
    if case == "workspace_legacy" {
        f.connect(&AppType::Codex, 0).await;
        assert!(groups::binding(&state.db.conn.lock().unwrap(), "codex")
            .unwrap()
            .is_none());
        let request = axum::http::Request::builder().method("POST").uri("/v1/responses").header("content-type", "application/json").body(axum::body::Body::from(json!({"model":f.owner.model(),"input":"synthetic original legacy request","stream":false}).to_string())).unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(8),
            f.proxy
                .build_router()
                .layer(axum::Extension(f.transport.clone()))
                .oneshot(request),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = tokio::time::timeout(
            Duration::from_secs(6),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("shared original vendor answer"));
        {
            let calls = f.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].headers["authorization"],
                format!("Bearer {}", f.owner.synthetic_key(0))
            );
            assert_eq!(calls[0].body["model"], f.owner.model());
            assert_eq!(calls[0].path, "/zen/v1/responses");
        }
        assert!(
            groups::binding(&state.db.conn.lock().unwrap(), "codex")
                .unwrap()
                .is_none(),
            "original path cannot silently publish a shared target"
        );
        state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "personal_two_apps" {
        for (app, expected) in [
            (AppType::Claude, "deepseek-v4-pro"),
            (AppType::Codex, "glm-5.2"),
        ] {
            for id in f.owner.ids() {
                let original = state
                    .db
                    .get_provider_by_id(id, app.as_str())
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    crate::relay::provider_config::selected_model(&app, &original.settings_config)
                        .as_deref(),
                    Some(expected),
                    "cross-app fixture preserves original different defaults"
                );
            }
        }
    }
    let model = if matches!(case, "personal_two_apps" | "personal_catalog_removed") {
        "deepseek-v4-pro".to_string()
    } else if case == "personal_unknown_model" {
        "not-a-real-original-catalog-model".to_string()
    } else if personal {
        "glm-5.2".to_string()
    } else {
        f.owner.model()
    };
    let mut members = f
        .owner
        .ids()
        .iter()
        .map(|id| groups::Member {
            source_id: id.clone(),
            model: model.clone(),
        })
        .collect::<Vec<_>>();
    if case == "personal_all_empty" {
        members.truncate(1);
    }
    // The original vault remains the only key owner before and after publication.
    let before = serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap();
    let prepared = tokio::task::block_in_place(|| {
        routing::prepare_group(
            state,
            routing::GroupDraft {
                id: None,
                expected_revision: None,
                name: "Original OpenCode shared group".into(),
                members: members.clone(),
            },
        )
    });
    if case == "personal_unknown_model" {
        assert!(
            prepared.is_err(),
            "unknown model cannot become a shared source"
        );
        assert!(f.calls.lock().unwrap().is_empty());
        return;
    }
    let prepared = prepared.unwrap();
    if case == "personal_stale_group" {
        let vault = state.db.secrets.read().unwrap();
        crate::vendor::creds::set_api_key(
            &state.db.conn.lock().unwrap(),
            &vault,
            701,
            "oc_sk_fixture_ChangedSyntheticCredential1234",
        )
        .unwrap();
        drop(vault);
        assert!(
            tokio::task::block_in_place(|| routing::confirm_group(state, prepared)).is_err(),
            "credential replacement must invalidate the reviewed source facts"
        );
        assert!(f.calls.lock().unwrap().is_empty());
        return;
    }
    let group = tokio::task::block_in_place(|| routing::confirm_group(state, prepared)).unwrap();
    let manual = case.ends_with("manual_failure") || case == "personal_key_revoked";
    let target = if manual {
        groups::RouteTarget::Manual {
            source_id: members[0].source_id.clone(),
            model: model.clone(),
        }
    } else {
        groups::RouteTarget::OrderedGroup {
            group_id: group.id.clone(),
        }
    };
    f.bind(
        AppType::Codex,
        target.clone(),
        usize::from(case == "personal_all_empty"),
    )
    .await;
    if matches!(
        case,
        "personal_policy_group_free" | "personal_policy_group_unknown"
    ) {
        let mut provider = state
            .db
            .get_provider_by_id(&f.owner.ids()[0], "codex")
            .unwrap()
            .unwrap();
        provider
            .meta
            .as_mut()
            .unwrap()
            .local_proxy_request_overrides = Some(crate::provider::LocalProxyRequestOverrides {
            headers: Default::default(),
            body: Some(
                json!({"models":[if case.ends_with("_free") { "glm-5.3-flash-free" } else { "synthetic-unclassified-model" }]}),
            ),
        });
        state.db.save_provider("codex", &provider).unwrap();
        let saved = serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap();
        let (status, text) = f.request(&AppType::Codex, true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("opencode.external_"));
        assert!(
            f.calls.lock().unwrap().is_empty(),
            "policy refusal must not send to A or fall through to healthy paid B"
        );
        assert_eq!(
            serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap(),
            saved
        );
        assert_eq!(
            groups::binding(&state.db.conn.lock().unwrap(), "codex")
                .unwrap()
                .unwrap()
                .target,
            target
        );
        state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "personal_snapshot_reseal" {
        let (old_status, _) = f.request(&AppType::Codex, true).await;
        let old_calls = f.calls.lock().unwrap().len();
        let next = groups::RouteTarget::Manual {
            source_id: members[1].source_id.clone(),
            model: model.clone(),
        };
        let prepared =
            tokio::task::block_in_place(|| routing::prepare_target(state, &AppType::Codex, next))
                .unwrap();
        assert_eq!(
            tokio::task::block_in_place(|| routing::apply_target(state, &AppType::Codex, prepared))
                .unwrap()
                .status,
            "completed"
        );
        let (new_status, new_body) = f.request(&AppType::Codex, true).await;
        assert_eq!(
            new_status,
            StatusCode::OK,
            "new owner must remain usable: {new_body}"
        );
        assert!(new_body.contains("shared original vendor answer"));
        {
            let calls = f.calls.lock().unwrap();
            assert_eq!(calls.len(), old_calls + 1);
            assert_eq!(
                calls.last().unwrap().headers["authorization"],
                format!("Bearer {}", f.owner.synthetic_key(1)),
                "fresh request really consumed B, not a fixture denial"
            );
            assert_eq!(calls.last().unwrap().body["model"], model);
        }
        assert!(!old_status.is_success(), "old group snapshot must not consume a reissued B credential even though a new request can");
        assert_eq!(
            old_calls, 1,
            "only A may be submitted before B identity changes"
        );
        assert_eq!(
            f.calls.lock().unwrap()[0].headers["authorization"],
            format!("Bearer {}", f.owner.synthetic_key(0))
        );
        state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "personal_glm_messages_unsupported" {
        f.connect(&AppType::Claude, 0).await;
        assert!(
            tokio::task::block_in_place(|| routing::prepare_target(
                state,
                &AppType::Claude,
                target
            ))
            .is_err(),
            "local union catalog is not proof of GLM Messages protocol support"
        );
        assert!(f.calls.lock().unwrap().is_empty());
        state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "personal_catalog_removed" {
        let prepared =
            tokio::task::block_in_place(|| routing::prepare_target(state, &AppType::Codex, target))
                .unwrap();
        for id in f.owner.ids() {
            let mut provider = state.db.get_provider_by_id(id, "codex").unwrap().unwrap();
            let rows = provider
                .settings_config
                .pointer_mut("/modelCatalog/models")
                .and_then(Value::as_array_mut)
                .expect("original vendor projection catalog");
            let before_len = rows.len();
            rows.retain(|row| row["model"] != "deepseek-v4-pro");
            assert!(
                rows.len() < before_len,
                "remove an actual original catalog row, not a synthetic nonexistent one"
            );
            assert!(!crate::relay::model_catalog::available_models(&provider)
                .iter()
                .any(|m| m == "deepseek-v4-pro"));
            state.db.save_provider("codex", &provider).unwrap();
        }
        assert!(
            tokio::task::block_in_place(|| routing::apply_target(state, &AppType::Codex, prepared))
                .is_err(),
            "catalog removal invalidates reviewed member qualification"
        );
        let (status, _) = f.request(&AppType::Codex, true).await;
        assert!(
            !status.is_success(),
            "saved intent cannot dispatch a model removed from original catalog"
        );
        assert!(f.calls.lock().unwrap().is_empty());
        state.proxy_service.stop().await.unwrap();
        return;
    }
    let apps = if case == "personal_two_apps" {
        f.bind(AppType::Claude, target.clone(), 0).await;
        vec![AppType::Codex, AppType::Claude]
    } else {
        vec![AppType::Codex]
    };
    assert_eq!(
        serde_json::to_value(state.db.get_all_providers("codex").unwrap()).unwrap(),
        before,
        "publication must not copy keys or rewrite source projections"
    );
    assert_eq!(
        state
            .db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM loongport_vendor", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2,
        "shared publication cannot create more account/key rows"
    );
    if case == "personal_stale_target" {
        let prepared =
            tokio::task::block_in_place(|| routing::prepare_target(state, &AppType::Codex, target))
                .unwrap();
        let vault = state.db.secrets.read().unwrap();
        crate::vendor::creds::set_api_key(
            &state.db.conn.lock().unwrap(),
            &vault,
            701,
            "oc_sk_fixture_ChangedSyntheticCredential1234",
        )
        .unwrap();
        drop(vault);
        assert!(
            tokio::task::block_in_place(|| routing::apply_target(state, &AppType::Codex, prepared))
                .is_err(),
            "credential replacement must reject stale app review"
        );
        assert!(f.calls.lock().unwrap().is_empty());
        state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "personal_empty_later" {
        // Existing reviewed group, later account becomes unusable before a NEW
        // request. Capturing its identity must not lease it or block healthy A.
        let vault = state.db.secrets.read().unwrap();
        crate::vendor::creds::set_api_key(&state.db.conn.lock().unwrap(), &vault, 702, "").unwrap();
    }
    if matches!(
        case,
        "personal_key_revoked" | "personal_empty_first" | "personal_all_empty"
    ) {
        let vault = state.db.secrets.read().unwrap();
        crate::vendor::creds::set_api_key(&state.db.conn.lock().unwrap(), &vault, 701, "").unwrap();
    }
    if case == "personal_all_empty" {
        assert_eq!(
            state.db.get_current_provider("codex").unwrap().as_deref(),
            Some(f.owner.ids()[1].as_str()),
            "healthy original legacy B is outside this one-member group"
        );
    }
    if case == "personal_corrupt_first" {
        state.db.conn.lock().unwrap().execute("UPDATE loongport_vendor SET api_key='synthetic-nonempty-corrupt-envelope' WHERE id=701", []).unwrap();
    }
    if case == "personal_unknown_first" {
        crate::vendor::creds::remove(&state.db.conn.lock().unwrap(), 701).unwrap();
    }
    if case == "personal_web_logout" {
        crate::vendor::creds::clear_token(&state.db.conn.lock().unwrap(), 701).unwrap();
    }
    if case == "personal_epoch" {
        state.db.secrets.set_blocked(true);
        state.db.secrets.set_blocked(false);
    }
    if streaming {
        use futures::StreamExt;
        let restore_proofs = if matches!(
            case,
            "personal_stream_sql_restore" | "personal_stream_same_owner_control"
        ) {
            Some(
                crate::services::native_source_guard::storage_fixture::seed_restore_proofs(
                    state.db.clone(),
                )
                .await,
            )
        } else {
            None
        };
        let response = f.request_raw(&AppType::Codex, true, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&bytes).contains("shared-visible-prefix.") {
                bytes.extend_from_slice(
                    &body
                        .next()
                        .await
                        .expect("visible prefix missing")
                        .expect("failed before visible output"),
                );
            }
        })
        .await
        .expect("original consumer must commit visible output before failure");
        if case == "personal_stream_epoch" {
            state.db.secrets.set_blocked(true);
            state.db.secrets.set_blocked(false);
        }
        if matches!(
            case,
            "personal_stream_sql_restore" | "personal_stream_same_owner_control"
        ) {
            tokio::time::timeout(Duration::from_secs(5), f.tail_waiting.notified())
                .await
                .unwrap();
            assert!(
                futures::poll!(body.next()).is_pending(),
                "tail must actually be gated before restore"
            );
            let proofs = restore_proofs.as_ref().unwrap();
            proofs.assert_current().await;
            let original_epoch = state.db.secrets.lifecycle_epoch();
            let sql = state.db.export_sql_string().unwrap();
            state
                .db
                .set_setting("synthetic_real_restore_marker", "after-snapshot")
                .unwrap();
            proofs.assert_current().await;
            assert!(state.db.secrets.read().unwrap().is_plaintext());
            let consumed_identity = || {
                state.db.conn.lock().unwrap().query_row(
                    "SELECT account_id,api_key,api_key_revision FROM loongport_vendor WHERE id=701", [],
                    |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)),
                ).unwrap()
            };
            let consumed_before = consumed_identity();
            let provider_before = serde_json::to_value(
                state
                    .db
                    .get_provider_by_id(&f.owner.ids()[0], "codex")
                    .unwrap(),
            )
            .unwrap();
            if case == "personal_stream_sql_restore" {
                tokio::task::block_in_place(|| state.db.import_sql_string(&sql)).unwrap();
                assert!(
                    state
                        .db
                        .get_setting("synthetic_real_restore_marker")
                        .unwrap()
                        .is_none(),
                    "real SQL image must actually replace the receiver"
                );
                assert!(state.db.secrets.lifecycle_epoch() > original_epoch);
                proofs.assert_same_credentials(&state.db);
                proofs.assert_retired().await;
                proofs.assert_fresh(state.db.clone()).await;
            } else {
                proofs.assert_same_credentials(&state.db);
                proofs.assert_current().await;
                assert_eq!(state.db.secrets.lifecycle_epoch(), original_epoch);
            }
            assert!(
                consumed_identity() == consumed_before,
                "actual streamed vendor account, key and revision remain byte-identical"
            );
            assert!(
                serde_json::to_value(
                    state
                        .db
                        .get_provider_by_id(&f.owner.ids()[0], "codex")
                        .unwrap()
                )
                .unwrap()
                    == provider_before,
                "actual streamed original provider remains identical"
            );
        }
        f.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(chunk) = body.next().await {
                match chunk {
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(_) => break,
                }
            }
        })
        .await
        .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        if case == "personal_stream_same_owner_control" {
            assert!(
                text.contains("forbidden-after-epoch"),
                "unchanged owner must admit the exact gated tail"
            );
            assert!(
                text.contains("response.completed"),
                "control proves the original producer's tail is complete"
            );
        } else {
            assert!(
                !text.contains("forbidden-after-epoch"),
                "retired session must suppress later original upstream content"
            );
            assert!(
                !text.contains("response.completed"),
                "partial response must not become completed"
            );
        }
        assert!(
            !text.contains("synthetic connection interrupted"),
            "private upstream failure must not leak"
        );
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "visible output must never replay on B");
        assert_eq!(calls[0].body["model"], model);
        assert_eq!(
            calls[0].headers["authorization"],
            format!("Bearer {}", f.owner.synthetic_key(0))
        );
        assert_eq!(
            calls[0].headers["x-opencode-session"],
            "ses_shared_original_123456"
        );
        drop(calls);
        state.proxy_service.stop().await.unwrap();
        return;
    }
    for app in &apps {
        let (status, body) = f.request(app, case != "personal_missing_session").await;
        if manual
            || matches!(
                case,
                "personal_epoch"
                    | "personal_missing_session"
                    | "personal_all_empty"
                    | "personal_corrupt_first"
                    | "personal_unknown_first"
            )
        {
            assert!(
                !status.is_success(),
                "must reject without another source: {body}"
            );
        } else {
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(body.contains("shared original vendor answer"));
        }
    }
    {
        let calls = f.calls.lock().unwrap();
        let expected = if matches!(
            case,
            "personal_epoch"
                | "personal_missing_session"
                | "personal_key_revoked"
                | "personal_all_empty"
                | "personal_corrupt_first"
                | "personal_unknown_first"
        ) {
            0
        } else if fail_first && !manual {
            2
        } else {
            apps.len()
        };
        assert_eq!(calls.len(), expected);
        for (i, call) in calls.iter().enumerate() {
            assert_eq!(call.body["model"], model);
            let index = if case == "personal_empty_first" {
                1
            } else if fail_first && !manual {
                i
            } else {
                0
            };
            let key = f.owner.synthetic_key(index);
            assert!(
                call.headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    == Some(format!("Bearer {key}").as_str())
                    || call.headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(key)
            );
            if personal {
                assert_eq!(
                    call.headers["x-opencode-session"],
                    "ses_shared_original_123456"
                );
                assert!(call.path.starts_with("/inference/go/"));
            } else {
                assert_eq!(call.path, "/zen/v1/responses");
            }
        }
    }
    assert_eq!(
        groups::read(&state.db.conn.lock().unwrap(), &group.id)
            .unwrap()
            .unwrap()
            .members,
        members
    );
    state.proxy_service.stop().await.unwrap();
}
