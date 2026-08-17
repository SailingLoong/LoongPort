//! 官网直连账号（vendor）层。与 [`crate::relay`]（中转站）**平级并列**。
//!
//! ## 为什么不复用 relay 的 Client
//!
//! 差异不在 HTTP 形状，在语义：中转站一个账号给**多个分组**（多档位、有倍率）、
//! key 列表明文可认领、站点域名要用户输入并探测；官网一个账号就**一个 endpoint**、
//! 无倍率、**明文只在创建那一刻给一次**、域名是编译期常量。
//! 硬合会造出一堆语义不成立的方法（`list_groups` 返回什么？倍率填什么？）。
//!
//! ## 为什么是 enum 而不是 trait
//!
//! `async fn` in trait 返回 RPITIT ⇒ **不是 dyn-compatible** ⇒ `Box<dyn _>` 不成立，
//! 而「按 vendor_id 取一个实现」正需要它；`async-trait` 不是本仓的直接依赖。
//! enum 静态分派零新依赖、编译期穷尽，与 [`crate::relay::platform_map`] 的风格一致。
//! 加第二家厂商 = 加一个变体 + 编译器把所有没覆盖的 match 点报出来。

pub mod bigmodel;
pub mod creds;
pub mod deepseek;
pub mod opencode;
pub mod provision;

use crate::error::AppError;

/// 支持的官网厂商。加一家就加一个变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    DeepSeek,
    BigModel,
    OpenCode,
}

impl Vendor {
    /// 稳定标识，进数据库与 provider id 的哈希。⚠️ 改它是迁移不是重构。
    pub fn vendor_id(&self) -> &'static str {
        match self {
            Vendor::DeepSeek => deepseek::VENDOR_ID,
            Vendor::BigModel => bigmodel::VENDOR_ID,
            Vendor::OpenCode => opencode::VENDOR_ID,
        }
    }

    pub const fn display_name(&self) -> &'static str {
        match self {
            Vendor::DeepSeek => "DeepSeek",
            Vendor::BigModel => "智谱 BigModel",
            // 厂商家族名（账号行头、登录窗标题用）；具体档位名由
            // [`plans`] 里那份 [`PlanInfo::display_name`] 给（"opencode Zen"/"opencode Go"）。
            Vendor::OpenCode => "opencode",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "deepseek" => Some(Vendor::DeepSeek),
            "bigmodel" => Some(Vendor::BigModel),
            "opencode" => Some(Vendor::OpenCode),
            _ => None,
        }
    }
}

// ─────────────────────── 接入变体（plan） ───────────────────────

/// 一个 vendor 账号在**配置展开层**的接入变体。
///
/// 单 plan 厂商（DeepSeek / BigModel）恰好一个，`id_segment` = `vendor_id`，行为与
/// 「一个账号一个 endpoint」的旧世界完全一致；多 plan 厂商（opencode 的 Zen / Go）
/// 同一个账号展开出多组 provider 记录，各自独立可切 —— 账号层（登录、key、余额）
/// 两档共享同一份，不因 plan 而分叉。
///
/// plan 是**编译期静态清单**，不是用户数据：不进数据库、无迁移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanInfo {
    /// 进 provider id 哈希（[`provision::provider_id_for`]）与远端 `tier_configs`
    /// 键的稳定段。⚠️ 单 plan 厂商的段恒等于 `vendor_id` —— 存量 id 靠它不变。
    pub id_segment: &'static str,
    /// provider 记录与档位行的展示名。
    pub display_name: &'static str,
}

/// 单 plan 厂商的档位清单：段 = `vendor_id`、名字 = 厂商名 —— 与「一个账号一个
/// endpoint」的旧世界完全一致（provider id 派生结果一个字节都没变）。
///
/// `display_name` 在这里各写一份字面量（const 上下文调不了 `Vendor::display_name`），
/// 一致性由测试 `single_plan_segments_mirror_the_vendor_identity` 钉住 —— 同
/// `MANAGED_ID_PREFIX` 那道跨文件闸的模式。
const DEEPSEEK_PLANS: &[PlanInfo] = &[PlanInfo {
    id_segment: deepseek::VENDOR_ID,
    display_name: "DeepSeek",
}];
const BIGMODEL_PLANS: &[PlanInfo] = &[PlanInfo {
    id_segment: bigmodel::VENDOR_ID,
    display_name: "智谱 BigModel",
}];

/// 这个厂商账号展开出的全部 plan。数组序即展示序。
pub fn plans(vendor: Vendor) -> &'static [PlanInfo] {
    match vendor {
        Vendor::DeepSeek => DEEPSEEK_PLANS,
        Vendor::BigModel => BIGMODEL_PLANS,
        Vendor::OpenCode => &opencode::Plan::PLAN_INFOS,
    }
}
/// 按 id 段反查 plan（`vendor_switch` 的入参校验、provider id 反查都走它）。
/// `None` = 这个厂商没有这个段。
pub fn plan_by_segment(vendor: Vendor, segment: &str) -> Option<PlanInfo> {
    plans(vendor)
        .iter()
        .find(|plan| plan.id_segment == segment)
        .copied()
}

// ─────────────────────── 厂商分发（同 relay::backend 的形状） ───────────────────────

/// 登录窗回传解析后的统一形态：凭据材料（进 `auth_token` 列）+ 账号身份。
///
/// deepseek 的 `auth_token` 是裸 JWT；bigmodel 是三件套的 JSON（[`bigmodel::Session`]）。
/// 列语义都是「调用该厂商 API 所需的全部凭据」，各模块自己序列化。
pub struct VendorSession {
    pub auth_token: String,
    pub account: VendorAccount,
}

/// 功能性登录页。远端配置的 `vendor_invite_urls`（维护者返利链接，
/// 已过 HTTPS + 同源闸）存在且属于该厂商时，优先打开它 —— 归因在服务端完成，
/// 对登录流程零侵入。
pub fn login_url(vendor: Vendor) -> String {
    let builtin = match vendor {
        Vendor::DeepSeek => deepseek::LOGIN_URL,
        Vendor::BigModel => bigmodel::LOGIN_URL,
        Vendor::OpenCode => opencode::LOGIN_URL,
    };
    let invite = crate::relay::remote_config::load_cached()
        .and_then(|config| config.vendor_invite_urls.get(vendor.vendor_id()).cloned())
        .filter(|url| {
            url::Url::parse(url)
                .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some(invite_host(vendor)))
        });
    invite.unwrap_or_else(|| builtin.to_string())
}

/// 邀请链接必须落在的 host（同源闸的判据）。
fn invite_host(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::DeepSeek => "platform.deepseek.com",
        Vendor::BigModel => "www.bigmodel.cn",
        Vendor::OpenCode => "opencode.ai",
    }
}

pub fn login_window_label(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::DeepSeek => deepseek::LOGIN_WINDOW_LABEL,
        Vendor::BigModel => bigmodel::LOGIN_WINDOW_LABEL,
        Vendor::OpenCode => opencode::LOGIN_WINDOW_LABEL,
    }
}

pub fn login_script(vendor: Vendor, login_hint: &str) -> String {
    match vendor {
        Vendor::DeepSeek => deepseek::login_script(login_hint),
        Vendor::BigModel => bigmodel::login_script(login_hint),
        Vendor::OpenCode => opencode::login_script(login_hint),
    }
}

pub fn parse_creds_navigation(
    vendor: Vendor,
    url: &url::Url,
) -> Option<Result<VendorSession, AppError>> {
    match vendor {
        Vendor::DeepSeek => deepseek::parse_creds_navigation(url).map(|result| {
            result.map(|creds| VendorSession {
                auth_token: creds.auth_token.clone(),
                account: VendorAccount::from(creds),
            })
        }),
        Vendor::BigModel => bigmodel::parse_creds_navigation(url).map(|result| {
            result.map(|(session, account)| VendorSession {
                auth_token: serde_json::to_string(&session).unwrap_or_default(),
                account,
            })
        }),
        // auth_token 暂存登录信号 JSON（页面读不到 HttpOnly cookie），
        // `commands::vendor::do_login` 采完 cookie 后经 `compose_session` 定稿。
        Vendor::OpenCode => opencode::parse_creds_navigation(url),
    }
}

pub async fn list_keys(vendor: Vendor, auth_token: &str) -> Result<Vec<VendorKey>, VendorError> {
    match vendor {
        Vendor::DeepSeek => deepseek::list_keys(auth_token).await,
        Vendor::BigModel => {
            bigmodel::list_keys(
                &bigmodel::parse_session(auth_token)
                    .map_err(|e| VendorError::Transient(e.to_string()))?,
            )
            .await
        }
        Vendor::OpenCode => opencode::list_keys(auth_token).await,
    }
}

pub async fn create_key(
    vendor: Vendor,
    auth_token: &str,
    name: &str,
) -> Result<String, VendorError> {
    match vendor {
        Vendor::DeepSeek => deepseek::create_key(auth_token, name).await,
        Vendor::BigModel => {
            bigmodel::create_key(
                &bigmodel::parse_session(auth_token)
                    .map_err(|e| VendorError::Transient(e.to_string()))?,
                name,
            )
            .await
        }
        Vendor::OpenCode => opencode::create_key(auth_token, name).await,
    }
}

pub async fn delete_key(
    vendor: Vendor,
    auth_token: &str,
    key: &VendorKey,
) -> Result<(), VendorError> {
    match vendor {
        Vendor::DeepSeek => deepseek::delete_key(auth_token, key).await,
        Vendor::BigModel => {
            bigmodel::delete_key(
                &bigmodel::parse_session(auth_token)
                    .map_err(|e| VendorError::Transient(e.to_string()))?,
                key,
            )
            .await
        }
        Vendor::OpenCode => opencode::delete_key(auth_token, key).await,
    }
}

pub async fn balance(
    vendor: Vendor,
    auth_token: &str,
) -> Result<Option<crate::provider::UsageResult>, VendorError> {
    match vendor {
        Vendor::DeepSeek => deepseek::balance(auth_token).await,
        Vendor::BigModel => {
            bigmodel::balance(
                &bigmodel::parse_session(auth_token)
                    .map_err(|e| VendorError::Transient(e.to_string()))?,
            )
            .await
        }
        Vendor::OpenCode => opencode::balance().await,
    }
}

/// 该厂商**某个 plan**下 `AppType` → `(base_url, model)`。
/// `id_segment` 必须来自 [`plans`] 的清单；不在清单里的段返回 `None`。
pub fn config_for(
    vendor: Vendor,
    id_segment: &str,
    app: &crate::app_config::AppType,
) -> Option<(String, String)> {
    match vendor {
        Vendor::DeepSeek if id_segment == deepseek::VENDOR_ID => deepseek::config_for(app),
        Vendor::BigModel if id_segment == bigmodel::VENDOR_ID => bigmodel::config_for(app),
        Vendor::OpenCode => opencode::Plan::from_id_segment(id_segment)?.config_for(app),
        _ => None,
    }
}

/// 该厂商**某个 plan**的 Claude 角色分档（生成与 `is_user_edited` 基准共用，见
/// [`provision::claude_roles_for`] 的铁律）。
pub fn claude_role_models(
    vendor: Vendor,
    id_segment: &str,
) -> crate::relay::provision::ClaudeRoleModels {
    match vendor {
        Vendor::DeepSeek => deepseek::claude_role_models(),
        Vendor::BigModel => bigmodel::claude_role_models(),
        // Zen 与 Go 的角色档不同（Go 目录没有 claude 系），必须按段分派。
        Vendor::OpenCode => opencode::Plan::from_id_segment(id_segment)
            .unwrap_or(opencode::Plan::Zen)
            .claude_role_models(),
    }
}

/// 该厂商**某个 plan**的生成风格（鉴权字段 / wire 协议）。
pub fn plan_style(vendor: Vendor, id_segment: &str) -> crate::relay::provision::ProvisionStyle {
    match vendor {
        Vendor::DeepSeek | Vendor::BigModel => crate::relay::provision::ProvisionStyle::default(),
        Vendor::OpenCode => opencode::Plan::from_id_segment(id_segment)
            .unwrap_or(opencode::Plan::Zen)
            .style(),
    }
}

/// 该厂商**某个 plan**能服务的模型清单（去重），供 provision 写进 `modelCatalog`
/// —— 省心模式的模型偏好过滤与托盘 app→模型映射都以目录为准，没有目录的档位
/// 在用户设了偏好时会被静默排除（2026-08-17 真实 smoke 实测 DeepSeek 直连中招）。
///
/// 派生而不是另列常量：取各平台 `config_for` 的主模型 + Claude 角色分档的取值，
/// 与生成配置同源 —— 两边各写一份清单迟早分叉。`[1M]` 是上下文长度变体后缀，
/// 目录收基础名（角色 env 才带后缀）。
pub fn catalog_models(vendor: Vendor, id_segment: &str) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    let push = |models: &mut Vec<String>, model: &str| {
        let base = model.trim_end_matches("[1M]");
        if !base.is_empty() && !models.iter().any(|m| m == base) {
            models.push(base.to_string());
        }
    };
    for app in crate::vendor::provision::VENDOR_APPS {
        if let Some((_, model)) = config_for(vendor, id_segment, &app) {
            push(&mut models, &model);
        }
    }
    let roles = claude_role_models(vendor, id_segment);
    for model in [
        &roles.opus,
        &roles.fable,
        &roles.sonnet,
        &roles.haiku,
        &roles.subagent,
    ] {
        push(&mut models, model);
    }
    models
}

/// 「管理 API Key」网页（帮助跳转）。`None` = 该厂商没有公开的密钥管理页。
pub fn api_keys_help_url(vendor: Vendor) -> Option<&'static str> {
    match vendor {
        Vendor::DeepSeek => Some(deepseek::API_KEYS_URL),
        Vendor::BigModel => Some(bigmodel::API_KEYS_URL),
        // keys 页在 workspace 内（`/workspace/{id}/keys`），没有静态直达 URL；
        // 落到站点首页，用户从自己的 workspace 一步可达。
        Vendor::OpenCode => Some(opencode::SITE_ORIGIN),
    }
}

/// 官网站点 origin（provider 的 website_url 等展示用）。
pub fn site_origin(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::DeepSeek => deepseek::SITE_ORIGIN,
        Vendor::BigModel => bigmodel::SITE_ORIGIN,
        Vendor::OpenCode => opencode::SITE_ORIGIN,
    }
}

/// 该厂商 API 调用的 base origin（余额兜底等）。
pub fn api_origin(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::DeepSeek => deepseek::API_ORIGIN,
        Vendor::BigModel => bigmodel::API_ORIGIN,
        Vendor::OpenCode => opencode::API_ORIGIN,
    }
}

/// 一把已存在的 key。**有意不含明文字段** —— 列表接口拿不到明文，
/// 让类型把这个事实钉住。删除靠这里的三元组定位（不是靠名字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorKey {
    pub name: String,
    /// 脱敏值（含 `*`）。删除请求里叫 `redacted_key`。
    pub redacted_key: String,
    /// Unix 秒。
    pub created_at: i64,
    pub tracking_id: String,
}

/// 登录后确认的账号身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorAccount {
    /// 厂商侧用户 id。**是 String 不是 i64** —— DeepSeek 给的是 UUID。
    pub account_id: String,
    /// 给人看的名字。
    pub label: String,
    /// 重登时预填进登录框的值（DeepSeek 是手机号）。
    pub login_identifier: String,
}

/// 结构化错误。命令层与 UI 按它分派，**不许靠字符串匹配**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorError {
    /// 登录态失效（DeepSeek 的 `code: 40002`）。⚠️ 只清 token，**不清 api_key**。
    AuthExpired,
    /// key 数量到上限（DeepSeek 是 100 把，`biz_code: 1`）。
    KeyLimitReached,
    /// 本该拿到明文却拿到脱敏值。见 [`deepseek::validate_plaintext_key`]。
    RedactedValueReturned,
    Transient(String),
}

impl From<VendorError> for crate::error::AppError {
    fn from(e: VendorError) -> Self {
        crate::error::AppError::Config(match e {
            VendorError::AuthExpired => "登录已过期，请重新登录".to_string(),
            VendorError::KeyLimitReached => {
                "账号内 API key 已达 100 上限，请到官网删除一些".to_string()
            }
            VendorError::RedactedValueReturned => {
                "官网返回的密钥是脱敏值而非明文，已中止".to_string()
            }
            VendorError::Transient(m) => m,
        })
    }
}

/// 本客户端建的 key 的名字。
///
/// ```text
/// LoongPort专用/a<account-id>
/// ```
///
/// ⚠️ **中文「专用」二字是维护者定的字面量**，用户要在官网列表里一眼认出来。
///
/// ## 为什么按**账号**而不按机器（2026-08-04 改，维护者实测推翻原设计）
///
/// 初版第二段是 `device_id`，理由是「多台机器各认自己那把，否则 A 机器改了 Key、
/// B 机器的配置就悄悄失效」。**那个理由站不住** —— 维护者在 relay 侧实测证伪：
///
/// `provision` **从不改动已有 Key**（认领到就直接用），能换掉 sk 的只有「用户去
/// 网页端手工删了重建」，而那种情况下不论按机器还是按账号，其它机器都一样要重新
/// provision。用 device_id 换来的不是安全，只是**每台机器各堆一份**。
///
/// 代价是真的：他一个 sub2api 账号下堆了 11 把、只有 3 把在用。
/// DeepSeek 这边上限 100 把，三台 Mac 各建一套同样是白耗。
///
/// 前缀 `a` 让人一眼看出哪段是账号（对齐 relay 侧
/// `LoongPort/a<account-id>/<platform>/<group-id>` 的写法）。
///
/// ## 与 relay 的差异：无 platform / group 段
///
/// DeepSeek 的一把 sk **六个平台通吃**（同一把同时能请求 `/v1`、`/anthropic`、
/// 根路径），所以没有「每平台一把」的概念 —— 两段就够。
///
/// ⚠️ 这个名字进了服务端、跨端可见，改它等于所有已建 key 认领不回来。
/// 本次改动**有意接受那个代价**（孤儿是一次性的，而按机器命名是每加一台永久 +1）。
pub fn key_name_for(account_id: &str) -> String {
    format!("LoongPort专用/a{account_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key 名字按**账号**而不按机器 —— 那样三台机器共用一把
    /// （理由见 `key_name_for` 的文档：按机器命名的理由已被实测推翻）。
    #[test]
    fn key_name_is_scoped_to_the_account_not_the_machine() {
        let name = key_name_for("11eb18b1-2784-43ba-8324-16c5eef7f72c");
        assert_eq!(
            name, "LoongPort专用/a11eb18b1-2784-43ba-8324-16c5eef7f72c",
            "两段：中文前缀 + a<account-id>"
        );
        // 同一个账号在任何机器上算出的名字都一样 —— 这就是「共用一把」的机制。
        assert_eq!(name, key_name_for("11eb18b1-2784-43ba-8324-16c5eef7f72c"));
        // 不同账号必须不同（否则会删到别人那把）。
        assert_ne!(name, key_name_for("other-account"));
    }

    #[test]
    fn vendor_id_round_trips() {
        assert_eq!(Vendor::from_id("deepseek"), Some(Vendor::DeepSeek));
        assert_eq!(Vendor::from_id("kimi"), None);
        assert_eq!(Vendor::DeepSeek.vendor_id(), "deepseek");
    }

    /// 单 plan 厂商的档位段与名字必须与 `Vendor` 的身份一致 —— 段错了存量
    /// provider id 失联，名字错了账号行头与档位名分叉（两个来源各改各的）。
    #[test]
    fn single_plan_segments_mirror_the_vendor_identity() {
        for vendor in [Vendor::DeepSeek, Vendor::BigModel] {
            let plans = plans(vendor);
            assert_eq!(plans.len(), 1, "{vendor:?} 是单 plan 厂商");
            assert_eq!(
                plans[0].id_segment,
                vendor.vendor_id(),
                "单 plan 段必须等于 vendor_id（存量 id 靠它不变）"
            );
            assert_eq!(
                plans[0].display_name,
                vendor.display_name(),
                "单 plan 名字必须等于厂商名"
            );
        }
        // 多 plan 厂商：Zen 段 = vendor_id（存量闸在 opencode 模块里），
        // 清单里没有重复段。
        let opencode_plans = plans(Vendor::OpenCode);
        assert!(opencode_plans.len() > 1);
        assert_eq!(opencode_plans[0].id_segment, Vendor::OpenCode.vendor_id());
        for (i, a) in opencode_plans.iter().enumerate() {
            for b in opencode_plans.iter().skip(i + 1) {
                assert_ne!(a.id_segment, b.id_segment, "段不能重复");
            }
        }
    }
}
