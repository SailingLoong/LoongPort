//! Codex OAuth Tauri Commands
//!
//! 提供 OpenAI ChatGPT Plus/Pro OAuth 认证相关的 Tauri 命令。
//!
//! 大部分认证命令通过通用 `auth_*` 命令（参见 `commands::auth`）暴露给前端，
//! 此处定义 State wrapper 以及 Codex OAuth 专属的订阅额度和模型列表查询命令。

use crate::proxy::providers::codex_oauth_auth::{CodexOAuthError, CodexOAuthManager};
use crate::services::model_fetch::FetchedModel;
use crate::services::subscription::{query_codex_quota, CredentialStatus, SubscriptionQuota};
use std::sync::Arc;
use tauri::State;

/// Codex OAuth 认证状态
///
/// `CodexOAuthManager` 内部已使用细粒度锁且所有方法均为 `&self`，因此这里
/// 直接持有 `Arc`，不再包一层 `RwLock`——避免任一命令持有粗粒度锁跨网络刷新
/// 时阻塞其他命令（切换 / 认证中心操作 / token 读取）。
pub struct CodexOAuthState(pub Arc<CodexOAuthManager>);

/// 查询 Codex OAuth (ChatGPT Plus/Pro) 订阅额度
///
/// - `account_id` 未指定时回退到 `CodexOAuthManager` 的默认账号
/// - 没有任何账号时返回 `not_found`，前端 `SubscriptionQuotaView` 会静默不渲染
/// - 复用 `services::subscription::query_codex_quota`，因此 wham/usage 端点协议
///   与 Codex CLI 路径完全一致
#[tauri::command(rename_all = "camelCase")]
pub async fn get_codex_oauth_quota(
    account_id: Option<String>,
    state: State<'_, CodexOAuthState>,
) -> Result<SubscriptionQuota, String> {
    let manager = &state.0;

    // 解析最终使用的账号 ID：显式 > 默认账号 > 无账号 (not_found)
    let resolved = match account_id {
        Some(id) => Some(id),
        None => manager.default_account_id().await,
    };
    let Some(id) = resolved else {
        return Ok(SubscriptionQuota::not_found("codex_oauth"));
    };

    // 获取（必要时自动刷新）access_token
    let token = match manager.get_valid_token_for_account(&id).await {
        Ok(t) => t,
        Err(e) => return quota_from_token_error(e),
    };

    // 瞬时传输失败以 Err 传播（前端 reject → retry + 保留上次成功值）。
    query_codex_quota(
        &token,
        Some(&id),
        "codex_oauth",
        "Codex OAuth access token expired or rejected. Please re-login via cc-switch.",
    )
    .await
}

fn quota_from_token_error(e: CodexOAuthError) -> Result<SubscriptionQuota, String> {
    match e {
        e @ CodexOAuthError::RefreshTokenInvalid => Ok(SubscriptionQuota::error(
            "codex_oauth",
            CredentialStatus::Expired,
            format!("Codex OAuth token unavailable: {e}"),
        )),
        CodexOAuthError::AccountNotFound(_) => Ok(SubscriptionQuota::not_found("codex_oauth")),
        // 网络、存储或恢复未完成并不证明凭据失效。保留 Err，让查询层
        // 重试并保留上次成功的额度，而不是覆盖成需要重新登录。
        e => Err(format!("Codex OAuth token unavailable: {e}")),
    }
}

/// 获取 Codex OAuth (ChatGPT Plus/Pro) 可用模型列表
///
/// ChatGPT Codex 反代使用 `chatgpt.com/backend-api/codex/*`，不是 OpenAI 兼容
/// `/v1/models`。这里复用托管 OAuth 账号的 access_token，直接读取 Codex 后端
/// 暴露的模型列表端点。
#[tauri::command(rename_all = "camelCase")]
pub async fn get_codex_oauth_models(
    account_id: Option<String>,
    state: State<'_, CodexOAuthState>,
) -> Result<Vec<FetchedModel>, String> {
    let manager = &state.0;
    let resolved = match account_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(id.to_string()),
        None => manager.default_account_id().await,
    };
    let Some(id) = resolved else {
        return Err("No ChatGPT account available".to_string());
    };

    let token = manager
        .get_valid_token_for_account(&id)
        .await
        .map_err(|e| format!("Codex OAuth token unavailable: {e}"))?;

    crate::services::codex_oauth_models::fetch_models_with_token(&token, &id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_token_error_preserves_definitive_states() {
        let expired = quota_from_token_error(CodexOAuthError::RefreshTokenInvalid).unwrap();
        assert!(matches!(
            expired.credential_status,
            CredentialStatus::Expired
        ));
        let missing =
            quota_from_token_error(CodexOAuthError::AccountNotFound("synthetic".into())).unwrap();
        assert!(matches!(
            missing.credential_status,
            CredentialStatus::NotFound
        ));
    }

    #[test]
    fn quota_token_error_rejects_non_definitive_failures() {
        for error in [
            CodexOAuthError::NetworkError("synthetic offline".into()),
            CodexOAuthError::IoError("synthetic disk error".into()),
            CodexOAuthError::ParseError("synthetic malformed response".into()),
            CodexOAuthError::TokenFetchFailed("403 forbidden".into()),
            CodexOAuthError::TokenFetchFailed("429 rate limit".into()),
            CodexOAuthError::TokenFetchFailed("503 unavailable".into()),
            CodexOAuthError::ProtectedStorage(crate::error::AppError::Config(
                "mode.verification_required".into(),
            )),
        ] {
            let expected = format!("Codex OAuth token unavailable: {error}");
            assert_eq!(quota_from_token_error(error).unwrap_err(), expected);
        }
    }
}
