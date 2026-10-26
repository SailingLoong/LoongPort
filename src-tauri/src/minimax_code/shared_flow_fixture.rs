//! Test-hook only: original native owner and original application HTTP handlers.
use super::{
    catalog::{self, Region},
    inference::FixtureTransport,
};
use crate::{
    app_config::AppType,
    provider::{Provider, ProviderMeta},
    services::{routing_groups as groups, shared_routing as routing},
    store::AppState,
};
use axum::{http::StatusCode, response::IntoResponse};
use futures::StreamExt;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn projection(app: &AppType, account: &str, model: &str) -> Provider {
    let config = if matches!(app, AppType::Claude) {
        json!({"env":{"ANTHROPIC_MODEL":model}})
    } else {
        json!({"auth":{},"config":format!("model = {model:?}\n")})
    };
    let mut provider = Provider::with_id(
        format!("{account}-{}", app.as_str()),
        "Synthetic native".into(),
        config,
        None,
    );
    provider.category = Some("custom".into());
    provider.meta=Some(serde_json::from_value::<ProviderMeta>(json!({"authBinding":{"source":"managed_account","authProvider":"minimax_code","accountId":account},"apiFormat":"anthropic","commonConfigEnabled":false})).unwrap());
    crate::services::provider::managed_inference::validate(app, &provider).unwrap();
    provider
}
pub(crate) async fn verify(state: &AppState, case: &str) {
    let owner = FixtureTransport::seed_accounts(
        state.db.secret_session(),
        &[
            ("synthetic-native-a", Region::Cn),
            ("synthetic-native-b", Region::Global),
        ],
    )
    .await
    .unwrap();
    let members = [
        ("synthetic-native-a", "MiniMax-M2.7"),
        ("synthetic-native-b", "MiniMax-M2.5"),
    ]
    .map(|(account, model)| groups::Member {
        source_id: crate::services::service_source_identity::native_source_id(
            "minimax_code",
            account,
            model,
        ),
        model: model.into(),
    });
    for app in [AppType::Claude, AppType::Codex] {
        for (account, model) in [
            ("synthetic-native-a", "MiniMax-M2.7"),
            ("synthetic-native-b", "MiniMax-M2.5"),
        ] {
            state
                .db
                .save_provider(app.as_str(), &projection(&app, account, model))
                .unwrap();
        }
    }
    let prepared_group = tokio::task::block_in_place(|| {
        routing::prepare_group(
            state,
            routing::GroupDraft {
                id: None,
                expected_revision: None,
                name: "Native shared".into(),
                members: members.to_vec(),
            },
        )
        .expect("verified original native projections must prepare one shared group")
    });
    if matches!(case, "preview_replace" | "refresh") {
        catalog::access(state.db.secret_session(), None, |doc, _| {
            let account = doc
                .accounts
                .iter_mut()
                .find(|a| a.id == "synthetic-native-a")
                .unwrap();
            if case == "preview_replace" {
                account.real_user_id = "synthetic-replaced-user".into();
            } else {
                account.credential_version += 1;
            }
            Ok(((), true))
        })
        .await
        .unwrap();
        if case == "preview_replace" {
            assert!(tokio::task::block_in_place(|| routing::confirm_group(
                state,
                prepared_group.clone()
            ))
            .is_err());
            assert!(
                groups::read(&state.db.conn.lock().unwrap(), &prepared_group.group.id)
                    .unwrap()
                    .is_none()
            );
            return;
        }
    }
    let group =
        tokio::task::block_in_place(|| routing::confirm_group(state, prepared_group)).unwrap();
    let calls = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let upstream_calls = calls.clone();
    let wire_headers = Arc::new(Mutex::new(Vec::new()));
    let captured_headers = wire_headers.clone();
    let upstream_db = state.db.clone();
    let upstream_case = case.to_string();
    let app=axum::Router::new().route("/{region}/mavis/api/v1/llm/v1/messages",axum::routing::post(move |axum::extract::Path(region):axum::extract::Path<String>,headers:axum::http::HeaderMap,axum::Json(body):axum::Json<serde_json::Value>| {
        let calls=upstream_calls.clone();
        let captured_headers=captured_headers.clone();
        let db=upstream_db.clone();
        let case=upstream_case.clone();
        async move {
            captured_headers.lock().unwrap().push(headers.clone());
            let auth=headers["authorization"].to_str().unwrap().to_owned();
            calls.lock().unwrap().push((region,auth.clone(),body["model"].as_str().unwrap().into()));
            if auth=="Bearer synthetic-native-a-access" && case=="replace" {
                catalog::access(db.secret_session(),None,|doc,_| {
                    doc.accounts.iter_mut().find(|a|a.id=="synthetic-native-b").unwrap().real_user_id="synthetic-replaced-user".into();
                    Ok(((),true))
                }).await.unwrap();
            }
            if auth=="Bearer synthetic-native-a-access" && case=="stream" {
                let prefix=concat!("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"synthetic-stream\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"MiniMax-M2.7\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n", "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n", "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"native-visible-prefix\"}}\n\n");
                let stream=futures::stream::once(async move {Ok::<_,std::io::Error>(prefix)}).chain(futures::stream::once(async {tokio::time::sleep(std::time::Duration::from_millis(80)).await;Err::<&str,_>(std::io::Error::other("synthetic interruption"))}));
                return axum::http::Response::builder().status(200).header("content-type","text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
            }
            if auth=="Bearer synthetic-native-a-access" && case!="revoke" { return (StatusCode::SERVICE_UNAVAILABLE,axum::Json(json!({"error":{"type":"api_error","message":"synthetic retryable"}}))).into_response(); }
            axum::Json(json!({"id":"synthetic-message","type":"message","role":"assistant","model":body["model"],"content":[{"type":"text","text":"native-fixture-ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}})).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transport = FixtureTransport::loopback(
        &owner,
        &format!("http://{}", listener.local_addr().unwrap()),
    )
    .await
    .unwrap();
    let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for app in [AppType::Claude, AppType::Codex] {
        if matches!(case, "replace" | "revoke" | "stream" | "recovery")
            && matches!(app, AppType::Codex)
        {
            continue;
        }
        let bootstrap = Provider::with_id(
            "synthetic-bootstrap".into(),
            "Bootstrap".into(),
            crate::relay::provider_config::settings_config_for(
                &app,
                "synthetic-bootstrap-key",
                "Fixture",
                "http://127.0.0.1:9",
                "bootstrap-model",
            )
            .unwrap(),
            None,
        );
        state.db.save_provider(app.as_str(), &bootstrap).unwrap();
        tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(state, app.clone(), &bootstrap.id)
        })
        .unwrap();
        state
            .proxy_service
            .set_takeover_for_app(app.as_str(), true)
            .await
            .unwrap();
        let target = if case == "manual" {
            groups::RouteTarget::Manual {
                source_id: members[0].source_id.clone(),
                model: members[0].model.clone(),
            }
        } else {
            groups::RouteTarget::OrderedGroup {
                group_id: group.id.clone(),
            }
        };
        let prepared =
            tokio::task::block_in_place(|| routing::prepare_target(state, &app, target)).unwrap();
        if case == "recovery" {
            let files = crate::mode::controller::files(&app).unwrap();
            let before = files
                .iter()
                .map(|f| std::fs::read(&f.path).ok())
                .collect::<Vec<_>>();
            let interrupted = tokio::task::block_in_place(|| {
                crate::mode::operation::failpoint::crash_at(Some("target"));
                let result = routing::apply_target(state, &app, prepared.clone());
                crate::mode::operation::failpoint::crash_at(None);
                result
            });
            assert!(interrupted.is_err());
            assert_eq!(
                tokio::task::block_in_place(|| routing::read_state(state, &app))
                    .unwrap()
                    .pending_request,
                Some(prepared.request.clone())
            );
            state.proxy_service.stop().await.unwrap();
            state.db.secret_session().set_blocked(true);
            state.db.secret_session().set_blocked(false);
            let reopened = AppState::new(Arc::new(
                crate::database::Database::init_with_secrets(state.db.secrets.clone()).unwrap(),
            ))
            .unwrap();
            let recovered = tokio::task::block_in_place(|| {
                routing::recover_target(&reopened, &app, prepared.request.clone())
            })
            .unwrap();
            assert_eq!(recovered.status, "completed");
            assert_eq!(
                files
                    .iter()
                    .map(|f| std::fs::read(&f.path).ok())
                    .collect::<Vec<_>>(),
                before
            );
            reopened.proxy_service.stop_with_restore().await.unwrap();
            upstream.abort();
            return;
        }
        let result =
            tokio::task::block_in_place(|| routing::apply_target(state, &app, prepared)).unwrap();
        assert_eq!(result.status, "completed");
        let proxy = crate::proxy::server::ProxyServer::new(
            crate::proxy::ProxyConfig::default(),
            state.db.clone(),
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
        );
        let router = proxy
            .build_router()
            .layer(axum::Extension(transport.clone()));
        let (path, mut body) = if matches!(app, AppType::Claude) {
            (
                "/v1/messages",
                json!({"model":"incoming-alias","max_tokens":16,"messages":[{"role":"user","content":"synthetic"}]}),
            )
        } else {
            (
                "/v1/responses",
                json!({"model":"incoming-alias","input":"synthetic","stream":false}),
            )
        };
        if case == "stream" {
            body["stream"] = json!(true);
        }
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer untrusted-client")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        if case == "revoke" {
            super::checkin::Engine {
                session: state.db.secret_session(),
                transport: &super::http::OfficialHttp,
            }
            .retire("synthetic-native-a")
            .await
            .unwrap();
            let mut chunks = response.into_body().into_data_stream();
            let mut bytes = Vec::new();
            let mut fenced = false;
            while let Some(chunk) = chunks.next().await {
                match chunk {
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(_) => {
                        fenced = true;
                        break;
                    }
                }
            }
            assert!(fenced);
            assert!(
                bytes.is_empty(),
                "retired owner must fence every payload byte"
            );
            continue;
        }
        if case == "stream" {
            assert_eq!(status, StatusCode::OK);
            let mut chunks = response.into_body().into_data_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = chunks.next().await {
                match chunk {
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(_) => break,
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("native-visible-prefix"));
            assert!(!text.contains("native-fixture-ok"));
            continue;
        }
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        if case == "replace" {
            assert!(status.is_client_error() || status.is_server_error());
            assert!(
                String::from_utf8_lossy(&body).contains("minimax_code.routing_identity_changed"),
                "must be captured owner identity rejection, not fixture capability denial: {}",
                String::from_utf8_lossy(&body)
            );
        } else if case == "manual" {
            assert!(
                status.is_server_error(),
                "{status} {}",
                String::from_utf8_lossy(&body)
            );
        } else {
            assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
            assert!(String::from_utf8_lossy(&body).contains("native-fixture-ok"));
        }
    }
    state.proxy_service.stop_with_restore().await.unwrap();
    upstream.abort();
    let headers = wire_headers.lock().unwrap();
    for row in headers.iter() {
        assert_eq!(row["user-agent"], "MiniMaxAgent");
        assert_eq!(row["x-mavis-agent-id"], "main");
        assert_ne!(row["authorization"], "Bearer untrusted-client");
        assert!(!row.contains_key("x-api-key"));
    }
    if matches!(case, "ordered" | "refresh") {
        assert_ne!(
            headers[0]["x-mavis-session-id"],
            headers[1]["x-mavis-session-id"]
        );
    }

    let a = (
        "cn".into(),
        "Bearer synthetic-native-a-access".into(),
        "MiniMax-M2.7".into(),
    );
    let b = (
        "global".into(),
        "Bearer synthetic-native-b-access".into(),
        "MiniMax-M2.5".into(),
    );
    assert_eq!(
        *calls.lock().unwrap(),
        if matches!(case, "replace" | "revoke" | "stream") {
            vec![a]
        } else if case == "manual" {
            vec![a.clone(), a]
        } else {
            vec![a.clone(), b.clone(), a, b]
        }
    );
}
