//! 模型对齐告警：客户端点名的模型 ≠ 实际计费模型时的知情层。
//!
//! 站点按请求里的模型名计费。代理侧的对齐机制（codex 的档位已选模型强制、
//! grok 的 upstream 锚定、claude 的默认兜底）保证出站模型可控，但客户端
//! （Codex Desktop 的会话模型记忆、`/model` 选型等）另选的模型被改写时，
//! 用户需要**知道**并有机会改主意——这就是本模块的全部职责：
//! 记录「要了 X、按 Y 发出」的事实，新告警时推事件 + 系统通知。
//!
//! 状态语义（spec：模型对齐告警与一键切换）：
//! - 活跃键 = (app, 档位, 客户端请求模型)：同一键只有一条记录，`sent`
//!   变化（档位切换后）更新值并重新告警；
//! - **自愈**：再次观察到该键一致的请求即移除活跃记录；
//! - **dismiss 静默**：用户点「保持档位模型」后，该 (键, sent) 对本会话
//!   不再告警（重启清零——重启后若仍在分叉会再报一次，自愈语义）；
//! - 检测不依赖用量日志开关（`enable_logging` 关着也要报）。

use crate::provider::Provider;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// 一条模型不符事实。`can_switch_to_requested` 是「改用」按钮的可用性
/// （请求模型是否在该档位可用模型列表内），由后端判定——前端只展示。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMismatch {
    pub app_type: String,
    pub provider_id: String,
    pub provider_name: String,
    pub requested_model: String,
    pub sent_model: String,
    pub can_switch_to_requested: bool,
}

/// 活跃键：同一档位上客户端点名的同一模型只有一条活跃记录。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActiveKey {
    app_type: String,
    provider_id: String,
    requested_model: String,
}

#[derive(Default)]
struct AlertState {
    active: HashMap<ActiveKey, ModelMismatch>,
    /// dismiss 过的 (键, sent) 对：本会话静默。
    dismissed: HashSet<(ActiveKey, String)>,
}

/// 模型对齐告警的会话级状态。随 ProxyService/ProxyServer 组装共享。
pub struct ModelAlignmentAlerts {
    state: Mutex<AlertState>,
}

impl Default for ModelAlignmentAlerts {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelAlignmentAlerts {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(AlertState::default()),
        }
    }

    /// 观察一次「客户端请求 `requested`、实际发出 `sent`」。
    ///
    /// 一致 ⇒ 自愈移除该键的活跃记录并返回 `None`；不一致 ⇒ 更新活跃集，
    /// 仅当这是一条**新**告警（首次出现，或 sent 发生变化）且未被 dismiss
    /// 过时返回事实本身（调用方据此推事件 / 通知）。
    pub fn observe_request(
        &self,
        app_type: &str,
        provider: &Provider,
        requested: &str,
        sent: &str,
    ) -> Option<ModelMismatch> {
        let key = ActiveKey {
            app_type: app_type.to_string(),
            provider_id: provider.id.clone(),
            requested_model: requested.to_string(),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if requested == sent {
            // 一致请求 = 该档位的分叉已结束：清掉它的全部活跃记录
            // （客户端换回档位模型的请求键与分叉记录的键不同，按键清永远
            // 清不掉——自愈按档位清，而非按请求模型清）。
            state
                .active
                .retain(|key, _| key.app_type != app_type || key.provider_id != provider.id);
            return None;
        }
        if state.dismissed.contains(&(key.clone(), sent.to_string())) {
            return None;
        }
        let mismatch = ModelMismatch {
            app_type: app_type.to_string(),
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            requested_model: requested.to_string(),
            sent_model: sent.to_string(),
            can_switch_to_requested: crate::relay::model_catalog::available_models(provider)
                .iter()
                .any(|model| model == requested),
        };
        let is_new = state
            .active
            .get(&key)
            .is_none_or(|existing| existing.sent_model != mismatch.sent_model);
        state.active.insert(key, mismatch.clone());
        is_new.then_some(mismatch)
    }

    /// 当前活跃的不符事实（展示序稳定：app、档位、请求模型）。
    pub fn list(&self) -> Vec<ModelMismatch> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries: Vec<ModelMismatch> = state.active.values().cloned().collect();
        entries.sort_by(|a, b| {
            (&a.app_type, &a.provider_id, &a.requested_model).cmp(&(
                &b.app_type,
                &b.provider_id,
                &b.requested_model,
            ))
        });
        entries
    }

    /// 用户选择「保持档位模型」：移出活跃集，该对本会话静默。
    pub fn dismiss(
        &self,
        app_type: &str,
        provider_id: &str,
        requested_model: &str,
        sent_model: &str,
    ) {
        let key = ActiveKey {
            app_type: app_type.to_string(),
            provider_id: provider_id.to_string(),
            requested_model: requested_model.to_string(),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active.remove(&key);
        state.dismissed.insert((key, sent_model.to_string()));
    }
}

/// 从请求体读客户端点名的模型（无模型字段 / 空串 ⇒ `None`，不参与检测）。
/// 消费者（forwarder）是 gui-gated 模块，这里同 gate。
#[cfg(feature = "gui")]
pub(crate) fn client_requested_model(body: &serde_json::Value) -> Option<String> {
    body.get("model")
        .and_then(|model| model.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// 转发路径上的检测收口：更新活跃集，新告警时推事件 + 系统通知。
///
/// 各 app 的对齐点（codex 的 preferred_model 强制、grok 的 upstream 锚定、
/// claude 的默认兜底）在**改写发生处**调用这里——对齐点天然知道差异。
/// 事件与通知依赖 tauri 运行时，与 forwarder 一起 gui-gate（headless 构建
/// 不参与转发）。
#[cfg(feature = "gui")]
pub(crate) fn observe_alignment(
    alerts: &ModelAlignmentAlerts,
    app_handle: Option<&tauri::AppHandle>,
    app_type: &str,
    provider: &Provider,
    requested: &str,
    sent: &str,
) {
    use tauri::Emitter;
    let Some(mismatch) = alerts.observe_request(app_type, provider, requested, sent) else {
        return;
    };
    log::info!(
        "[ModelAlignment] {app_type} 档位「{}」：客户端模型 {requested} ≠ 出站 {sent}（已按档位对齐）",
        provider.name
    );
    let Some(handle) = app_handle else {
        return;
    };
    if let Err(error) = handle.emit(crate::events::MODEL_MISMATCH, &mismatch) {
        log::warn!(
            "[ModelAlignment] 发射 {} 事件失败: {error}",
            crate::events::MODEL_MISMATCH
        );
    }
    notify_os(handle, &mismatch);
}

/// 每对不符只发一次的系统通知（活跃集节流之外的第二道节流由调用侧的
/// 「新告警才走到这里」保证）。
#[cfg(feature = "gui")]
fn notify_os(handle: &tauri::AppHandle, mismatch: &ModelMismatch) {
    use tauri_plugin_notification::NotificationExt;
    // 语言判定唯源托盘那份（settings.language 优先、系统区域回退），
    // 别在这里长出第二套判定顺序。
    let language = crate::tray::tray_language();
    let (title, body) = notification_text(&language, mismatch);
    if let Err(error) = handle
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        log::debug!("[ModelAlignment] 系统通知发送失败: {error}");
    }
}

/// 通知文案（四语言，与托盘同款语言码）。只说事实与去向，细节留给应用内横幅。
#[cfg(feature = "gui")]
fn notification_text(language: &str, mismatch: &ModelMismatch) -> (&'static str, String) {
    let app = mismatch.app_type.as_str();
    match language {
        "en" => (
            "Model differs from the selected tier",
            format!(
                "{app} requested {req}; requests were forwarded using {sent}.",
                req = mismatch.requested_model,
                sent = mismatch.sent_model
            ),
        ),
        "zh-TW" => (
            "模型與檔位選擇不一致",
            format!(
                "{app} 要求使用 {req}；已按檔位選擇的 {sent} 轉發。",
                req = mismatch.requested_model,
                sent = mismatch.sent_model
            ),
        ),
        "ja" => (
            "モデルが階層の選択と一致しません",
            format!(
                "{app} は {req} を要求しました。階層で選択した {sent} で転送しています。",
                req = mismatch.requested_model,
                sent = mismatch.sent_model
            ),
        ),
        _ => (
            "模型与档位选择不一致",
            format!(
                "{app} 请求使用 {req}；已按档位选择的 {sent} 转发。",
                req = mismatch.requested_model,
                sent = mismatch.sent_model
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tier(id: &str) -> Provider {
        Provider::with_id(
            id.into(),
            format!("Tier {id}"),
            json!({
                "config": "model = \"gpt-5.6-sol\"\n",
                "modelCatalog": { "models": [ { "model": "gpt-5.6-sol" } ] }
            }),
            None,
        )
    }

    #[test]
    fn new_divergence_alerts_once_until_sent_changes() {
        let alerts = ModelAlignmentAlerts::new();
        let provider = tier("relay");
        let first = alerts
            .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-sol")
            .expect("首次分叉要产生告警");
        assert_eq!(first.requested_model, "gpt-6-astra");
        assert_eq!(first.sent_model, "gpt-5.6-sol");
        assert!(
            alerts
                .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-sol")
                .is_none(),
            "同一对重复出现不重复告警"
        );
        // 档位切换后 sent 变化 = 新事实，重新告警。
        assert!(alerts
            .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-luna")
            .is_some());
    }

    #[test]
    fn aligned_request_heals_the_active_entry() {
        let alerts = ModelAlignmentAlerts::new();
        let provider = tier("relay");
        alerts
            .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-sol")
            .unwrap();
        assert_eq!(alerts.list().len(), 1);
        assert!(alerts
            .observe_request("codex", &provider, "gpt-5.6-sol", "gpt-5.6-sol")
            .is_none());
        assert!(alerts.list().is_empty(), "一致请求自愈清除活跃记录");
    }

    #[test]
    fn dismiss_silences_the_pair_for_the_session() {
        let alerts = ModelAlignmentAlerts::new();
        let provider = tier("relay");
        alerts
            .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-sol")
            .unwrap();
        alerts.dismiss("codex", "relay", "gpt-6-astra", "gpt-5.6-sol");
        assert!(alerts.list().is_empty());
        assert!(
            alerts
                .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-sol")
                .is_none(),
            "dismiss 过的对本会话静默"
        );
        // sent 变化仍是新事实：告警回来。
        assert!(alerts
            .observe_request("codex", &provider, "gpt-6-astra", "gpt-5.6-luna")
            .is_some());
    }

    #[test]
    fn can_switch_reflects_tier_inventory_membership() {
        let alerts = ModelAlignmentAlerts::new();
        let provider = tier("relay"); // 目录里只有 gpt-5.6-sol
        let mismatch = alerts
            .observe_request("codex", &provider, "gpt-5.6-sol", "other")
            .unwrap();
        assert!(mismatch.can_switch_to_requested);
        let mismatch = alerts
            .observe_request("codex", &provider, "claude-opus-5", "gpt-5.6-sol")
            .unwrap();
        assert!(!mismatch.can_switch_to_requested);
    }

    #[test]
    fn list_is_stable_and_scoped_per_app_and_provider() {
        let alerts = ModelAlignmentAlerts::new();
        let a = tier("a");
        let b = tier("b");
        alerts
            .observe_request("codex", &b, "x-model", "y-model")
            .unwrap();
        alerts
            .observe_request("codex", &a, "x-model", "z-model")
            .unwrap();
        alerts
            .observe_request("claude", &a, "x-model", "z-model")
            .unwrap();
        let listed = alerts.list();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].provider_id, "a");
        assert_eq!(listed[0].app_type, "claude", "app 名参与排序");
    }

    #[test]
    fn client_requested_model_ignores_empty_and_missing() {
        assert_eq!(
            client_requested_model(&json!({ "model": " gpt-5.6-sol " })),
            Some("gpt-5.6-sol".to_string())
        );
        assert_eq!(client_requested_model(&json!({ "model": "  " })), None);
        assert_eq!(client_requested_model(&json!({})), None);
    }
}
