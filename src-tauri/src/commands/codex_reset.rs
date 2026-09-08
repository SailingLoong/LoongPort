//! Codex 全局重置预告：读社区 feed（含缓存），供额度面板展示。
//!
//! 按需拉取（与 get_subscription_quota 同款节奏：面板打开/手动刷新时由前端
//! invoke），服务层带 1 小时缓存与旧缓存兜底，见 `services/codex_reset.rs`。

use crate::services::codex_reset;

#[tauri::command]
pub async fn get_codex_reset_feed() -> Result<codex_reset::CodexResetFeed, String> {
    codex_reset::get_feed().await
}
