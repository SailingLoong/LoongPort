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

/// 拼装环境快照。全部字段 best-effort：单项失败降级为缺省值并记 log，
/// 不让「数不清 provider」这类小故障拖垮整个导出。
pub(crate) async fn gather_environment(
    db: &std::sync::Arc<crate::database::Database>,
) -> EnvironmentFacts {
    // 工具版本探测是子进程批次（秒级），只在做包时按需跑。
    let tool_versions = crate::commands::misc::get_tool_versions(None, None)
        .await
        .unwrap_or_default();
    let proxy = crate::commands::global_proxy::get_upstream_proxy_status();
    let settings = crate::settings::get_settings();

    let db = std::sync::Arc::clone(db);
    let (db_json, site_origins) = match tauri::async_runtime::spawn_blocking(
        move || -> Result<(Value, Vec<String>), AppError> {
            let conn = db
                .conn
                .lock()
                .map_err(|e| AppError::Message(format!("获取数据库连接失败: {e}")))?;
            let schema_version = crate::database::loongport_schema::read_stored_version(&conn)?;
            let providers: i64 = conn
                .query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))
                .map_err(|e| AppError::Database(e.to_string()))?;
            let relay_accounts: i64 = conn
                .query_row("SELECT COUNT(*) FROM loongport_relay", [], |row| row.get(0))
                .map_err(|e| AppError::Database(e.to_string()))?;
            let mut stmt = conn
                .prepare("SELECT DISTINCT site_origin FROM loongport_relay ORDER BY site_origin")
                .map_err(|e| AppError::Database(e.to_string()))?;
            let site_origins = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| AppError::Database(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AppError::Database(e.to_string()))?;
            let site_count = site_origins.len() as i64;
            Ok((
                json!({
                    "schemaVersion": schema_version,
                    "counts": {
                        "providers": providers,
                        "relaySites": site_count,
                        "relayAccounts": relay_accounts,
                    },
                }),
                site_origins,
            ))
        },
    )
    .await
    {
        Ok(Ok(facts)) => facts,
        Ok(Err(e)) => {
            // best-effort：计数/站点清单读不出来不该拖垮整个导出。
            log::warn!("诊断包环境计数读取失败（降级为缺省）: {e}");
            (json!({}), Vec::new())
        }
        Err(e) => {
            log::warn!("诊断包环境计数任务失败（降级为缺省）: {e}");
            (json!({}), Vec::new())
        }
    };

    let manifest = json!({
        "appVersion": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "portableMode": crate::commands::misc::portable_mode_enabled(),
        "settings": {
            "language": settings.language,
            "receiveBetaUpdates": settings.receive_beta_updates,
            "crowdMetricsEnabled": settings.crowd_metrics_enabled,
        },
        "proxy": {
            "enabled": proxy.enabled,
            // 代理 URL 可能带用户名/密码 —— 掩码后仅留 scheme://host:port。
            "url": proxy.proxy_url.as_deref().map(crate::proxy::http_client::mask_url),
        },
        "toolVersions": tool_versions,
        "db": db_json,
    });

    EnvironmentFacts {
        manifest,
        site_origins,
    }
}

/// 组装完整的诊断包 zip。诊断导出与反馈回传（附带诊断段时）共用。
pub(crate) async fn build_diagnostics_bundle(
    db: &std::sync::Arc<crate::database::Database>,
    include_sites: bool,
) -> Result<Vec<u8>, AppError> {
    let facts = gather_environment(db).await;
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
