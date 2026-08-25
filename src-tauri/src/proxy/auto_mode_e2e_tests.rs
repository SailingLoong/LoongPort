//! 省心模式端到端（HTTP 线路）测试。
//!
//! `auto_strategy` / `provider_router` 的单元测试各自钉住排序与选路判定；
//! 这里把整条链拉直了打真实流量：本地 axum mock 上游 + 真实 `ProxyServer`
//! + reqwest 客户端，验证三条产品语义在线路上的落点：
//!
//! 1. 活跃会话外（无亲和）流量落最便宜档位；
//! 2. 活跃会话内不切换 —— 当前档位 30 分钟内有流量时，更便宜的档位不抢
//!    （中途换供应商丢提示词缓存，未命中按全价计费，是硬约束）；
//! 3. 当前档位故障时请求仍成功（请求内故障转移 + 熔断），恢复后回到在用档位。
//!
//! headless 边界：`FailoverSwitchManager::do_switch` 的热切换只在有
//! `AppHandle` 时执行（托盘/事件/写 live 都依赖它），本测试无 GUI，
//! DB「当前档位」不会因故障转移而变 —— 所以断言落点是「哪家 mock 收到了
//! 流量」与客户端最终拿到谁的响应，不断言热切换副作用。

use super::auto_strategy;
use super::server::ProxyServer;
use super::types::ProxyConfig;
use crate::app_config::AppType;
use crate::database::Database;
use crate::provider::Provider;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

/// 可编程 mock 上游：默认 200，可翻成 500 模拟故障；记录命中数与鉴权头。
/// `foreign` 开关让 mock 回 OpenAI 形状（模拟换芯转发，供被动监控 E2E）。
#[derive(Clone)]
struct MockUpstreamState {
    hits: Arc<AtomicUsize>,
    status: Arc<RwLock<u16>>,
    auth_header: Arc<RwLock<Option<String>>>,
    foreign: Arc<RwLock<bool>>,
    marker: &'static str,
}

struct MockUpstream {
    state: MockUpstreamState,
    port: u16,
}

impl MockUpstream {
    async fn spawn(marker: &'static str) -> Self {
        let state = MockUpstreamState {
            hits: Arc::new(AtomicUsize::new(0)),
            status: Arc::new(RwLock::new(200)),
            auth_header: Arc::new(RwLock::new(None)),
            foreign: Arc::new(RwLock::new(false)),
            marker,
        };
        let app = axum::Router::new()
            .fallback(handle_mock)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock upstream");
        let port = listener.local_addr().expect("mock local addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { state, port }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> usize {
        self.state.hits.load(Ordering::SeqCst)
    }

    async fn set_status(&self, status: u16) {
        *self.state.status.write().await = status;
    }

    async fn set_foreign(&self, foreign: bool) {
        *self.state.foreign.write().await = foreign;
    }

    async fn auth_header(&self) -> Option<String> {
        self.state.auth_header.read().await.clone()
    }
}

async fn handle_mock(
    State(state): State<MockUpstreamState>,
    headers: HeaderMap,
    _body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    state.hits.fetch_add(1, Ordering::SeqCst);
    *state.auth_header.write().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let status = *state.status.read().await;
    if status == 200 {
        if *state.foreign.read().await {
            // OpenAI chat.completions 形状出现在 Claude 线路 = 异源指纹（换芯转发）
            return (
                StatusCode::OK,
                Json(json!({
                    "object": "chat.completion",
                    "model": "gpt-5.6",
                    "choices": [{
                        "message": { "role": "assistant", "content": state.marker }
                    }],
                    "usage": {},
                })),
            );
        }
        (
            StatusCode::OK,
            Json(json!({
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": "claude-e2e",
                "content": [{ "type": "text", "text": state.marker }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 1, "output_tokens": 1 },
            })),
        )
    } else {
        (
            StatusCode::from_u16(status).expect("valid status"),
            Json(json!({
                "type": "error",
                "error": { "type": "api_error", "message": "mock upstream failure" },
            })),
        )
    }
}

struct TempHome {
    #[allow(dead_code)]
    dir: TempDir,
    original_home: Option<String>,
    original_userprofile: Option<String>,
    original_test_home: Option<String>,
}

impl TempHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp home");
        let original_home = std::env::var("HOME").ok();
        let original_userprofile = std::env::var("USERPROFILE").ok();
        let original_test_home = std::env::var("CC_SWITCH_TEST_HOME").ok();

        std::env::set_var("HOME", dir.path());
        std::env::set_var("USERPROFILE", dir.path());
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        crate::settings::reload_settings().expect("reload settings");

        Self {
            dir,
            original_home,
            original_userprofile,
            original_test_home,
        }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.original_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match &self.original_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        match &self.original_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

struct E2eFixture {
    _home: TempHome,
    db: Arc<Database>,
    server: ProxyServer,
    port: u16,
    cheap: MockUpstream,
    expensive: MockUpstream,
    cheap_id: String,
    expensive_id: String,
    /// 真协调器（被动消费 worker 在跑）；持有它保活 worker 生命周期
    #[allow(dead_code)]
    verification: Arc<crate::relay::model_verification::coordinator::ModelVerificationCoordinator>,
}

impl E2eFixture {
    /// 两个托管档位（便宜 0.5 / 贵 2.0，均无单价 → 组内按倍率比），
    /// 故障转移开、熔断 1 次失败即开且冷却 0（每条请求都能立即探测恢复）。
    async fn new() -> Self {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("init db"));

        let cheap = MockUpstream::spawn("served-by-cheap").await;
        let expensive = MockUpstream::spawn("served-by-expensive").await;

        let cheap_id =
            crate::relay::provision::provider_id_for("https://cheap.example", Some(1), 11);
        let expensive_id =
            crate::relay::provision::provider_id_for("https://expensive.example", Some(1), 22);

        let tier_provider = |id: &str, name: &str, mock: &MockUpstream, token: &str| {
            Provider::with_id(
                id.to_string(),
                name.to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": mock.base_url(),
                        "ANTHROPIC_AUTH_TOKEN": token,
                    }
                }),
                None,
            )
        };
        db.save_provider(
            "claude",
            &tier_provider(&cheap_id, "Cheap Tier", &cheap, "tok-cheap"),
        )
        .expect("save cheap tier");
        db.save_provider(
            "claude",
            &tier_provider(&expensive_id, "Expensive Tier", &expensive, "tok-expensive"),
        )
        .expect("save expensive tier");
        db.set_tier_rate_multiplier("claude", &cheap_id, Some(0.5))
            .expect("set cheap multiplier");
        db.set_tier_rate_multiplier("claude", &expensive_id, Some(2.0))
            .expect("set expensive multiplier");

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.enabled = true;
        config.auto_failover_enabled = true;
        config.max_retries = 3;
        config.circuit_failure_threshold = 1;
        config.circuit_timeout_seconds = 0;
        db.update_proxy_config_for_app(config).await.unwrap();

        auto_strategy::set_enabled(&db, "claude", true).expect("enable auto mode");

        // 真协调器：被动异常经消费 worker 落库（干净流量不落库，既有三条
        // 测试的正常 Claude 形状不会产生任何报告）
        let verification = Arc::new(
            crate::relay::model_verification::coordinator::ModelVerificationCoordinator::new(
                db.clone(),
            ),
        );
        let server = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..Default::default()
            },
            db.clone(),
            None,
            verification.passive_ingress(),
        );
        let info = server.start().await.expect("start proxy server");

        Self {
            _home,
            db,
            server,
            port: info.port,
            cheap,
            expensive,
            cheap_id,
            expensive_id,
            verification,
        }
    }

    fn set_current(&self, id: &str) {
        self.db.set_current_provider("claude", id).unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some(id)).unwrap();
    }

    /// 预置某档位最近有流量（亲和窗口判据读 proxy_request_logs）。
    fn seed_recent_activity(&self, provider_id: &str) {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model,
                input_tokens, output_tokens, total_cost_usd,
                latency_ms, first_token_ms, status_code, created_at
            ) VALUES (?1, ?2, 'claude', 'm', 1, 1, '0', 10, 10, 200, ?3)",
            rusqlite::params![
                format!("e2e-activity-{provider_id}"),
                provider_id,
                chrono::Utc::now().timestamp()
            ],
        )
        .unwrap();
    }
}

/// 走真实代理端口的 Claude 请求；`session` 模拟同一会话的连续请求。
async fn send_message(port: u16, session: &str) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build client")
        .post(format!("http://127.0.0.1:{port}/v1/messages"))
        .header("x-api-key", "client-key-should-be-overridden")
        .header("anthropic-version", "2023-06-01")
        .header("x-claude-code-session-id", session)
        .json(&json!({
            "model": "claude-e2e",
            "max_tokens": 16,
            "stream": false,
            "messages": [{ "role": "user", "content": "hi" }],
        }))
        .send()
        .await
        .expect("send request")
}

async fn response_text(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body: Value = resp.json().await.expect("parse response body");
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    body["content"][0]["text"]
        .as_str()
        .expect("marker text")
        .to_string()
}

/// 闲置（无当前档位活跃会话）时，省心模式把流量交给最便宜档位。
#[tokio::test]
#[serial]
async fn idle_traffic_goes_to_cheapest_tier() {
    let fx = E2eFixture::new().await;

    let marker = response_text(send_message(fx.port, "sess-e2e-idle-000001").await).await;
    assert_eq!(marker, "served-by-cheap");
    let marker = response_text(send_message(fx.port, "sess-e2e-idle-000002").await).await;
    assert_eq!(marker, "served-by-cheap");

    assert_eq!(fx.cheap.hits(), 2);
    assert_eq!(fx.expensive.hits(), 0, "更贵的档位不应被触碰");
    fx.server.stop().await.expect("stop server");
}

/// 活跃会话不切换：当前档位（贵）30 分钟内有流量时，更便宜的档位不抢，
/// 同一 session 连续请求全部落在当前档位；鉴权头注入的是该档位自己的 token。
#[tokio::test]
#[serial]
async fn active_session_sticks_to_current_tier_over_cheaper_one() {
    let fx = E2eFixture::new().await;
    fx.set_current(&fx.expensive_id);
    fx.seed_recent_activity(&fx.expensive_id);

    for i in 0..3 {
        let marker = response_text(send_message(fx.port, "sess-e2e-sticky-000001").await).await;
        assert_eq!(marker, "served-by-expensive", "第 {} 条请求被抢走了", i + 1);
    }

    assert_eq!(fx.expensive.hits(), 3);
    assert_eq!(fx.cheap.hits(), 0, "便宜档位不得抢活跃会话");
    assert_eq!(
        fx.expensive.auth_header().await.as_deref(),
        Some("Bearer tok-expensive"),
        "转发必须注入档位自己的 token，而不是客户端的"
    );
    fx.server.stop().await.expect("stop server");
}

/// 手动模式：用户清单序优先于策略序（便宜者让位）——首页看板拖拽落定的
/// 语义在真实选路链上的落点。
#[tokio::test]
#[serial]
async fn manual_order_overrides_strategy_in_routing() {
    let fx = E2eFixture::new().await;
    // 手动序：贵档第一（与 cheapest 策略序相反）
    auto_strategy::set_mode(&fx.db, "claude", auto_strategy::EasyModeMode::Manual).unwrap();
    auto_strategy::set_manual_order(
        &fx.db,
        "claude",
        &[fx.expensive_id.clone(), fx.cheap_id.clone()],
    )
    .unwrap();

    let marker = response_text(send_message(fx.port, "sess-e2e-manual-00001").await).await;
    assert_eq!(marker, "served-by-expensive");
    assert_eq!(fx.expensive.hits(), 1);
    assert_eq!(fx.cheap.hits(), 0, "策略上更便宜的档位必须让位给手动序");
    fx.server.stop().await.expect("stop server");
}

/// 在用档位（便宜）故障：请求内故障转移到下一家、客户端始终拿到 200；
/// 恢复后回到在用档位。熔断 1 次失败即开 + 冷却 0 ⇒ 每条请求允许探测一次。
#[tokio::test]
#[serial]
async fn failing_tier_fails_over_and_returns_when_recovered() {
    let fx = E2eFixture::new().await;
    fx.set_current(&fx.cheap_id);
    fx.seed_recent_activity(&fx.cheap_id);

    fx.cheap.set_status(500).await;

    // 两条请求：每条先探测在用档位（500 → 熔断计失败），请求内落到贵档位
    let marker = response_text(send_message(fx.port, "sess-e2e-failover-0001").await).await;
    assert_eq!(marker, "served-by-expensive");
    let marker = response_text(send_message(fx.port, "sess-e2e-failover-0001").await).await;
    assert_eq!(marker, "served-by-expensive");
    assert_eq!(fx.cheap.hits(), 2, "每条请求只应探测故障档位一次");
    assert_eq!(fx.expensive.hits(), 2);

    // 在用档位恢复 → 探测成功，回到在用档位（省心模式不因一次故障就弃用它）
    fx.cheap.set_status(200).await;
    let marker = response_text(send_message(fx.port, "sess-e2e-failover-0001").await).await;
    assert_eq!(marker, "served-by-cheap");
    assert_eq!(fx.cheap.hits(), 3);
    assert_eq!(fx.expensive.hits(), 2, "恢复后不应继续占用备胎档位");
    fx.server.stop().await.expect("stop server");
}

/// 4. 被动模型监控：换芯流量（Claude 线路回 OpenAI 形状）→ 异源指纹 Anomaly
///    落库 + history(source=passive) + 档位看板点亮；干净档位零报告；
///    转发本身不受观察影响（客户端仍拿到 200 与原样响应体）。
#[tokio::test]
#[serial]
async fn passive_anomaly_lands_and_surfaces() {
    let fx = E2eFixture::new().await;

    // 便宜档（无亲和 → 省心选它）回 OpenAI 形状冒充 Claude
    fx.cheap.set_foreign(true).await;
    let resp = send_message(fx.port, "sess-e2e-passive-0001").await;
    assert_eq!(resp.status(), 200, "观察不得影响转发");
    let body: Value = resp.json().await.expect("parse impersonated body");
    assert_eq!(body["object"], "chat.completion", "响应体原样到达客户端");

    // 等被动消费 worker 落库（异步）
    let reports = {
        let mut reports = Vec::new();
        for _ in 0..250 {
            reports = crate::relay::model_verification::store::list_for_provider_ids(
                &fx.db,
                std::slice::from_ref(&fx.cheap_id),
            )
            .unwrap();
            if !reports.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        reports
    };
    assert_eq!(reports.len(), 1, "换芯档位必须落一份异常报告");
    assert_eq!(
        reports[0].verdict,
        crate::relay::model_verification::types::Verdict::Anomaly
    );
    assert!(reports[0].facts.iter().any(|fact| {
        fact.code == crate::relay::model_verification::types::EvidenceCode::ForeignProtocol
            && fact.outcome == crate::relay::model_verification::types::EvidenceOutcome::Failed
    }));
    let serialized = serde_json::to_string(&reports[0]).unwrap();
    assert!(!serialized.contains("gpt-5.6"), "证据不得携带响应内容");

    // history 记 passive 来源
    let history = crate::relay::model_verification::history::list(
        &fx.db,
        &crate::relay::model_verification::types::TargetScope::new(fx.cheap_id.clone(), "claude"),
    )
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].source,
        crate::relay::model_verification::types::VerificationSource::Passive
    );

    // 档位看板点亮异常
    let state = crate::store::AppState::new(fx.db.clone());
    let board = crate::commands::auto_mode::tier_board_impl(&state, "claude")
        .await
        .unwrap();
    let tier = board
        .tiers
        .iter()
        .find(|t| t.provider_id == fx.cheap_id)
        .expect("cheap tier on board");
    assert_eq!(tier.verification_verdict.as_deref(), Some("anomaly"));

    // 对照：干净档位（贵档未被打过流量）与正常形状流量都不产报告
    assert!(
        crate::relay::model_verification::store::list_for_provider_ids(
            &fx.db,
            std::slice::from_ref(&fx.expensive_id),
        )
        .unwrap()
        .is_empty()
    );

    fx.cheap.set_foreign(false).await;
    let marker = response_text(send_message(fx.port, "sess-e2e-passive-0002").await).await;
    assert_eq!(marker, "served-by-cheap");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let reports = crate::relay::model_verification::store::list_for_provider_ids(
        &fx.db,
        std::slice::from_ref(&fx.cheap_id),
    )
    .unwrap();
    assert_eq!(reports.len(), 1, "干净流量不落库，异常报告不被干净流量稀释");
    fx.server.stop().await.expect("stop server");
}
