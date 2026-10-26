//! Synthetic original WorkBuddy and MiniMax owners, original shared router only.
use super::inference::FixtureTransport as WorkBuddyTransport;
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

fn projection(app: &AppType, platform: &str, account: &str, model: &str) -> Provider {
    let config = if matches!(app, AppType::Claude) {
        json!({"env":{"ANTHROPIC_MODEL":model}})
    } else {
        json!({"auth":{},"config":format!("model = {model:?}\n")})
    };
    let mut p = Provider::with_id(
        format!("{platform}-{}", app.as_str()),
        "Synthetic native".into(),
        config,
        None,
    );
    p.category = Some("custom".into());
    p.meta=Some(serde_json::from_value::<ProviderMeta>(json!({"authBinding":{"source":"managed_account","authProvider":platform,"accountId":account},"apiFormat":if platform=="workbuddy"{"openai_chat"}else{"anthropic"},"commonConfigEnabled":false})).unwrap());
    crate::services::provider::managed_inference::validate(app, &p).unwrap();
    p
}
fn source_id(platform: &str, account: &str, model: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "native:{platform}:{}",
        hex::encode(Sha256::digest(
            serde_json::to_vec(&(platform, account, model)).unwrap()
        ))
    )
}
pub(crate) async fn verify(state: &AppState, case: &str) {
    let accounts =
        WorkBuddyTransport::seed_accounts(state.db.secret_session(), &["synthetic-wb-a"])
            .await
            .unwrap();
    let wb = &accounts[0];
    let mm_owner = crate::minimax_code::inference::FixtureTransport::seed_accounts(
        state.db.secret_session(),
        &[(
            "synthetic-mm-b",
            crate::minimax_code::catalog::Region::Global,
        )],
    )
    .await
    .unwrap();
    for app in [AppType::Claude, AppType::Codex] {
        state
            .db
            .save_provider(
                app.as_str(),
                &projection(&app, "workbuddy", wb, "synthetic-wb-model"),
            )
            .unwrap();
        state
            .db
            .save_provider(
                app.as_str(),
                &projection(&app, "minimax_code", "synthetic-mm-b", "MiniMax-M2.7"),
            )
            .unwrap();
    }
    if case == "source" {
        let rows = crate::commands::get_service_sources_impl(state).unwrap();
        let row = rows
            .iter()
            .find(|r| r.source_id == source_id("workbuddy", wb, "synthetic-wb-model"));
        assert!(row.is_some(),"authorized original WorkBuddy account with configured model must be available without optional ranking snapshot");
        let row = row.unwrap();
        assert_eq!(row.bindings.len(), 2);
        assert_ne!(row.bindings[0].provider_id, row.bindings[1].provider_id);
        let dto = serde_json::to_value(row).unwrap();
        assert_eq!(dto["account"]["platform"], "workbuddy");
        assert!(dto["account"].get("region").is_none());
        assert!(!dto.to_string().contains("fixture-access"));
        return;
    }
    let mut members = vec![
        groups::Member {
            source_id: source_id("workbuddy", wb, "synthetic-wb-model"),
            model: "synthetic-wb-model".into(),
        },
        groups::Member {
            source_id: source_id("minimax_code", "synthetic-mm-b", "MiniMax-M2.7"),
            model: "MiniMax-M2.7".into(),
        },
    ];
    if case == "unconfigured_model" {
        members[0].model = "not-configured".into();
    }
    if matches!(case, "revive" | "replace" | "mm_skip" | "mm_manual_revoked") {
        members.reverse();
    }
    let prepared = tokio::task::block_in_place(|| {
        routing::prepare_group(
            state,
            routing::GroupDraft {
                id: None,
                expected_revision: None,
                name: "Mixed original owners".into(),
                members: members.clone(),
            },
        )
    });
    if case == "unconfigured_model" {
        assert!(prepared.is_err());
        return;
    }
    let prepared = prepared
        .expect("original configured native owners must prepare mixed group without discovery");
    if case == "preview_revoke" {
        assert!(
            super::inference::set_inference_consent(state.db.secret_session(), wb, false)
                .await
                .unwrap()
        );
        assert!(
            tokio::task::block_in_place(|| routing::confirm_group(state, prepared.clone()))
                .is_err()
        );
        assert!(
            groups::read(&state.db.conn.lock().unwrap(), &prepared.group.id)
                .unwrap()
                .is_none()
        );
        return;
    }
    let group = tokio::task::block_in_place(|| routing::confirm_group(state, prepared)).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let wc = calls.clone();
    let mc = calls.clone();
    let wb_case = case.to_string();
    let mm_case = case.to_string();
    let native_db = state.db.clone();
    let wb_id = wb.clone();
    let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refresh_count = refreshes.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let upstream_release = release.clone();
    let upstream=axum::Router::new()
    .route("/v2/plugin/auth/token/refresh",axum::routing::post(move |headers:axum::http::HeaderMap| {let count=refresh_count.clone();async move {
        assert_eq!(headers["x-refresh-token"],"fixture-refresh-synthetic-wb-a");count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        axum::Json(json!({"code":0,"data":{"accessToken":"fixture-rotated-access","refreshToken":"fixture-rotated-refresh","expiresIn":3600,"uid":"synthetic-wb-a","domain":"www.workbuddy.cn","platform":"workbuddy"}}))
    }}))
    .route("/v2/chat/completions",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<serde_json::Value>| {
        let calls=wc.clone();let case=wb_case.clone();let release=upstream_release.clone();async move {
            calls.lock().unwrap().push("workbuddy");
            assert_eq!(body["model"],"synthetic-wb-model");assert_eq!(body["stream"],true);assert_ne!(headers["authorization"],"Bearer untrusted-client");assert_eq!(headers["x-domain"],"www.workbuddy.cn");assert_eq!(headers["x-user-id"],"synthetic-wb-a");assert_eq!(headers["user-agent"],"WorkBuddy/5.5.6");if case!="refresh" {assert_eq!(headers["authorization"],"Bearer fixture-access-synthetic-wb-a");}
            if matches!(case.as_str(),"denied11004"|"denied11140") {
                let code=if case=="denied11004"{11004}else{11140};
                return (StatusCode::FORBIDDEN,axum::Json(json!({"code":code,"message":"synthetic exact business denial"}))).into_response();
            }
            if matches!(case.as_str(),"refresh"|"stream"|"revoke"|"mm_skip") {
                if case=="refresh" {assert_eq!(headers["authorization"],"Bearer fixture-rotated-access");}
                let first="data: {\"id\":\"synthetic-wb\",\"object\":\"chat.completion.chunk\",\"model\":\"synthetic-wb-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"workbuddy-visible-prefix.\"},\"finish_reason\":null}]}\n\n";
                if case=="stream" {
                    let stream=async_stream::stream! {
                        yield Ok::<_,std::io::Error>(first);
                        release.notified().await;
                        yield Err(std::io::Error::other("synthetic interruption"));
                    };
                    return axum::http::Response::builder().status(200).header("content-type","text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                let done="data: {\"id\":\"synthetic-wb\",\"object\":\"chat.completion.chunk\",\"model\":\"synthetic-wb-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\ndata: [DONE]\n\n";
                return ([("content-type","text/event-stream")],format!("{first}{done}")).into_response();
            }
            (StatusCode::TOO_MANY_REQUESTS,axum::Json(json!({"code":429,"message":"synthetic technical rate limit"}))).into_response()
        }
    }))
    .route("/global/mavis/api/v1/llm/v1/messages",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<serde_json::Value>| {
        let calls=mc.clone();let case=mm_case.clone();let db=native_db.clone();let id=wb_id.clone();async move {
            calls.lock().unwrap().push("minimax_code");assert_eq!(body["model"],"MiniMax-M2.7");assert_eq!(headers["user-agent"],"MiniMaxAgent");
            if matches!(case.as_str(),"revive"|"replace") {
                if case=="revive" {
                    assert!(super::inference::set_inference_consent(db.secret_session(),&id,false).await.unwrap());
                    assert!(super::inference::set_inference_consent(db.secret_session(),&id,true).await.unwrap());
                } else {WorkBuddyTransport::mutate_fixture_grant(db.secret_session(),"synthetic-wb-a","replace").await.unwrap();}
                // The new authorization is valid for a NEW request; only the captured old route must reject it.
                assert!(super::inference::Lease::acquire(db.secrets.clone(),&id).await.is_ok());
                return (StatusCode::SERVICE_UNAVAILABLE,axum::Json(json!({"error":{"type":"api_error","message":"synthetic retryable"}}))).into_response();
            }
            let data=concat!("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"synthetic\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"MiniMax-M2.7\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n", "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n", "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"mixed-native-ok\"}}\n\n", "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n", "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n", "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
            ([("content-type","text/event-stream")],data).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let wt = WorkBuddyTransport::loopback(state.db.secret_session(), &origin).unwrap();
    let mt = crate::minimax_code::inference::FixtureTransport::loopback(&mm_owner, &origin)
        .await
        .unwrap();
    if case == "refresh" {
        WorkBuddyTransport::mutate_fixture_grant(
            state.db.secret_session(),
            "synthetic-wb-a",
            "expire",
        )
        .await
        .unwrap();
    }
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    for app in [AppType::Claude, AppType::Codex] {
        if matches!(
            case,
            "revive"
                | "replace"
                | "revoke"
                | "wb_skip"
                | "mm_skip"
                | "wb_manual_revoked"
                | "mm_manual_revoked"
                | "all_revoked"
                | "runtime_model_changed"
        ) && matches!(app, AppType::Codex)
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
                "bootstrap",
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
        let target = if matches!(case, "manual" | "wb_manual_revoked" | "mm_manual_revoked") {
            groups::RouteTarget::Manual {
                source_id: members[0].source_id.clone(),
                model: members[0].model.clone(),
            }
        } else {
            groups::RouteTarget::OrderedGroup {
                group_id: group.id.clone(),
            }
        };
        let result = tokio::task::block_in_place(|| {
            let p = routing::prepare_target(state, &app, target).unwrap();
            routing::apply_target(state, &app, p).unwrap()
        });
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
            .layer(axum::Extension(wt.clone()))
            .layer(axum::Extension(mt.clone()));
        let (path, mut body) = if matches!(app, AppType::Claude) {
            (
                "/v1/messages",
                json!({"model":"incoming-alias","stream":true,"max_tokens":16,"messages":[{"role":"user","content":"synthetic"}]}),
            )
        } else {
            (
                "/v1/responses",
                json!({"model":"incoming-alias","stream":true,"input":"synthetic"}),
            )
        };
        if case == "nonstream" {
            body["stream"] = json!(false);
        }
        if matches!(case, "wb_skip" | "wb_manual_revoked" | "all_revoked") {
            assert!(
                super::inference::set_inference_consent(state.db.secret_session(), wb, false)
                    .await
                    .unwrap()
            );
        }
        if matches!(case, "mm_skip" | "mm_manual_revoked" | "all_revoked") {
            crate::minimax_code::checkin::Engine {
                session: state.db.secret_session(),
                transport: &crate::minimax_code::http::OfficialHttp,
            }
            .retire("synthetic-mm-b")
            .await
            .unwrap();
        }
        if case == "runtime_model_changed" {
            state
                .db
                .save_provider(
                    app.as_str(),
                    &projection(&app, "workbuddy", wb, "changed-configured-model"),
                )
                .unwrap();
        }
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            router.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer untrusted-client")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .expect("fixture must reach original response headers")
        .unwrap();
        let status = response.status();
        if matches!(case, "revoke" | "stream") {
            assert_eq!(status, StatusCode::OK);
            if case == "revoke" {
                WorkBuddyTransport::retire_fixture_grant(
                    state.db.secret_session(),
                    "synthetic-wb-a",
                )
                .await
                .unwrap();
            }
            let mut stream = response.into_body().into_data_stream();
            let mut bytes = vec![];
            let mut failed = false;
            if case == "stream" {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while !String::from_utf8_lossy(&bytes).contains("workbuddy-visible-prefix") {
                        bytes.extend_from_slice(
                            &stream
                                .next()
                                .await
                                .expect("visible output absent")
                                .expect("precommit error"),
                        );
                    }
                })
                .await
                .expect("original consumer must publish visible output before synthetic failure");
                release.notify_one();
            }
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            if case == "revoke" {
                assert!(failed);
                assert!(
                    bytes.is_empty(),
                    "retired account must fence all buffered output"
                );
            } else {
                assert!(String::from_utf8_lossy(&bytes).contains("workbuddy-visible-prefix"));
            }
            continue;
        }
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        if matches!(case, "revive" | "replace") {
            assert!(status.is_client_error() || status.is_server_error());
            assert!(
                String::from_utf8_lossy(&bytes).contains("workbuddy.routing_identity_changed"),
                "must reject captured identity before fixture capability: {}",
                String::from_utf8_lossy(&bytes)
            );
        } else if matches!(
            case,
            "manual"
                | "denied11004"
                | "denied11140"
                | "wb_manual_revoked"
                | "mm_manual_revoked"
                | "all_revoked"
                | "runtime_model_changed"
        ) {
            assert!(status.is_client_error() || status.is_server_error());
        } else if case == "nonstream" {
            assert_eq!(status, StatusCode::BAD_REQUEST);
        } else {
            assert_eq!(
                status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            assert!(String::from_utf8_lossy(&bytes).contains(
                if matches!(case, "refresh" | "mm_skip") {
                    "workbuddy-visible-prefix"
                } else {
                    "mixed-native-ok"
                }
            ));
        }
    }
    assert_eq!(
        groups::read(&state.db.conn.lock().unwrap(), &group.id).unwrap(),
        Some(group.clone()),
        "runtime availability must not rewrite durable group intent"
    );
    state.proxy_service.stop_with_restore().await.unwrap();
    server.abort();
    let expected = match case {
        "revive" | "replace" => vec!["minimax_code"],
        "revoke" => vec!["workbuddy"],
        "nonstream"
        | "wb_manual_revoked"
        | "mm_manual_revoked"
        | "all_revoked"
        | "runtime_model_changed" => vec![],
        "wb_skip" => vec!["minimax_code"],
        "mm_skip" => vec!["workbuddy"],
        "manual" | "denied11004" | "denied11140" | "stream" | "refresh" => {
            vec!["workbuddy", "workbuddy"]
        }
        _ => vec!["workbuddy", "minimax_code", "workbuddy", "minimax_code"],
    };
    assert_eq!(*calls.lock().unwrap(), expected);
    if case == "nonstream" {
        assert_eq!(wt.attempts(), 0);
        assert_eq!(mt.attempts(), 0);
    }
    assert_eq!(
        refreshes.load(std::sync::atomic::Ordering::SeqCst),
        usize::from(case == "refresh")
    );
}
