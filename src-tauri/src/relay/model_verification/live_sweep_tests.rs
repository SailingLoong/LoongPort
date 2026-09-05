//! 实弹扫尾（dev 工具）：读本机真实数据库里的全部 Claude/Codex 托管档位，
//! 用**现行探针代码**逐档验证并打印判定。判定规则改动后的全量自测入口。
//!
//! 默认忽略且必须显式设 env 才会真正打真实站点（防误触、防 CI）：
//!
//! ```text
//! LOONGPORT_LIVE_VERIFY_SWEEP=1 \
//!   cargo test --lib live_sweep -- --ignored --nocapture
//! ```
//!
//! 只读不写：探针本身不落库，本测试也不调 store。打印只含本地 provider
//! id / app / 模型名与判定，不打印站点 URL、密钥或响应原文。
#![cfg(test)]

use std::str::FromStr;

use crate::{
    app_config::AppType,
    database::Database,
    relay::model_verification::{
        capability_profiles::CapabilityProfile, protocols, target::ResolvedTarget,
        types::TargetKey, verdict,
    },
};

#[tokio::test]
#[ignore = "实弹：真实站点与真实 token，仅本地自测手动运行"]
async fn live_sweep_verifies_every_managed_tier_with_current_probes() {
    if std::env::var("LOONGPORT_LIVE_VERIFY_SWEEP").is_err() {
        return;
    }
    let db = Database::init().expect("open real local database");
    let rows: Vec<(String, String, String)> = {
        let conn = db.conn.lock().unwrap();
        let mut statement = conn
            .prepare(
                "SELECT id, app_type, settings_config FROM providers
                 WHERE category = 'aggregator' AND app_type IN ('claude', 'codex')
                 ORDER BY app_type, id",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    let client = reqwest::Client::new();
    // 展开成 (provider_id, app_type, model) 工作项，6 路并发跑（不同档位
    // 之间互不影响；同一站的少量并发由 429 重试兜底）。
    let mut work: Vec<(String, String, String)> = Vec::new();
    for (provider_id, app_type, settings_json) in rows {
        for model in models_for(&app_type, &settings_json) {
            work.push((provider_id.clone(), app_type.clone(), model));
        }
    }
    let total = work.len();

    use futures::stream::{FuturesUnordered, StreamExt};
    let mut in_flight = FuturesUnordered::new();
    let mut problem_rows = 0usize;
    for (provider_id, app_type, model) in work {
        while in_flight.len() >= 6 {
            problem_rows += report(in_flight.next().await);
        }
        let app = AppType::from_str(&app_type).expect("filtered app types");
        let db = &db;
        let client = &client;
        in_flight.push(async move {
            let Ok(resolved) =
                ResolvedTarget::resolve(db, TargetKey::new(&provider_id, &app_type, &model))
            else {
                println!("SKIP  {provider_id} {app_type} {model} (resolve failed)");
                return 0;
            };
            let profile = CapabilityProfile::for_target(&app, &model);
            let result = match app {
                AppType::Codex => {
                    protocols::openai_responses::run_balanced(client, &resolved, &profile).await
                }
                _ => protocols::anthropic::run_balanced(client, &resolved, &profile).await,
            };
            match result {
                Ok((facts, diagnostics)) => {
                    let facts = verdict::dedupe_facts(facts);
                    let (verdict, _) = verdict::evaluate(app, &profile, &facts);
                    let failed: Vec<String> = facts
                        .iter()
                        .filter(|fact| {
                            matches!(
                                fact.outcome,
                                crate::relay::model_verification::types::EvidenceOutcome::Failed
                            )
                        })
                        .map(|fact| format!("{:?}", fact.code))
                        .collect();
                    if failed.is_empty() {
                        println!(
                            "PASS  {provider_id} {app_type} {model} → {}",
                            verdict.as_str()
                        );
                        0
                    } else {
                        println!(
                            "FAIL  {provider_id} {app_type} {model} → {} failed={failed:?}",
                            verdict.as_str()
                        );
                        // 内置失败诊断：打印各失败腿的响应头 200 字符（定位形状用）。
                        for diagnostic in &diagnostics {
                            let head: String = diagnostic.response.chars().take(200).collect();
                            let code = format!("{:?}", diagnostic.code);
                            println!(
                                "DIAG  {provider_id} {model} [{}] {code}: {head}",
                                diagnostic.probe
                            );
                        }
                        1
                    }
                }
                Err(error) => {
                    println!("ERR   {provider_id} {app_type} {model} → {error:?}");
                    1
                }
            }
        });
    }
    while let Some(result) = in_flight.next().await {
        problem_rows += report(Some(result)).saturating_sub(0);
    }
    println!("==== sweep done: {total} targets, {problem_rows} problem rows ====");
}

fn report(result: Option<usize>) -> usize {
    result.unwrap_or(0)
}

/// 每档要验的模型集：claude 取档位当前模型；codex 取 config 的 model 行 +
/// modelCatalog 全量（弹窗里挑模型的实际来源）。
fn models_for(app_type: &str, settings_json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(settings_json) else {
        return Vec::new();
    };
    let mut models = Vec::new();
    if app_type == "claude" {
        if let Some(model) = value
            .pointer("/env/ANTHROPIC_MODEL")
            .and_then(|m| m.as_str())
        {
            models.push(model.to_string());
        }
        return models;
    }
    let toml = value.get("config").and_then(|c| c.as_str()).unwrap_or("");
    for line in toml.lines() {
        if let Some(model) = line.strip_prefix("model = ") {
            models.push(model.trim().trim_matches('"').to_string());
        }
    }
    if let Some(catalog) = value
        .pointer("/modelCatalog/models")
        .and_then(|m| m.as_array())
    {
        for entry in catalog {
            if let Some(name) = entry.get("model").and_then(|m| m.as_str()) {
                if !models.iter().any(|existing| existing == name) {
                    models.push(name.to_string());
                }
            }
        }
    }
    models
}

/// 失败诊断：非流式工具探针的响应骨架（item 类型/status/model，无内容）。
async fn inspect_tool_response(
    client: &reqwest::Client,
    resolved: &ResolvedTarget,
    app: &AppType,
) -> Option<String> {
    use crate::relay::model_verification::protocols::send_and_read;
    let model = resolved.target().model.clone();
    let (url, body) = match app {
        AppType::Codex => (
            format!(
                "{}/responses",
                resolved.protocol_base().trim_end_matches('/')
            ),
            serde_json::json!({
                "model": model, "input": "Call report_probe with ready set to true.",
                "max_output_tokens": 1024, "store": false,
                "tools": [{"type": "function", "name": "report_probe",
                    "description": "Return the fixed verification object.",
                    "parameters": {"type": "object",
                        "properties": {"ready": {"type": "boolean"}},
                        "required": ["ready"], "additionalProperties": false},
                    "strict": true}],
                "tool_choice": {"type": "function", "name": "report_probe"},
            }),
        ),
        _ => (
            format!(
                "{}/v1/messages",
                resolved.protocol_base().trim_end_matches('/')
            ),
            serde_json::json!({
                "model": model, "max_tokens": 64,
                "messages": [{"role": "user", "content":
                    "Call report_probe with an object containing ready: true."}],
                "tools": [{
                    "name": "report_probe", "description": "Return a fixed verification object.",
                    "input_schema": {"type": "object",
                        "properties": {"ready": {"type": "boolean"}},
                        "required": ["ready"]},
                }],
                "tool_choice": {"type": "tool", "name": "report_probe"},
            }),
        ),
    };
    let mut request = client.post(&url).json(&body);
    if matches!(app, AppType::Codex) {
        request = request.bearer_auth(resolved.api_key());
    } else {
        request = request
            .header("x-api-key", resolved.api_key())
            .header("anthropic-version", "2023-06-01");
    }
    let bytes = send_and_read(request).await.ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let items: Vec<String> = value
        .get("output")
        .or_else(|| value.get("content"))
        .and_then(|o| o.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.get("type").and_then(|t| t.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Some(format!(
        "status={:?} model={:?} stop={:?} items={items:?} keys={:?}",
        value
            .get("status")
            .and_then(|s| s.as_str())
            .or_else(|| value.get("type").and_then(|t| t.as_str())),
        value.get("model").and_then(|m| m.as_str()),
        value.get("stop_reason").and_then(|r| r.as_str()),
        value
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
    ))
}

/// 失败诊断：非流式 core 响应骨架（对象键 + 前 96 字节形状，无完整内容）。
async fn inspect_core_response(
    client: &reqwest::Client,
    resolved: &ResolvedTarget,
    app: &AppType,
) -> Option<String> {
    use crate::relay::model_verification::protocols::send_and_read;
    let model = resolved.target().model.clone();
    let (url, mut body) = match app {
        AppType::Codex => (
            format!(
                "{}/responses",
                resolved.protocol_base().trim_end_matches('/')
            ),
            serde_json::json!({
                "model": model, "input": "Reply with ready.",
                "max_output_tokens": 512, "store": false,
            }),
        ),
        _ => (
            format!(
                "{}/v1/messages",
                resolved.protocol_base().trim_end_matches('/')
            ),
            serde_json::json!({
                "model": model, "max_tokens": 32,
                "messages": [{"role": "user", "content": "Reply with the word ready."}],
            }),
        ),
    };
    let _ = &mut body;
    let mut request = client.post(&url).json(&body);
    if matches!(app, AppType::Codex) {
        request = request.bearer_auth(resolved.api_key());
    } else {
        request = request
            .header("x-api-key", resolved.api_key())
            .header("anthropic-version", "2023-06-01");
    }
    let bytes = send_and_read(request).await.ok()?;
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(96)]).to_string();
    // SSE 体：额外解析 terminal response 的骨架（status/output 类型/键）。
    let sse_detail = if head.trim_start().starts_with(':') || head.contains("event:") {
        let text = String::from_utf8_lossy(&bytes);
        text.split("\n\n")
            .filter_map(|chunk| {
                let data = chunk
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                serde_json::from_str::<serde_json::Value>(&data).ok()
            })
            .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("response.completed"))
            .and_then(|v| v.get("response").cloned())
            .map(|resp| {
                let items: Vec<String> = resp
                    .get("output")
                    .and_then(|o| o.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|i| i.get("type").and_then(|t| t.as_str()))
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                format!(
                    " | terminal status={:?} model={:?} output={items:?} keys={:?}",
                    resp.get("status").and_then(|s| s.as_str()),
                    resp.get("model").and_then(|m| m.as_str()),
                    resp.as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>())
                )
            })
            .unwrap_or_else(|| " | 无 response.completed".into())
    } else {
        String::new()
    };
    Some(format!("len={} head={head:?}{sse_detail}", bytes.len()))
}
