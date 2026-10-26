//! Original Provider SQL/vault -> reviewed shared target -> real adapters.
//! Only a fresh fixture can redirect fixed official Chat output to loopback.
use super::{server::ProxyServer, ProxyConfig, ProxyError};
use crate::{
    app_config::AppType,
    database::Database,
    live::engine::DeviceStore,
    mode::state::{self, Mode},
    provider::Provider,
    secrets::{session::SecretSession, testing::TestHome},
    services::{
        openrouter_source::{self, RowRef, SourceFact},
        routing_groups as groups, shared_routing as routing, ProviderService,
    },
    store::AppState,
};
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;
const KEYS: [&str; 2] = [
    "synthetic-openrouter-shared-key-a",
    "synthetic-openrouter-shared-key-b",
];
const MODEL: &str = "synthetic/shared-configured-model";
const OFFICIAL: &str = "https://openrouter.ai/api/v1/chat/completions";
const CANARY: &str = "synthetic-target-only-canary-key";
fn unavailable() -> ProxyError {
    ProxyError::ConfigError("synthetic_openrouter_transport_unavailable".into())
}

#[derive(Clone)]
pub(crate) struct FixtureTransport {
    session: Arc<SecretSession>,
    root: PathBuf,
    epoch: u64,
    ids: [String; 2],
    source_ids: [String; 2],
    origin: String,
    client: reqwest::Client,
}
pub(crate) struct FixtureOutput {
    url: String,
    client: reqwest::Client,
}
impl FixtureOutput {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
    pub(crate) fn client(&self) -> reqwest::Client {
        self.client.clone()
    }
}
/// Invoked only at the existing output boundary after real auth/format work.
/// Test-hooks builds cannot send fixture-shaped official requests without this capability.
pub(crate) fn prepare_output(
    extensions: &http::Extensions,
    session: &Arc<SecretSession>,
    provider: &Provider,
    url: &str,
    app: &AppType,
) -> Result<Option<FixtureOutput>, ProxyError> {
    let parsed = url::Url::parse(url).map_err(|_| unavailable())?;
    if parsed.host_str() != Some("openrouter.ai") {
        return Ok(None);
    }
    if url != OFFICIAL {
        return Err(unavailable());
    }
    let cap = extensions
        .get::<FixtureTransport>()
        .ok_or_else(unavailable)?;
    if !Arc::ptr_eq(session, &cap.session)
        || session.root().canonicalize().map_err(|_| unavailable())? != cap.root
        || session.lifecycle_epoch() != cap.epoch
    {
        return Err(unavailable());
    }
    let _vault = session.read().map_err(|_| unavailable())?;
    let index = cap
        .source_ids
        .iter()
        .zip(&cap.ids)
        .position(|(source, original)| provider.id == *source || provider.id == *original)
        .ok_or_else(unavailable)?;
    if crate::relay::provider_config::extract_api_key(&provider.settings_config, app).as_deref()
        != Some(KEYS[index])
    {
        return Err(unavailable());
    }
    Ok(Some(FixtureOutput {
        url: format!("{}{}", cap.origin, parsed.path()),
        client: cap.client.clone(),
    }))
}
struct Capture {
    headers: HeaderMap,
    body: Value,
    path: String,
}
struct Fixture {
    state: AppState,
    proxy: ProxyServer,
    ids: [String; 2],
    facts: [SourceFact; 2],
    transport: FixtureTransport,
    calls: Arc<Mutex<Vec<Capture>>>,
    upstream: tokio::task::JoinHandle<()>,
    release: Arc<tokio::sync::Notify>,
    _home: TestHome,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl Fixture {
    async fn new(case: &str) -> Self {
        let plaintext = case.starts_with("plaintext_");
        let case = case.strip_prefix("plaintext_").unwrap_or(case);
        let fail_first = matches!(
            case,
            "manual" | "ordered" | "collision" | "reseal_b" | "generic_save_b"
        );
        let _ = rustls::crypto::ring::default_provider().install_default();
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(if plaintext {
            crate::services::native_source_guard::storage_fixture::fresh_plaintext_database()
        } else {
            crate::secrets::testing::initialize_database().unwrap()
        });
        Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap()).unwrap();
        crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
        for app in ["claude", "codex"] {
            super::application_routing::migrate(&db, app).unwrap();
        }
        let ids = [
            "synthetic-openrouter-origin-a".to_string(),
            "synthetic-openrouter-origin-b".to_string(),
        ];
        for (index, id) in ids.iter().enumerate() {
            let settings = json!({"auth":{"OPENAI_API_KEY":KEYS[index]},"modelCatalog":{"models":[{"model":MODEL}]},"config":format!("model_provider = \"openrouter\"\nmodel = {MODEL:?}\n[model_providers.openrouter]\nname = \"OpenRouter\"\nbase_url = \"https://openrouter.ai/api/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = true\n")});
            let mut provider = Provider::with_id(
                id.clone(),
                format!("Synthetic original OpenRouter {index}"),
                settings,
                Some("https://openrouter.ai".into()),
            );
            provider.category = Some("aggregator".into());
            if case == "model_options" {
                provider
                    .meta
                    .get_or_insert_with(Default::default)
                    .local_proxy_request_overrides = Some(
                    crate::provider::LocalProxyRequestOverrides {
                        headers: std::collections::HashMap::from([
                            ("X-Synthetic-Option".into(), "preserved".into()),
                            (
                                "Authorization".into(),
                                "Bearer synthetic-forbidden-override".into(),
                            ),
                            ("Host".into(), "synthetic-forbidden.invalid".into()),
                        ]),
                        body: Some(
                            json!({"model":"synthetic-forbidden-model-override","temperature":0.2,"stream":true}),
                        ),
                    },
                );
            }
            db.save_provider("codex", &provider).unwrap();
        }
        let facts: [SourceFact; 2] = ids
            .iter()
            .map(|id| {
                let vault = db.secrets.read().unwrap();
                let conn = db.conn.lock().unwrap();
                openrouter_source::read_on(
                    &conn,
                    &vault,
                    &RowRef {
                        origin_app: "codex".into(),
                        provider_id: id.clone(),
                    },
                )
                .unwrap()
                .expect("original official Provider row must be visible")
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let upstream_release = release.clone();
        let original_db = db.clone();
        let original_ids = ids.clone();
        let behavior = case.to_owned();
        let router = axum::Router::new().fallback(axum::routing::post(move |uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let captured = captured.clone(); let release = upstream_release.clone(); let db = original_db.clone(); let ids = original_ids.clone(); let behavior = behavior.clone();
            async move {
                let first = headers.get("authorization").and_then(|h| h.to_str().ok()) == Some(format!("Bearer {}", KEYS[0]).as_str());
                let ordinal = { let mut calls = captured.lock().unwrap(); let ordinal = calls.len(); calls.push(Capture { headers, body: body.clone(), path: uri.path().into() }); ordinal };
                if first && matches!(behavior.as_str(), "reseal_b" | "generic_save_b") {
                    let original = db.get_provider_by_id(&ids[1], "codex").unwrap().unwrap();
                    if behavior == "reseal_b" {
                        // Explicit original credential intent, not incidental ciphertext resealing.
                        db.save_provider_with_credential_intent("codex", &original, true).unwrap();
                    } else {
                        db.save_provider("codex", &original).unwrap();
                    }
                }
                if first && fail_first { return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({"error":{"type":"api_error","message":"synthetic source unavailable"}}))).into_response(); }
                if uri.path() != "/api/v1/chat/completions" { return StatusCode::IM_A_TEAPOT.into_response(); }
                if ordinal == 0 && matches!(behavior.as_str(), "stream_partial" | "stream_epoch") {
                    let prefix = format!("data: {}\n\n", json!({"id":"chat_shared_stream","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{"role":"assistant","content":"original-visible-prefix."},"finish_reason":null}]}));
                    let stream = async_stream::stream! {
                        yield Ok::<_, std::io::Error>(axum::body::Bytes::from(prefix));
                        release.notified().await;
                        if behavior == "stream_epoch" {
                            yield Ok(axum::body::Bytes::from(format!("data: {}\n\ndata: [DONE]\n\n", json!({"id":"chat_shared_stream","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{"content":"forbidden-after-epoch."},"finish_reason":"stop"}]}))));
                        } else {
                            yield Err(std::io::Error::other("private synthetic interrupted stream"));
                        }
                    };
                    return axum::http::Response::builder().status(200).header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
                }
                axum::Json(json!({"id":"chatcmpl_original","object":"chat.completion","model":body["model"],"choices":[{"index":0,"message":{"role":"assistant","content":"original OpenRouter shared answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let origin = format!("http://{address}");
        // This target-app row is NOT a source projection: separate credentials,
        // loopback endpoint and same local ID deliberately detect accidental borrowing.
        let canary = Provider::with_id(
            ids[0].clone(),
            "Independent target canary".into(),
            json!({"env":{"ANTHROPIC_BASE_URL":format!("{origin}/target-canary"),"ANTHROPIC_AUTH_TOKEN":CANARY,"ANTHROPIC_MODEL":"synthetic/canary"}}),
            None,
        );
        db.save_provider("claude", &canary).unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let session = db.secrets.clone();
        let root = session.root().canonicalize().unwrap();
        assert!(root.starts_with(home.path().canonicalize().unwrap()));
        let transport = FixtureTransport {
            session: session.clone(),
            root,
            epoch: session.lifecycle_epoch(),
            ids: ids.clone(),
            source_ids: [facts[0].source_id.clone(), facts[1].source_id.clone()],
            origin,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        };
        let state = AppState::new(db.clone()).unwrap();
        state::update(
            &DeviceStore::for_device(),
            &session.read().unwrap(),
            |live| {
                for app in ["claude", "codex"] {
                    live.apps.entry(app.into()).or_default().mode = Some(Mode::Direct);
                }
                Ok(())
            },
        )
        .unwrap();
        let mut config = db.get_proxy_config().await.unwrap();
        config.listen_address = "127.0.0.1".into();
        config.listen_port = 0;
        db.update_proxy_config(config).await.unwrap();
        let proxy = ProxyServer::new(
            ProxyConfig::default(),
            db,
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(super::model_alignment::ModelAlignmentAlerts::new()),
        );
        Self {
            state,
            proxy,
            ids,
            facts,
            transport,
            calls,
            upstream,
            release,
            _home: home,
        }
    }
    async fn bind(&self, app: AppType, target: groups::RouteTarget, selected: usize) {
        let state = self.state.clone();
        let id = self.ids[selected].clone();
        let switching_app = app.clone();
        tokio::task::spawn_blocking(move || {
            ProviderService::switch(&state, switching_app, &id).unwrap()
        })
        .await
        .unwrap();
        self.state
            .proxy_service
            .set_takeover_for_app(app.as_str(), true)
            .await
            .unwrap();
        let mut config = self
            .state
            .db
            .get_proxy_config_for_app(app.as_str())
            .await
            .unwrap();
        config.max_retries = 0;
        config.non_streaming_timeout = 5;
        self.state
            .db
            .update_proxy_config_for_app(config)
            .await
            .unwrap();
        let prepared = tokio::task::block_in_place(|| {
            routing::prepare_target(&self.state, &app, target.clone())
        })
        .unwrap();
        let request = prepared.request.clone();
        assert_eq!(
            tokio::task::block_in_place(|| routing::apply_target(&self.state, &app, prepared))
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            tokio::task::block_in_place(|| routing::query_target(&self.state, &app, request))
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            groups::binding(&self.state.db.conn.lock().unwrap(), app.as_str())
                .unwrap()
                .unwrap()
                .target,
            target
        );
    }
    async fn request_raw(
        &self,
        app: &AppType,
        cap: Option<FixtureTransport>,
        streaming: bool,
    ) -> axum::response::Response {
        let mut body = if *app == AppType::Claude {
            json!({"model":"client-alias-must-not-win","max_tokens":32,"messages":[{"role":"user","content":"synthetic shared request"}],"stream":false})
        } else {
            json!({"model":"client-alias-must-not-win","input":"synthetic shared request","stream":false})
        };
        body["stream"] = json!(streaming);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(if *app == AppType::Claude {
                "/v1/messages"
            } else {
                "/v1/responses"
            })
            .header("content-type", "application/json")
            .header("authorization", "Bearer synthetic-untrusted-inbound")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let mut router = self.proxy.build_router();
        if let Some(cap) = cap {
            router = router.layer(axum::Extension(cap));
        }
        let response = tokio::time::timeout(Duration::from_secs(8), router.oneshot(request))
            .await
            .unwrap()
            .unwrap();
        response
    }
    async fn request(&self, app: &AppType, cap: bool) -> (StatusCode, String) {
        let response = self
            .request_raw(app, cap.then(|| self.transport.clone()), false)
            .await;
        Self::consume(response).await
    }
    async fn consume(response: axum::response::Response) -> (StatusCode, String) {
        let status = response.status();
        let body = tokio::time::timeout(
            Duration::from_secs(6),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }
}
fn raw_provider_rows(db: &Database) -> Vec<(String, String, String, String)> {
    let conn = db.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id,app_type,settings_config,meta FROM providers ORDER BY app_type,id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}
async fn assert_request_log(f: &Fixture, app: &AppType, source_id: &str) {
    let filters = crate::services::usage_stats::LogFilters {
        app_type: Some(app.as_str().into()),
        status_code: Some(200),
        ..Default::default()
    };
    let rows = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = f.state.db.get_request_logs(&filters, 0, 20).unwrap();
            if !page.data.is_empty() {
                break page.data;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("original asynchronous request logger must persist actual observed usage");
    assert!(rows.iter().all(|r| r.app_type == app.as_str() && r.provider_id == source_id && r.model == MODEL), "request log keeps consumer app and canonical source, not borrowed origin app or target canary");
}

pub(crate) async fn verify(case: &str) {
    let fail_first = matches!(
        case,
        "manual" | "ordered" | "collision" | "reseal_b" | "generic_save_b"
    );
    let f = Fixture::new(case).await;
    let case = case.strip_prefix("plaintext_").unwrap_or(case);
    if case == "pricing_reference" {
        let db = &f.state.db;
        db.set_default_cost_multiplier("claude", "2.5")
            .await
            .unwrap();
        db.set_pricing_model_source("claude", "response")
            .await
            .unwrap();
        db.set_default_cost_multiplier("codex", "9").await.unwrap();
        db.set_pricing_model_source("codex", "request")
            .await
            .unwrap();
        let logger = super::usage::logger::UsageLogger::new(db);
        let fallback = logger
            .resolve_pricing_config(&f.facts[0].source_id, "claude")
            .await;
        assert_eq!(
            fallback.0.to_string(),
            "2.5",
            "no owner override keeps consumer default"
        );
        assert_eq!(fallback.1, "response");
        let mut row = db.get_provider_by_id(&f.ids[0], "codex").unwrap().unwrap();
        let meta = row.meta.get_or_insert_with(Default::default);
        meta.cost_multiplier = Some("1.25".into());
        meta.pricing_model_source = Some("request".into());
        db.save_provider("codex", &row).unwrap();
        let resolved = logger
            .resolve_pricing_config(&f.facts[0].source_id, "claude")
            .await;
        assert_eq!(
            resolved.0.to_string(),
            "1.25",
            "canonical source follows original row override, not target same-ID row"
        );
        assert_eq!(resolved.1, "request");
        assert!(f.calls.lock().unwrap().is_empty());
        return;
    }
    if case == "limits_reference" {
        let db = &f.state.db;
        let mut row = db.get_provider_by_id(&f.ids[0], "codex").unwrap().unwrap();
        let meta = row.meta.get_or_insert_with(Default::default);
        meta.limit_daily_usd = Some("9".into());
        meta.limit_monthly_usd = Some("11".into());
        db.save_provider("codex", &row).unwrap();
        // Explicit synthetic history, not real billing. This reduced candidate
        // intentionally preserves the legacy per-provider/per-app statistics.
        // Canonical shared-consumer history remains separately attributed.
        {
            let conn = db.conn.lock().unwrap();
            for (request, id, app, cost) in [
                ("synthetic-old-origin", f.ids[0].as_str(), "codex", "1"),
                (
                    "synthetic-shared-claude",
                    f.facts[0].source_id.as_str(),
                    "claude",
                    "2",
                ),
                (
                    "synthetic-shared-codex",
                    f.facts[0].source_id.as_str(),
                    "codex",
                    "3",
                ),
                (
                    "synthetic-independent-canary",
                    f.ids[0].as_str(),
                    "claude",
                    "100",
                ),
            ] {
                conn.execute("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,total_cost_usd,latency_ms,status_code,created_at) VALUES (?1,?2,?3,?4,?5,1,200,strftime('%s','now'))", rusqlite::params![request,id,app,MODEL,cost]).unwrap();
            }
            conn.execute("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,total_cost_usd) VALUES (date('now','localtime'),'claude',?1,?2,'4')", rusqlite::params![f.facts[0].source_id,MODEL]).unwrap();
        }
        let before = db.get_request_logs(&Default::default(), 0, 20).unwrap();
        let limits = db.check_provider_limits(&f.ids[0], "codex").unwrap();
        assert_eq!(
            limits.daily_usage, "1.000000",
            "legacy interface intentionally excludes canonical shared consumers"
        );
        assert_eq!(limits.monthly_usage, "1.000000");
        assert!(!limits.daily_exceeded);
        assert!(!limits.monthly_exceeded);
        let canary = db.check_provider_limits(&f.ids[0], "claude").unwrap();
        assert_eq!(canary.daily_usage, "100.000000");
        // The retained contract still enforces its original row's configured
        // threshold; excluding shared aliases must not disable legacy limits.
        row.meta.as_mut().unwrap().limit_daily_usd = Some("0.5".into());
        row.meta.as_mut().unwrap().limit_monthly_usd = Some("0.5".into());
        db.save_provider("codex", &row).unwrap();
        let original_limit = db.check_provider_limits(&f.ids[0], "codex").unwrap();
        assert!(original_limit.daily_exceeded);
        assert!(original_limit.monthly_exceeded);
        {
            let conn = db.conn.lock().unwrap();
            let shared: (i64, f64) = conn.query_row(
                "SELECT COUNT(*), SUM(CAST(total_cost_usd AS REAL)) FROM proxy_request_logs WHERE provider_id=?1",
                [&f.facts[0].source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(shared, (2, 5.0), "shared request history remains intact");
            let rollup: String = conn.query_row(
                "SELECT total_cost_usd FROM usage_daily_rollups WHERE provider_id=?1 AND app_type='claude'",
                [&f.facts[0].source_id], |row| row.get(0),
            ).unwrap();
            assert_eq!(rollup, "4", "shared rollup remains separately attributed");
        }
        let after = db.get_request_logs(&Default::default(), 0, 20).unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap(),
            "legacy statistics must not rewrite or remove shared-consumer history"
        );
        assert!(f.calls.lock().unwrap().is_empty());
        return;
    }
    let before = serde_json::to_value([
        f.state.db.get_all_providers("codex").unwrap(),
        f.state.db.get_all_providers("claude").unwrap(),
    ])
    .unwrap();
    if case == "source" {
        let rows = crate::commands::get_service_sources_impl(&f.state).unwrap();
        for fact in &f.facts {
            let row = rows
                .iter()
                .find(|row| row.source_id == fact.source_id)
                .expect("safe local source inventory");
            let serialized = serde_json::to_string(row).unwrap();
            let wire = serde_json::to_value(row).unwrap();
            assert_eq!(
                wire["account"],
                json!({"kind":"localProvider","platform":"openrouter","id":fact.source_id,"originApp":"codex","originProviderId":fact.origin.provider_id})
            );
            let bindings = wire["bindings"].as_array().unwrap();
            assert_eq!(
                bindings.len(),
                1,
                "only the real origin row is a stored binding"
            );
            assert_eq!(bindings[0]["appId"], "codex");
            assert_eq!(bindings[0]["providerId"], fact.origin.provider_id);
            assert_eq!(bindings[0]["model"], MODEL);
            let targets = wire["supportedTargets"].as_array().unwrap();
            assert_eq!(targets.len(), 2);
            for app in ["claude", "codex"] {
                assert_eq!(
                    targets.iter().find(|target| target["appId"] == app),
                    Some(&json!({"appId":app,"model":MODEL,"canSelect":true}))
                );
            }
            for secret in KEYS {
                assert!(!serialized.contains(secret));
            }
            assert!(!serialized.contains(&fact.stable_evidence));
        }
        return;
    }
    let mut members = f
        .facts
        .iter()
        .map(|fact| groups::Member {
            source_id: fact.source_id.clone(),
            model: MODEL.into(),
        })
        .collect::<Vec<_>>();
    if case == "all_empty" {
        members.truncate(1);
    }
    let prepared = tokio::task::block_in_place(|| {
        routing::prepare_group(
            &f.state,
            routing::GroupDraft {
                id: None,
                expected_revision: None,
                name: "Original OpenRouter rows".into(),
                members: members.clone(),
            },
        )
    })
    .unwrap();
    if case == "stale" {
        let mut row = f
            .state
            .db
            .get_provider_by_id(&f.ids[0], "codex")
            .unwrap()
            .unwrap();
        row.settings_config["auth"]["OPENAI_API_KEY"] = json!("synthetic-replaced-original-key");
        f.state.db.save_provider("codex", &row).unwrap();
        assert!(
            tokio::task::block_in_place(|| routing::confirm_group(&f.state, prepared)).is_err()
        );
        assert!(f.calls.lock().unwrap().is_empty());
        return;
    }
    let group = tokio::task::block_in_place(|| routing::confirm_group(&f.state, prepared)).unwrap();
    let target = if matches!(case, "manual" | "manual_empty") {
        groups::RouteTarget::Manual {
            source_id: members[0].source_id.clone(),
            model: MODEL.into(),
        }
    } else {
        groups::RouteTarget::OrderedGroup {
            group_id: group.id.clone(),
        }
    };
    let apps = if matches!(case, "two_apps" | "collision" | "model_options") {
        vec![AppType::Codex, AppType::Claude]
    } else {
        vec![AppType::Codex]
    };
    for app in &apps {
        f.bind(
            app.clone(),
            target.clone(),
            usize::from(matches!(case, "all_empty" | "corrupt" | "unknown")),
        )
        .await;
    }
    f.state
        .db
        .update_provider_health(
            &f.ids[0],
            "claude",
            false,
            Some("untouched-target-canary".into()),
        )
        .await
        .unwrap();
    let canary_health = serde_json::to_value(
        f.state
            .db
            .get_provider_health(&f.ids[0], "claude")
            .await
            .unwrap(),
    )
    .unwrap();
    if case == "generic_save_b" {
        let (status, _) = f.request(&AppType::Codex, true).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "ordinary same-content save is not credential reissue intent"
        );
        assert_eq!(
            f.calls.lock().unwrap().len(),
            2,
            "captured original B remains admissible after noncredential save"
        );
        f.state.proxy_service.stop().await.unwrap();
        return;
    }
    if case == "reseal_b" {
        let (old_status, _) = f.request(&AppType::Codex, true).await;
        let old_calls = f.calls.lock().unwrap().len();
        let fresh = tokio::task::block_in_place(|| {
            routing::prepare_target(
                &f.state,
                &AppType::Codex,
                groups::RouteTarget::Manual {
                    source_id: f.facts[1].source_id.clone(),
                    model: MODEL.into(),
                },
            )
        })
        .unwrap();
        assert_eq!(
            tokio::task::block_in_place(|| routing::apply_target(&f.state, &AppType::Codex, fresh))
                .unwrap()
                .status,
            "completed"
        );
        let (fresh_status, fresh_body) = f.request(&AppType::Codex, true).await;
        assert_eq!(
            fresh_status,
            StatusCode::OK,
            "same-key fresh owner must be usable: {fresh_body}"
        );
        {
            let calls = f.calls.lock().unwrap();
            assert_eq!(calls.len(), old_calls + 1);
            assert_eq!(
                calls.last().unwrap().headers["authorization"],
                format!("Bearer {}", KEYS[1])
            );
            assert_eq!(calls.last().unwrap().body["model"], MODEL);
        }
        assert!(
            !old_status.is_success(),
            "old captured request must not adopt newly resealed B"
        );
        assert_eq!(
            old_calls, 1,
            "B is fenced before upstream while a NEW request remains usable"
        );
        f.state.proxy_service.stop().await.unwrap();
        return;
    }
    if matches!(case, "empty_first" | "manual_empty" | "all_empty") {
        let mut row = f
            .state
            .db
            .get_provider_by_id(&f.ids[0], "codex")
            .unwrap()
            .unwrap();
        row.settings_config["auth"]["OPENAI_API_KEY"] = json!("");
        f.state.db.save_provider("codex", &row).unwrap();
    }
    if case == "all_empty" {
        assert_eq!(
            f.state.db.get_current_provider("codex").unwrap().as_deref(),
            Some(f.ids[1].as_str()),
            "healthy original B is outside the A-only group"
        );
    }
    if case == "corrupt" {
        f.state.db.conn.lock().unwrap().execute("UPDATE providers SET settings_config='synthetic-corrupt-original-row' WHERE id=?1 AND app_type='codex'", [&f.ids[0]]).unwrap();
    }
    if case == "unknown" {
        f.state.db.delete_provider("codex", &f.ids[0]).unwrap();
    }
    let raw_before_dispatch = raw_provider_rows(&f.state.db);
    if matches!(case, "stream_partial" | "stream_epoch") {
        use futures::StreamExt;
        let response = f
            .request_raw(&AppType::Codex, Some(f.transport.clone()), true)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&bytes).contains("original-visible-prefix.") {
                bytes.extend_from_slice(
                    &stream
                        .next()
                        .await
                        .expect("visible prefix absent")
                        .expect("upstream failed before visible output"),
                );
            }
        })
        .await
        .expect("observe actual client output before inducing failure");
        if case == "stream_epoch" {
            f.state.db.secrets.set_blocked(true);
            f.state.db.secrets.set_blocked(false);
        }
        f.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(_) => break,
                }
            }
        })
        .await
        .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("forbidden-after-epoch"));
        assert!(!text.contains("response.completed"));
        assert!(!text.contains("private synthetic interrupted stream"));
        assert_eq!(
            f.calls.lock().unwrap().len(),
            1,
            "committed output never replays on B"
        );
        if case == "stream_epoch" {
            // Mint a new sealed test capability only after the SAME original owner
            // is available again. The old stream already passed cap validation;
            // its tail must be stopped by the production Fence, not this cap.
            let mut fresh_cap = f.transport.clone();
            {
                let vault = f.state.db.secrets.read().unwrap();
                let conn = f.state.db.conn.lock().unwrap();
                for (i, id) in f.ids.iter().enumerate() {
                    let current = openrouter_source::read_on(
                        &conn,
                        &vault,
                        &RowRef {
                            origin_app: "codex".into(),
                            provider_id: id.clone(),
                        },
                    )
                    .unwrap()
                    .unwrap();
                    assert_eq!(current.source_id, fresh_cap.source_ids[i]);
                }
                fresh_cap.epoch = f.state.db.secrets.lifecycle_epoch();
            }
            let response = f.request_raw(&AppType::Codex, Some(fresh_cap), false).await;
            let (status, body) = Fixture::consume(response).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "fresh ordinary-unlock request remains usable: {body}"
            );
            assert_eq!(f.calls.lock().unwrap().len(), 2);
        }
        assert_eq!(raw_provider_rows(&f.state.db), raw_before_dispatch);
        f.state.proxy_service.stop().await.unwrap();
        return;
    }
    let active_apps = if case == "collision" {
        vec![AppType::Claude]
    } else {
        apps
    };
    for app in &active_apps {
        let (status, body) = f.request(app, case != "missing_cap").await;
        if matches!(
            case,
            "manual" | "missing_cap" | "manual_empty" | "all_empty" | "corrupt" | "unknown"
        ) {
            assert!(!status.is_success(), "{body}");
        } else {
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(body.contains("original OpenRouter shared answer"));
        }
    }
    assert_eq!(
        raw_provider_rows(&f.state.db),
        raw_before_dispatch,
        "dispatch must not copy or alter any persisted provider row"
    );
    if !matches!(
        case,
        "empty_first" | "manual_empty" | "all_empty" | "corrupt" | "unknown"
    ) {
        assert_eq!(
            serde_json::to_value([
                f.state.db.get_all_providers("codex").unwrap(),
                f.state.db.get_all_providers("claude").unwrap()
            ])
            .unwrap(),
            before,
            "shared publication/dispatch cannot copy key or source rows into target app"
        );
    }
    assert_eq!(
        serde_json::to_value(
            f.state
                .db
                .get_provider_health(&f.ids[0], "claude")
                .await
                .unwrap()
        )
        .unwrap(),
        canary_health,
        "origin failures must not mutate same-ID target row"
    );
    {
        let calls = f.calls.lock().unwrap();
        let expected = if matches!(
            case,
            "missing_cap" | "manual_empty" | "all_empty" | "corrupt" | "unknown"
        ) {
            0
        } else if case == "manual" {
            1
        } else if fail_first {
            2
        } else {
            active_apps.len()
        };
        assert_eq!(calls.len(), expected);
        for (index, call) in calls.iter().enumerate() {
            assert_eq!(call.path, "/api/v1/chat/completions");
            assert_eq!(call.body["model"], MODEL);
            assert!(call.body["messages"].is_array());
            let key = if case == "empty_first" {
                KEYS[1]
            } else if fail_first && case != "manual" {
                KEYS[index]
            } else {
                KEYS[0]
            };
            assert_eq!(call.headers["authorization"], format!("Bearer {key}"));
            if case == "model_options" {
                assert_eq!(call.body["temperature"], json!(0.2));
                assert_ne!(
                    call.body["stream"],
                    json!(true),
                    "original protected stream behavior remains unchanged"
                );
                assert_eq!(call.headers["x-synthetic-option"], "preserved");
                assert_ne!(
                    call.headers.get("host").and_then(|v| v.to_str().ok()),
                    Some("synthetic-forbidden.invalid")
                );
            }
        }
    }
    if matches!(case, "collision" | "two_apps") {
        for app in &active_apps {
            assert_request_log(
                &f,
                app,
                &f.facts[usize::from(case == "collision")].source_id,
            )
            .await;
        }
    }
    if case == "collision" {
        let health = f
            .state
            .db
            .get_provider_health(&f.ids[0], "codex")
            .await
            .unwrap();
        assert!(
            health.consecutive_failures >= 1,
            "failure belongs to origin Codex row"
        );
    }
    assert_eq!(
        groups::read(&f.state.db.conn.lock().unwrap(), &group.id)
            .unwrap()
            .unwrap()
            .members,
        members
    );
    f.state.proxy_service.stop().await.unwrap();
}
