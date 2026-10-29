//! Synthetic original WorkBuddy and MiniMax owners, original shared router only.
use super::inference::FixtureTransport as WorkBuddyTransport;
use crate::{
    app_config::AppType,
    provider::{Provider, ProviderMeta},
    services::{routing_groups as groups, shared_routing as routing},
    store::AppState,
};
use axum::{http::StatusCode, response::IntoResponse};
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
// Export SQL embeds the wall clock. Compare every logical DB fact and the full
// serialized image instead, including physical changes an SQL dump would omit.
fn database_image(state: &AppState) -> (String, Vec<u8>) {
    let conn = state.db.conn.lock().unwrap();
    (
        crate::database::Database::content_digest(&conn).unwrap(),
        conn.serialize(rusqlite::MAIN_DB).unwrap().to_vec(),
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
    if case == "paused_import" {
        // A genuine old source-schema import, not a new model selection. Every
        // row/token is synthetic; the source is opened read-only by the real importer.
        let original = state
            .db
            .get_provider_by_id("workbuddy-claude", "claude")
            .unwrap()
            .unwrap();
        state
            .db
            .set_current_provider("claude", &original.id)
            .unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some(&original.id)).unwrap();
        let mut runtime = state.db.get_proxy_config_for_app("claude").await.unwrap();
        runtime.enabled = true;
        state.db.update_proxy_config_for_app(runtime).await.unwrap();
        state
            .db
            .save_live_backup(
                "claude",
                &json!({"env":{"ANTHROPIC_MODEL":"before-import-sentinel"}}).to_string(),
            )
            .await
            .unwrap();
        let owned = crate::secrets::owned_file::OwnedFile::registered(super::store::FILE).unwrap();
        let account_before = std::fs::read(owned.path(state.db.secret_session())).unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        {
            let conn = rusqlite::Connection::open(source.path()).unwrap();
            conn.pragma_update(None, "user_version", 16).unwrap();
            // The import owner's historical providers fixture shape (no v31 columns).
            conn.execute_batch(
                "CREATE TABLE providers (
                id TEXT NOT NULL, app_type TEXT NOT NULL, name TEXT NOT NULL,
                settings_config TEXT NOT NULL, website_url TEXT, category TEXT,
                created_at INTEGER, sort_index INTEGER, notes TEXT, icon TEXT,
                icon_color TEXT, meta TEXT NOT NULL DEFAULT '{}',
                is_current BOOLEAN NOT NULL DEFAULT 0, in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
                PRIMARY KEY(id,app_type));",
            )
            .unwrap();
            for app in [AppType::Claude, AppType::Codex] {
                for p in state
                    .db
                    .get_all_providers(app.as_str())
                    .unwrap()
                    .into_values()
                {
                    conn.execute("INSERT INTO providers(id,app_type,name,settings_config,website_url,category,created_at,sort_index,notes,icon,icon_color,meta,is_current,in_failover_queue) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)", rusqlite::params![p.id,app.as_str(),p.name,p.settings_config.to_string(),p.website_url,p.category,p.created_at,p.sort_index.map(|v| i64::try_from(v).unwrap()),p.notes,p.icon,p.icon_color,serde_json::to_string(&p.meta).unwrap(),p.id==original.id && app==AppType::Claude,p.in_failover_queue]).unwrap();
                }
            }
        }
        let source_before = std::fs::read(source.path()).unwrap();
        let report = tokio::task::block_in_place(|| {
            crate::relay::cc_switch_import::execute_import(state.clone(), source.path())
        })
        .unwrap();
        assert!(report.success);
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| !warning.starts_with("导入后同步 claude 失败:")
                    && !warning.contains("workbuddy.inference_paused")),
            "import must complete its original provider synchronization: {:?}",
            report.warnings
        );
        let restored_backup = state.db.get_live_backup("claude").await.unwrap().unwrap();
        let restored_backup: serde_json::Value =
            serde_json::from_str(&restored_backup.original_config).unwrap();
        assert_eq!(restored_backup["env"]["ANTHROPIC_MODEL"], "synthetic-wb-model", "the real import must replace the old backup sentinel, not merely retain same-valued rows");
        assert_eq!(std::fs::read(source.path()).unwrap(), source_before);
        let after = state
            .db
            .get_provider_by_id(&original.id, "claude")
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        assert_eq!(
            state.db.get_current_provider("claude").unwrap().as_deref(),
            Some(original.id.as_str())
        );
        assert_eq!(
            std::fs::read(owned.path(state.db.secret_session())).unwrap(),
            account_before
        );
        assert!(matches!(
            super::inference::Lease::acquire(state.db.secrets.clone(), wb).await,
            Err(super::model::Failure::InferencePaused)
        ));
        return;
    }
    if case == "paused_source" {
        let before = serde_json::to_value(state.db.get_all_providers("claude").unwrap()).unwrap();
        let overview = crate::commands::application_overview(state, &AppType::Claude).unwrap();
        let row = overview
            .configurations
            .iter()
            .find(|row| row.provider_id == "workbuddy-claude")
            .unwrap();
        assert!(
            !row.can_select,
            "old WorkBuddy configuration must be paused, not selectable"
        );
        assert_eq!(
            serde_json::to_value(row).unwrap()["presentation"]["switchBlockedReason"],
            "workbuddyInferencePaused"
        );
        let rows = crate::commands::get_service_sources_impl(state).unwrap();
        assert!(
            rows.iter()
                .all(|row| row.source_id != source_id("workbuddy", wb, "synthetic-wb-model")),
            "paused owner must not mint a dispatchable shared source proof"
        );
        assert_eq!(
            serde_json::to_value(state.db.get_all_providers("claude").unwrap()).unwrap(),
            before
        );
        assert!(
            rows.iter()
                .find(|row| row.source_id
                    == source_id("minimax_code", "synthetic-mm-b", "MiniMax-M2.7"))
                .unwrap()
                .bindings
                .iter()
                .all(|binding| binding.can_select)
        );
        return;
    }
    if case == "source" {
        let rows = crate::commands::get_service_sources_impl(state).unwrap();
        assert!(rows
            .iter()
            .all(|r| r.source_id != source_id("workbuddy", wb, "synthetic-wb-model")));
        for app in [AppType::Claude, AppType::Codex] {
            let overview = crate::commands::application_overview(state, &app).unwrap();
            let row = overview
                .configurations
                .iter()
                .find(|r| r.provider_id == format!("workbuddy-{}", app.as_str()))
                .unwrap();
            assert!(!row.can_select);
            let dto = serde_json::to_value(row).unwrap();
            assert_eq!(
                dto["presentation"]["switchBlockedReason"],
                "workbuddyInferencePaused"
            );
            assert!(!dto.to_string().contains("fixture-access"));
        }
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
        members.remove(0);
        members[0].model = "not-configured".into();
    }
    if matches!(case, "revive" | "replace" | "mm_skip" | "mm_manual_revoked") {
        members.reverse();
    }
    if case == "all_paused" {
        members.truncate(1);
    }
    if matches!(case, "preview_revoke" | "mm_manual") {
        members.remove(0);
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
    if matches!(case, "unconfigured_model" | "paused_preview") {
        assert!(prepared.is_err());
        return;
    }
    if case == "preview_revoke" {
        // Still test the real review/retire/commit race with the healthy original
        // MiniMax owner. WorkBuddy is no longer eligible even at preview time.
        let prepared = prepared.unwrap();
        crate::minimax_code::checkin::Engine {
            session: state.db.secret_session(),
            transport: &crate::minimax_code::http::OfficialHttp,
        }
        .retire("synthetic-mm-b")
        .await
        .unwrap();
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
    // Exact persisted pre-pause intent: no new reviewed WorkBuddy authorization.
    let group = groups::save_named(
        &mut state.db.conn.lock().unwrap(),
        "synthetic-legacy-group",
        None,
        "Original order",
        &members,
    )
    .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let wc = calls.clone();
    let mc = calls.clone();
    let mm_case = case.to_string();
    let native_db = state.db.clone();
    let wb_id = wb.clone();
    let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refresh_count = refreshes.clone();
    let upstream=axum::Router::new()
    .route("/v2/plugin/auth/token/refresh",axum::routing::post(move |headers:axum::http::HeaderMap| {let count=refresh_count.clone();async move {
        assert_eq!(headers["x-refresh-token"],"fixture-refresh-synthetic-wb-a");count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        axum::Json(json!({"code":0,"data":{"accessToken":"fixture-rotated-access","refreshToken":"fixture-rotated-refresh","expiresIn":3600,"uid":"synthetic-wb-a","domain":"www.workbuddy.cn","platform":"workbuddy"}}))
    }}))
    .route("/v2/chat/completions", axum::routing::post(move || {
        wc.lock().unwrap().push("workbuddy");
        async { StatusCode::BAD_REQUEST }
    }))
    .route("/global/mavis/api/v1/llm/v1/messages",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<serde_json::Value>| {
        let calls=mc.clone();let case=mm_case.clone();let db=native_db.clone();let id=wb_id.clone();async move {
            calls.lock().unwrap().push("minimax_code");assert_eq!(body["model"],"MiniMax-M2.7");assert_eq!(headers["user-agent"],"MiniMaxAgent");
            if matches!(case.as_str(),"revive"|"replace") {
                if case=="revive" {
                    assert!(super::inference::set_inference_consent(db.secret_session(),&id,false).await.unwrap());
                    assert_eq!(super::inference::set_inference_consent(db.secret_session(),&id,true).await, Err(super::model::Failure::InferencePaused));
                } else {WorkBuddyTransport::mutate_fixture_grant(db.secret_session(),"synthetic-wb-a","replace").await.unwrap();}
                // Replacing/re-enabling a saved grant cannot resurrect a paused capability.
                assert!(matches!(super::inference::Lease::acquire(db.secrets.clone(),&id).await, Err(super::model::Failure::InferencePaused)));
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
        if matches!(case, "paused_select" | "paused_legacy_select") {
            use crate::services::application_selection::{RoutingOrder, TierSelection};
            let provider_id = format!("workbuddy-{}", app.as_str());
            let mut order = state
                .db
                .get_all_providers(app.as_str())
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            order.sort();
            let profiles = crate::services::order_profiles::get(&state.db, app.as_str()).unwrap();
            let current_profile = profiles
                .profiles
                .iter()
                .find(|profile| profile.name == profiles.current)
                .unwrap();
            if order == current_profile.provider_ids {
                order.reverse();
            }
            assert_ne!(
                order, current_profile.provider_ids,
                "fixture must request a real order change"
            );
            crate::services::order_profiles::validate_order(
                &state.db,
                app.as_str(),
                &profiles.current,
                &order,
            )
            .unwrap();
            let before = database_image(state);
            let live_before = crate::services::provider::read_live_settings(app.clone()).unwrap();
            // Codex managed account rows intentionally have a fixed two-field
            // projection. Generic model editing adds a catalog and is already
            // rejected by the original projection validator, before selection.
            // Keep that rejection rather than moving the pause gate ahead of it.
            if app == AppType::Codex && case == "paused_select" {
                let error = tokio::task::block_in_place(|| {
                    crate::services::application_selection::select_with_commit(
                        state,
                        &app,
                        &TierSelection {
                            provider_id: provider_id.clone(),
                            model: Some("synthetic-wb-model".into()),
                        },
                        Some(&RoutingOrder {
                            profile_name: profiles.current.clone(),
                            provider_ids: order.clone(),
                        }),
                    )
                })
                .err()
                .expect("invalid managed model projection must be rejected");
                assert!(
                    error
                        .to_string()
                        .contains("managedAccount.invalidProjection"),
                    "{error}"
                );
                assert!(database_image(state) == before);
                assert_eq!(
                    crate::services::provider::read_live_settings(app.clone()).unwrap(),
                    live_before
                );
            }
            let results = [
                state
                    .proxy_service
                    .switch_proxy_target(app.as_str(), &provider_id)
                    .await,
                tokio::task::block_in_place(|| {
                    crate::services::ProviderService::switch(state, app.clone(), &provider_id)
                })
                .map(|_| ())
                .map_err(|e| e.to_string()),
                tokio::task::block_in_place(|| {
                    crate::services::application_selection::select_with_commit(
                        state,
                        &app,
                        &TierSelection {
                            provider_id,
                            // Selecting a fixed Codex account is not a generic model edit.
                            model: if app == AppType::Codex {
                                None
                            } else {
                                Some("synthetic-wb-model".into())
                            },
                        },
                        Some(&RoutingOrder {
                            profile_name: profiles.current,
                            provider_ids: order,
                        }),
                    )
                })
                .map(|_| ())
                .map_err(|e| e.to_string()),
            ];
            for (entrypoint, result) in results.into_iter().enumerate() {
                assert!(result.is_err(), "{app:?} selection entrypoint {entrypoint} must not publish a paused model target");
                let error = result.err().unwrap();
                assert!(
                    error.contains("workbuddy.inference_paused"),
                    "{app:?} selection entrypoint {entrypoint}: {error}"
                );
            }
            assert!(
                database_image(state) == before,
                "no provider, model preference, order, current target or backup may change"
            );
            assert_eq!(
                crate::services::provider::read_live_settings(app.clone()).unwrap(),
                live_before
            );
            continue;
        }
        let target = if matches!(
            case,
            "manual"
                | "wb_manual_revoked"
                | "mm_manual_revoked"
                | "paused_manual"
                | "mm_manual"
                | "denied11004"
                | "denied11140"
                | "stream"
                | "revoke"
                | "nonstream"
                | "refresh"
        ) {
            groups::RouteTarget::Manual {
                source_id: members[0].source_id.clone(),
                model: members[0].model.clone(),
            }
        } else {
            groups::RouteTarget::OrderedGroup {
                group_id: group.id.clone(),
            }
        };
        groups::set_target(
            &mut state.db.conn.lock().unwrap(),
            app.as_str(),
            target.clone(),
            None,
            if matches!(target, groups::RouteTarget::OrderedGroup { .. }) {
                Some(group.revision)
            } else {
                None
            },
        )
        .unwrap();
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
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        if matches!(case, "mixed" | "paused_group" | "wb_skip" | "mm_manual") {
            assert_eq!(
                status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            assert!(String::from_utf8_lossy(&bytes).contains("mixed-native-ok"));
        } else {
            assert!(
                status.is_client_error() || status.is_server_error(),
                "{case}: {status}"
            );
            assert!(!String::from_utf8_lossy(&bytes).contains("workbuddy-visible-prefix"));
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
        "mixed" | "paused_group" | "mm_manual" => vec!["minimax_code", "minimax_code"],
        "wb_skip" | "revive" | "replace" => vec!["minimax_code"],
        _ => vec![],
    };
    assert_eq!(*calls.lock().unwrap(), expected);
    assert_eq!(wt.attempts(), 0, "no paused model attempt or refresh");
    assert_eq!(mt.attempts(), expected.len());
    assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 0);
}
