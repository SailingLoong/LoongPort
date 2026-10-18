//! 诊断包导出与反馈回传共用的环境事实收集 + 导出命令层。
//!
//! 「环境里有什么事实」归这里拼装（版本/OS/工具版本/代理/设置摘要/计数/站点清单），
//! 「包里长什么样」归 [`crate::diagnostics_export`]。两处消费：
//! 本地导出（[`export_diagnostics`]）与反馈回传（`commands::feedback`）。

use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;
use tauri_plugin_dialog::DialogExt;

use crate::diagnostics_export::{build_diagnostics_zip, collect_diagnostics};
use crate::error::AppError;
use crate::store::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExportResult {
    pub file_path: String,
    pub bytes: u64,
}

/// 环境事实：manifest 正文 + 站点域名清单（后者仅在用户勾选时进包）。
pub(crate) struct EnvironmentFacts {
    pub manifest: Value,
    pub site_origins: Vec<String>,
}

/// Basic metadata is always safe to send. Detailed collection is lazy and only
/// performed after the caller passes the existing diagnostics consent switch.
pub(crate) async fn gather_environment(
    db: &std::sync::Arc<crate::database::Database>,
    include_diagnostics: bool,
) -> EnvironmentFacts {
    gather_environment_internal(db, include_diagnostics, None).await
}

/// Feedback collects configuration only for explicitly selected applications.
pub(crate) async fn gather_feedback_environment(
    db: &std::sync::Arc<crate::database::Database>,
    include_diagnostics: bool,
    selected_apps: Vec<String>,
) -> EnvironmentFacts {
    gather_environment_internal(db, include_diagnostics, Some(selected_apps)).await
}

async fn gather_environment_internal(
    db: &std::sync::Arc<crate::database::Database>,
    include_diagnostics: bool,
    selected_apps: Option<Vec<String>>,
) -> EnvironmentFacts {
    use crate::diagnostics_snapshot::{environment_manifest, status, CollectionStatus};

    let db = std::sync::Arc::clone(db);
    match tauri::async_runtime::spawn_blocking(move || {
        let mut site_origins = Vec::new();
        let manifest = environment_manifest(include_diagnostics, || {
            let db_facts = (|| -> Result<Value, AppError> {
                let conn = db.conn.try_lock().map_err(|_| AppError::Message("database unavailable".into()))?;
                let schema_version = crate::database::loongport_schema::read_stored_version(&conn)?;
                let providers: i64 = conn.query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))
                    .map_err(|e| AppError::Database(e.to_string()))?;
                let relay_accounts: i64 = conn.query_row("SELECT COUNT(*) FROM loongport_relay", [], |row| row.get(0))
                    .map_err(|e| AppError::Database(e.to_string()))?;
                let mut stmt = conn.prepare("SELECT DISTINCT site_origin FROM loongport_relay ORDER BY site_origin")
                    .map_err(|e| AppError::Database(e.to_string()))?;
                site_origins = stmt.query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| AppError::Database(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| AppError::Database(e.to_string()))?;
                Ok(json!({
                    "status":"ok",
                    "data": {"schemaVersion":schema_version,"counts": {
                        "providers":providers,"relaySites":site_origins.len(),"relayAccounts":relay_accounts,
                    }},
                }))
            })().unwrap_or_else(|_| status(CollectionStatus::Unavailable));
            let settings = crate::settings::get_settings();
            let proxy = crate::commands::global_proxy::get_upstream_proxy_status();
            let mut details = json!({
                "schemaVersion":1,
                "portableMode":crate::commands::misc::portable_mode_enabled(),
                "settings": {
                    "receiveBetaUpdates":settings.receive_beta_updates,
                    "crowdMetricsEnabled":settings.crowd_metrics_enabled,
                },
                // No proxy URL, host, username, or port enters metadata.
                "proxy": {"enabled":proxy.enabled,"configured":proxy.proxy_url.is_some()},
                "db":db_facts,
                "toolVersions":crate::commands::misc::local_tool_diagnostics(),
            });
            if let Some(selected) = &selected_apps {
                let mut applications = crate::diagnostics_snapshot::selected_application_snapshots(selected);
                if serde_json::to_vec(&applications).map_or(true, |bytes| bytes.len() > 128 * 1024) {
                    applications = status(CollectionStatus::Truncated);
                }
                details["applications"] = applications;
            } else {
                // Preserve the explicitly requested standalone export behavior.
                details["codex"] = crate::diagnostics_snapshot::codex_snapshot(&crate::codex_config::get_codex_config_path());
            }
            details
        });
        EnvironmentFacts { manifest, site_origins }
    }).await {
        Ok(facts) => facts,
        Err(_) => EnvironmentFacts {
            manifest: environment_manifest(include_diagnostics, || status(CollectionStatus::Unavailable)),
            site_origins: Vec::new(),
        },
    }
}

/// 组装完整的诊断包 zip。诊断导出与反馈回传（附带诊断段时）共用。
pub(crate) async fn build_diagnostics_bundle(
    db: &std::sync::Arc<crate::database::Database>,
    include_sites: bool,
) -> Result<Vec<u8>, AppError> {
    let facts = gather_environment(db, true).await;
    let origins = include_sites.then(|| facts.site_origins.clone());
    tauri::async_runtime::spawn_blocking(move || {
        collect_diagnostics(facts.manifest, origins).and_then(build_diagnostics_zip)
    })
    .await
    .map_err(|e| AppError::Message(format!("诊断包构建任务失败: {e}")))?
}

/// 导出诊断包到用户选择的位置。`Ok(None)` = 用户在保存对话框取消。
#[tauri::command]
pub async fn export_diagnostics(
    include_sites: bool,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<DiagnosticsExportResult>, String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let default_name = format!(
        "loongport-diagnostics-v{}-{stamp}.zip",
        env!("CARGO_PKG_VERSION")
    );
    let Some(target) = app
        .dialog()
        .file()
        .add_filter("ZIP", &["zip"])
        .set_file_name(&default_name)
        .blocking_save_file()
        .map(|path| path.to_string())
    else {
        return Ok(None);
    };

    let db = state.db.clone();
    let zip_bytes = build_diagnostics_bundle(&db, include_sites)
        .await
        .map_err(|e| e.to_string())?;

    let target_path = std::path::PathBuf::from(&target);
    std::fs::write(&target_path, &zip_bytes)
        .map_err(|e| crate::error::AppError::io(&target_path, e).to_string())?;

    log::info!(
        "诊断包已导出：{}（{} 字节，含站点清单：{include_sites}）",
        target,
        zip_bytes.len()
    );
    Ok(Some(DiagnosticsExportResult {
        file_path: target,
        bytes: zip_bytes.len() as u64,
    }))
}
