//! 中转站扣费对账：`relay_reconciliation` 命令（纯读）。
//!
//! 逻辑主体在 [`crate::relay::reconcile`]（`Database::reconciliation_report`）——
//! 这里只是薄壳：读 relay 行 → 按 [`belongs_to_relay`]（严格归属：未登录的行不认
//! 别人账号的档位）归属出该站名下全部托管档位 → 交给窗口计算。
//! 归属口径与 `relay_balance_inputs` 同一份（plan §二：唯一数据源）。

use tauri::State;

use crate::app_config::AppType;
use crate::commands::relay::belongs_to_relay;
use crate::error::AppError;
use crate::relay::creds;
use crate::relay::reconcile::ReconciliationReport;
use crate::services::ProviderService;
use crate::AppState;

/// 该中转站名下全部托管档位的 `(provider_id, app_type)`。
///
/// 与 `relay_balance_inputs` 同款纪律：**跨全部 app 扫** —— 档位可能只挂在某一个
/// CLI 下，漏扫一个 app 就少算一块成本，对账比值会失真。归属判据用
/// [`belongs_to_relay`]（严格版）：成本是要记到这一行头上的事实，未登录的行
/// 不认别人账号的档位，否则 B 的消费会被算进 A 的对账。
fn relay_provider_keys(state: &AppState, relay: &creds::Relay) -> Vec<(String, String)> {
    let mut keys = Vec::new();
    for app_type in AppType::all() {
        let Ok(providers) = ProviderService::list(state, app_type.clone()) else {
            continue;
        };
        for provider in providers.values() {
            if belongs_to_relay(provider, &relay.site_origin, relay.account_id) {
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

// 测试留白说明：归属判据（belongs_to_relay）与余额路径共用一份，它在
// commands/relay.rs 有分支矩阵测试；窗口数学在 relay/reconcile.rs 有自己的测试。
// 下面留的只有一条端到端回归：未登录行不许把别人账号的成本算进对账。

#[cfg(test)]
mod tests {
    use super::*;

    fn relay_row(site: &str, account_id: Option<i64>) -> creds::Relay {
        creds::Relay {
            id: 1,
            site_origin: site.to_string(),
            site_name: "test".to_string(),
            backend_kind: crate::relay::backend::BackendKind::Sub2Api,
            api_base_url: format!("{site}/v1"),
            account_id,
            account_label: String::new(),
            login_identifier: String::new(),
            auth_token: String::new(),
            refresh_token: None,
            token_expires_at: None,
            user_agent: None,
            cf_clearance: None,
            pricing_synced_at: None,
            sort_index: 0,
        }
    }

    /// ⭐ **未登录的 relay 行，不能把同站别人账号的档位成本算进对账。**
    ///
    /// Task 3 review 抓出的：`relay_provider_keys` 若走宽松判据
    /// （`belongs_to_account`，对 `account_id: None` 一律「算是」），B 账号的消费
    /// 会被记到 A（未登录行）的对账里。会红的改法：换回 `belongs_to_account`。
    #[test]
    fn an_unlogged_relay_row_gets_no_costs_from_another_accounts_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的托管档位（id 由 provision 的生成端产出，满足 is_managed 形状）。
        let b_tier = crate::relay::provision::provider_id_for(site, Some(9), 1);
        let provider: crate::provider::Provider = serde_json::from_value(serde_json::json!({
            "id": b_tier,
            "name": "B 的档位",
            "settingsConfig": { "auth": { "OPENAI_API_KEY": "sk-b" } },
            "websiteUrl": site,
            "meta": { "loongportAccountId": 9 }
        }))
        .expect("反序列化 provider");
        db.save_provider("codex", &provider).expect("seed B");

        let state = AppState::new(db.clone());

        let unlogged = relay_row(site, None);
        assert!(
            relay_provider_keys(&state, &unlogged).is_empty(),
            "⭐ 未登录的行不该把别人账号的档位算进自己的成本归属"
        );

        let owner = relay_row(site, Some(9));
        assert_eq!(
            relay_provider_keys(&state, &owner),
            vec![(b_tier.clone(), "codex".to_string())],
            "档位自己的账号必须能归属到它（对照组，证明上面不是本来就扫不到）"
        );
    }
}
