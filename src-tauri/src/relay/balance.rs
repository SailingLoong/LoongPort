//! 一行余额的**有序回落链**：cc-switch → sub2api sk → 网页登录态。
//!
//! ## 为什么要一条链，而不是各自一条路
//!
//! 原来「中转站行」与「官网行」各有一条**只走网页登录态**的余额路：中转站走 JWT 打
//! `/api/v1/user/profile`，官网走厂商的网页会话。两条路的共同前提是「登录态还活着」，
//! 而 sk 是**独立凭据** —— 登录态过期时 sk 照样能调用，用户却看不到余额，连充值入口
//! 都跟着消失（充值按钮只在有余额时渲染）。
//!
//! 而 cc-switch 本来就有一套「用 sk 查余额 → [`UsageResult`]」的实现
//! （[`crate::services::balance::get_balance`]，认 DeepSeek / StepFun / SiliconFlow /
//! OpenRouter / Novita），fork 却没用上它。
//!
//! ## 顺序是维护者定的，且顺序本身有语义
//!
//! | 步 | 路 | 命中谁 |
//! |---|---|---|
//! | 1 | [`crate::services::balance::get_balance`]（上游，按 base_url 主机名认厂商） | 官网行（DeepSeek 等） |
//! | 2 | [`api::usage_with_api_key`]（sub2api 的 `GET /v1/usage`，**sk 鉴权**） | sub2api 中转站行 |
//! | 3 | 网页登录态（见 [`SessionFallback`]） | NewAPI 中转站行 —— **它只有这一条** |
//!
//! ⚠️ **顺序写反不会报错**，只会让每一行白打一轮无用请求：
//! - 中转站的站点域名 cc-switch **认不出** ⇒ 第 1 步对它是**零请求的空转**
//!   （`detect_provider` 返回 `None` 时直接 `Ok(success:false)`，不发任何请求），
//!   所以把它放在最前面不花代价。
//! - 反过来，官网行若先走第 2 步，就是朝 `api.deepseek.com/v1/usage` 打一个必定
//!   404 的请求。
//!
//! ⚠️ **第 3 步不能删**：NewAPI 中转站没有 sk 鉴权的 `/v1/usage`，JWT 是它唯一的路。
//!
//! ## 三步都拿不到 ⇒ `success:false`，**不是 `Err`**
//!
//! 这条决定前端是「渲染失败态 + 重试入口」还是「整块静默消失」。改造前的死路正是
//! 后者：余额由一个依赖键为 `id:accountLabel` 的 effect 拉，某一行失败过一次、键不变
//! ⇒ effect 永不重跑 ⇒ 那一行整个会话都没有余额，而充值按钮又只在有余额时存在 ⇒
//! 用户连重试的入口都看不到。返回 `success:false` 让 react-query 拿到一个**可显示的
//! 失败值**，用量条渲染失败态并保留刷新按钮。
//!
//! 语义与 [`crate::services::balance`] 那份完全一致（`Err` 只留给瞬时传输失败），
//! 所以两边的结果能进同一个前端组件。

use futures::future::join_all;

use crate::provider::{UsageData, UsageResult};
use crate::relay::{api, backend, creds};

/// 一行的查询材料。
///
/// **打成结构体而不是三个平铺参数**：`site_origin` 与 `base_url` 都是 URL 形状的
/// `&str`，调换了编译器不会报，而后果是第 1 步认错厂商、第 2 步打错站点。
#[derive(Debug, Clone, Copy)]
pub struct BalanceQuery<'a> {
    /// 站点根（形如 `https://example.com`），第 2 步拿它拼 `/v1/usage`。
    pub site_origin: &'a str,
    /// 第 1 步用来**认厂商**的 base_url（[`crate::services::balance`] 按主机名判）。
    /// 官网行给厂商的 API 根（`https://api.deepseek.com`），中转站行给站点算出的那个。
    pub base_url: &'a str,
    /// 这一行名下的 sk。第 1、2 步各自并发试完，第一把问出结果的胜出。
    pub api_keys: &'a [String],
}

/// 第 3 步用哪条**网页登录态**路。
///
/// 两类行在这一步天生不同：中转站走 sub2api 的 JWT `/user/profile`，官网走 DeepSeek
/// 自己的 `/api/v0/users/get_user_summary`。前两步（sk）两边完全一样，所以差异**只在
/// 这一个 enum 上** —— 顺序本身仍然只在本模块定义一次，调用方无从改动它。
pub enum SessionFallback<'a> {
    /// sub2api / NewAPI 中转站的 JWT 路。**NewAPI 站只有这一条**，不能删。
    Relay(&'a creds::Relay),
    /// 官网（DeepSeek）的网页登录态路。
    Vendor { auth_token: &'a str },
    /// 这一行没有可用登录态 ⇒ 跳过第 3 步，别去打一个必定 401 的请求。
    None,
}

/// 链上的一步。**做成 enum + [`BalanceStep::next`] 而不是三条顺序语句**：顺序是这个
/// 模块的核心契约，让它成为一个能被测试直接断言的值，而不是散在控制流里的行序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalanceStep {
    CcSwitch,
    Sub2Api,
    Session,
}

impl BalanceStep {
    fn next(self) -> Option<Self> {
        match self {
            Self::CcSwitch => Some(Self::Sub2Api),
            Self::Sub2Api => Some(Self::Session),
            Self::Session => None,
        }
    }
}

/// [`resolve`] 的产物。
pub struct Resolved {
    /// 给前端的余额结果。**永远是 `Ok` 语义**，失败体现为 `success:false`。
    pub usage: UsageResult,
    /// 官网那条路遇到的结构化错误（只有第 3 步的 [`SessionFallback::Vendor`] 会填）。
    /// 命令层靠它判 [`crate::vendor::VendorError::AuthExpired`] 要不要清 token ——
    /// 本仓规矩是不许靠字符串匹配分派，所以它顺着这里带出去，命令层不必为了拿这一个
    /// 判断再打一次同样的请求。
    pub vendor_error: Option<crate::vendor::VendorError>,
}

/// 按固定顺序查询余额，第一个成功的结果胜出。
///
/// **永不返回 `Err`**：三条路都失败时回 `success:false` + 拼起来的错因，
/// 见模块文档最后一节。
pub async fn resolve(query: BalanceQuery<'_>, session: SessionFallback<'_>) -> Resolved {
    let mut errors: Vec<String> = Vec::new();
    let mut vendor_error = None;
    let mut step = Some(BalanceStep::CcSwitch);

    while let Some(current) = step {
        let hit = match current {
            BalanceStep::CcSwitch => {
                cc_switch_balance(query.base_url, query.api_keys, &mut errors).await
            }
            BalanceStep::Sub2Api => {
                sub2api_balance(query.site_origin, query.api_keys, &mut errors).await
            }
            BalanceStep::Session => session_balance(&session, &mut errors, &mut vendor_error).await,
        };
        if let Some(usage) = hit {
            return Resolved {
                usage,
                vendor_error,
            };
        }
        step = current.next();
    }

    Resolved {
        usage: UsageResult {
            success: false,
            data: None,
            // 三步的失败原因**全带出去**：只留最后一条会把「sk 全都 401」这类真原因
            // 盖成一句「登录态过期」，而那恰恰指错了要用户做的动作。
            error: Some(if errors.is_empty() {
                "查不到余额：这一行还没有可用的密钥或登录态".to_string()
            } else {
                errors.join("；")
            }),
        },
        vendor_error,
    }
}

/// 第 1 步：cc-switch 的按厂商实现。
///
/// 中转站域名它认不出 ⇒ 每把 sk 都是**零请求**的 `Ok(success:false)`，白跑不花钱。
async fn cc_switch_balance(
    base_url: &str,
    api_keys: &[String],
    errors: &mut Vec<String>,
) -> Option<UsageResult> {
    let results = join_all(
        api_keys
            .iter()
            .map(|api_key| crate::services::balance::get_balance(base_url, api_key)),
    )
    .await;

    for result in results {
        match result {
            Ok(usage) if usage.success => return Some(usage),
            Ok(usage) => errors.extend(usage.error),
            Err(error) => errors.push(error),
        }
    }
    None
}

/// 第 2 步：sub2api 的 sk 鉴权 `/v1/usage`。**只认 `balance`**（见 [`api::usage_with_api_key`]）。
async fn sub2api_balance(
    site_origin: &str,
    api_keys: &[String],
    errors: &mut Vec<String>,
) -> Option<UsageResult> {
    let results = join_all(
        api_keys
            .iter()
            .map(|api_key| api::usage_with_api_key(site_origin, api_key)),
    )
    .await;

    for result in results {
        match result {
            Ok(usage) if usage.success => return Some(usage),
            Ok(usage) => errors.extend(usage.error),
            Err(error) => errors.push(error.to_string()),
        }
    }
    None
}

/// 第 3 步：网页登录态。两类行各有自己的接口，见 [`SessionFallback`]。
async fn session_balance(
    session: &SessionFallback<'_>,
    errors: &mut Vec<String>,
    vendor_error: &mut Option<crate::vendor::VendorError>,
) -> Option<UsageResult> {
    match session {
        SessionFallback::None => None,
        SessionFallback::Relay(relay) => {
            match backend::RuntimeBackend::for_relay(relay).balance().await {
                Ok(balance) => Some(wallet_usage(balance.balance)),
                Err(error) => {
                    errors.push(error.to_string());
                    None
                }
            }
        }
        SessionFallback::Vendor { auth_token } => {
            match crate::vendor::deepseek::balance(auth_token).await {
                Ok(Some(usage)) => Some(usage),
                // 登录态活着但这个账号没有钱包 ⇒ 确实没有余额可显示，不当成错误，
                // 也不编造一个 0（见 `deepseek::wallet_usage`）。
                Ok(None) => None,
                Err(error) => {
                    errors.push(crate::error::AppError::from(error.clone()).to_string());
                    *vendor_error = Some(error);
                    None
                }
            }
        }
    }
}

/// JWT 路拿到的钱包余额包成 [`UsageResult`]。
///
/// **只填 `remaining`，不填 `total` / `used`** —— 钱包没有「总额」这个概念（充多少是
/// 多少），编造一个 `total` 会让前端的「剩余不足 10%」配色按一个假分母算。
///
/// 单位写 `USD`：sub2api 的钱包就是美元计价（`/v1/usage` 自己回的也是 `"USD"`）。
/// 留空会让同一行在两条路之间切换时数字旁边的单位忽隐忽现。
fn wallet_usage(balance: f64) -> UsageResult {
    UsageResult {
        success: true,
        data: Some(vec![UsageData {
            plan_name: Some("钱包余额".to_string()),
            remaining: Some(balance),
            unit: Some("USD".to_string()),
            extra: None,
            is_valid: None,
            invalid_message: None,
            total: None,
            used: None,
        }]),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 走到 `success_at` 那一步为止，链上依次经过了哪些步。
    fn steps_until_success(success_at: BalanceStep) -> Vec<BalanceStep> {
        let mut visited = Vec::new();
        let mut step = Some(BalanceStep::CcSwitch);
        while let Some(current) = step {
            visited.push(current);
            if current == success_at {
                break;
            }
            step = current.next();
        }
        visited
    }

    /// ⭐ **顺序是这个模块的全部价值，而写反了不会报任何错。**
    ///
    /// 反过来的两种后果都只是「悄悄变慢/变错」：中转站行会先朝一个认不出它的厂商表
    /// 白问一轮，官网行会先朝 `api.deepseek.com/v1/usage` 打一个必定 404 的请求。
    #[test]
    fn balance_fallback_order_is_fixed() {
        assert_eq!(
            steps_until_success(BalanceStep::CcSwitch),
            vec![BalanceStep::CcSwitch],
            "cc-switch 命中就该收工，不该继续问 sub2api"
        );
        assert_eq!(
            steps_until_success(BalanceStep::Sub2Api),
            vec![BalanceStep::CcSwitch, BalanceStep::Sub2Api],
            "sub2api 必须排在 cc-switch 之后"
        );
        assert_eq!(
            steps_until_success(BalanceStep::Session),
            vec![
                BalanceStep::CcSwitch,
                BalanceStep::Sub2Api,
                BalanceStep::Session
            ],
            "网页登录态是最后一条 —— 它是 NewAPI 唯一的路，但前两条不需要登录态"
        );
    }

    /// ⭐ 三步都拿不到时返回 `success:false` 而**不是 `Err`**。
    ///
    /// 这条决定前端是渲染「失败态 + 刷新按钮」还是让整块余额区消失（那样用户连重查
    /// 的入口都没有 —— 正是改造前那个死路）。
    #[tokio::test]
    async fn all_steps_failing_returns_unsuccessful_usage_result() {
        let resolved = resolve(
            BalanceQuery {
                site_origin: "https://relay.example",
                base_url: "",
                api_keys: &[],
            },
            SessionFallback::None,
        )
        .await;

        assert!(!resolved.usage.success);
        assert!(resolved.usage.data.is_none());
        assert!(
            resolved.usage.error.is_some(),
            "失败必须带原因，否则前端只能显示一个空白的失败态"
        );
    }

    /// ⭐ **订阅型分组的额度不是钱包余额。**
    ///
    /// 那种响应有 `remaining`、没有 `balance`。认 `remaining` 会把「这个分组今天还剩
    /// 多少额度」显示成「账户里还有多少钱」—— 数字看着像真的，含义完全不同。
    #[test]
    fn subscription_usage_without_balance_is_not_wallet_balance() {
        let result = api::parse_usage_with_api_key_response(
            r#"{"mode":"unrestricted","isValid":true,"planName":"月付组","remaining":42.0,"unit":"USD"}"#,
        )
        .expect("订阅型响应仍是合法 JSON");

        assert!(!result.success, "没有 balance 就是「没问出钱包余额」");
        assert!(result.data.is_none(), "不能把订阅额度当成余额透出去");
    }

    #[test]
    fn wallet_usage_preserves_balance_plan_name_and_unit() {
        let result = api::parse_usage_with_api_key_response(
            r#"{"mode":"unrestricted","isValid":true,"planName":"钱包余额","remaining":12.5,"unit":"USD","balance":12.5}"#,
        )
        .expect("钱包型响应应能解析");

        assert!(result.success);
        let usage = result
            .data
            .and_then(|data| data.into_iter().next())
            .expect("应返回一条钱包余额");
        assert_eq!(usage.plan_name.as_deref(), Some("钱包余额"));
        assert_eq!(usage.remaining, Some(12.5));
        assert_eq!(usage.unit.as_deref(), Some("USD"));
    }
}
