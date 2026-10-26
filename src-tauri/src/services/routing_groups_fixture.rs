//! Synthetic localhost requests through the existing ProxyServer. No external accounts.
use crate::{
    app_config::AppType,
    database::Database,
    provider::{Provider, ProviderMeta},
};
use axum::{
    extract::State,
    http::{StatusCode, Uri},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Upstream {
    name: &'static str,
    fail: bool,
    case: String,
    database: Arc<Database>,
    changed: Arc<std::sync::atomic::AtomicBool>,
    attempts: Arc<Mutex<Vec<(String, String)>>>,
}
async fn handle(
    State(state): State<Upstream>,
    uri: Uri,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let model = body["model"].as_str().unwrap_or("").to_string();
    state
        .attempts
        .lock()
        .unwrap()
        .push((state.name.into(), model.clone()));
    if state.name == "a"
        && !state
            .changed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        if matches!(state.case.as_str(), "snapshot" | "source_changed") {
            use crate::services::routing_groups as groups;
            let mut group = {
                let conn = state.database.conn.lock().unwrap();
                groups::read(&conn, "daily").unwrap().unwrap()
            };
            let b_id = group.members[1].source_id.clone();
            if state.case == "source_changed" {
                for app in [AppType::Claude, AppType::Codex] {
                    let mut provider = state
                        .database
                        .get_provider_by_id(&b_id, app.as_str())
                        .unwrap()
                        .unwrap();
                    if matches!(app, AppType::Claude) {
                        provider.settings_config["env"]["ANTHROPIC_MODEL"] = json!("model-new");
                    } else {
                        let config = provider.settings_config["config"]
                            .as_str()
                            .unwrap()
                            .replace("model-b", "model-new");
                        provider.settings_config["config"] = json!(config);
                    }
                    state
                        .database
                        .save_provider(app.as_str(), &provider)
                        .unwrap();
                }
            }
            group.members.reverse();
            if state.case == "source_changed" {
                group.members[0].model = "model-new".into();
            }
            groups::save(
                &mut state.database.conn.lock().unwrap(),
                "daily",
                Some(group.revision),
                &group.members,
            )
            .unwrap();
        }
        if state.case == "auth_lease" {
            state.database.conn.lock().unwrap().execute("UPDATE loongport_client_credentials SET allowed_models='[\"requested-alias\"]'",[]).unwrap();
        }
    }
    if state.case == "stream" && state.name == "a" {
        use futures::StreamExt;
        let prefix = format!(
            "event: message_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\n",
            json!({"type":"message_start","message":{"id":"fixture-stream","model":model,"usage":{"input_tokens":1}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"fixture-visible-prefix"}})
        );
        let body = axum::body::Body::from_stream(
            futures::stream::once(async move { Ok::<_, std::io::Error>(prefix) }).chain(
                futures::stream::once(async {
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                    Err::<String, _>(std::io::Error::other(
                        "fixture interruption after visible output",
                    ))
                }),
            ),
        );
        return axum::http::Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(body)
            .unwrap();
    }
    if state.fail {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"message":"synthetic transient failure"}})),
        )
            .into_response();
    }
    let response = if uri.path().contains("messages") {
        json!({"id":"fixture-message","type":"message","role":"assistant","model":model,"content":[{"type":"text","text":"fixture-ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}})
    } else {
        json!({"id":"fixture-chat","object":"chat.completion","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":"fixture-ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
    };
    (StatusCode::OK, Json(response)).into_response()
}
async fn upstream(
    database: Arc<Database>,
    case: &str,
    name: &'static str,
    fail: bool,
    attempts: Arc<Mutex<Vec<(String, String)>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(handle).with_state(Upstream {
        name,
        fail,
        attempts,
        database,
        case: case.into(),
        changed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}

pub async fn verify(case: &str) {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    if case == "schema" {
        verify_schema();
        return;
    }
    let db = Arc::new(Database::memory().unwrap());
    let (a_url, a_task) = upstream(
        db.clone(),
        case,
        "a",
        matches!(
            case,
            "fallback"
                | "snapshot"
                | "source_changed"
                | "manual"
                | "exhausted"
                | "group_budget"
                | "legacy_budget"
        ),
        attempts.clone(),
    )
    .await;
    let (b_url, b_task) =
        upstream(db.clone(), case, "b", case == "exhausted", attempts.clone()).await;
    let outside = if matches!(case, "exhausted" | "group_budget") {
        Some(upstream(db.clone(), case, "outside", false, attempts.clone()).await)
    } else {
        None
    };
    let a_id = crate::relay::managed::provider_id_for("https://a.example", Some(11), 1);
    let b_id = crate::relay::managed::provider_id_for("https://b.example", Some(22), 1);
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("INSERT INTO loongport_relay(id,site_origin,account_id) VALUES(1,'https://a.example',11),(2,'https://b.example',22)",[]).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT version FROM loongport_schema_version WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            version,
            i64::from(crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION),
            "fresh DB must have the actual versioned schema"
        );
    }
    for app in [AppType::Claude, AppType::Codex] {
        for (id, base, model, account, site) in [
            (&a_id, &a_url, "model-a", 11, "https://a.example"),
            (&b_id, &b_url, "model-b", 22, "https://b.example"),
        ] {
            let settings = crate::relay::provider_config::settings_config_for(
                &app,
                "synthetic-key",
                "Fixture",
                base,
                model,
            )
            .unwrap();
            let mut provider =
                Provider::with_id(id.clone(), "Fixture".into(), settings, Some(site.into()));
            provider.meta = Some(ProviderMeta {
                loongport_account_id: Some(account),
                api_format: matches!(app, AppType::Codex).then(|| "openai_chat".into()),
                ..Default::default()
            });
            db.save_provider(app.as_str(), &provider).unwrap();
        }
        if let Some((base, _)) = &outside {
            let settings = crate::relay::provider_config::settings_config_for(
                &app,
                "synthetic-key",
                "Outside group",
                base,
                "model-outside",
            )
            .unwrap();
            let mut provider =
                Provider::with_id("outside".into(), "Outside group".into(), settings, None);
            provider.meta = Some(ProviderMeta {
                api_format: matches!(app, AppType::Codex).then(|| "openai_chat".into()),
                ..Default::default()
            });
            db.save_provider(app.as_str(), &provider).unwrap();
        }
        db.set_current_provider(app.as_str(), &b_id).unwrap();
        crate::proxy::application_routing::set_order(
            &db,
            app.as_str(),
            &[b_id.clone(), a_id.clone()],
        )
        .unwrap();
        if outside.is_some() {
            db.set_current_provider(app.as_str(), "outside").unwrap();
            crate::proxy::application_routing::set_order(
                &db,
                app.as_str(),
                &["outside".into(), a_id.clone(), b_id.clone()],
            )
            .unwrap();
        }
        let mut config = db.get_proxy_config_for_app(app.as_str()).await.unwrap();
        config.enabled = true;
        config.auto_failover_enabled = true;
        config.max_retries = if matches!(case, "group_budget" | "legacy_budget") {
            0
        } else {
            3
        };
        db.update_proxy_config_for_app(config).await.unwrap();
    }
    {
        use crate::services::routing_groups as groups;
        let mut conn = db.conn.lock().unwrap();
        let mut members = vec![
            groups::Member {
                source_id: a_id.clone(),
                model: "model-a".into(),
            },
            groups::Member {
                source_id: b_id.clone(),
                model: "model-b".into(),
            },
        ];
        if case == "unavailable" {
            members.truncate(1);
        }
        if case == "unknown_model" {
            members[0].model = "unverified-model".into();
        }
        let group = groups::save(&mut conn, "daily", None, &members).unwrap();
        if !matches!(case, "legacy" | "legacy_budget") {
            for app in ["claude", "codex"] {
                groups::bind(&mut conn, app, "daily", None, group.revision).unwrap();
            }
        }
    }
    if case == "legacy_budget" {
        for app in ["claude", "codex"] {
            db.set_current_provider(app, &a_id).unwrap();
            crate::proxy::application_routing::set_order(&db, app, &[a_id.clone(), b_id.clone()])
                .unwrap();
        }
    }
    if case == "manual" {
        for app in ["claude", "codex"] {
            crate::services::routing_groups::set_target(
                &mut db.conn.lock().unwrap(),
                app,
                crate::services::routing_groups::RouteTarget::Manual {
                    source_id: a_id.clone(),
                    model: "model-a".into(),
                },
                Some(1),
                None,
            )
            .unwrap();
        }
    }
    if case == "unavailable" {
        for app in ["claude", "codex"] {
            crate::proxy::application_routing::set_tier_blocked(&db, app, &a_id, true).unwrap();
        }
    }
    if matches!(case, "historical" | "missing_schema") {
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "DROP TABLE loongport_application_route_bindings;DROP TABLE loongport_routing_groups;",
        )
        .unwrap();
        if case == "historical" {
            // This fixture models pre-pool v29, not latest with an old stamp.
            conn.execute_batch("ALTER TABLE providers DROP COLUMN credential_revision; ALTER TABLE loongport_vendor DROP COLUMN api_key_revision;").unwrap();
            conn.execute(
                "UPDATE loongport_schema_version SET version=29 WHERE id=1",
                [],
            )
            .unwrap();
            assert_historical_v29_owner_shape(&conn);
        }
    }
    let mut caller_keys = std::collections::HashMap::new();
    if matches!(case, "auth_override" | "auth_allowed" | "auth_lease") {
        db.set_setting("client_access_migration_version", "1")
            .unwrap();
        for app in ["claude", "codex"] {
            let models = if case == "auth_override" {
                vec!["requested-alias".into()]
            } else {
                vec!["requested-alias".into(), "model-a".into(), "model-b".into()]
            };
            let credential = db
                .issue_client_credential("fixture-client", app, &models)
                .unwrap();
            let key = db
                .resolve_client_credential(&credential.id, "fixture-client", app)
                .unwrap();
            caller_keys.insert(app, key);
        }
    }
    let coordinator =
        crate::relay::model_verification::coordinator::ModelVerificationCoordinator::new(
            db.clone(),
        );
    let server = crate::proxy::server::ProxyServer::new(
        crate::proxy::types::ProxyConfig {
            listen_port: 0,
            ..Default::default()
        },
        db.clone(),
        None,
        coordinator.passive_ingress(),
        Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
    );
    let info = server.start().await.unwrap();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let mut results = Vec::new();
    for (path, mut body) in [
        (
            "/v1/messages",
            json!({"model":"requested-alias","max_tokens":8,"messages":[{"role":"user","content":"fixture"}]}),
        ),
        (
            "/v1/responses",
            json!({"model":"requested-alias","input":"fixture","stream":false}),
        ),
    ] {
        if matches!(case, "stream" | "auth_lease") && path != "/v1/messages" {
            continue;
        }
        if case == "stream" {
            body["stream"] = json!(true);
        }
        let mut request = client
            .post(format!("http://127.0.0.1:{}{}", info.port, path))
            .json(&body);
        let app = if path == "/v1/messages" {
            "claude"
        } else {
            "codex"
        };
        if let Some(key) = caller_keys.get(app) {
            request = request.header("x-loongport-client-key", key);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) if case == "auth_lease" => {
                assert!(
                    error.is_request() && !error.is_timeout(),
                    "expected fenced connection close, not timeout or setup failure: {error}"
                );
                results.push((0, String::new()));
                continue;
            }
            Err(error) => panic!("fixture HTTP request failed: {error}"),
        };
        let status = response.status().as_u16();
        if matches!(case, "stream" | "auth_lease") {
            use futures::StreamExt;
            let mut bytes = response.bytes_stream();
            let mut text = String::new();
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Ok(chunk) => text.push_str(&String::from_utf8_lossy(&chunk)),
                    Err(_) => break,
                }
            }
            results.push((status, text));
        } else {
            results.push((status, response.text().await.unwrap()));
        }
    }
    let status = server.get_status().await;
    server.stop().await.unwrap();
    a_task.abort();
    b_task.abort();
    if let Some((_, task)) = outside {
        task.abort();
    }
    assert_eq!(
        status.failover_count, 0,
        "shared group success must not schedule old current-provider persistence"
    );
    let expected_current = if matches!(case, "exhausted" | "group_budget") {
        "outside"
    } else if case == "legacy_budget" {
        a_id.as_str()
    } else {
        b_id.as_str()
    };
    for app in ["claude", "codex"] {
        assert_eq!(
            db.get_current_provider(app).unwrap().as_deref(),
            Some(expected_current),
            "forwarding must not rewrite the pre-request legacy current"
        );
    }
    if matches!(case, "unavailable" | "unknown_model" | "missing_schema") {
        for (status, body) in results {
            assert_ne!(status, 200, "unavailable group must fail closed: {body}");
        }
        assert!(
            attempts.lock().unwrap().is_empty(),
            "must not fall back outside group"
        );
        return;
    }
    if case == "exhausted" {
        for (status, body) in results {
            assert!(
                status >= 500,
                "both retryable upstream failures must terminate: {status} {body}"
            );
            assert!(!body.contains("fixture-ok"));
        }
        assert_eq!(
            *attempts.lock().unwrap(),
            vec![
                ("a".into(), "model-a".into()),
                ("b".into(), "model-b".into()),
                ("a".into(), "model-a".into()),
                ("b".into(), "model-b".into()),
            ],
            "both applications must exhaust A then B, never successful outside legacy current"
        );
        return;
    }
    if matches!(case, "manual" | "legacy_budget") {
        for (status, _) in results {
            assert_ne!(status, 200);
        }
        assert_eq!(
            *attempts.lock().unwrap(),
            vec![
                ("a".into(), "model-a".into()),
                ("a".into(), "model-a".into())
            ],
            "manual must reach chosen A through same gateway, never B"
        );
        return;
    }
    if case == "stream" {
        assert_eq!(results[0].0, 200);
        assert!(results[0].1.contains("fixture-visible-prefix"));
        assert_eq!(
            *attempts.lock().unwrap(),
            vec![("a".into(), "model-a".into())],
            "stream output must never replay on B"
        );
        return;
    }
    if case == "auth_lease" {
        let revoked:i64=db.conn.lock().unwrap().query_row("SELECT count(*) FROM loongport_client_credentials WHERE allowed_models='[\"requested-alias\"]'",[],|row|row.get(0)).unwrap();
        assert_eq!(
            revoked, 2,
            "upstream must have actually revoked member scopes"
        );
        assert!(
            results[0].1.is_empty(),
            "revoked actual-model scope must fence every response byte"
        );
        assert_eq!(
            *attempts.lock().unwrap(),
            vec![("a".into(), "model-a".into())]
        );
        return;
    }
    if case == "auth_override" {
        for (status, body) in results {
            assert_eq!(
                status, 401,
                "allowed alias must not grant group member model: {body}"
            );
        }
        assert!(
            attempts.lock().unwrap().is_empty(),
            "unauthorized group model reached upstream"
        );
        return;
    }
    if case == "source_changed" {
        assert_eq!(results[0].0, 400);
        assert!(results[0].1.contains("account_capacity.provider_changed"));
        assert_eq!(results[1].0, 200);
        assert_eq!(
            *attempts.lock().unwrap(),
            vec![
                ("a".into(), "model-a".into()),
                ("b".into(), "model-new".into())
            ]
        );
        return;
    }
    for (status, body) in results {
        assert_eq!(status, 200, "{body}");
    }
    let expected = if matches!(case, "legacy" | "historical") {
        vec![
            ("b".into(), "model-b".into()),
            ("b".into(), "model-b".into()),
        ]
    } else if case == "snapshot" {
        vec![
            ("a".into(), "model-a".into()),
            ("b".into(), "model-b".into()),
            ("b".into(), "model-b".into()),
        ]
    } else if matches!(case, "fallback" | "group_budget") {
        vec![
            ("a".into(), "model-a".into()),
            ("b".into(), "model-b".into()),
            ("a".into(), "model-a".into()),
            ("b".into(), "model-b".into()),
        ]
    } else {
        vec![
            ("a".into(), "model-a".into()),
            ("a".into(), "model-a".into()),
        ]
    };
    assert_eq!(
        *attempts.lock().unwrap(),
        expected,
        "both applications must consume the shared ordered model snapshot, not legacy current B"
    );
}

fn assert_historical_v29_owner_shape(conn: &rusqlite::Connection) {
    for (table, column) in [
        ("providers", "credential_revision"),
        ("loongport_vendor", "api_key_revision"),
    ] {
        let columns = conn
            .prepare(&format!("PRAGMA table_xinfo({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            !columns.iter().any(|name| name == column),
            "historical v29 fixture retained v31 owner column"
        );
    }
}

fn verify_schema() {
    use crate::services::routing_groups as groups;
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("DROP TABLE loongport_application_route_bindings;DROP TABLE loongport_routing_groups;ALTER TABLE providers DROP COLUMN credential_revision; ALTER TABLE loongport_vendor DROP COLUMN api_key_revision;CREATE TABLE loongport_routing_groups(id TEXT);UPDATE loongport_schema_version SET version=29 WHERE id=1;").unwrap();
    assert_historical_v29_owner_shape(&conn);
    assert!(crate::database::loongport_schema::apply(&conn).is_err());
    assert_eq!(
        crate::database::loongport_schema::read_stored_version(&conn).unwrap(),
        29
    );
    let created:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='loongport_application_route_bindings')",[],|r|r.get(0)).unwrap();
    assert!(
        !created,
        "failed migration must roll back newly created binding table"
    );
    assert!(groups::validate_schema(&conn).is_err());
    conn.execute_batch("DROP TABLE loongport_routing_groups;")
        .unwrap();
    crate::database::loongport_schema::apply(&conn).unwrap();
    groups::validate_schema(&conn).unwrap();
    assert_eq!(
        crate::database::loongport_schema::read_stored_version(&conn).unwrap(),
        crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
    );
}

pub fn wire_fixture() -> Value {
    use crate::mode::state::{SaveRequest, SaveScope};
    use crate::services::{routing_groups as groups, shared_routing as owner};
    let request = SaveRequest {
        id: "00000000-0000-4000-8000-000000000001".into(),
        scope: SaveScope::ApplicationRouting,
        provider_id: None,
        draft_digest: "a".repeat(64),
        revision: "b".repeat(64),
    };
    let manual = groups::RouteTarget::Manual {
        source_id: "synthetic-source".into(),
        model: "model-a".into(),
    };
    let ordered = groups::RouteTarget::OrderedGroup {
        group_id: "00000000-0000-4000-8000-000000000002".into(),
    };
    let group = groups::Group {
        id: "00000000-0000-4000-8000-000000000002".into(),
        name: "Daily".into(),
        revision: 2,
        members: vec![groups::Member {
            source_id: "synthetic-source".into(),
            model: "model-a".into(),
        }],
    };
    json!({
        "manual":serde_json::to_value(owner::PreparedRouteTarget{target:manual.clone(),request:request.clone(),expected_binding_revision:None,group_revision:None}).unwrap(),
        "ordered":serde_json::to_value(owner::PreparedRouteTarget{target:ordered,request:request.clone(),expected_binding_revision:Some(1),group_revision:Some(2)}).unwrap(),
        "preparedGroup":serde_json::to_value(owner::PreparedGroup{group:group.clone(),expected_revision:Some(1),draft_digest:"c".repeat(64),affected_app_ids:vec!["claude".into(),"codex".into()]}).unwrap(),
        "emptyState":serde_json::to_value(owner::SharedRoutingState{app_id:"claude".into(),binding:None,groups:vec![group.clone()],can_apply:false,blocked_reason:Some("routing_groups.connection_required".into()),pending_request:None}).unwrap(),
        "pendingState":serde_json::to_value(owner::SharedRoutingState{app_id:"claude".into(),binding:Some(groups::Binding{target:manual,revision:1}),groups:vec![group],can_apply:false,blocked_reason:Some("routing_groups.recovery_required".into()),pending_request:Some(request.clone())}).unwrap(),
        "completed":serde_json::to_value(owner::RouteRequestResult{request,status:"completed".into()}).unwrap()
    })
}
