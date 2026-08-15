//! 中转站扣费对账：`relay_reconciliation` 命令（纯读）。
//!
//! 逻辑主体在 [`crate::relay::reconcile`]（`Database::reconciliation_report`）——
//! 这里只是薄壳：读 relay 行 → 按 [`belongs_to_account`] 归属出该站名下全部
//! 托管档位 → 交给窗口计算。归属判据全仓只有那一份（plan §二：唯一数据源）。

use tauri::State;

use crate::app_config::AppType;
use crate::commands::relay::belongs_to_account;
use crate::error::AppError;
use crate::relay::creds;
use crate::relay::reconcile::ReconciliationReport;
use crate::services::ProviderService;
use crate::AppState;

/// 该中转站名下全部托管档位的 `(provider_id, app_type)`。
///
/// 与 `relay_balance_inputs` 同款纪律：**跨全部 app 扫** —— 档位可能只挂在某一个
/// CLI 下，漏扫一个 app 就少算一块成本，对账比值会失真。
fn relay_provider_keys(state: &AppState, relay: &creds::Relay) -> Vec<(String, String)> {
    let mut keys = Vec::new();
    for app_type in AppType::all() {
        let Ok(providers) = ProviderService::list(state, app_type.clone()) else {
            continue;
        };
        for provider in providers.values() {
            if belongs_to_account(provider, &relay.site_origin, relay.account_id) {
                keys.push((provider.id.clone(), app_type.as_str().to_string()));
            }
        }
    }
    keys
}

/// 某一行中转站的扣费对账报告。
///
/// 显式指定查不到就报错，绝不回落到其它站点（与 [`crate::commands::relay_balance`]
/// 同一套纪律）。名下没有托管档位也照常返回报告 —— 那是「估算全为 0」的合法状态，
/// 窗口会标 `InsufficientData`，让前端展示而不是报错。
#[tauri::command]
pub async fn relay_reconciliation(
    state: State<'_, AppState>,
    relay_id: i64,
) -> Result<ReconciliationReport, String> {
    reconcile(state.inner(), relay_id).map_err(|e| e.to_string())
}

fn reconcile(state: &AppState, relay_id: i64) -> Result<ReconciliationReport, AppError> {
    let relay = {
        let conn = state
            .db
            .conn
            .lock()
            .map_err(|e| AppError::Database(format!("获取数据库连接失败: {e}")))?;
        creds::get(&conn, relay_id)?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {relay_id} 的中转站")))?
    };
    let provider_keys = relay_provider_keys(state, &relay);
    state.db.reconciliation_report(relay_id, &provider_keys)
}

// 测试留白说明：命令层只有「读行 + 归属 + 委托」三步薄壳，归属判据
// （belongs_to_account）在 commands/relay.rs 有自己的测试，窗口数学在
// relay/reconcile.rs 有自己的测试；AppState（要起全套服务）在这里造不出来，
// 纯透传逻辑不为测而测。
