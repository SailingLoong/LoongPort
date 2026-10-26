//! Synthetic original ZCode account owner -> safe shared source inventory.
//! No restore, grant mutation, refresh, claim, or official network operation.
use crate::{
    app_config::AppType,
    database::Database,
    provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta},
    secrets::testing::TestHome,
    services::provider::managed_inference,
    store::AppState,
    zcode_accounts::inference::FixtureTransport,
};
use serde_json::json;
use std::sync::Arc;
const MODEL: &str = "synthetic-zcode-shared-model";

pub(crate) async fn verify(case: &str) {
    let plaintext = case.starts_with("plaintext_");
    let case = case.strip_prefix("plaintext_").unwrap_or(case);
    let source_case = case.strip_prefix("source_context_");
    let case = match source_case {
        Some("native" | "selected" | "none") => "two_apps",
        Some("stream") => "stream_retire",
        Some(_) => panic!("unknown source-context fixture"),
        None => case,
    };
    assert!(matches!(
        case,
        "source"
            | "manual"
            | "ordered"
            | "two_apps"
            | "legacy"
            | "missing_transport"
            | "stream_retire"
            | "mixed"
    ));
    let _home = TestHome::new().unwrap();
    crate::settings::reload_settings().unwrap();
    let db = Arc::new(if plaintext {
        crate::services::native_source_guard::storage_fixture::fresh_plaintext_database()
    } else {
        Database::memory().unwrap()
    });
    crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
    let owner = FixtureTransport::seed_accounts(&db.secrets).await.unwrap();
    const NATIVE: &str = "11111111-2222-4333-8444-555555555555";
    const SELECTED: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const NEXT: &str = "99999999-8888-4777-8666-555555555555";
    let effective = match source_case {
        Some("native") => Some(NATIVE),
        Some("selected" | "stream") => Some(SELECTED),
        _ => None,
    };
    if source_case.is_some() {
        owner.seed_source_fixture(
            &db,
            (source_case != Some("none")).then_some(NATIVE),
            matches!(source_case, Some("selected" | "stream")).then_some(SELECTED),
        );
    }
    let _routing_capability = owner.install_routing_capability(&db.secrets).unwrap();
    let account = owner.account_id(0).to_owned();
    for index in 0..2 {
        for app in [AppType::Claude, AppType::Codex] {
            let mut provider = Provider::with_id(
                projection_id(index, app.as_str()),
                "Synthetic original ZCode projection".into(),
                match app {
                    AppType::Claude => json!({"env":{"ANTHROPIC_MODEL":MODEL}}),
                    AppType::Codex => json!({"auth":{},"config":format!("model = {MODEL:?}\n")}),
                    _ => unreachable!(),
                },
                None,
            );
            provider.category = Some("custom".into());
            provider.meta = Some(ProviderMeta {
                common_config_enabled: Some(false),
                api_format: Some("anthropic".into()),
                auth_binding: Some(AuthBinding {
                    source: AuthBindingSource::ManagedAccount,
                    auth_provider: Some("zcode_start_plan".into()),
                    account_id: Some(owner.account_id(index).to_owned()),
                }),
                ..Default::default()
            });
            managed_inference::validate(&app, &provider).unwrap();
            db.save_provider(app.as_str(), &provider).unwrap();
        }
    }
    let mm_owner = if case == "mixed" {
        let owner = crate::minimax_code::inference::FixtureTransport::seed_accounts(
            &db.secrets,
            &[(
                "synthetic-zcode-mm-b",
                crate::minimax_code::catalog::Region::Global,
            )],
        )
        .await
        .unwrap();
        for app in [AppType::Claude, AppType::Codex] {
            let mut row = db
                .get_provider_by_id(&projection_id(1, app.as_str()), app.as_str())
                .unwrap()
                .unwrap();
            row.id = format!("synthetic-mm-{}", app.as_str());
            row.settings_config = if app == AppType::Claude {
                json!({"env":{"ANTHROPIC_MODEL":"MiniMax-M2.7"}})
            } else {
                json!({"auth":{},"config":"model = \"MiniMax-M2.7\"\n"})
            };
            let binding = row.meta.as_mut().unwrap().auth_binding.as_mut().unwrap();
            binding.auth_provider = Some("minimax_code".into());
            binding.account_id = Some("synthetic-zcode-mm-b".into());
            managed_inference::validate(&app, &row).unwrap();
            db.save_provider(app.as_str(), &row).unwrap();
        }
        Some(owner)
    } else {
        None
    };
    let state = AppState::new(db.clone()).unwrap();
    let sources = crate::commands::get_service_sources_impl(&state).unwrap();
    let source = sources
        .iter()
        .find(|source| {
            source
                .bindings
                .iter()
                .any(|binding| binding.provider_id == "synthetic-zcode-codex")
        })
        .expect("original authorized ZCode account must project a shared source");
    assert_eq!(
        source.bindings.len(),
        2,
        "two existing projections share one canonical identity"
    );
    for app in ["claude", "codex"] {
        let binding = source
            .bindings
            .iter()
            .find(|binding| binding.app_id == app)
            .unwrap();
        assert_eq!(binding.provider_id, format!("synthetic-zcode-{app}"));
        assert_eq!(binding.model.as_deref(), Some(MODEL));
    }
    let wire = serde_json::to_value(source).unwrap();
    assert_eq!(wire["account"]["kind"], "nativeAccount");
    assert_eq!(wire["account"]["platform"], "zcode_start_plan");
    assert_eq!(wire["account"]["id"], account);
    assert!(wire["account"].get("region").is_none());
    let encoded = serde_json::to_string(&wire).unwrap();
    for index in 0..2 {
        assert!(!encoded.contains(owner.synthetic_token(index)));
    }
    if source_case.is_some() {
        for id in [NATIVE, SELECTED, NEXT] {
            assert!(
                !encoded.contains(id),
                "source DTO must not expose device identity"
            );
        }
    }
    if case == "source" {
        for app in [AppType::Claude, AppType::Codex] {
            let overview = crate::commands::application_overview(&state, &app).unwrap();
            for index in 0..2 {
                let configuration = overview
                    .configurations
                    .iter()
                    .find(|row| row.provider_id == projection_id(index, app.as_str()))
                    .expect("original ZCode projection must remain visible in the overview");
                assert_eq!(configuration.service_name.as_deref(), Some("ZCode"));
            }
        }
        return;
    }
    use crate::services::{routing_groups as groups, shared_routing as routing};
    use axum::{
        http::{HeaderMap, StatusCode, Uri},
        response::IntoResponse,
    };
    let mut members = (0..2)
        .map(|index| {
            let source = sources
                .iter()
                .find(|source| {
                    source
                        .bindings
                        .iter()
                        .any(|binding| binding.provider_id == projection_id(index, "codex"))
                })
                .unwrap();
            groups::Member {
                source_id: source.source_id.clone(),
                model: MODEL.into(),
            }
        })
        .collect::<Vec<_>>();
    if case == "mixed" {
        let mm = sources
            .iter()
            .find(|source| {
                source
                    .bindings
                    .iter()
                    .any(|binding| binding.provider_id == "synthetic-mm-codex")
            })
            .expect("original MiniMax owner must remain independently available");
        members[1] = groups::Member {
            source_id: mm.source_id.clone(),
            model: "MiniMax-M2.7".into(),
        };
    }
    assert_ne!(members[0].source_id, members[1].source_id);
    let tokens = [
        owner.synthetic_token(0).to_owned(),
        owner.synthetic_token(1).to_owned(),
    ];
    let calls = Arc::new(std::sync::Mutex::new(Vec::<(
        String,
        HeaderMap,
        serde_json::Value,
    )>::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    let upstream_release = release.clone();
    let streaming = case == "stream_retire";
    let captured = calls.clone();
    let first_token = tokens[0].clone();
    let fail_first = matches!(case, "manual" | "ordered" | "mixed");
    let upstream = axum::Router::new().fallback(axum::routing::post(move |uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<serde_json::Value>| {
        let captured = captured.clone(); let first_token = first_token.clone(); let release = upstream_release.clone();
        async move {
            let first = headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(first_token.as_str());
            captured.lock().unwrap().push((uri.path().into(), headers, body.clone()));
            if first && streaming {
                let prefix = format!("event: message_start\ndata: {}\n\nevent: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\n", json!({"type":"message_start","message":{"id":"msg_zcode_stream","type":"message","role":"assistant","model":MODEL,"content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}),json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"original-visible-zcode-prefix.".repeat(64)}}));
                let output = async_stream::stream! {
                    yield Ok::<_, std::io::Error>(axum::body::Bytes::from(prefix));
                    release.notified().await;
                    yield Ok(axum::body::Bytes::from(format!("event: content_block_delta\ndata: {}\n\nevent: content_block_stop\ndata: {}\n\nevent: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"forbidden-after-account-retired"}}),json!({"type":"content_block_stop","index":0}),json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),json!({"type":"message_stop"}))));
                };
                return axum::http::Response::builder().status(200).header("content-type","text/event-stream").body(axum::body::Body::from_stream(output)).unwrap();
            }

            if first && fail_first {
                return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({"type":"error","error":{"type":"api_error","message":"synthetic A unavailable"}}))).into_response();
            }
            axum::Json(json!({"id":"msg_synthetic_zcode","type":"message","role":"assistant","model":body["model"],"content":[{"type":"text","text":"original ZCode shared answer"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":2}})).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let transport = FixtureTransport::loopback(&owner, &origin).unwrap();
    let mm_transport = if let Some(owner) = &mm_owner {
        Some(
            crate::minimax_code::inference::FixtureTransport::loopback(owner, &origin)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap()).unwrap();
    let mut proxy_config = db.get_proxy_config().await.unwrap();
    proxy_config.listen_address = "127.0.0.1".into();
    proxy_config.listen_port = 0;
    db.update_proxy_config(proxy_config).await.unwrap();
    let apps = if case == "two_apps" {
        vec![AppType::Claude, AppType::Codex]
    } else if case == "stream_retire" {
        vec![AppType::Claude]
    } else {
        vec![AppType::Codex]
    };
    for app in &apps {
        super::application_routing::migrate(&db, app.as_str()).unwrap();
        {
            let vault = db.secrets.read().unwrap();
            crate::mode::state::update_app(
                &crate::live::engine::DeviceStore::for_device(),
                &vault,
                app.as_str(),
                |entry| {
                    entry.mode = Some(crate::mode::state::Mode::Direct);
                    Ok(())
                },
            )
            .unwrap();
        }
        let bootstrap = Provider::with_id(
            format!("synthetic-bootstrap-{}", app.as_str()),
            "Synthetic connection bootstrap".into(),
            crate::relay::provider_config::settings_config_for(
                app,
                "synthetic-bootstrap-key",
                "Bootstrap",
                "http://127.0.0.1:9",
                "bootstrap-model",
            )
            .unwrap(),
            None,
        );
        db.save_provider(app.as_str(), &bootstrap).unwrap();
        tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(&state, app.clone(), &bootstrap.id)
        })
        .unwrap();
        state
            .proxy_service
            .set_takeover_for_app(app.as_str(), true)
            .await
            .unwrap();
        let mut config = db.get_proxy_config_for_app(app.as_str()).await.unwrap();
        config.max_retries = 0;
        config.non_streaming_timeout = 5;
        db.update_proxy_config_for_app(config).await.unwrap();
    }
    if case == "legacy" {
        tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(
                &state,
                AppType::Codex,
                &projection_id(0, "codex"),
            )
        })
        .unwrap();
        assert!(groups::binding(&db.conn.lock().unwrap(), "codex")
            .unwrap()
            .is_none());
        assert_eq!(
            super::application_routing::current_provider_id_checked(&db, "codex")
                .unwrap()
                .as_deref(),
            Some(projection_id(0, "codex").as_str())
        );
        assert_eq!(db.get_current_provider("codex").unwrap().as_deref(),Some("synthetic-bootstrap-codex"),"legacy direct pointer remains dormant while original mode owner selects the proxy route");
    }
    let before = serde_json::to_value([
        db.get_all_providers("claude").unwrap(),
        db.get_all_providers("codex").unwrap(),
    ])
    .unwrap();
    if case != "legacy" {
        let reviewed = tokio::task::block_in_place(|| {
            routing::prepare_group(
                &state,
                routing::GroupDraft {
                    id: None,
                    expected_revision: None,
                    name: "Original ZCode shared group".into(),
                    members: members.clone(),
                },
            )
        })
        .unwrap();
        let group =
            tokio::task::block_in_place(|| routing::confirm_group(&state, reviewed)).unwrap();
        for app in &apps {
            let target = if case == "manual" {
                groups::RouteTarget::Manual {
                    source_id: members[0].source_id.clone(),
                    model: MODEL.into(),
                }
            } else {
                groups::RouteTarget::OrderedGroup {
                    group_id: group.id.clone(),
                }
            };
            let prepared = tokio::task::block_in_place(|| {
                routing::prepare_target(&state, app, target.clone())
            })
            .unwrap();
            let request = prepared.request.clone();
            assert_eq!(
                tokio::task::block_in_place(|| routing::apply_target(&state, app, prepared))
                    .unwrap()
                    .status,
                "completed"
            );
            assert_eq!(
                tokio::task::block_in_place(|| routing::query_target(&state, app, request))
                    .unwrap()
                    .status,
                "completed"
            );
            assert_eq!(
                groups::binding(&db.conn.lock().unwrap(), app.as_str())
                    .unwrap()
                    .unwrap()
                    .target,
                target
            );
        }
    }
    let proxy = super::server::ProxyServer::new(
        super::ProxyConfig::default(),
        db.clone(),
        None,
        crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
        Arc::new(super::model_alignment::ModelAlignmentAlerts::new()),
    );
    if case == "stream_retire" {
        use futures::StreamExt;
        let response = request(&proxy, &AppType::Claude, Some(&transport), None, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut chunks = response.into_body().into_data_stream();
        let mut output = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&output).contains("original-visible-zcode-prefix.") {
                output.extend_from_slice(
                    &chunks
                        .next()
                        .await
                        .expect("actual prefix required")
                        .expect("prefix must be visible before retirement"),
                );
            }
        })
        .await
        .unwrap();
        if source_case.is_some() {
            owner.assert_source_fixture_headers(&calls.lock().unwrap()[0].1, effective);
            owner.seed_source_fixture(&db, Some(NEXT), Some(NEXT));
        }
        transport
            .retire_first_fixture_account(&db.secrets)
            .await
            .unwrap();
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(chunk) = chunks.next().await {
                match chunk {
                    Ok(bytes) => output.extend_from_slice(&bytes),
                    Err(_) => break,
                }
            }
        })
        .await
        .unwrap();
        let old_calls = calls.lock().unwrap().len();
        // Fresh B with the SAME capability distinguishes production output fencing
        // from an indiscriminate test transport-capability rejection.
        let prepared = tokio::task::block_in_place(|| {
            routing::prepare_target(
                &state,
                &AppType::Claude,
                groups::RouteTarget::Manual {
                    source_id: members[1].source_id.clone(),
                    model: MODEL.into(),
                },
            )
        })
        .unwrap();
        assert_eq!(
            tokio::task::block_in_place(|| routing::apply_target(
                &state,
                &AppType::Claude,
                prepared
            ))
            .unwrap()
            .status,
            "completed"
        );
        let fresh = request(&proxy, &AppType::Claude, Some(&transport), None, false).await;
        assert_eq!(fresh.status(), StatusCode::OK);
        let fresh_body = axum::body::to_bytes(fresh.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&fresh_body).contains("original ZCode shared answer"));
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), old_calls + 1);
            assert_eq!(calls.last().unwrap().1["x-api-key"], tokens[1]);
            assert_eq!(calls.last().unwrap().2["model"], MODEL);
            if source_case.is_some() {
                for (_, headers, _) in calls.iter() {
                    owner.assert_source_fixture_headers(headers, effective);
                }
            }
        }
        assert_eq!(old_calls, 1, "committed A stream cannot replay on B");
        let text = String::from_utf8_lossy(&output);
        assert!(!text.contains("forbidden-after-account-retired"));
        assert!(!text.contains("message_stop"));
        assert_eq!(transport.attempts(), 2);
        assert_eq!(
            serde_json::to_value([
                db.get_all_providers("claude").unwrap(),
                db.get_all_providers("codex").unwrap()
            ])
            .unwrap(),
            before
        );
        state.proxy_service.stop().await.unwrap();
        server.abort();
        return;
    }
    for app in &apps {
        let response = request(
            &proxy,
            app,
            (case != "missing_transport").then_some(&transport),
            mm_transport.as_ref(),
            false,
        )
        .await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        if matches!(case, "manual" | "missing_transport") {
            assert!(!status.is_success(), "{text}");
        } else {
            assert_eq!(status, StatusCode::OK, "{text}");
            assert!(text.contains("original ZCode shared answer"));
        }
        if source_case.is_some() {
            owner
                .assert_source_fixture_headers(&calls.lock().unwrap().last().unwrap().1, effective);
            // Mutate only generated next-session settings/observed native state.
            // A second real HTTP request must retain the first effective choice.
            owner.seed_source_fixture(&db, Some(NEXT), Some(NEXT));
        }
    }
    let expected_count = match case {
        "manual" | "legacy" => 1,
        "missing_transport" => 0,
        _ => 2,
    };
    assert_eq!(
        transport.attempts(),
        if case == "mixed" { 1 } else { expected_count }
    );
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), expected_count);
        for (index, (path, headers, body)) in calls.iter().enumerate() {
            if case == "mixed" && index == 1 {
                assert_eq!(path, "/global/mavis/api/v1/llm/v1/messages");
                assert_eq!(
                    headers["authorization"],
                    "Bearer synthetic-zcode-mm-b-access"
                );
                assert_eq!(headers["user-agent"], "MiniMaxAgent");
                assert_eq!(body["model"], "MiniMax-M2.7");
                continue;
            }
            let account_index = if case == "ordered" { index } else { 0 };
            assert_eq!(path, "/api/v1/zcode-plan/anthropic/v1/messages");
            assert_eq!(headers["x-api-key"], tokens[account_index]);
            assert_eq!(
                headers["authorization"],
                format!("Bearer {}", tokens[account_index])
            );
            assert_eq!(headers["anthropic-version"], "2023-06-01");
            assert_eq!(headers["accept-encoding"], "identity");
            assert_eq!(body["model"], MODEL);
            assert!(body["messages"].is_array());
        }
    }
    assert_eq!(
        serde_json::to_value([
            db.get_all_providers("claude").unwrap(),
            db.get_all_providers("codex").unwrap()
        ])
        .unwrap(),
        before,
        "publication and requests must not copy credentials or mutate projections"
    );
    if case == "legacy" {
        assert!(groups::binding(&db.conn.lock().unwrap(), "codex")
            .unwrap()
            .is_none());
    }
    state.proxy_service.stop().await.unwrap();
    server.abort();
}
fn projection_id(index: usize, app: &str) -> String {
    if index == 0 {
        format!("synthetic-zcode-{app}")
    } else {
        format!("synthetic-zcode-b-{app}")
    }
}

async fn request(
    proxy: &super::server::ProxyServer,
    app: &AppType,
    transport: Option<&FixtureTransport>,
    mm: Option<&crate::minimax_code::inference::FixtureTransport>,
    streaming: bool,
) -> axum::response::Response {
    use tower::ServiceExt;
    let body = if *app == AppType::Claude {
        json!({"model":"client-alias","max_tokens":32,"stream":streaming,"messages":[{"role":"user","content":"synthetic shared request"}]})
    } else {
        json!({"model":"client-alias","stream":streaming,"input":"synthetic shared request"})
    };
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(if *app == AppType::Claude {
            "/v1/messages"
        } else {
            "/v1/responses"
        })
        .header("content-type", "application/json")
        .header("authorization", "Bearer synthetic-untrusted-client")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let mut router = proxy.build_router();
    if let Some(transport) = transport {
        router = router.layer(axum::Extension(transport.clone()));
    }
    if let Some(mm) = mm {
        router = router.layer(axum::Extension(mm.clone()));
    }
    router.oneshot(req).await.unwrap()
}
