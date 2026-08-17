//! 真实上游 × 本地代理的复合 smoke（默认 `#[ignore]`，显式运行才发真实请求）。
//!
//! 与 `auto_mode_e2e_tests`（mock 上游）互补：那条钉链路语义，这条对着
//! **真实中转站/官网端点**走一遍转发器，验证真实 wire 格式、用量落库与
//! 真实错误形状不会在转发层走样。
//!
//! 运行方式（测试自己把库复制进隔离 home，绝不写 `~/.loongport` 原件）：
//!
//! ```text
//! LOONGPORT_SMOKE_DB=~/.loongport/loongport.db \
//! LOONGPORT_SMOKE_APP=claude \
//! cargo test --lib -- proxy::real_upstream_smoke_tests -- --ignored --nocapture
//! ```
//!
//! 断言刻意宽松：真实站点可能当天故障（2026-08-17 BestApi 全线 502/503），
//! smoke 的目的是**观察与落库验证**，不是判活。判据只有三条：代理不崩、
//! 每条请求拿到确定的 HTTP 状态、`proxy_request_logs` 里有对应的落库行。

use super::server::ProxyServer;
use super::types::ProxyConfig;
use crate::database::Database;
use serial_test::serial;
use std::sync::Arc;
use tempfile::TempDir;

struct TempHome {
    #[allow(dead_code)]
    dir: TempDir,
    original_home: Option<String>,
    original_userprofile: Option<String>,
    original_test_home: Option<String>,
}

impl TempHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("create temp home");
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

/// 把 `LOONGPORT_SMOKE_DB` 指向的库复制进隔离 home 并初始化。
/// 返回 (home, db)：home 活到测试结束，中途 drop 会把 HOME 指回空。
fn smoke_db() -> (TempHome, Arc<Database>) {
    let src = std::env::var("LOONGPORT_SMOKE_DB")
        .expect("需要 LOONGPORT_SMOKE_DB 指向一份真实库（会复制，不动原件）");
    let home = TempHome::new();
    let dir = home.dir.path().join(crate::config::APP_DIR_NAME);
    std::fs::create_dir_all(&dir).expect("create app dir");
    std::fs::copy(&src, dir.join("loongport.db")).expect("copy smoke db");

    let db = Arc::new(Database::init().expect("init smoke db"));
    (home, db)
}

/// 档位的第一个模型名（modelCatalog 优先，codex 形状回落 config TOML 的 model=）。
fn first_model(settings: &serde_json::Value) -> Option<String> {
    if let Some(model) = settings
        .pointer("/modelCatalog/models/0/model")
        .and_then(|v| v.as_str())
    {
        return Some(model.to_string());
    }
    let config = settings.get("config").and_then(|v| v.as_str())?;
    for line in config.lines() {
        if let Some(rest) = line.trim().strip_prefix("model") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// 真实上游复合 smoke：起本地代理（headless），把请求按省心模式排序打到真实档位。
#[tokio::test]
#[serial]
#[ignore = "发真实请求花真钱；用 LOONGPORT_SMOKE_DB 显式开启"]
async fn real_upstream_roundtrip_through_local_proxy() {
    // 测试二进制不跑 Tauri 的 .setup（那里装了进程级 rustls CryptoProvider），
    // 出站 HTTPS 第一步就 panic —— 装同一个 ring provider；重复安装返回 Err，忽略。
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (_home, db) = smoke_db();
    let app = std::env::var("LOONGPORT_SMOKE_APP").unwrap_or_else(|_| "claude".to_string());

    let tiers: Vec<_> = db
        .get_all_providers(&app)
        .expect("list providers")
        .values()
        .filter(|p| crate::relay::is_managed(&p.id))
        .cloned()
        .collect();
    assert!(!tiers.is_empty(), "{app} 没有托管档位，smoke 无从下手");

    let mut config = db.get_proxy_config_for_app(&app).await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    config.max_retries = 2;
    config.circuit_failure_threshold = 2;
    db.update_proxy_config_for_app(config).await.unwrap();
    crate::proxy::auto_strategy::set_enabled(&db, &app, true).unwrap();

    let server = ProxyServer::new(
        ProxyConfig {
            listen_port: 0,
            ..Default::default()
        },
        db.clone(),
        None,
    );
    let info = server.start().await.expect("start proxy");

    // 每档一条最短请求（按省心模式排序取前三，钱花在刀刃上）
    let router = super::provider_router::ProviderRouter::new(db.clone());
    let ranked = router.select_providers(&app).await.expect("rank tiers");
    let mut statuses = Vec::new();
    for tier in ranked.iter().take(3) {
        let model = first_model(&tier.settings_config).unwrap_or_else(|| {
            panic!(
                "档位 {} 取不出模型名，先补 modelCatalog 再 smoke",
                tier.name
            )
        });
        let body = if app == "claude" || app == "claude-desktop" {
            serde_json::json!({
                "model": model, "max_tokens": 16, "stream": false,
                "messages": [{ "role": "user", "content": "hi" }],
            })
        } else {
            serde_json::json!({ "model": model, "input": "hi", "max_output_tokens": 16 })
        };
        let path = if app == "claude" || app == "claude-desktop" {
            "/v1/messages"
        } else {
            "/v1/responses"
        };
        let resp = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://127.0.0.1:{}{path}", info.port))
            .header("x-claude-code-session-id", "sess-real-smoke-0001")
            .json(&body)
            .send()
            .await
            .expect("proxy 请求必须拿到 HTTP 响应（代理崩了才算失败）");
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        println!(
            "[smoke] {} → {} | {} | {}",
            tier.name,
            status.as_u16(),
            text.chars().take(120).collect::<String>(),
            tier.id
        );
        statuses.push((tier.name.clone(), status.as_u16()));
    }

    // 落库验证：真实流量必须在 proxy_request_logs 留痕（省心模式的亲和/TTFT/成本都吃这张表）。
    // 按 provider×状态分组打印，把「迭代目标档」与「实际服务档」的归因分开
    //（迭代标签只说明第几条请求，谁真正服务以这里的 provider_id 为准）。
    println!("[smoke] 每条请求的客户端状态：{statuses:?}");
    {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT provider_id, status_code, COUNT(*), MIN(latency_ms), MAX(first_token_ms)
                 FROM proxy_request_logs WHERE session_id LIKE 'sess-real-smoke%'
                 GROUP BY provider_id, status_code ORDER BY MIN(created_at)",
            )
            .expect("prepare 归因查询");
        let rows: Vec<(String, i64, i64, i64, Option<i64>)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .expect("归因查询")
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        for (provider_id, code, n, latency, ttft) in &rows {
            println!(
                "[smoke] 落库 {provider_id} → {code} ×{n}（latency {latency}ms, ttft {ttft:?}）"
            );
        }
        assert!(!rows.is_empty(), "真实流量没有落 proxy_request_logs");
    }
    server.stop().await.expect("stop server");
}

/// 看板实拍：真实库副本上的聚合事实（顺序/倍率/单价/耗时/余额）。
/// 不发模型请求（余额是免费 GET），观察首页省心视图将看到什么。
#[tokio::test]
#[serial]
#[ignore = "需要 LOONGPORT_SMOKE_DB；余额查询打真实站点（GET /v1/usage，免费）"]
async fn real_tier_board_snapshot() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (_home, db) = smoke_db();
    let state = crate::store::AppState::new(db);
    let board = crate::tier_board_impl(&state, "claude")
        .await
        .expect("tier board");
    println!(
        "[smoke] mode={} strategy={} model={:?} current={:?}",
        board.mode, board.strategy, board.model, board.current_provider_id
    );
    for tier in board.tiers.iter().take(8) {
        println!(
            "[smoke] #{} {} ×{:?} ${:?}/M ttft={:?}ms balance=${:?} current={}",
            tier.position,
            tier.name,
            tier.rate_multiplier,
            tier.unit_price_per_million,
            tier.avg_first_token_ms,
            tier.balance_usd,
            tier.is_current
        );
    }
    assert!(!board.tiers.is_empty(), "真实库里应有托管档位");
}

/// 手动序优先实拍：把看板第一名设为手动序第一，发一条真实请求，
/// 断言它被**首先尝试**（落库出现该档的行——失败也会留痕，语义与站点健康无关）。
#[tokio::test]
#[serial]
#[ignore = "需要 LOONGPORT_SMOKE_DB；发一条真实请求（小额花费）"]
async fn real_manual_order_routes_to_user_first_pick() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (_home, db) = smoke_db();
    let state = crate::store::AppState::new(db);

    // 手动序 = [看板第 2 位, 看板第 1 位, 其余反转]：第 2 位提到第 1 之前，
    // 与自动策略序不同（否则证明力为零）。断言探针选「必有 HTTP 交换」的档位
    // ——网络层失败（连接拒绝/超时）不写 proxy_request_logs，只有 HTTP 交换
    // （2xx/4xx/5xx）留痕；newapi/sub2api 站点总会回 HTTP。
    let board = crate::tier_board_impl(&state, "claude")
        .await
        .expect("tier board");
    assert!(board.tiers.len() >= 2, "至少两档才能构造证明性手动序");
    let mut manual: Vec<String> = vec![
        board.tiers[1].provider_id.clone(),
        board.tiers[0].provider_id.clone(),
    ];
    let mut rest: Vec<String> = board
        .tiers
        .iter()
        .skip(2)
        .map(|t| t.provider_id.clone())
        .collect();
    rest.reverse();
    manual.extend(rest);
    let first = manual[0].clone();
    let model = board.tiers[1]
        .effective_model
        .clone()
        .expect("手动序第一档要有有效模型");
    crate::proxy::auto_strategy::set_mode(
        &state.db,
        "claude",
        crate::proxy::auto_strategy::EasyModeMode::Manual,
    )
    .unwrap();
    crate::proxy::auto_strategy::set_manual_order(&state.db, "claude", &manual).unwrap();

    let mut config = state.db.get_proxy_config_for_app("claude").await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    config.max_retries = 0; // 只尝试手动序第一档：落库行是谁，选路序就是谁
    state.db.update_proxy_config_for_app(config).await.unwrap();
    crate::proxy::auto_strategy::set_enabled(&state.db, "claude", true).unwrap();

    let server = ProxyServer::new(
        ProxyConfig {
            listen_port: 0,
            ..Default::default()
        },
        state.db.clone(),
        None,
    );
    let info = server.start().await.expect("start proxy");
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", info.port))
        .header("x-claude-code-session-id", "sess-real-manual-0001")
        .json(&serde_json::json!({
            "model": model, "max_tokens": 16, "stream": false,
            "messages": [{ "role": "user", "content": "hi" }],
        }))
        .send()
        .await
        .expect("拿到 HTTP 响应");
    println!("[smoke] 手动序第一档 {} → {}", first, resp.status());

    // 落库 = 每条客户端请求一行（最终尝试的档位）；max_retries=0 ⇒ 唯一一次
    // 尝试就是手动序第一档 —— 行的 provider_id 直接证明选路吃了手动序
    //（若选路仍按自动序，会先打看板第一名，落库的就是另一个 id）。
    {
        let conn = state.db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT provider_id, status_code FROM proxy_request_logs
                 WHERE session_id LIKE 'sess-real-manual%'",
            )
            .expect("prepare");
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        for (pid, code) in &rows {
            println!("[smoke] 手动序请求落库 {pid} → {code}");
        }
        assert_eq!(rows.len(), 1, "max_retries=0 应恰有一行");
        assert_eq!(
            rows[0].0, first,
            "唯一尝试的档位必须是手动序第一档（自动序会先打看板第一名）"
        );
    }
    server.stop().await.expect("stop server");
}
