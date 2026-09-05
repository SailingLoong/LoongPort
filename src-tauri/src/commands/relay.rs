//! LoongPort 中转站的 Tauri 命令层。
//!
//! 中转站命令：
//!
//! | 命令 | 干什么 |
//! |---|---|
//! | [`relay_status`] | 首启该弹哪个弹窗、当前是什么状态 |
//! | [`relay_import_site`] | 发现协议；必要时让用户在可见 WebView 完成网页验证，并在同一会话登录 |
//! | [`relay_login`] | 开登录 WebView，等凭据回来 |
//! | [`relay_provision`] | 拉分组 → 每组备好 sk → 写成 codex provider |
//! | [`relay_switch_tier`] | 选分组 → 退 ChatGPT → 切换 → 重开 |
//!
//! ## 为什么切换编排在 Rust 侧而不是前端
//!
//! 「退出 ChatGPT → 切换 → 重开」如果写在前端的按钮回调里，那么**托盘快切、deeplink 导入、
//! 项目快照**这三条路径都会绕过它（它们在 Rust 侧直接调 `ProviderService::switch`），用户
//! 从托盘切完就会发现 codex 还连着旧分组。放在这一层是让「切换分组」只有一个入口。
//!
//! ⚠️ 编排在这一层**不等于**别处进不来 —— 那要靠 [`crate::relay::managed`] 的守卫。
//! 已收口的是托盘（列表里剔掉托管项）与 `switch_provider` / `update_provider` /
//! `delete_provider` 三条通用命令。
//!
//! **项目快照走的是「提示而不是替他退」**（`services::profile::apply`，2026-08-04）：
//! 它切 codex 供应商后会往 warnings 里加一句「请重启 ChatGPT」，而**不**替用户退掉那个
//! app —— 应用快照是「一次动作切一批 app」，用户点的是「切到这个项目」，把它读成
//! 「同意关掉我正开着的 ChatGPT」是过度解释（`switch_provider` 的文档把这条定死了：
//! `None` = 不碰 ChatGPT）。那句 warning 经 `profiles.applyWarnings` 的 toast 到达用户。
//!
//! **deeplink 导入仍直接调 `ProviderService::switch`**（要构造一条带 `enabled=true` 的
//! deep link 才碰得到，优先级低于上面几条）。

use futures::future::join_all;
use serde::Serialize;
use std::{
    future::Future,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::events::{emit_provider_switched, PURCHASE_CLOSED};
use crate::provider::Provider;
use crate::relay::{
    api, backend, balance, browser_bridge, chatgpt_app, creds, discovery, imagegen_mcp, login,
    model_verification::target as verification_target, newapi, newapi_provision, newapi_purchase,
    platform_map, pricing, provider_fingerprint, provision, purchase, reconcile, remote_config,
    site_config,
};
use crate::services::ProviderService;
use crate::store::AppState;

/// 默认中转站域名。域名输入框的底纹词，用户直接点确定就用它。
///
/// ⚠️ **它不再是维护者自己的站**（2026-08-04 从 `bestapi.store` 改过来 —— 那个站
/// 没有精力持续运维，默认值不该指向一个自己都不盯着的站）。那个巧合曾让三处文档把
/// 「默认站」与「维护者自己的站」写成一件事（本文档、[`crate::relay::aff`] 的
/// `aff_code_for` 与它那条「维护者自己的站有意缺席」的测试）—— 别再绑回去。
///
/// ⇒ 换这个值时**要重新确认它在 [`crate::relay::aff`] / [`crate::relay::promo`]
/// 两张内置表里各该有什么**（两张表各自按 host 查，彼此独立，但都与「谁是默认站」
/// 有关：默认站是最常被走到的那条路）。当前这个站在 aff 内置表里、不在 promo 内置表里，
/// 前者**本模块 `tests` 里有一条闸钉着**（跟着这个常量一起改，它会当场告诉你）。
//
// ⚠️ 有意**不写那条测试的名字**：rustdoc 的 intra-doc link 链不进 `#[cfg(test)]`，
// 写成反引号裸名字就没有任何东西能验它 —— 2026-08-04 同一次改名里连漏两处指针
// （两路 review 各抓一次）。指「本模块 tests 里」而不指名字，改名就不会让它悬空。
const DEFAULT_SITE: &str = "790053500.com";
pub(crate) const RELAY_DIRECTORY_UPDATED_EVENT: &str = "relay-directory-updated";

// `DEFAULT_MODEL` 住在 `provision` 里 —— `pick_model` 要在「问不出模型列表」时
// 回落到它。这里只 `use`，避免在命令层另写一份。
use provision::DEFAULT_MODEL;

/// 等用户走完登录流程的上限（秒）。
///
/// 5 分钟够走完注册 + 邮箱验证码 + 2FA。**超时不是错误** —— 用户可能就是走开了，
/// 那时安静收场（返回 `false`）而不是弹一条他看不懂的失败。
///
/// 提成常量而不是内联 `300`：日志里要把它打出来（「最多 N 秒」），
/// 两处各写一个字面量迟早对不上（`vendor.rs` 的 `LOGIN_TIMEOUT` 同一形状）。
const LOGIN_TIMEOUT_SECS: u64 = 300;

/// 加站弹窗需要的后端状态。
///
/// ## 为什么只剩两个字段（2026-08-04 收缩）
///
/// 原来它有 9 个字段，服务的是已删的 LoongPort 独立页那个**单站视图**
/// （顶部显示「当前站是 X、登录的是 Y、已过期了没、有几个档位」）。中转站行现在
/// 每行各显示自己的状态、数据走 [`relay_list_relays`]，那个「当前站」的概念
/// 连带消失 ⇒ 那 7 个字段前端一个都不读了。
///
/// 其中 `tier_count` 还有实际成本（遍历整个 provider 表数托管项），
/// 而这条命令是**首屏渲染要等的东西**。
///
/// ⚠️ 删的是**没有消费者**的字段，不是「暂时没用」的字段 —— 将来真要「当前站」
/// 这个概念时该重新想清楚它的语义（多行并列的界面里「当前」指什么），而不是
/// 留着这几个没人读的字段当预留。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    /// 域名输入框的底纹词。
    pub default_site: String,
    /// 当前是否没有任何中转站或官网账号配置，前端据此决定是否显示首次引导。
    pub should_prompt_add_site: bool,
}

/// 一个已添加站点的后端摘要。
///
/// 当前消费者是「一个站都没有吗」的自动引导判据（只数条数）。新增站点只有在
/// 注册或登录成功后才会写入；启动建表路径也会清理旧版遗留的未认证占位行。
///
/// 2026-08-04 一并收缩：原来还有 `id` / `site_name` / `label` / `logged_in` /
/// `is_current` 五个字段，服务的是已删独立页顶部那个**站点切换器**（要显示名、
/// 要标出当前选中的是哪个、要能按 id 切换）。那个控件删了之后没有消费者。
///
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteInfo {
    pub site_origin: String,
    /// 该站已有几个后端已识别身份的账号。未登录占位行不计入。
    pub account_count: usize,
}

/// 合并“发现站点 + 同一会话登录”的导入结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub relay_id: i64,
    pub site_origin: String,
    pub site_name: String,
    pub backend_kind: discovery::BackendKind,
}

/// 导入失败的机器可读种类。
///
/// 前端按 kind 映射本地化文案（`RelayDirectoryPage` 的 `importErrorMessage`），
/// 所以**一个 kind 只能承载一种用户行动指引**：`UnsupportedSite` 是「协议没识别出来，
/// 完成网页验证可能有用」；「站点不在签名目录、请手动添加」是另一种指引，归
/// [`RelayImportErrorKind::NotInDirectory`] —— 两者共用了同一个 kind 时，前端只能
/// 对一半场景说对的话（曾实锤：不在目录的站被引导去「完成网页验证」，永远无效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayImportErrorKind {
    UnsupportedSite,
    NotInDirectory,
    ProtocolConflict,
    Transport,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayImportError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<RelayImportErrorKind>,
    pub message: String,
}

impl RelayImportError {
    fn message(message: impl Into<String>) -> Self {
        Self {
            kind: None,
            message: message.into(),
        }
    }
}

impl From<AppError> for RelayImportError {
    fn from(error: AppError) -> Self {
        Self {
            kind: None,
            message: error.to_string(),
        }
    }
}

impl From<discovery::DiscoveryError> for RelayImportError {
    fn from(error: discovery::DiscoveryError) -> Self {
        Self {
            kind: Some(match error.kind {
                discovery::DiscoveryErrorKind::UnsupportedSite => {
                    RelayImportErrorKind::UnsupportedSite
                }
                discovery::DiscoveryErrorKind::ProtocolConflict => {
                    RelayImportErrorKind::ProtocolConflict
                }
                discovery::DiscoveryErrorKind::Transport => RelayImportErrorKind::Transport,
            }),
            message: error.message,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LoginResult {
    logged_in: bool,
}

impl ImportResult {
    fn authenticated(site: DiscoveredRelaySite, relay_id: i64) -> Self {
        Self {
            relay_id,
            site_origin: site.site_origin,
            site_name: site.site_name,
            backend_kind: site.backend_kind,
        }
    }
}

#[derive(Debug, Clone)]
struct DiscoveredRelaySite {
    site_origin: String,
    site_name: String,
    api_base_url: String,
    backend_kind: discovery::BackendKind,
}

#[derive(Debug, Clone)]
struct BrowserLoginContext {
    site: DiscoveredRelaySite,
    login_script: String,
}

#[derive(Debug, Clone, Copy)]
enum IncompleteImportReason {
    Closed,
    TimedOut,
}

fn incomplete_new_site_import_error(reason: IncompleteImportReason) -> RelayImportError {
    let message = match reason {
        IncompleteImportReason::Closed => "注册或登录尚未完成",
        IncompleteImportReason::TimedOut => "注册或登录等待超时，请重试",
    };
    RelayImportError {
        kind: Some(RelayImportErrorKind::Cancelled),
        message: message.into(),
    }
}

enum BrowserLoginOutcome {
    Sub2ApiCredentials(login::Credentials),
    NewApiSession(newapi::RefreshedSession),
    Error(RelayImportError),
    Closed,
}

enum BrowserLoginCredential {
    Sub2Api(login::Credentials),
    NewApiUserId(i64),
}

enum RefreshWait<T, I> {
    Interrupted(I),
    Refreshed(Result<T, AppError>),
}

/// Refresh-token rotation is a non-cancellable write once the HTTP request starts: the server
/// may invalidate the old cookie before the client observes the rotated one. An interrupt stops
/// future polling, but this helper drains the bounded refresh request and preserves any success.
async fn await_refresh_preserving_rotation<T, I>(
    refresh: impl Future<Output = Result<T, AppError>>,
    interrupt: impl Future<Output = I>,
) -> RefreshWait<T, I> {
    tokio::pin!(refresh);
    tokio::select! {
        biased;
        refreshed = &mut refresh => RefreshWait::Refreshed(refreshed),
        interrupted = interrupt => match refresh.await {
            Ok(value) => RefreshWait::Refreshed(Ok(value)),
            Err(_) => RefreshWait::Interrupted(interrupted),
        },
    }
}

/// 一个可选的档位。
///
/// `group_id` / `rate_multiplier` 是 `Option`：列表命令从本地 DB 读，而倍率只在 provision
/// 时从服务端拿到。**用 `Option` 而不是填 0 占位** —— 0 倍率意味着"免费"，UI 会把它显示成
/// 最便宜的一档，那是错的。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierInfo {
    pub provider_id: String,
    /// 这个档位落在哪个 CLI 上（`AppType::as_str()`，如 `"codex"` / `"claude"`）。
    ///
    /// ## 为什么必须有它
    ///
    /// [`do_provision`] 一次探**全部平台**，返回的 `tiers` 是全平台的，而 UI 那一行
    /// 只显示当前 app 的档位。没有这个字段，前端拿到一堆档位却分不出哪条是自己的
    /// ⇒ 「这个站没有该平台的分组」与「拉取失败」在界面上长得一样（都是零档位），
    /// 而前者重试一百次也不会有、后者重试有意义。
    ///
    /// [`list_tiers_impl`] 那条路填的是它被查询的那个 app（那条命令按 app 查，
    /// 结果天然同质），所以两条路的语义一致：**这条档位属于哪个 CLI**。
    pub app_id: String,
    pub group_name: String,
    pub display_name: String,
    /// The model currently written into this provider's Codex config.
    pub model: String,
    /// Model ids discovered from this tier's `/v1/models` endpoint.
    /// An empty list means no complete remote catalog is available.
    pub models: Vec<String>,
    pub rate_multiplier: Option<f64>,
    pub is_current: bool,
    /// 后端是否支持对这个档位执行模型验证。
    pub can_verify_models: bool,
    /// 用户在 cc-switch 编辑页改过这个档位的配置吗。
    ///
    /// 判据是**存库标记** `providers.user_edited`（编辑页置位、恢复默认复位），
    /// 不是内容比对 —— 手动改到和默认一样也仍算「已手工维护」。
    ///
    /// `None` = 读标记失败（防御）。UI 在 `None` 时
    /// 什么标记都不显示：`false` 是在断言「刷新不会覆盖你的改动」，
    /// 而事实是「不知道」—— 让用户误信比不说更糟。
    ///
    /// ⚠️ 只有 [`list_relays_impl`] 填得出它（判据要 `api_base_url`，那在
    /// `creds` 里按站点存）。[`relay_list_tiers`] 那条路恒为 `None` ——
    /// 它的调用方不显示这个标记，见该命令的文档。
    pub user_edited: Option<bool>,
    /// 服务端说这个分组允许生图（`allow_image_generation`）。
    ///
    /// ⚠️ **纯生图分组不靠这个字段识别** —— 它们在 `codex-image` 那一栏，
    /// 所在的列表本身就说明了这件事（见 [`provision::image_tier_app_type`]）。
    /// 这个字段的价值在**混合分组**：实测 `pro池` 这类有文本模型的分组也是 `true`，
    /// 它们留在 codex 栏而同时支持生图。
    ///
    /// `None` = **判不了**：这是纯服务端信息（分组的开关），本地配置里没有它。
    /// 只有 provision 那条路填得出，[`list_relays_impl`] 恒为 `None`。
    /// UI 在 `None` 时不显示标记 —— 与 `user_edited` 同一条原则：不知道就别断言。
    pub allow_image_generation: Option<bool>,
    /// 这个档位的调用参数里应用了站长自报声明（`relay/site_config.rs`）的站点
    /// origin。非空 = UI 显示「站点推荐配置」标注；`None` = 纯内置默认。
    /// 与 `user_edited` 不同，它是**存库事实**（meta.siteDeclaredOrigin），
    /// 不是现算判据。
    pub site_declared_origin: Option<String>,
}

/// 「中转站 × 分组」页的一行中转站，连带它在当前 app 下的档位。
///
/// spec §三 定的是 `RelayRow { ..., tiers: Vec<TierRow> }`，这里**复用已有的
/// [`TierInfo`] 而不新建 `TierRow`** —— 两者字段本就一致（含那个关键的
/// `rate_multiplier: Option<f64>`），再建一个只会让同一个概念有两种形状，
/// 前端也得写两套类型（CLAUDE.md §一：能复用就复用）。
///
/// **它是只读本地的**，与 [`RelayStatus`] 的首屏契约一致（不发网络请求）——
/// 所以 `tiers` 里的 `rate_multiplier` 恒为 `None`，倍率要等用户主动刷新（provision）
/// 才有值。这不是缺陷：填 0 占位会让 UI 显示成「最便宜的一档」，那是错的。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayRow {
    pub id: i64,
    pub site_origin: String,
    pub site_name: String,
    /// 登录后的账号名（昵称优先，回落邮箱），未登录为空串。
    /// 同一个站可以挂多个账号，所以「登录了」不够 —— 得说清是**哪个**账号。
    pub account_label: String,
    /// 后端根据当前行的所有托管档位计算出的展示状态。
    pub status: RelayRowStatus,
    /// 当前 app 下是否有档位正在使用。
    pub is_current: bool,
    /// 这一行是否具备余额查询所需的凭据（有效登录态或至少一把托管 SK）。
    pub can_query_balance: bool,
    /// 后端是否确认这条账号行可以打开签名目录配置的购买入口。
    pub can_purchase: bool,
    /// 「查看用量」入口资格：`usage_url` 已配置且登录态满足开窗条件（与
    /// `can_purchase` 同一判据，只是查另一个入口字段）。
    pub can_view_usage: bool,
    /// 这一行是否可以重新拉取最新账号信息、额度、可用分组与倍率。
    pub can_refresh: bool,
    /// 这一行名下**正被某个 app 当作当前项**的档位。空 = 可直接删；
    /// 非空 = 前端删除弹窗换成「点名 app」的强删变体，确认后带 `force` 重删。
    ///
    /// ⚠️ 这里与 [`relay_remove_site`] 的闸是**同一次扫描**（`apps_using_this_accounts_tiers`），
    /// 不再另设一个 `can_delete: bool` —— 那会让「能不能删」与「谁在用」两个表述分叉
    /// （尺子 1.4）：前者前端自己 `is_empty()` 派生即可。
    pub usage_blockers: Vec<UsageBlocker>,
    /// 删除确认框使用哪一种后端定义的业务语义。
    pub remove_confirmation: RemoveConfirmation,
    /// 这个中转站在**当前 app_id** 下已备好的档位。
    pub tiers: Vec<TierInfo>,
}

/// `RelayRow::usage_blockers` 的元素：哪个 app 正在把哪条档位当当前项。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBlocker {
    /// `AppType::as_str()`，与前端 `AppId` 同一套字符串（前端拿它查显示名）。
    pub app: String,
    /// 正在被当作当前项的那条档位的名字。
    pub tier_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoveConfirmation {
    NeverLoggedIn,
    Configured,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RelayRowStatus {
    NotLoggedIn,
    SessionExpired,
    SessionExpiredUsable,
    NoTiers,
    Ready,
}

/// 备好密钥的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionSummary {
    pub tiers: Vec<TierInfo>,
    /// 失败的分组与原因。**不为空也不代表整体失败** —— 成功的那些照样能用。
    pub failures: Vec<FailureInfo>,
    /// 这次新建了几把 sk（其余是认领到的已有 Key）。
    ///
    /// 给用户看的：第二次进来应该是 0（全部认领到），若每次都在新建，说明认领逻辑有问题
    /// 正在给他账号里堆垃圾 Key。
    pub keys_created: usize,
    /// Imported non-managed providers removed because LoongPort now owns the same credential.
    pub merged_providers: Vec<MergedProviderInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedProviderInfo {
    pub name: String,
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshNotice {
    None,
    Updated,
    UpdatedWithKeys,
    OtherPlatforms,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshFailureKind {
    KeyLimit,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshFailure {
    pub name: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<RefreshFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSummary {
    pub notice: RefreshNotice,
    pub refreshed_accounts: usize,
    pub tiers: usize,
    pub keys_created: usize,
    pub other_platform_tiers: usize,
    pub merged_providers: usize,
    pub failures: Vec<RefreshFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshedBalanceKind {
    Relay,
    Vendor,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshedBalance {
    pub kind: RefreshedBalanceKind,
    pub row_id: i64,
    pub result: balance::RowBalanceResult,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    pub summary: RefreshSummary,
    pub balances: Vec<RefreshedBalance>,
}

fn refresh_summary(
    app_type: &AppType,
    relay_results: Vec<(String, Result<ProvisionSummary, AppError>)>,
    vendor_results: Vec<(
        String,
        Result<super::vendor::VendorProvisionSummary, super::vendor::VendorActionError>,
    )>,
) -> RefreshSummary {
    let mut refreshed_accounts = 0;
    let mut tiers = 0;
    let mut keys_created = 0;
    let mut other_platform_tiers = 0;
    let mut merged_providers = 0;
    let mut failures = Vec::new();

    for (name, result) in relay_results {
        match result {
            Ok(summary) => {
                refreshed_accounts += 1;
                let current = summary
                    .tiers
                    .iter()
                    .filter(|tier| tier.app_id == app_type.as_str())
                    .count();
                tiers += current;
                if current == 0 && summary.failures.is_empty() {
                    other_platform_tiers += summary.tiers.len();
                }
                keys_created += summary.keys_created;
                merged_providers += summary.merged_providers.len();
                failures.extend(summary.failures.into_iter().map(|failure| RefreshFailure {
                    name: failure.group_name,
                    reason: failure.reason,
                    kind: None,
                    help_url: None,
                }));
            }
            Err(error) => failures.push(RefreshFailure {
                name,
                reason: error.to_string(),
                kind: None,
                help_url: None,
            }),
        }
    }
    for (name, result) in vendor_results {
        match result {
            Ok(summary) => {
                refreshed_accounts += 1;
                let current = usize::from(
                    summary
                        .platforms
                        .iter()
                        .any(|platform| platform == app_type.as_str()),
                );
                tiers += current;
                if current == 0 {
                    other_platform_tiers += summary.platforms.len();
                }
                merged_providers += summary.merged_providers.len();
                keys_created += usize::from(summary.key_created);
            }
            Err(error) => failures.push(RefreshFailure {
                name,
                reason: error.message,
                kind: error.kind.map(|kind| match kind {
                    super::vendor::VendorActionErrorKind::KeyLimit => RefreshFailureKind::KeyLimit,
                }),
                help_url: error.help_url,
            }),
        }
    }

    let notice = if refreshed_accounts == 0 {
        RefreshNotice::None
    } else if tiers == 0 && other_platform_tiers > 0 && failures.is_empty() {
        RefreshNotice::OtherPlatforms
    } else if keys_created > 0 {
        RefreshNotice::UpdatedWithKeys
    } else {
        RefreshNotice::Updated
    };
    RefreshSummary {
        notice,
        refreshed_accounts,
        tiers,
        keys_created,
        other_platform_tiers,
        merged_providers,
        failures,
    }
}

pub(crate) struct RelayRefreshOutcome {
    name: String,
    row_id: i64,
    provision: Option<Result<ProvisionSummary, AppError>>,
    balance: Result<balance::RowBalanceResult, AppError>,
    failures: Vec<RefreshFailure>,
}

pub(crate) struct VendorRefreshOutcome {
    pub name: String,
    pub row_id: i64,
    pub provision:
        Option<Result<super::vendor::VendorProvisionSummary, super::vendor::VendorActionError>>,
    pub balance: Result<balance::RowBalanceResult, AppError>,
}

pub(crate) fn finish_refresh_result(
    app_type: &AppType,
    relay_outcomes: Vec<RelayRefreshOutcome>,
    vendor_outcomes: Vec<VendorRefreshOutcome>,
) -> RefreshResult {
    let mut relay_results = Vec::with_capacity(relay_outcomes.len());
    let mut vendor_results = Vec::with_capacity(vendor_outcomes.len());
    let mut balances = Vec::new();
    let mut balance_failures = Vec::new();

    for outcome in relay_outcomes {
        let name = outcome.name;
        if let Some(provision) = outcome.provision {
            relay_results.push((name.clone(), provision));
        }
        balance_failures.extend(outcome.failures);
        collect_refreshed_balance(
            &mut balances,
            &mut balance_failures,
            name,
            RefreshedBalanceKind::Relay,
            outcome.row_id,
            outcome.balance,
        );
    }
    for outcome in vendor_outcomes {
        let name = outcome.name;
        if let Some(provision) = outcome.provision {
            vendor_results.push((name.clone(), provision));
        }
        collect_refreshed_balance(
            &mut balances,
            &mut balance_failures,
            name,
            RefreshedBalanceKind::Vendor,
            outcome.row_id,
            outcome.balance,
        );
    }

    let successful_balances = balances
        .iter()
        .filter(|balance| balance.result.usage.success)
        .count();
    let mut summary = refresh_summary(app_type, relay_results, vendor_results);
    summary.refreshed_accounts = summary.refreshed_accounts.max(successful_balances);
    summary.failures.extend(balance_failures);
    if matches!(summary.notice, RefreshNotice::None) && summary.refreshed_accounts > 0 {
        summary.notice = RefreshNotice::Updated;
    }

    RefreshResult { summary, balances }
}

fn relay_refresh_targets(
    state: &AppState,
    app_type: &AppType,
) -> Result<Vec<(i64, String, bool)>, AppError> {
    list_relays_impl(state, app_type.clone()).map(|rows| {
        rows.into_iter()
            .filter_map(|row| {
                let name = if row.account_label.is_empty() {
                    row.site_name
                } else {
                    row.account_label
                };
                (row.can_refresh || row.can_query_balance).then_some((
                    row.id,
                    name,
                    row.can_refresh,
                ))
            })
            .collect()
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RelayPricingRefreshSummary {
    attempted: usize,
    succeeded: usize,
    failed: Vec<(i64, String)>,
}

async fn refresh_due_relay_pricing_rows<F, Fut>(
    relays: Vec<creds::Relay>,
    now: i64,
    interval: std::time::Duration,
    refresh: F,
) -> RelayPricingRefreshSummary
where
    F: Fn(creds::Relay) -> Fut + Sync,
    Fut: Future<Output = Result<(), AppError>> + Send,
{
    use futures::StreamExt;

    let due: Vec<_> = relays
        .into_iter()
        .filter(|relay| relay.account_id.is_some() && !relay.pricing_is_fresh(now, interval))
        .collect();
    let attempted = due.len();
    let mut results = futures::stream::iter(due.into_iter().map(|relay| {
        let refresh = &refresh;
        async move {
            let relay_id = relay.id;
            (relay_id, refresh(relay).await)
        }
    }))
    .buffer_unordered(2);
    let mut summary = RelayPricingRefreshSummary {
        attempted,
        ..Default::default()
    };
    while let Some((relay_id, result)) = results.next().await {
        match result {
            Ok(()) => summary.succeeded += 1,
            Err(error) => summary.failed.push((relay_id, error.to_string())),
        }
    }
    summary
}

pub(crate) async fn refresh_due_relay_pricing(
    app_handle: tauri::AppHandle,
) -> Result<(), AppError> {
    let (relays, db) = {
        let state = app_handle.state::<AppState>();
        (with_conn(&state, creds::list)?, Arc::clone(&state.db))
    };
    let now = chrono::Utc::now().timestamp();
    let summary = refresh_due_relay_pricing_rows(
        relays,
        now,
        crate::maintenance::config::RELAY_PRICING_REFRESH_INTERVAL,
        move |relay| {
            let app_handle = app_handle.clone();
            let db = Arc::clone(&db);
            async move {
                // 倍率拉取是只读请求，走 401→续期→重试：`token_expires_at = NULL`
                // 的行靠它从「永不续期、到点暴毙」的降级态自愈。
                relay_read_with_refresh_retry(&app_handle, relay.id, |op| {
                    // 闭包要能调两次（原请求 + 重试），future 各自持有克隆出来的 Arc。
                    let db = Arc::clone(&db);
                    async move {
                        let updates = pricing::fetch_rate_updates(&op).await?;
                        pricing::apply_rate_updates(&db, &updates)?;
                        let conn = db.conn.lock().map_err(|error| {
                            AppError::Database(format!("获取数据库连接失败: {error}"))
                        })?;
                        creds::mark_pricing_synced(&conn, op.id, chrono::Utc::now().timestamp())
                    }
                })
                .await
            }
        },
    )
    .await;

    for (relay_id, error) in &summary.failed {
        log::warn!("中转站 #{relay_id} 后台倍率刷新失败: {error}");
    }
    log::info!(
        "中转站后台倍率刷新完成: attempted={}, succeeded={}, failed={}",
        summary.attempted,
        summary.succeeded,
        summary.failed.len()
    );
    Ok(())
}

async fn refresh_relay_outcome(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
    name: String,
    refresh_config: bool,
) -> RelayRefreshOutcome {
    let provision = if refresh_config {
        Some(refresh_relay_provision(app_handle, relay_id).await)
    } else {
        None
    };
    let balance = relay_balance_impl(app_handle, relay_id).await;
    RelayRefreshOutcome {
        name,
        row_id: relay_id,
        provision,
        balance,
        failures: Vec::new(),
    }
}

async fn refresh_relay_result(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
    app_type: &AppType,
) -> RefreshResult {
    let name = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, |conn| creds::get(conn, relay_id))
            .ok()
            .flatten()
            .map(|relay| {
                if relay.account_label.is_empty() {
                    relay.site_name
                } else {
                    relay.account_label
                }
            })
            .unwrap_or_else(|| format!("中转站 #{relay_id}"))
    };
    finish_refresh_result(
        app_type,
        vec![refresh_relay_outcome(app_handle, relay_id, name, true).await],
        Vec::new(),
    )
}

#[tauri::command]
pub async fn relay_refresh(
    app_handle: tauri::AppHandle,
    relay_id: i64,
    app: String,
) -> Result<RefreshResult, String> {
    let app_type = AppType::from_str(&app).map_err(|error| error.to_string())?;
    Ok(refresh_relay_result(&app_handle, relay_id, &app_type).await)
}

#[tauri::command]
pub async fn relay_refresh_all(
    app_handle: tauri::AppHandle,
    app: String,
) -> Result<RefreshResult, String> {
    let app_type = AppType::from_str(&app).map_err(|error| error.to_string())?;
    let (relay_targets, vendor_targets) = {
        let state = app_handle.state::<AppState>();
        (
            relay_refresh_targets(state.inner(), &app_type).map_err(|error| error.to_string())?,
            super::vendor::vendor_refresh_targets(state.inner(), &app_type)
                .map_err(|error| error.to_string())?,
        )
    };

    let relay_futures = relay_targets
        .into_iter()
        .map(|(row_id, name, refresh_config)| {
            let app_handle = app_handle.clone();
            async move { refresh_relay_outcome(&app_handle, row_id, name, refresh_config).await }
        });
    let vendor_futures = vendor_targets
        .into_iter()
        .map(|(row_id, name, refresh_config)| {
            let app_handle = app_handle.clone();
            async move {
                let state = app_handle.state::<AppState>();
                let (provision, balance) =
                    super::vendor::refresh_vendor_account(state.inner(), row_id, refresh_config)
                        .await;
                VendorRefreshOutcome {
                    name,
                    row_id,
                    provision,
                    balance,
                }
            }
        });
    let (relay_outcomes, vendor_outcomes) =
        tokio::join!(join_all(relay_futures), join_all(vendor_futures));

    Ok(finish_refresh_result(
        &app_type,
        relay_outcomes,
        vendor_outcomes,
    ))
}

fn collect_refreshed_balance(
    balances: &mut Vec<RefreshedBalance>,
    failures: &mut Vec<RefreshFailure>,
    name: String,
    kind: RefreshedBalanceKind,
    row_id: i64,
    result: Result<balance::RowBalanceResult, AppError>,
) {
    match result {
        Ok(result) => {
            if !result.usage.success {
                failures.push(RefreshFailure {
                    name,
                    reason: result
                        .usage
                        .error
                        .clone()
                        .unwrap_or_else(|| "余额刷新失败".to_string()),
                    kind: None,
                    help_url: None,
                });
            }
            balances.push(RefreshedBalance {
                kind,
                row_id,
                result,
            });
        }
        Err(error) => failures.push(RefreshFailure {
            name,
            reason: error.to_string(),
            kind: None,
            help_url: None,
        }),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureInfo {
    pub group_name: String,
    pub reason: String,
}

/// 切换结果，前端据此出话。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchTierResult {
    pub provider_name: String,
    /// ChatGPT 退出前是不是在跑（决定切换后要不要替用户重开）。
    pub chatgpt_was_running: bool,
    /// 有没有重新打开它。
    pub chatgpt_relaunched: bool,
    /// 非致命的问题（如重开失败），如实带给用户。
    ///
    /// 「退不掉 ChatGPT」**不在这里** —— 那种情况整个命令返回 Err、配置不动，见
    /// [`switch_tier_impl`]。
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
// `rename_all` 只转变体名；变体字段要另加 `rename_all_fields`（serde 1.0.185+）。
// 漏掉它时 `target_name` 蛇形下发、前端读 `targetName` 得 undefined，
// 确认弹窗静默打不开 —— 闸在 tests::switch_tier_command_result_wire_contract。
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SwitchTierCommandResult {
    ConfirmationRequired {
        target_name: String,
    },
    Switched {
        #[serde(flatten)]
        result: SwitchTierResult,
    },
}

/// 读当前状态。
///
/// **只读本地**，不发网络请求 —— 这是首屏渲染要等的东西，不该卡在网络上。
/// 「凭据是不是真的还活着」由 [`relay_check_session`] 单独探，前端拿到本地状态先渲染，
/// 再让探活的结果去修正它。
#[tauri::command]
pub fn relay_status(state: State<'_, AppState>) -> Result<RelayStatus, String> {
    relay_status_impl(state.inner()).map_err(|e| e.to_string())
}

/// 探一遍**每一行**已登录的凭据是不是真的还能用，并清掉确认失效的那些**会话**。
///
/// 为什么需要这个：行 DTO 的 `logged_in` 只看本地记的过期时间。而凭据可能在网页端被
/// 撤销、账号被禁用、会话被踢掉 —— 那些情况下本地看起来一切正常，用户点任何操作才会
/// 撞到错误。第 2 次打开 app 到第 100 次都走这条路，不能共用第 1 次的假设。
///
/// ## 为什么是逐行而不是「探当前站」（2026-08-04 改）
///
/// 原来它探的是 `creds::load()` 那一行（全局 `is_current = 1`），返回一个 bool。
/// 那个形状只对「同时只有一个站」的旧界面成立 —— 中转站区是**多行并列**的，
/// 探一行的活等于让另外 N-1 行继续显示错的状态，而用户看不出区别。
///
/// 现在返回**这次被清掉会话的行 id**（空 = 全都还好）。前端据此提示并刷新。
///
/// ⚠️ **清的是会话，不是这一行的全部凭据**：分组与 sk 不受影响，用户点一次
/// 「重新登录」就复原（见 `creds::clear_session`）。
///
/// 未登录的行直接跳过：`usable_relay` 对它们必然 Err，白打一次请求还得过滤噪音。
#[tauri::command]
pub async fn relay_check_session(app_handle: tauri::AppHandle) -> Result<Vec<i64>, String> {
    check_session(&app_handle).await.map_err(|e| e.to_string())
}

async fn check_session(app_handle: &tauri::AppHandle) -> Result<Vec<i64>, AppError> {
    let targets: Vec<i64> = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, creds::list)?
            .into_iter()
            .filter(|op| !op.auth_token.is_empty())
            .map(|op| op.id)
            .collect()
    };

    let mut expired = Vec::new();
    // **串行而不是 join_all**：这些请求打的往往是同一个中转站（同一个 IP 段、
    // 同一份 rate limit），而这是启动时的后台探活、没人在等它返回。
    // 并发省下的几百毫秒换来的是撞限流的风险，不值得。
    for id in targets {
        // usable_relay 会在快过期时先续期、并顺手补齐缺失的账号身份（见它的文档）；
        // 拿 /user/profile 当探活请求（最便宜的鉴权端点）。撞上「登录已过期」类 401
        // 时先静默续期再重试一次 —— 那是 `token_expires_at = NULL` 的降级态行唯一
        // 的自救机会（详见 relay_read_with_refresh_retry 的文档）。
        let probe = relay_read_with_refresh_retry(app_handle, id, |op| async move {
            backend::RuntimeBackend::for_relay(&op).balance().await
        })
        .await;

        if let Err(e) = probe {
            // 「登录态已失效」是 api 层对不可恢复的那一类 401 的措辞（账号被禁 /
            // 会话被撤销 / 用户不存在）。这类清掉本地**会话**、让用户重新登录。
            //
            // ⚠️ **只清会话，不清账号身份**（`clear_session` 而不是 `clear_credentials`）——
            // 分组与 sk 写在各自的 provider 配置里，压根没失效；把 `account_id` 一起
            // 抹掉会让档位按归属过滤时被判成「不是这一行的」⇒ 整片从界面消失，
            // 用户以为密钥没了。完整的三连后果见 `creds::clear_session` 的文档。
            //
            // 其它失败（网络不通、中转站关了用户面板返 403）**连会话都不清** ——
            // 那不是凭据的问题，清掉只会逼用户在网络恢复后白重登一次。
            if should_clear_credentials_after_probe_error(&e) {
                let state = app_handle.state::<AppState>();
                with_conn(&state, |conn| creds::clear_session(conn, id))?;
                let msg = e.to_string();
                log::info!("中转站 {id} 登录态已失效，已清除会话（分组与密钥保留）：{msg}");
                expired.push(id);
            } else {
                let msg = e.to_string();
                log::warn!("中转站 {id} 探活失败但保留凭据（可能只是网络问题）：{msg}");
            }
        }
    }
    Ok(expired)
}

/// 本机还没有任何中转站 / 厂商账号 —— 即「新用户」这一事实的唯一判据。
///
/// `RelayStatus.should_prompt_add_site` 与新人引导（`commands::onboarding`）都读它：
/// 同一个业务事实只算一次，两处不会因为各写一份判据而分叉。
pub(crate) fn user_has_no_accounts(state: &AppState) -> Result<bool, AppError> {
    with_conn(state, |conn| {
        Ok(creds::list(conn)?.is_empty() && crate::vendor::creds::list(conn)?.is_empty())
    })
}

fn relay_status_impl(state: &AppState) -> Result<RelayStatus, AppError> {
    let should_prompt_add_site = user_has_no_accounts(state)?;
    Ok(RelayStatus {
        default_site: DEFAULT_SITE.to_string(),
        should_prompt_add_site,
    })
}

/// 匿名统计的上报端点配好了没。
///
/// ## 为什么前端需要这个事实
///
/// 首启告知弹窗（`StatsNoticeDialog`）在问用户「同不同意上传」。而端点还是占位
/// （`stats::ENDPOINT` 含 `.invalid`）时，**同意与不同意的实际后果完全相同** ——
/// 一个字节都不会发出去（`lib.rs` 那个上报任务第一道闸就是 `is_configured`）。
///
/// 那时弹这一屏是**向用户征求一个没有意义的同意**：它消耗用户对弹窗的信任，
/// 却换不到任何数据。所以前端拿这个值当弹窗的前置条件。
///
/// ⚠️ **有意不把它并进 [`RelayStatus`]**：那条命令是**首屏渲染要等的东西**
/// （它的文档为此删掉过一个有遍历开销的字段），而这个事实只有统计告知那一屏要用。
/// 单独一条命令让它不参与首屏的关键路径。
///
/// ⇒ **端点配好那天这里自动放行**，不需要有人记得回来撤掉什么开关 ——
/// 判据就是端点本身，不是一个另行维护的标记。
#[tauri::command]
pub fn relay_stats_endpoint_configured() -> bool {
    crate::relay::stats::is_configured()
}

/// 推荐中转站（首启屏那几个按钮）。
///
/// ## 为什么读缓存而不是现拉
///
/// 与 [`relay_login`] 里取 aff 码同一个理由：拉取由启动时那个后台任务做
/// （`lib.rs`，延迟 5 秒），这里只同步读一份磁盘文件（含重新验签）——
/// **不让用户对着一个转圈的弹窗等一次网络往返**。
///
/// ⇒ **首启第一次打开时这里通常是空的**（那 5 秒还没到，或者根本没网）。
/// 那不是错误：UI 拿到空数组就只显示手动输入框，与这个功能上线前的样子一致。
/// 下次启动就有了（缓存已落盘）。
///
/// 返回空数组的三种情形都正常：没网 / 还没拉到 / 维护者临时撤空了列表。
#[tauri::command]
pub fn relay_list_sponsors() -> Vec<crate::relay::remote_config::Sponsor> {
    // 不返 `Result` —— 拿不到推荐不是错误，是「今天没有推荐」。
    // 返 Err 会让前端不得不写一个 catch 去把错误咽掉，那是把非错误伪装成错误。
    crate::relay::remote_config::load_cached()
        .map(|cfg| cfg.sponsors)
        .unwrap_or_default()
}

#[tauri::command]
pub async fn relay_list_directory(
    app_handle: tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, String> {
    if let Some(cached) =
        crate::relay::leaderboard::read_cached(kind).map_err(|error| error.to_string())?
    {
        if !crate::relay::leaderboard::is_cache_fresh(kind, chrono::Utc::now().timestamp()) {
            tauri::async_runtime::spawn(async move {
                if let Err(error) = refresh_stale_directory_and_emit(&app_handle, kind).await {
                    log::warn!("background VeriDrop refresh for {kind:?} failed: {error}");
                }
            });
        }
        return Ok(cached);
    }

    refresh_stale_directory_and_emit(&app_handle, kind)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn relay_refresh_directory(
    app_handle: tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, String> {
    force_refresh_directory_and_emit(&app_handle, kind)
        .await
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayDirectoryUpdated {
    kind: crate::relay::leaderboard::LeaderboardKind,
}

async fn force_refresh_directory_and_emit(
    app_handle: &tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, AppError> {
    // 手动刷新按钮：榜单同步刷（返回值立刻要给 UI），transit 摘要异步刷
    // ——不能让几十个站的快照抓取把按钮卡住十几秒，刷完走事件广播。
    spawn_transit_refresh_and_emit(app_handle.clone());
    let outcome = crate::relay::leaderboard::refresh(kind).await?;
    emit_directory_update(app_handle, kind, outcome)
}

/// 异步刷一轮 transit 摘要，完成后广播全部榜单的更新事件。
///
/// maintenance 周期任务与手动刷新共用这一条：榜单与 transit 是两份数据、
/// 各刷各的；前端对 `relay-directory-updated` 的反应是重拉列表，届时
/// 读取路径会把新摘要合并进去（见 `leaderboard::decorate_transit`）。
pub(crate) fn spawn_transit_refresh_and_emit(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let config = crate::relay::remote_config::load_cached().unwrap_or_default();
        let hosts = crate::relay::leaderboard::managed_site_hosts(&config);
        if hosts.is_empty() {
            return;
        }
        crate::relay::transit::refresh_for_hosts(&hosts).await;
        emit_all_directory_updates(&app_handle);
    });
}

/// 广场数据在「命令层之外」被更新（transit 后台刷新）后的广播：
/// 4 个榜单各发一次同名事件，前端作废重拉。与 [`emit_directory_update`]
/// 共用事件契约，前端不区分来源。
pub(crate) fn emit_all_directory_updates(app_handle: &tauri::AppHandle) {
    for kind in crate::relay::leaderboard::LeaderboardKind::ALL {
        if let Err(error) = app_handle.emit(
            RELAY_DIRECTORY_UPDATED_EVENT,
            RelayDirectoryUpdated { kind },
        ) {
            log::warn!("发送广场更新事件失败（{kind:?}）: {error}");
        }
    }
}

async fn refresh_stale_directory_and_emit(
    app_handle: &tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, AppError> {
    let outcome = crate::relay::leaderboard::refresh_if_stale(kind).await?;
    emit_directory_update(app_handle, kind, outcome)
}

fn emit_directory_update(
    app_handle: &tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
    outcome: crate::relay::leaderboard::RefreshOutcome,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, AppError> {
    if outcome.updated {
        app_handle
            .emit(
                RELAY_DIRECTORY_UPDATED_EVENT,
                RelayDirectoryUpdated { kind },
            )
            .map_err(|error| AppError::Message(format!("发送 VeriDrop 更新事件失败: {error}")))?;
    }
    Ok(outcome.leaderboard)
}

pub(crate) async fn refresh_stale_directories(
    app_handle: tauri::AppHandle,
) -> Result<(), AppError> {
    use futures::StreamExt;

    let now = chrono::Utc::now().timestamp();
    let stale = crate::relay::leaderboard::LeaderboardKind::ALL
        .into_iter()
        .filter(|kind| !crate::relay::leaderboard::is_cache_fresh(*kind, now));
    let mut refreshes = futures::stream::iter(stale.map(|kind| {
        let app_handle = app_handle.clone();
        async move { refresh_stale_directory_and_emit(&app_handle, kind).await }
    }))
    .buffer_unordered(2);

    let mut failures = Vec::new();
    while let Some(result) = refreshes.next().await {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "部分 VeriDrop 榜单刷新失败: {}",
            failures.join("; ")
        )))
    }
}

/// 发现并导入一个第三方中转站。
///
/// 先走原生 HTTP fast path；未识别时不猜失败原因，也不把它宣判成某种站点，
/// 而是打开协议无关的可见 WebView。用户可自行完成任意网页验证；验证后的候选响应
/// 回到 Rust 严格识别，随后在**同一个 WebView 会话**继续注册/登录。
#[tauri::command]
pub async fn relay_import_site(
    app_handle: tauri::AppHandle,
    site: String,
) -> Result<ImportResult, RelayImportError> {
    import_site(&app_handle, &site, BrowserEntrySource::Manual, None).await
}

/// 从已验签的中转站目录导入。
///
/// 与手工输入分开成一个命令：目录策略可以声明 `/keys` 这类站点专属入口，
/// 但调用方不能靠传一个布尔值把任意业务路径升级成受信入口。这里重新读取并验证
/// 当前签名配置：完全匹配其中 HTTPS `entry_url` 的地址会保留 path/query/fragment；
/// 其余受管站点（sponsors / aff / promo —— **与广场曝光同一份名单**，见
/// `leaderboard::managed_site_hosts`）按手工输入的安全规则打开 origin 或协议登录页。
///
/// 两种拒绝（配置缓存缺失 / 站点不在受管名单）都报 `NotInDirectory` 而不是
/// `UnsupportedSite`：前者的正确指引是「换手动输入框添加」，后者的指引是
/// 「完成网页验证」——把不同指引压进同一个 kind，前端只能对一半场景说对话。
#[tauri::command]
pub async fn relay_import_directory_site(
    app_handle: tauri::AppHandle,
    site: String,
) -> Result<ImportResult, RelayImportError> {
    let config = crate::relay::remote_config::load_cached().ok_or_else(|| RelayImportError {
        kind: Some(RelayImportErrorKind::NotInDirectory),
        message: "该站点需要手动添加".into(),
    })?;
    let source = directory_entry_source(&config, &site).ok_or_else(|| RelayImportError {
        kind: Some(RelayImportErrorKind::NotInDirectory),
        message: "该站点需要手动添加".into(),
    })?;
    import_site(&app_handle, &site, source, None).await
}

// `pub(crate)`：新人引导（`commands::onboarding`）走同一条导入链路，只是入口
// 标记不同（见 `BrowserEntrySource::Onboarding`）。
//
// `promo_override`：这一窗**必给**的优惠码（star 领礼的奖励码，绕过码表 ——
// 码表是所有导入无条件预填的，奖励码要 gate 在 star 后面）。常规导入传 `None`。
pub(crate) async fn import_site(
    app_handle: &tauri::AppHandle,
    input: &str,
    entry_source: BrowserEntrySource,
    promo_override: Option<&str>,
) -> Result<ImportResult, RelayImportError> {
    let input = if input.trim().is_empty() {
        DEFAULT_SITE
    } else {
        input
    };
    let site_origin = api::normalize_site_origin(input).map_err(RelayImportError::from)?;

    let initial_detected = match discovery::probe_site(&site_origin).await {
        Ok(detected) => Some(detected),
        Err(error) => {
            let error = recoverable_native_discovery_error(error)?;
            // 这里只记录 fast path 没识别出来；不根据 HTTP 状态、验证产品或响应正文
            // 推断站点类型。可见 WebView 才是所有网页验证共用的下一步。
            log::info!(
                "原生站点发现未识别 {}，切换到浏览器辅助发现：{}",
                site_origin,
                error
            );
            None
        }
    };

    let requested_origin = site_origin.clone();
    let site_origin = import_anchor_origin(site_origin, initial_detected.as_ref());
    if site_origin != requested_origin {
        log::info!(
            "站点 {} 探针重定向到 {}，导入窗按最终 origin 锚定",
            requested_origin,
            site_origin
        );
    }

    let result = browser_import(
        app_handle,
        input,
        site_origin,
        initial_detected,
        entry_source,
        promo_override,
    )
    .await;

    // 首个站点接入成功 → 事后邀请「点 Star 领注册礼」（用户有了使用感觉再弹，
    // 见 `commands::onboarding`）。三道闸与一次性标志都在那边；这里只发信号，
    // 不挡导入返回 —— 邀约要拉网络，让用户先看到导入成功的界面。
    if result.is_ok() {
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            crate::commands::onboarding::offer_star_reward_after_first_import(&handle).await;
        });
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserEntrySource {
    Manual,
    SignedDirectory,
    /// 新人引导的官方站注册窗（见 [`crate::relay::onboarding`]）。落页与探测行为
    /// 同 `Manual`；差别只有窗口标题和额外注入的新人礼包横幅。
    Onboarding,
}

fn directory_entry_source(
    config: &crate::relay::remote_config::RemoteConfig,
    input: &str,
) -> Option<BrowserEntrySource> {
    let Ok(candidate) = browser_entry_url(input) else {
        return None;
    };
    // 第一段：签名目录条目享有专属入口（`/keys`、`/register` 这类受信 path），
    // 完全匹配才给 `SignedDirectory`。
    let signed = config
        .relay_directory
        .sites
        .iter()
        .find_map(|(host, site)| {
            if let Some(entry) = site.entry_url.as_deref() {
                if url::Url::parse(entry).is_ok_and(|declared| declared.scheme() == "https") {
                    return (browser_entry_url(entry).ok()? == candidate)
                        .then_some(BrowserEntrySource::SignedDirectory);
                }
            }

            (!host.is_empty() && browser_entry_url(host).ok()? == candidate)
                .then_some(BrowserEntrySource::Manual)
        });
    if signed.is_some() {
        return signed;
    }

    // 第二段：受管全集（sponsors / aff / promo 也算 —— **与广场曝光同一份名单**，
    // 见 `leaderboard::managed_site_hosts` 的唯源注释）按 Manual 保守规则回落。
    // 只认**裸 origin**：带 path 的输入不匹配，防止任意业务路径被当成受信入口。
    // wawapi.top 实测踩出的洞就在这：aff 名单让它进了广场，第一段却拒了它的接入。
    crate::relay::leaderboard::managed_site_hosts(config)
        .iter()
        .find_map(|host| {
            (browser_entry_url(host).ok()? == candidate).then_some(BrowserEntrySource::Manual)
        })
}

fn recoverable_native_discovery_error(
    error: discovery::DiscoveryError,
) -> Result<discovery::DiscoveryError, RelayImportError> {
    if error.kind == discovery::DiscoveryErrorKind::ProtocolConflict {
        Err(error.into())
    } else {
        Ok(error)
    }
}

/// 导入窗的锚定 origin：探针跟随重定向落在**别的 origin**（典型：裸域全路径 301
/// 到 `www.`）时，以最终落地 origin 为准。
///
/// `browser_import` 里所有同源约束都锚在这一个值上 —— 登录/注册入口、注入脚本的
/// origin 守卫、落库的 `site_origin`。用户输入的 origin 被站点 301 走时，页面实际
/// 停在最终 origin 上，锚若留在请求 origin，守卫（见 `login::login_script` 与
/// `discovery::browser_probe_script` 的 origin 早退）会静默吞掉全部回传：探针没有
/// 结果、凭据永远不来、导入干等到超时。探针没跑成（回退浏览器辅助发现）或没有
/// 重定向时，保持用户输入的 origin。
fn import_anchor_origin(requested: String, detected: Option<&discovery::DetectedSite>) -> String {
    detected
        .and_then(|site| site.final_origin.clone())
        .filter(|final_origin| *final_origin != requested)
        .unwrap_or(requested)
}

/// 生成浏览器首次打开的地址：站点 origin 与后端归一化规则一致，但保留用户给的
/// path/query/fragment（例如邀请链接 `/register?aff=...`）。
fn browser_entry_url(input: &str) -> Result<url::Url, AppError> {
    let input = if input.trim().is_empty() {
        DEFAULT_SITE
    } else {
        input.trim()
    };
    let site_origin = api::normalize_site_origin(input)?;
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let supplied = url::Url::parse(&with_scheme)
        .map_err(|e| AppError::InvalidInput(format!("域名格式不对: {e}")))?;
    let mut entry = url::Url::parse(&site_origin)
        .map_err(|e| AppError::InvalidInput(format!("域名格式不对: {e}")))?;
    entry.set_path(supplied.path());
    entry.set_query(supplied.query());
    entry.set_fragment(supplied.fragment());
    Ok(entry)
}

fn browser_entry_is_origin(url: &url::Url) -> bool {
    url.path() == "/" && url.query().is_none() && url.fragment().is_none()
}

fn browser_entry_is_auth_page(url: &url::Url) -> bool {
    let path = url.path().trim_end_matches('/');
    if matches!(path, "/login" | "/register") {
        return true;
    }
    url.fragment()
        .map(|fragment| fragment.trim_end_matches('/'))
        .is_some_and(|fragment| matches!(fragment, "/login" | "/register"))
}

/// 选择共用导入 WebView 的首次地址。
///
/// 登录/注册链接属于明确的可交互页面，保留其 path/query/fragment；其它业务/API 路径
/// 不能假定能在 WebView 中展示。协议未知时先打开 origin 让用户完成任意网页验证，识别后
/// 再由协议适配层导航到登录/注册页；协议已知时直接使用该协议入口。
fn browser_start_url(
    input: &str,
    site_origin: &str,
    detected: Option<&discovery::DetectedSite>,
    entry_source: BrowserEntrySource,
) -> Result<url::Url, AppError> {
    let entry = browser_entry_url(input)?;
    if entry_source == BrowserEntrySource::SignedDirectory || browser_entry_is_auth_page(&entry) {
        return Ok(entry);
    }

    let Some(detected) = detected else {
        return url::Url::parse(site_origin)
            .map_err(|error| AppError::InvalidInput(format!("站点 origin 地址不对: {error}")));
    };

    let url = backend::browser_login_url(site_origin, detected.backend_kind, "");
    url::Url::parse(&url)
        .map_err(|error| AppError::InvalidInput(format!("登录页地址不对: {error}")))
}

fn browser_login_context(
    site_origin: &str,
    detected: discovery::DetectedSite,
    aff_code: Option<&str>,
    promo_code: Option<&str>,
) -> BrowserLoginContext {
    let backend_kind = detected.backend_kind;
    let login_script =
        backend::browser_login_script(site_origin, backend_kind, "", aff_code, promo_code);
    let api_base_url = api::site_api_root(site_origin, &detected.api_base_url);
    let site_name = if detected.site_name.trim().is_empty() {
        site_origin
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    } else {
        detected.site_name
    };
    BrowserLoginContext {
        site: DiscoveredRelaySite {
            site_origin: site_origin.to_string(),
            site_name,
            api_base_url,
            backend_kind,
        },
        login_script,
    }
}

fn newapi_refresh_cookie_from_window(
    window: &tauri::WebviewWindow,
    refresh_url: &url::Url,
) -> Result<Option<String>, AppError> {
    // Tauri documents a Windows deadlock if cookies_for_url runs in a synchronous navigation
    // or window callback. This function is called only by the outer async select loops below.
    let cookies = window
        .cookies_for_url(refresh_url.clone())
        .map_err(|error| AppError::Config(format!("读取 NewAPI 登录会话失败: {error}")))?;
    Ok(newapi::extract_refresh_cookie(&cookies))
}

/// 从登录窗读 Cloudflare 放行 cookie。
///
/// 与相邻两个 NewAPI cookie 读取函数受同一条约束：`cookies_for_url` 只能在外层 async
/// 循环里调，不能在同步的导航/窗口回调里调（Tauri 记录了 Windows 上的死锁）。
///
/// **读不到不算错**：绝大多数站没开托管挑战，`None` 是正常结果，
/// 绝不能因此把一次成功的登录判失败。读 cookie 本身出错也只降级成 `None` ——
/// 凭据已经到手了，为一个可选的加速项让整次登录失败不划算。
fn cf_clearance_from_window(window: &tauri::WebviewWindow, site_origin: &str) -> Option<String> {
    let url = url::Url::parse(site_origin).ok()?;
    match window.cookies_for_url(url) {
        Ok(cookies) => login::extract_cf_clearance(&cookies),
        Err(error) => {
            log::warn!("读取 Cloudflare 放行 cookie 失败（不影响登录）: {error}");
            None
        }
    }
}

fn newapi_session_cookie_from_window(
    window: &tauri::WebviewWindow,
    session_url: &url::Url,
) -> Result<Option<String>, AppError> {
    let cookies = window
        .cookies_for_url(session_url.clone())
        .map_err(|error| AppError::Config(format!("读取 NewAPI 登录会话失败: {error}")))?;
    Ok(newapi::extract_session_cookie(&cookies))
}

/// 登录 / 导入流程里的 NewAPI 会话刷新（reqwest 直连，不经 WebView）。
///
/// ## 有意豁免充值窗口的 lease 闸（B 检查点①裁决）
///
/// [`usable_relay`] 的续期路径对持 lease 的 NewAPI relay 报「充值窗口正在使用」，
/// 防止后台续期把充值窗口里种着的 refresh cookie 轮换作废。本函数**不走**那条闸：
/// 它服务的是登录 / 导入流程 —— 用户主动重建会话的时刻，此时旧的充值窗口即使
/// 还开着也已被用户视作废弃，让登录拿到最新会话优先级更高。
///
/// 残余风险边界：重登后，旧充值窗口的 monitor 只持久化**它自己 cookie store 里**
/// 观察到的轮换；那个 incognito store 与新登录写入的库凭据从此各自演化，旧窗口的
/// 会话先失效属预期 —— 用户已经用「重新登录」表达了从头再来。
async fn refresh_newapi_browser_session(
    site_origin: &str,
    refresh_cookie: &str,
) -> Result<newapi::RefreshedSession, AppError> {
    newapi::refresh_session(site_origin, refresh_cookie, None).await
}

/// 解析这一窗登录脚本要预填的 (aff, promo) 码。
///
/// `promo_override` 是**这一窗专属**的奖励码（star 领礼），优先于码表 ——
/// 码表对所有导入无条件生效，而奖励码必须 gate 在 star 后面，两者是
/// 不同的 owner，谁也不吞谁。
fn resolve_login_codes(
    site_origin: &str,
    promo_override: Option<&str>,
) -> (Option<String>, Option<String>) {
    let cached_config = crate::relay::remote_config::load_cached();
    (
        crate::relay::remote_config::resolve_aff_code(cached_config.as_ref(), site_origin),
        promo_override.map(str::to_string).or_else(|| {
            crate::relay::remote_config::resolve_promo_code(cached_config.as_ref(), site_origin)
        }),
    )
}

/// 残留登录窗销毁后等 label 释放的上限。事件循环正常处理 `Destroyed` 只要几十毫秒；
/// 这个上限只兜「事件循环长时间不转」的病态情形，到点就照常重开。
const STALE_LOGIN_WINDOW_DESTROY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// 销毁残留的登录窗，并**等 label 真正释放**后才返回。
///
/// ## 为什么必须等（2026-08-16 用户日志实锤）
///
/// `destroy()` 是异步生效的：它只清运行时自己的窗口表，manager 那份注册表
/// （`get_webview_window` 与窗口重建共用）要等事件循环处理完 `Destroyed` 才清。
/// 不等就重建，`WebviewWindowBuilder::build` 会撞
/// `a webview with label 'loongport-login' already exists` —— 连续两次导入第二次
/// 必现，导入直接失败。
///
/// 轮询 `get_webview_window` 直到 `None`：那份注册表正是 label 冲突的判据，
/// 它清了，`build` 就一定能过。
///
/// 用 `destroy()` 而不是 `close()`：close 派发的是可被拦截的关闭**请求**
/// （`CloseRequested`，主窗口的最小化到托盘就是靠它拦的），拦下后 label 继续被占；
/// destroy 直接销毁、拦不住。
async fn destroy_stale_login_window<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>) {
    destroy_stale_login_window_with_timeout(app_handle, STALE_LOGIN_WINDOW_DESTROY_TIMEOUT).await
}

/// 参数化超时只为可测：MockRuntime 不驱动事件循环，label 永不释放
/// （见 newapi_purchase 超时用例里同款说明），生产超时下限走上面的常量。
async fn destroy_stale_login_window_with_timeout<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    timeout: std::time::Duration,
) {
    let Some(stale) = app_handle.get_webview_window(login::LOGIN_WINDOW_LABEL) else {
        return;
    };
    log::info!("发现残留的登录窗口，销毁后重开");
    let _ = stale.destroy();

    let deadline = std::time::Instant::now() + timeout;
    while app_handle
        .get_webview_window(login::LOGIN_WINDOW_LABEL)
        .is_some()
    {
        if std::time::Instant::now() >= deadline {
            log::warn!(
                "残留登录窗口 {} 销毁未在 {timeout:?} 内生效，继续重开（label 仍被占用时建窗会失败）",
                login::LOGIN_WINDOW_LABEL
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn browser_import(
    app_handle: &tauri::AppHandle,
    input: &str,
    site_origin: String,
    initial_detected: Option<discovery::DetectedSite>,
    entry_source: BrowserEntrySource,
    promo_override: Option<&str>,
) -> Result<ImportResult, RelayImportError> {
    // 上一次导入留下的窗口（NewAPI 凭据到手即关、sub2api 留窗都可能产生）：
    // 每次导入必须是全新的 incognito 会话，所以销毁重开而不是复用导航。
    destroy_stale_login_window(app_handle).await;

    let (login_aff_code, login_promo_code) = resolve_login_codes(&site_origin, promo_override);
    let entry_url =
        browser_start_url(input, &site_origin, initial_detected.as_ref(), entry_source)?;
    let navigate_after_detection =
        initial_detected.is_none() && browser_entry_is_origin(&entry_url);

    let initial_backend = initial_detected
        .as_ref()
        .map(|detected| format!("{:?}", detected.backend_kind));
    let initial_context = initial_detected.map(|detected| {
        browser_login_context(
            &site_origin,
            detected,
            login_aff_code.as_deref(),
            login_promo_code.as_deref(),
        )
    });

    // 在下面把 entry_source 遮蔽成诊断字符串之前先记下入口类型。
    let is_onboarding = entry_source == BrowserEntrySource::Onboarding;
    let entry_source = if is_onboarding {
        "onboarding"
    } else if browser_entry_is_auth_page(&entry_url) {
        "supplied_auth_page"
    } else if initial_backend.is_some() {
        "protocol_login_page"
    } else {
        "site_origin"
    };
    log::info!(
        "{}",
        crate::diagnostics::DiagnosticEvent::new("relay.browser_import", "window_opening")
            .field_display("site", crate::url_for_log(&site_origin))
            .field_display("entry", crate::url_for_log(entry_url.as_str()))
            .field_display("initial_backend", format_args!("{initial_backend:?}"))
            .field("entry_source", entry_source)
    );

    let context = Arc::new(Mutex::new(initial_context));
    let last_probe_summary = Arc::new(Mutex::new(None::<String>));
    let (creds_tx, mut creds_rx) = tokio::sync::mpsc::channel::<BrowserLoginCredential>(1);
    let (error_tx, mut error_rx) = tokio::sync::mpsc::channel::<RelayImportError>(1);
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::channel::<()>(1);

    let context_for_load = Arc::clone(&context);
    let app_for_nav = app_handle.clone();
    let context_for_nav = Arc::clone(&context);
    let last_probe_summary_for_nav = Arc::clone(&last_probe_summary);
    let site_origin_for_nav = site_origin.clone();
    let aff_for_nav = login_aff_code.clone();
    let promo_for_nav = login_promo_code.clone();
    let probe_error_tx = error_tx.clone();
    let credential_error_tx = error_tx.clone();

    // 新人引导的注册窗用自己的标题：新用户没有「添加中转站」这个上下文，
    // 标题要说清这个窗口为什么自己弹出来（见 relay::onboarding 的文档）。
    let window_title = if is_onboarding {
        crate::relay::onboarding::register_window_title()
    } else {
        format!("添加中转站 {site_origin}")
    };

    // 协议探针脚本所有入口都注入；新人引导额外带上新人礼包横幅（只在 /register
    // 显示，脚本自己管显隐 —— 见 relay::onboarding::register_gift_banner_js）。
    let mut init_script =
        discovery::browser_probe_script(&site_origin, discovery::PROBE_CANDIDATES);
    if is_onboarding {
        init_script.push_str(&crate::relay::onboarding::register_gift_banner_js());
    }

    let window = tauri::WebviewWindowBuilder::new(
        app_handle,
        login::LOGIN_WINDOW_LABEL,
        tauri::WebviewUrl::External(entry_url),
    )
    .title(window_title)
    .inner_size(480.0, 720.0)
    .resizable(true)
    // 一次导入只使用这一份纯内存会话：网页验证、协议探测、注册/登录都不换窗口，
    // 同时也不复用上一次导入的站点 cookie 或 token。
    .incognito(true)
    // ⭐ **放行 window.open / target=_blank 弹窗**（`NewWindowResponse::Allow`）。
    //
    // 不设这个 handler 时 wry 会**静默拒绝**所有新窗口请求（macOS 的
    // `createWebViewWith` 返回 nil、WebView2 `SetHandled(true)`），页面上就是
    // 「点了没反应」：弹窗式 OAuth（老版 new-api 系站点的 GitHub 登录就是这么
    // 发起的）和 sub2api 的支付弹窗全被吞掉。
    //
    // `Allow` 是浏览器保真语义：macOS 用 opener 的 WKWebViewConfiguration 建
    // 子 WKWebView —— 同一 dataStore（cookie/localStorage 共享，本窗口注入脚本
    // 的 setItem 劫持与轮询兜底照常接得住弹窗路径写入的登录态），
    // `window.opener` / `window.close()` 正常工作；Windows 走 WebView2 运行时
    // 默认弹窗（同 profile）。子窗口是裸 WebView：没有注入脚本、没有
    // `on_navigation` 拦截、没有 Tauri IPC，不新增攻击面。
    //
    // 代价（明说）：子窗口由 wry 管理、不在 Tauri 窗口表里，本窗口销毁时不会
    // 跟着关（用户手动关即可）—— 浏览器里多个标签页本来也是这个行为。
    .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Allow)
    // 所有导入都统一注入协议无关的候选抓取器。脚本不认识 Cloudflare、HTTP 403
    // 或任何其它验证产品；协议未知时，用户验证完成后它自然会在同源会话里读到候选响应。
    // fast path 已识别时，Rust context 已有值，重复探测回传会被忽略。
    .initialization_script(init_script)
    .on_page_load(move |webview, payload| {
        log::info!(
            "站点导入窗页面加载 {:?}：{}",
            payload.event(),
            crate::url_for_log(payload.url().as_str())
        );

        let login_script = context_for_load
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|ctx| ctx.login_script.clone()));
        if let Some(script) = login_script.filter(|script| !script.is_empty()) {
            if let Err(error) = webview.eval(&script) {
                log::warn!("站点登录脚本重注入失败: {error}");
            }
        }
    })
    .on_navigation(move |url| {
        if let Some(result) = discovery::parse_probe_navigation(url) {
            let batch = match result {
                Ok(batch) => batch,
                Err(error) => {
                    log::warn!(
                        "{}",
                        crate::diagnostics::DiagnosticEvent::new(
                            "relay.browser_probe.callback",
                            "parse_failed",
                        )
                        .field_display("site", crate::url_for_log(&site_origin_for_nav))
                        .field(
                            "error_chain",
                            crate::diagnostics::format_error_chain(&error),
                        )
                    );
                    let _ = probe_error_tx.try_send(error.into());
                    return false;
                }
            };

            // 同一候选正文可能在后续页面重复回传。识别成功一次后便以 Rust 侧状态为准，
            // 不重复存站点、不重复导航。
            if context_for_nav
                .lock()
                .map(|guard| guard.is_some())
                .unwrap_or(false)
            {
                return false;
            }

            let probe_summary = discovery::probe_batch_summary(&batch.responses);
            let probe_summary_changed = match last_probe_summary_for_nav.lock() {
                Ok(mut guard) => {
                    let changed = guard.as_deref() != Some(probe_summary.as_str());
                    *guard = Some(probe_summary.clone());
                    changed
                }
                Err(_) => true,
            };

            let detected = match discovery::converge_probe_responses(&batch.responses) {
                Ok(detected) => detected,
                Err(error) if error.kind == discovery::DiscoveryErrorKind::UnsupportedSite => {
                    if probe_summary_changed {
                        log::info!(
                            "{}",
                            crate::diagnostics::DiagnosticEvent::new(
                                "relay.browser_probe",
                                "unmatched",
                            )
                            .field_display("site", crate::url_for_log(&site_origin_for_nav))
                            .field("probe", probe_summary.clone())
                        );
                    }
                    // 页面可能仍在验证或跳转，继续在同一 WebView 会话中轮询。
                    return false;
                }
                Err(error) => {
                    log::warn!(
                        "{}",
                        crate::diagnostics::DiagnosticEvent::new(
                            "relay.browser_probe",
                            "conflict",
                        )
                        .field_display("site", crate::url_for_log(&site_origin_for_nav))
                        .field("probe", probe_summary.clone())
                        .field("error_chain", crate::diagnostics::format_error_chain(&error))
                    );
                    let _ = probe_error_tx.try_send(error.into());
                    return false;
                }
            };
            let browser_context = browser_login_context(
                &site_origin_for_nav,
                detected,
                aff_for_nav.as_deref(),
                promo_for_nav.as_deref(),
            );
            let backend_kind = browser_context.site.backend_kind;
            let login_script = browser_context.login_script.clone();
            match context_for_nav.lock() {
                Ok(mut guard) if guard.is_none() => *guard = Some(browser_context),
                Ok(_) => return false,
                Err(_) => {
                    let _ =
                        probe_error_tx.try_send(RelayImportError::message("站点导入状态不可用"));
                    return false;
                }
            }
            log::info!(
                "{}",
                crate::diagnostics::DiagnosticEvent::new("relay.browser_probe", "matched")
                    .field_display("site", crate::url_for_log(&site_origin_for_nav))
                    .field_display("backend", format_args!("{backend_kind:?}"))
                    .field("probe", probe_summary)
            );

            let Some(window) = app_for_nav.get_webview_window(login::LOGIN_WINDOW_LABEL) else {
                let _ = probe_error_tx.try_send(RelayImportError::message("站点导入窗口已关闭"));
                return false;
            };

            let (next_action, next_step) = if navigate_after_detection {
                let login_url = backend::browser_login_url(&site_origin_for_nav, backend_kind, "");
                let result = url::Url::parse(&login_url)
                    .map_err(|error| format!("登录页地址不对: {error}"))
                    .and_then(|url| window.navigate(url).map_err(|error| error.to_string()));
                ("navigate_login_page", result)
            } else if !login_script.is_empty() {
                (
                    "inject_login_script",
                    window
                        .eval(&login_script)
                        .map_err(|error| error.to_string()),
                )
            } else {
                ("await_page_login", Ok(()))
            };
            match next_step {
                Ok(()) => log::info!(
                    "{}",
                    crate::diagnostics::DiagnosticEvent::new(
                        "relay.browser_import.continue",
                        "completed",
                    )
                    .field_display("site", crate::url_for_log(&site_origin_for_nav))
                    .field_display("backend", format_args!("{backend_kind:?}"))
                    .field("action", next_action)
                ),
                Err(error) => {
                    log::warn!(
                        "{}",
                        crate::diagnostics::DiagnosticEvent::new(
                            "relay.browser_import.continue",
                            "failed",
                        )
                        .field_display("site", crate::url_for_log(&site_origin_for_nav))
                        .field_display("backend", format_args!("{backend_kind:?}"))
                        .field("action", next_action)
                        .field("error", error.clone())
                    );
                    let _ = probe_error_tx.try_send(RelayImportError::message(error));
                }
            }
            return false;
        }

        if let Some(result) = newapi::parse_session_navigation(url) {
            match result {
                Ok(user_id) => {
                    let _ = creds_tx.try_send(BrowserLoginCredential::NewApiUserId(user_id));
                }
                Err(error) => {
                    log::warn!("NewAPI 登录回传解析失败: {error}");
                    let _ = credential_error_tx.try_send(error.into());
                }
            }
            return false;
        }

        // 浏览器代拉 API 请求的回传（`loongport-creds://api-<id>`）。
        if app_for_nav
            .state::<AppState>()
            .browser_bridge
            .handle_navigation(url)
        {
            return false;
        }

        match login::parse_creds_navigation(url) {
            None => true,
            Some(Ok(credentials)) => {
                let _ = creds_tx.try_send(BrowserLoginCredential::Sub2Api(credentials));
                false
            }
            Some(Err(error)) => {
                log::warn!("凭据回传解析失败: {error}");
                let message = error.to_string();
                let _ = credential_error_tx.try_send(error.into());
                let _ = app_for_nav.emit("relay-login-error", message);
                false
            }
        }
    })
    .build()
    .inspect_err(|error| log::error!("站点导入窗口创建失败: {error}"))
    .map_err(|error| AppError::Config(format!("打开站点导入窗口失败: {error}")))?;

    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = closed_tx.try_send(());
        }
    });

    let refresh_url = newapi::refresh_url(&site_origin)?;
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS), async {
        let mut cookie_poll = tokio::time::interval(std::time::Duration::from_millis(500));
        let mut newapi_user_id = None;
        loop {
            tokio::select! {
                biased;
                _ = closed_rx.recv() => break BrowserLoginOutcome::Closed,
                credentials = creds_rx.recv() => match credentials {
                    Some(BrowserLoginCredential::Sub2Api(mut credentials)) => {
                        // 趁窗口还在，把 CF 放行 cookie 一并收走：登录之后所有 API 都走
                        // reqwest，而它过不了托管挑战，只能靠这个 cookie 放行。
                        credentials.cf_clearance = cf_clearance_from_window(&window, &site_origin);
                        break BrowserLoginOutcome::Sub2ApiCredentials(credentials)
                    }
                    Some(BrowserLoginCredential::NewApiUserId(user_id)) => {
                        newapi_user_id = Some(user_id);
                    }
                    None => break BrowserLoginOutcome::Closed,
                },
                error = error_rx.recv() => break error
                    .map(BrowserLoginOutcome::Error)
                    .unwrap_or(BrowserLoginOutcome::Closed),
                _ = cookie_poll.tick() => {
                    let is_newapi = context
                        .lock()
                        .ok()
                        .and_then(|guard| {
                            guard.as_ref().map(|context| context.site.backend_kind)
                        })
                        == Some(discovery::BackendKind::NewApi);
                    if !is_newapi {
                        continue;
                    }
                    let refresh_cookie = match newapi_refresh_cookie_from_window(&window, &refresh_url) {
                        Ok(Some(refresh_cookie)) => refresh_cookie,
                        Ok(None) => {
                            let Some(user_id) = newapi_user_id else { continue };
                            let session_url = match newapi::session_token_url(&site_origin) {
                                Ok(url) => url,
                                Err(error) => break BrowserLoginOutcome::Error(error.into()),
                            };
                            let session_cookie = match newapi_session_cookie_from_window(&window, &session_url) {
                                Ok(Some(cookie)) => cookie,
                                Ok(None) => continue,
                                Err(error) => break BrowserLoginOutcome::Error(error.into()),
                            };
                            match newapi::exchange_session(&site_origin, &session_cookie, user_id).await {
                                Ok(session) => break BrowserLoginOutcome::NewApiSession(session),
                                Err(error) => break BrowserLoginOutcome::Error(error.into()),
                            }
                        }
                        Err(error) => break BrowserLoginOutcome::Error(error.into()),
                    };
                    let interrupt = async {
                        tokio::select! {
                            biased;
                            _ = closed_rx.recv() => BrowserLoginOutcome::Closed,
                            error = error_rx.recv() => error
                                .map(BrowserLoginOutcome::Error)
                                .unwrap_or(BrowserLoginOutcome::Closed),
                        }
                    };
                    match await_refresh_preserving_rotation(
                        refresh_newapi_browser_session(&site_origin, &refresh_cookie),
                        interrupt,
                    )
                    .await
                    {
                        RefreshWait::Interrupted(outcome) => break outcome,
                        RefreshWait::Refreshed(Ok(session)) => {
                            break BrowserLoginOutcome::NewApiSession(session)
                        }
                        RefreshWait::Refreshed(Err(error)) => {
                            break BrowserLoginOutcome::Error(error.into())
                        }
                    }
                }
            }
        }
    })
    .await;

    match outcome {
        Ok(BrowserLoginOutcome::Sub2ApiCredentials(credentials)) => {
            let browser_context = context
                .lock()
                .map_err(|_| AppError::Config("站点导入状态不可用".into()))?
                .clone()
                .ok_or_else(|| AppError::Config("尚未识别出受支持的站点协议".into()))?;
            let account = resolve_login_account_identity(app_handle, &site_origin, &credentials)
                .await
                .map_err(|e| {
                    AppError::Config(format!("登录成功但读取账号信息失败：{e}。请重试登录。"))
                })?;
            let (final_relay_id, account_id) = persist_new_relay_login_credentials(
                app_handle,
                &browser_context.site,
                credentials,
                account,
            )
            .await?;

            // 先卸掉窗口的续期能力再贴提示条：从这一刻起 refresh lineage 的唯一
            // 持有者是本仓 DB（为什么必须卸见 `login::strip_refresh_keys_js`）。
            if let Err(e) = window.eval(login::strip_refresh_keys_js()) {
                log::warn!("卸掉登录窗续期能力失败（窗口可能已关）：{e}");
            }
            let _ = window.set_title(&format!("已连接 {site_origin} — 可关闭此窗口"));
            let _ = window.eval(login::CONNECTED_BANNER_JS);
            log::info!("浏览器辅助导入登录成功：{site_origin}（账号 id={account_id}）");
            Ok(ImportResult::authenticated(
                browser_context.site,
                final_relay_id,
            ))
        }
        Ok(BrowserLoginOutcome::NewApiSession(session)) => {
            let browser_context = context
                .lock()
                .map_err(|_| AppError::Config("站点导入状态不可用".into()))?
                .clone()
                .ok_or_else(|| AppError::Config("尚未识别出受支持的站点协议".into()))?;
            let state = app_handle.state::<AppState>();
            let (final_relay_id, account_id) =
                persist_new_relay_newapi_session(&state, &browser_context.site, &session)?;

            // ⚠️ **NewAPI 登录窗必须当场关掉**，与 sub2api「留着但卸掉续期能力」不同。
            //
            // NewAPI 的登录态是 HttpOnly refresh cookie（名字见 `relay::newapi`），JS 删不掉 ⇒
            // 没法像 sub2api 那样卸掉窗口的续期能力。而它的一次性轮换比 sub2api
            // 激进得多：access token 只有 15 分钟（new-api `auth_token.go` 的
            // `AccessTokenTTL`），页面每 15 分钟就会拿 cookie 续期一次；更糟的是
            // **登录流程本身已经用原生侧轮换过一轮**（`refresh_newapi_browser_session`
            // → 轮换后的新 cookie 落进 DB）⇒ 窗口 cookie store 里那颗此刻就是废票，
            // 页面下一次续期拿废票去换 ⇒ 服务端判 reuse ⇒ **整个会话族被撤销**，
            // DB 里那份也跟着死。窗口多活 15 分钟就是一颗定时炸弹。
            //
            // 代价（明说）：用户少了一个「在登录窗里逛面板」的入口 —— 但面板余额
            // 主界面就有、充值有专门的充值窗（`newapi_purchase` 会种 cookie），
            // 登录窗的浏览价值撑不过它会造成的破坏。
            let _ = window.destroy();
            log::info!(
                "浏览器辅助导入登录成功：{site_origin}（账号 id={account_id}，登录窗已随凭据交接关闭）"
            );
            Ok(ImportResult::authenticated(
                browser_context.site,
                final_relay_id,
            ))
        }
        Ok(BrowserLoginOutcome::Error(error)) => {
            let _ = window.destroy();
            Err(error)
        }
        Ok(BrowserLoginOutcome::Closed) => {
            let backend_kind = context
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|context| context.site.backend_kind));
            let probe_detail = last_probe_summary
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .unwrap_or_else(|| "未收到协议探针回传".into());
            log::info!(
                "{}",
                crate::diagnostics::DiagnosticEvent::new("relay.browser_import", "closed")
                    .field_display("site", crate::url_for_log(&site_origin))
                    .field_display("backend", format_args!("{backend_kind:?}"))
                    .field("probe", probe_detail)
            );
            Err(incomplete_new_site_import_error(
                IncompleteImportReason::Closed,
            ))
        }
        Err(_) => {
            let backend_kind = context
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|context| context.site.backend_kind));
            let probe_detail = last_probe_summary
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .unwrap_or_else(|| "未收到协议探针回传".into());
            log::warn!(
                "{}",
                crate::diagnostics::DiagnosticEvent::new("relay.browser_import", "timeout")
                    .field_display("site", crate::url_for_log(&site_origin))
                    .field_display("backend", format_args!("{backend_kind:?}"))
                    .field("probe", probe_detail)
                    .field("timeout_seconds", LOGIN_TIMEOUT_SECS)
            );
            let _ = window.destroy();
            Err(incomplete_new_site_import_error(
                IncompleteImportReason::TimedOut,
            ))
        }
    }
}

/// 开登录窗，等凭据回来。
///
/// 凭据由注入脚本经一次被拦下的自定义 scheme 跳转送回（见 [`login`]）。本命令在收到凭据、
/// 或用户关掉窗口、或超时之后返回。
///
/// `relay_id` 指定登录**哪一行**，**必填**。
///
/// 没有「回落到当前站」这条路：那要靠全局 `is_current` 定位，而界面是多行并列的
/// ⇒ 用户点第 3 行的「重新登录」可能给第 1 行登了录。新增站点则走
/// [`relay_import_site`]，只在注册或登录成功后创建完整账号行。
#[tauri::command]
pub async fn relay_login(
    app_handle: tauri::AppHandle,
    relay_id: i64,
    app: String,
) -> Result<Option<RefreshResult>, String> {
    let app_type = AppType::from_str(&app).map_err(|error| error.to_string())?;
    let login = do_login(&app_handle, relay_id)
        .await
        .map_err(|error| error.to_string())?;
    if !login.logged_in {
        return Ok(None);
    }
    Ok(Some(
        refresh_relay_result(&app_handle, relay_id, &app_type).await,
    ))
}

async fn do_login(app_handle: &tauri::AppHandle, target_id: i64) -> Result<LoginResult, AppError> {
    // 记下行 id —— 凭据要写回这一行，而 `save_credentials` 可能因为发现重复账号
    // 而把它合并到别的行去。
    // 顺带取出登录标识：重登时预填进登录框，用户只需补密码与人机验证。
    let op = load_validated_relay(app_handle, target_id).await?;
    let (relay_id, site_origin, login_identifier, backend_kind) =
        (op.id, op.site_origin, op.login_identifier, op.backend_kind);

    // 已经有一个登录窗时：**销毁它再开新的**，而不是聚焦了就早退。
    //
    // 「聚焦已有的」听起来更礼貌，但它会卡死：残留窗口可能是隐藏状态（被别处 hide 过、
    // 或某次 close 请求被拦下），而 `set_focus` 对不可见窗口是 no-op —— 用户点了登录什么
    // 都没发生，且因为 label 被占，再点多少次都一样，只能重启 app。
    //
    // 直接销毁重开则总能给用户一个可见的窗口。代价是「他正在填的表单没了」，但能走到这里
    // 说明上一轮的 `do_login` 已经返回（否则那边还持有窗口），也就是那个窗口已经没人在等它
    // 的凭据了 —— 留着它反而是个陷阱。
    destroy_stale_login_window(app_handle).await;

    // 邀请码走三层回落：**远端（上次拉到并缓存的）> 编译期内置**。
    // 在这里解析而不是在 `login_script` 里查表 —— 那样远端那层永远进不来。
    //
    // 读缓存而不是现拉：拉取由启动时那个后台任务做（见 `lib.rs`），
    // 这里只同步读一份磁盘文件（含重新验签），不让用户等一次网络往返。
    // 缓存不存在 / 验签不过 ⇒ `load_cached` 返回 None ⇒ 自动落到内置那层。
    // 重登没有奖励码 override —— 那是 star 领礼注册窗专属的入参。
    let (login_aff_code, login_promo_code) = resolve_login_codes(&site_origin, None);

    // 落哪个页面由「这一行登录过没有」决定：新加的站落 `/register`，重登落 `/login`。
    let url = url::Url::parse(&backend::browser_login_url(
        &site_origin,
        backend_kind,
        &login_identifier,
    ))
    .map_err(|e| AppError::Config(format!("登录页地址不对: {e}")))?;

    // ⚠️ **这条链路的日志是刻意加密的**（2026-08-04，用户实测白屏后加）。
    //
    // 在此之前 `login.rs` 与本函数**一条日志都没有**，于是「登录窗白屏」这个现象在日志上
    // 完全不可观测 —— 拿到用户的日志也只能看到应用启动，之后一片空白，根因只能靠猜。
    // 下面几条各自回答一个具体问题：要加载哪个页面 / 窗口建出来了吗 / 页面开始加载了吗 /
    // 加载完了吗 / 最后等到了什么。少任何一条都会让某一类白屏无法定位。
    log::info!(
        "打开登录窗：{}（重登={}，邀请码={}，优惠码={}）",
        url,
        !login_identifier.is_empty(),
        login_aff_code.is_some(),
        login_promo_code.is_some()
    );

    // 凭据经这个 channel 从导航回调回到本函数。容量 1：只需要第一份。
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BrowserLoginCredential>(1);
    // 用户自己关掉窗口的信号。没有它就只能干等 5 分钟超时。
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::channel::<()>(1);

    let handle_for_nav = app_handle.clone();
    let initialization_script = backend::browser_login_script(
        &site_origin,
        backend_kind,
        &login_identifier,
        login_aff_code.as_deref(),
        login_promo_code.as_deref(),
    );
    let backend_kind_for_nav = backend_kind;
    let window = tauri::WebviewWindowBuilder::new(
        app_handle,
        login::LOGIN_WINDOW_LABEL,
        tauri::WebviewUrl::External(url),
    )
    .title(format!("登录 {site_origin}"))
    .inner_size(480.0, 720.0)
    .resizable(true)
    // ⚠️ **每次登录都必须是全新的登录态**（2026-08-03 加，用户实测发现）。
    //
    // 不加这个的后果：Tauri 的 WebView 默认与整个 app 共享一份**持久化** profile
    // （macOS 在 `~/Library/WebKit/<bundle-id>/`，cookie 与 localStorage 都在里面，
    // 跨窗口、跨重启都还在）。于是：
    //
    // 1. 用户删掉某个中转站 —— `creds::remove` 是真 DELETE，本地记录确实没了
    // 2. 重新添加同一个站，开登录窗
    // 3. **那个站的 localStorage 里旧 token 还在**（我们从没清过）⇒ sub2api 的 SPA
    //    认出「已登录」直接跳 dashboard，压根不显示登录表单
    // 4. `login_script` 的轮询兜底（本来是为「用户已登录状态打开页面」设计的）
    //    把那把旧 token 捞出来回传 ⇒ 看起来像「删除是假删除」
    //
    // 真正的后果比「看起来没删掉」严重两层：
    // - **同一个站永远只能挂第一个登录过的账号** —— 想加第二个账号根本加不进来，
    //   而「同站多账号」是这个功能的核心能力（`Relay` 的去重认的是服务端 account_id，
    //   正是为了支持它）
    // - **隐私问题**：用户以为删掉了中转站，那个站的登录 cookie 还留在本机
    //
    // `incognito(true)` 在 macOS 上映射成 `WKWebsiteDataStore::nonPersistentDataStore`
    // （wry 0.55 `wkwebview/mod.rs`），Windows/Linux 上 wry 也各有实现 ——
    // 一份纯内存存储，窗口关掉就没了，也读不到 app 那份持久 profile。
    //
    // 为什么不用 `clear_all_browsing_data()`：它清的是**全部站点**的数据（
    // wry 那边是 `removeDataOfTypes_modifiedSince` 传 1970 年），会把用户在别的
    // 中转站、以及 app 内其它 WebView 的登录态一起冲掉；而且它是异步的，
    // 没有完成回调可等 ⇒ 存在「还没清完页面就加载了」的竞态。
    .incognito(true)
    // 放行 window.open 弹窗：重登同样会遇到弹窗式 OAuth（GitHub/Google 登录），
    // 理由与 `browser_import` 那段逐条相同（子窗口共享会话与 opener 语义）。
    .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Allow)
    // sub2api 的 localStorage 回传脚本只注入到 sub2api 窗口。NewAPI 的 HttpOnly
    // refresh cookie 由外层 async 循环原生读取，绝不交给 JavaScript。
    .initialization_script(initialization_script)
    // ⭐ **白屏的关键判据就在这两个事件上**：
    //
    // - 两条都没有 ⇒ WebView 压根没开始加载（创建失败 / URL 不可达 / 被拦）
    // - 只有 `Started` 没有 `Finished` ⇒ 卡在加载中（网络慢、资源拉不下来）
    // - 两条都有但仍白屏 ⇒ 页面加载完了而 JS 没渲染出来（SPA 报错 / 脚本被 CSP 拦）
    //
    // 三种情况的修法完全不同，而肉眼看到的都是「一个白窗」—— 所以这两行不是可选的调试
    // 输出，是这个功能唯一的诊断入口。
    .on_page_load(|webview, payload| {
        log::info!(
            "登录窗页面加载 {:?}：{}",
            payload.event(),
            crate::url_for_log(payload.url().as_str())
        );
        let _ = webview;
    })
    .on_navigation(move |url| {
        if backend_kind_for_nav == discovery::BackendKind::NewApi {
            if let Some(result) = newapi::parse_session_navigation(url) {
                match result {
                    Ok(user_id) => {
                        let _ = tx.try_send(BrowserLoginCredential::NewApiUserId(user_id));
                    }
                    Err(error) => log::warn!("NewAPI 登录回传解析失败: {error}"),
                }
                return false;
            }
        } else if backend_kind_for_nav != discovery::BackendKind::Sub2Api {
            return true;
        }
        // 浏览器代拉 API 请求的回传（`loongport-creds://api-<id>`）。
        if handle_for_nav
            .state::<AppState>()
            .browser_bridge
            .handle_navigation(url)
        {
            return false;
        }
        match login::parse_creds_navigation(url) {
            // 普通导航，放行。
            None => true,
            Some(Ok(creds)) => {
                // 用 try_send：这个回调不能 await，而我们只要第一份凭据，
                // 满了就说明已经收到过了。
                let _ = tx.try_send(BrowserLoginCredential::Sub2Api(creds));
                false
            }
            Some(Err(e)) => {
                log::warn!("凭据回传解析失败: {e}");
                let _ = handle_for_nav.emit("relay-login-error", e.to_string());
                false
            }
        }
    })
    .build()
    .inspect_err(|e| log::error!("登录窗口创建失败: {e}"))
    .map_err(|e| AppError::Config(format!("打开登录窗口失败: {e}")))?;
    log::info!("登录窗口已创建，等待凭据回传（最多 {LOGIN_TIMEOUT_SECS} 秒）");

    // 用户关窗时立刻收工，不用等满超时。
    //
    // 只认 `Destroyed`（窗口真的没了）而不是 `CloseRequested`（可被拦下的关闭请求）——
    // 后者在某些平台上会先于实际销毁触发，甚至可能被取消。
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = closed_tx.try_send(());
        }
    });

    let refresh_url = newapi::refresh_url(&site_origin)?;
    // 等 sub2api 凭据、NewAPI HttpOnly refresh cookie 或用户关窗。5 分钟够走完注册 +
    // 邮箱验证 + 2FA；超时不是错误，用户可能就是走开了。
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS), async {
        let mut cookie_poll = tokio::time::interval(std::time::Duration::from_millis(500));
        let mut newapi_user_id = None;
        loop {
            tokio::select! {
                biased;
                _ = closed_rx.recv() => break BrowserLoginOutcome::Closed,
                creds = rx.recv() => match creds {
                    Some(BrowserLoginCredential::Sub2Api(mut credentials)) => {
                        // 趁窗口还在，把 CF 放行 cookie 一并收走：登录之后所有 API 都走
                        // reqwest，而它过不了托管挑战，只能靠这个 cookie 放行。
                        credentials.cf_clearance = cf_clearance_from_window(&window, &site_origin);
                        break BrowserLoginOutcome::Sub2ApiCredentials(credentials)
                    }
                    Some(BrowserLoginCredential::NewApiUserId(user_id)) => {
                        newapi_user_id = Some(user_id);
                    }
                    None => break BrowserLoginOutcome::Closed,
                },
                _ = cookie_poll.tick(), if backend_kind == discovery::BackendKind::NewApi => {
                    let refresh_cookie = match newapi_refresh_cookie_from_window(&window, &refresh_url) {
                        Ok(Some(refresh_cookie)) => refresh_cookie,
                        Ok(None) => {
                            let Some(user_id) = newapi_user_id else { continue };
                            let session_url = match newapi::session_token_url(&site_origin) {
                                Ok(url) => url,
                                Err(error) => break BrowserLoginOutcome::Error(error.into()),
                            };
                            let session_cookie = match newapi_session_cookie_from_window(&window, &session_url) {
                                Ok(Some(cookie)) => cookie,
                                Ok(None) => continue,
                                Err(error) => break BrowserLoginOutcome::Error(error.into()),
                            };
                            match newapi::exchange_session(&site_origin, &session_cookie, user_id).await {
                                Ok(session) => break BrowserLoginOutcome::NewApiSession(session),
                                Err(error) => break BrowserLoginOutcome::Error(error.into()),
                            }
                        }
                        Err(error) => break BrowserLoginOutcome::Error(error.into()),
                    };
                    match await_refresh_preserving_rotation(
                        refresh_newapi_browser_session(&site_origin, &refresh_cookie),
                        async {
                            let _ = closed_rx.recv().await;
                            BrowserLoginOutcome::Closed
                        },
                    )
                    .await
                    {
                        RefreshWait::Interrupted(outcome) => break outcome,
                        RefreshWait::Refreshed(Ok(session)) => {
                            break BrowserLoginOutcome::NewApiSession(session)
                        }
                        RefreshWait::Refreshed(Err(error)) => {
                            break BrowserLoginOutcome::Error(error.into())
                        }
                    }
                }
            }
        }
    })
    .await;

    match outcome {
        Ok(BrowserLoginOutcome::Sub2ApiCredentials(c)) => {
            let account = resolve_login_account_identity(app_handle, &site_origin, &c)
                .await
                .map_err(|e| {
                    AppError::Config(format!("登录成功但读取账号信息失败：{e}。请重试登录。"))
                })?;
            let (_final_relay_id, account_id) =
                persist_login_credentials(app_handle, relay_id, c, account).await?;

            // **不关窗**，但先把窗口的续期能力卸掉，再把标题改成「已连接」并贴提示条。
            //
            // 为什么不关：用户拿到凭据的那一刻，页面往往刚跳到 dashboard（sub2api 登录成功后
            // `router.push(redirectTo)`，注册成功后 `push('/dashboard')`）—— 那上面有余额、
            // 充值入口、渠道状态，都是他接着要用的东西。我们把窗口关掉等于替他决定「你看完了」。
            //
            // 更糟的一种：用户之前登录过，`/login` 的路由守卫会把他直接重定向到 dashboard，
            // 而注入脚本的轮询会在几百毫秒内拿到已有 token —— 窗口开了就关，用户一眼都没看到。
            //
            // 为什么又必须先卸续期能力：留着窗 + 留着 `refresh_token`，站点页面到点的
            // 自动续期会把一次性 refresh token 轮换进**这个 incognito 窗口的内存里**
            // （关窗即失），本仓 DB 里那把当场作废 ⇒ 用户被迫重登。删除两个续期键后
            // 窗口只剩只读登录态（判据与源码依据见 `login::strip_refresh_keys_js`），
            // 浏览器代拉不受影响（它重放的是我们自己的请求头）。
            if let Err(e) = window.eval(login::strip_refresh_keys_js()) {
                log::warn!("卸掉登录窗续期能力失败（窗口可能已关）：{e}");
            }
            let _ = window.set_title(&format!("已连接 {site_origin} — 可关闭此窗口"));
            let _ = window.eval(login::CONNECTED_BANNER_JS);

            log::info!("登录成功：{site_origin}（账号 id={account_id}）");
            Ok(LoginResult { logged_in: true })
        }
        Ok(BrowserLoginOutcome::NewApiSession(session)) => {
            let state = app_handle.state::<AppState>();
            let (_final_relay_id, account_id) =
                persist_newapi_login_session(&state, relay_id, &session)?;

            // ⚠️ **NewAPI 登录窗必须当场关掉**（理由全文见 `browser_import` 里同型分支）：
            // HttpOnly cookie 卸不掉续期能力、access token 只有 15 分钟、登录流程
            // 本身已轮换过一轮 ⇒ 窗口里那颗 cookie 已是废票，页面下一次续期
            // 会被服务端判 reuse、撤销整个会话族。
            let _ = window.destroy();

            log::info!("登录成功：{site_origin}（账号 id={account_id}，登录窗已随凭据交接关闭）");
            Ok(LoginResult { logged_in: true })
        }
        Ok(BrowserLoginOutcome::Error(error)) => {
            let _ = window.destroy();
            Err(AppError::Config(error.message))
        }
        // 用户关掉了窗口，或超时。都不是错误。
        //
        // 用 `destroy()` 而不是 `close()`：后者派的是可被拦截的关闭**请求**，会经过
        // `lib.rs` 里那个全局 `CloseRequested` 回调 —— 一旦将来有人放宽那道 label 守卫，
        // `close()` 就会被 `prevent_close` 吃掉，留下一个隐藏但仍占着 label 的僵尸窗口，
        // 而它会让下一次 `relay_login` 命中上面「已开着就聚焦」的早退，登录卡死。
        // `destroy()` 直接销毁、不发事件、拦不住。
        //
        // 超时那条也走这里：用户走开了，留一个卡在登录页的窗口没有意义。
        //
        // ⚠️ **两条分支的日志必须分开**（用户实测白屏后加）：「用户自己关的」与「等满超时」
        // 在界面上都表现为「窗口没了、什么也没发生」，但对我们是两件完全不同的事 ——
        // 前者是正常收场，后者说明**凭据回传这条链路断了**（页面没渲染 / 脚本没注入 /
        // 用户卡在人机验证）。合成一条日志就等于放弃了区分它们的唯一手段。
        Ok(BrowserLoginOutcome::Closed) => {
            log::info!("用户关闭了登录窗口（未完成登录）：{site_origin}");
            let _ = window.destroy();
            Ok(LoginResult { logged_in: false })
        }
        Err(_) => {
            log::warn!(
                "登录等待超时（{LOGIN_TIMEOUT_SECS} 秒内没收到凭据）：{site_origin} —— \
                 若用户当时看到的是白屏，对照上面 `登录窗页面加载` 那几行判断是哪一类"
            );
            let _ = window.destroy();
            Ok(LoginResult { logged_in: false })
        }
    }
}

/// 登录成功后取账号身份（去重键 + 展示名 + 登录标识的来源）。
///
/// 先走 reqwest fast path —— 绝大多数站这么拿就好。当站点启用了 Cloudflare 这类
/// **指纹级**防护（reqwest 这种非浏览器 HTTP 栈必撞 403 HTML，README 里 `api.aijws.com`
/// 就是实例），[`api::Client::send`] 内部会走浏览器代拉钩子：由仍开着的登录窗在
/// **页面上下文**里同源重放同一份请求。登录窗本身就是真实浏览器，是唯一能过这种
/// 防护的通道。判据是「HTTP 403 + 正文不是 JSON」—— sub2api 的 API 出错
/// （403 权限类）回的是 JSON 信封，正文非 JSON 说明根本不是 API 在说话。
async fn resolve_login_account_identity(
    app_handle: &tauri::AppHandle,
    site_origin: &str,
    credentials: &login::Credentials,
) -> Result<api::Account, AppError> {
    api::Client::new(
        site_origin,
        &credentials.auth_token,
        None,
        credentials.user_agent.as_deref(),
        credentials.cf_clearance.as_deref(),
    )?
    .with_browser_fallback(browser_api_fallback(app_handle))
    .account()
    .await
}

/// 构造浏览器代拉钩子：被防护层拦下的请求由登录窗在页面上下文里原样重放。
///
/// [`api::Client::send`] 撞上「403 + 正文非 JSON」时调用（见那边的说明），把**同一份
/// 请求**递进来。这里取当前登录窗（`loongport-login`；sub2api 登录成功后**留着**、
/// 但已卸掉续期能力）、把请求注入页面 fetch，经 `loongport-creds://api-<id>` 回传
/// （[`browser_bridge`] 按 id 认领）。窗口不在（用户关了，或 NewAPI 登录窗在凭据
/// 交接时已自动关闭——它的 HttpOnly cookie 卸不掉续期能力，留着必炸 lineage）时
/// 返回可读错误 —— 这类站只能靠真实浏览器过防护。
fn browser_api_fallback(app_handle: &tauri::AppHandle) -> api::BrowserApiFallback {
    let handle = app_handle.clone();
    Arc::new(move |request: reqwest::Request| {
        let handle = handle.clone();
        Box::pin(async move {
            let bridge = handle.state::<AppState>().browser_bridge.clone();
            let Some(window) = handle.get_webview_window(login::LOGIN_WINDOW_LABEL) else {
                return Err(AppError::Config(
                    "站点开启了浏览器指纹级防护，直连请求被拦，且登录窗口已关闭——请重新登录后重试"
                        .into(),
                ));
            };

            let (tx, rx) = tokio::sync::oneshot::channel();
            let req_id = bridge.register(tx);
            let script = browser_bridge::api_fetch_script(&request, &req_id);
            if let Err(error) = window.eval(&script) {
                bridge.forget(&req_id);
                return Err(AppError::Config(format!(
                    "浏览器代拉脚本注入失败（登录窗口不可用）: {error}"
                )));
            }

            // 给页面上的 fetch + 回传留出时间。窗口可能在凭据到手后被用户关掉，那时
            // 回传永远不来 —— 靠超时收场而不是干等，把「关窗」与「回传真的到了」分开。
            match tokio::time::timeout(
                std::time::Duration::from_secs(browser_bridge::FETCH_TIMEOUT_SECS),
                rx,
            )
            .await
            {
                Ok(Ok(Ok(response))) => Ok(response),
                Ok(Ok(Err(message))) => Err(AppError::Config(format!("浏览器代拉失败: {message}"))),
                Ok(Err(_)) => Err(AppError::Config("浏览器代拉回传通道已关闭".into())),
                Err(_) => Err(AppError::Config(format!(
                    "浏览器代拉等待回传超时（{} 秒）",
                    browser_bridge::FETCH_TIMEOUT_SECS
                ))),
            }
        })
    })
}

async fn persist_login_credentials(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
    credentials: login::Credentials,
    account: api::Account,
) -> Result<(i64, i64), AppError> {
    // 账号身份由调用方先取好（`resolve_login_account_identity`）：去重键是
    // 「域名 + 账号」，而账号只有登录后才知道；取不到账号 = 登录不能算成功。
    let account_id = account.id;

    let state = app_handle.state::<AppState>();
    let final_relay_id = with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            relay_id,
            creds::AccountIdentity {
                id: account.id,
                label: &account.display_name(),
                // 昵称与登录标识不是同一个事实；sub2api 登录框需要邮箱。
                login_identifier: &account.email,
            },
            &credentials.auth_token,
            credentials.refresh_token.as_deref(),
            credentials.token_expires_at,
            creds::SessionEnvironment {
                user_agent: credentials.user_agent.as_deref(),
                cf_clearance: credentials.cf_clearance.as_deref(),
            },
        )
    })?;

    Ok((final_relay_id, account_id))
}

async fn persist_new_relay_login_credentials(
    app_handle: &tauri::AppHandle,
    site: &DiscoveredRelaySite,
    credentials: login::Credentials,
    account: api::Account,
) -> Result<(i64, i64), AppError> {
    let account_id = account.id;
    let state = app_handle.state::<AppState>();
    let account_label = account.display_name();
    let final_relay_id = with_conn(&state, |conn| {
        creds::save_authenticated_relay(
            conn,
            creds::AuthenticatedRelay {
                site: creds::RelaySite {
                    site_origin: &site.site_origin,
                    site_name: &site.site_name,
                    api_base_url: &site.api_base_url,
                    backend_kind: site.backend_kind,
                },
                account: creds::AccountIdentity {
                    id: account.id,
                    label: &account_label,
                    login_identifier: &account.email,
                },
                auth_token: &credentials.auth_token,
                refresh_token: credentials.refresh_token.as_deref(),
                token_expires_at: credentials.token_expires_at,
                session: creds::SessionEnvironment {
                    user_agent: credentials.user_agent.as_deref(),
                    cf_clearance: credentials.cf_clearance.as_deref(),
                },
            },
        )
    })?;

    Ok((final_relay_id, account_id))
}

fn persist_newapi_login_session(
    state: &AppState,
    relay_id: i64,
    refreshed: &newapi::RefreshedSession,
) -> Result<(i64, i64), AppError> {
    let account = backend::newapi_runtime_account(&refreshed.account);
    let final_relay_id = with_conn(state, |conn| {
        creds::save_credentials(
            conn,
            relay_id,
            runtime_account_identity(&account),
            &refreshed.access_token,
            (!refreshed.refresh_cookie.trim().is_empty())
                .then_some(refreshed.refresh_cookie.as_str()),
            refreshed.access_expires_at,
            // NewAPI 登录不走 sub2api 那条 WebView 回传，两个字段都没有可写的值。
            creds::SessionEnvironment::default(),
        )
    })?;

    Ok((final_relay_id, account.id))
}

fn persist_new_relay_newapi_session(
    state: &AppState,
    site: &DiscoveredRelaySite,
    refreshed: &newapi::RefreshedSession,
) -> Result<(i64, i64), AppError> {
    let account = backend::newapi_runtime_account(&refreshed.account);
    let final_relay_id = with_conn(state, |conn| {
        creds::save_authenticated_relay(
            conn,
            creds::AuthenticatedRelay {
                site: creds::RelaySite {
                    site_origin: &site.site_origin,
                    site_name: &site.site_name,
                    api_base_url: &site.api_base_url,
                    backend_kind: site.backend_kind,
                },
                account: runtime_account_identity(&account),
                auth_token: &refreshed.access_token,
                refresh_token: (!refreshed.refresh_cookie.trim().is_empty())
                    .then_some(refreshed.refresh_cookie.as_str()),
                token_expires_at: refreshed.access_expires_at,
                session: creds::SessionEnvironment::default(),
            },
        )
    })?;

    Ok((final_relay_id, account.id))
}

/// 取一份**能用**的凭据：token 快过期时先静默续期。
///
/// 没有这一步的话，token 一过期用户就得重新走一遍 WebView 登录 —— 而 sub2api 的
/// `/auth/login` 有 20 次/分钟的限流，反复登录会把自己锁在外面。
///
/// ## `relay_id` 是必填的：**没有「回落到当前站」这条路**
///
/// 界面是多行并列的，「当前站」这个概念在这里不成立 —— 靠它定位会让
/// 「给 A 获取密钥」静默作用到 B 上（那是 review 抓出过的真实并发正确性问题，
/// 见 [`relay_provision`] 的文档）。2026-08-04 连带 `is_current` 一起删掉了
/// 那条 `Option` 分支。
async fn usable_relay<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
) -> Result<creds::Relay, AppError> {
    let op = load_validated_relay(app_handle, relay_id).await?;

    if op.token_looks_valid(chrono::Utc::now().timestamp()) {
        // ⭐ **token 够用，但账号身份可能缺** —— 补一次再返回。
        //
        // 「有 `auth_token` 却没 `account_id`」是个实测到的死局：
        // [`creds::Relay::token_looks_valid`] 对 `token_expires_at = NULL` 返回
        // `true`（有意的乐观降级）⇒ 这里直接早退 ⇒ 永远走不到下面那条**续期后打
        // profile** 的路径，而那原本是唯一拿得到 `account.id` 的地方。
        // 于是用户点任何刷新（provision / 余额 / 充值都经过本函数）都补不上。
        //
        // 后果不止少个字段：`account_id` 为空 ⇒ `save_credentials` 的去重查不到它
        // ⇒ 同一个账号重新登录会**新建一行**而不是合并，站点列表里堆重复。
        //
        // 放在这里而不是各调用点：本函数是 provision / balance / purchase /
        // check_session 的**必经点**，补一处就全覆盖。
        if op.account_id.is_none() {
            return Ok(backfill_account_identity(app_handle, op).await);
        }
        return Ok(op);
    }

    let (renewed, refreshed) = refresh_relay_session(app_handle, &op).await?;

    // 顺手刷一次账号身份：用户可能在中转站那边改了昵称或邮箱，而续期响应里没有账号信息
    // （`/auth/refresh` 只回 token），所以只有在这里额外打一次 profile 才发现得了。
    // 不刷的话站点选择器上会一直挂着旧标签 —— 而他改邮箱的动机往往就是「换个能认的」。
    if refreshed.account.is_some() {
        return Ok(renewed);
    }

    Ok(backfill_account_identity(app_handle, renewed).await)
}

/// 续期一次并落库（充值窗口独占闸 → `refresh_session` → 持久化）。
///
/// 从 [`usable_relay`] 的尾部提出来的公共路径：主动续期（token 已知过期）与
/// [`relay_read_with_refresh_retry`] 的被动续期（撞上 401 才发现过期）走的是
/// 同一段代码 —— 两处各写一遍，「充值窗口独占」那道闸迟早只挡住一边。
///
/// 返回 `(落库后的 Relay, 续期响应)`：调用方有的只要新凭据，有的还要看响应里
/// 有没有账号信息（NewAPI 回、sub2api 不回）来决定要不要补打一次 profile。
async fn refresh_relay_session<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    op: &creds::Relay,
) -> Result<(creds::Relay, backend::RefreshedSession), AppError> {
    let state = app_handle.state::<AppState>();
    // ⭐ 充值窗口持有这个 NewAPI 账号的 refresh 轮换独占权时，后台续期不得抢跑：
    // NewAPI 的 refresh cookie 一次性轮换，这里并发续期会把充值窗口里那颗 cookie
    // 立刻作废（用户充值到一半被踢回登录页）。闸放在 `token_looks_valid` 早退**之后**：
    // token 仍然有效时根本不走续期，不受影响；sub2api 的续期也不受影响。
    if op.backend_kind == creds::BackendKind::NewApi && state.purchase_sessions.is_active(op.id) {
        return Err(AppError::Config(
            "充值窗口正在使用这个账号的登录态，请关闭充值窗口后重试".into(),
        ));
    }
    let refreshed = backend::RuntimeBackend::for_relay(op)
        .refresh_session(op.refresh_token.as_deref())
        .await?;
    let renewed = persist_refreshed_session(&state, op, &refreshed)?;
    Ok((renewed, refreshed))
}

/// 跑一次**只读**的站点请求；撞上「登录已过期」类 401 且手里还有 refresh token 时，
/// 先静默续期一次、再用新凭据重跑原请求 —— 续期也救不回来才把**原错误**交出去。
///
/// ## 为什么必须有它（2026-08-17 bestapi.store 线上事故）
///
/// `token_expires_at = NULL` 的行（登录快照没带回过期时间的站点）走的是
/// [`creds::Relay::token_looks_valid`] 的乐观降级：永远「看起来有效」，于是
/// [`usable_relay`] 的主动续期**永不触发**。access token 在服务端到 24h 过期后，
/// 启动探活撞上 401「登录已过期」直接清会话 —— refresh token 一次没用过就被连坐，
/// 用户被迫重登，体感就是「登录态撑不过一两天」。
///
/// 撞上 401 先续期再重试，也是上游 sub2api 自己前端的 401 拦截器做法。续期响应
/// 会带回 `expires_at`，落库之后这行就回到「过期时间已知」的健康轨道 —— 这条
/// 路径是降级态的自愈入口，不只是补救。
///
/// ## 边界（都有意为之）
///
/// - **只包只读请求**（余额探活 / 倍率刷新）。写操作（provision 建密钥、充值）不
///   整体重跑：第一次请求可能已部分生效，盲目重试会开出第二把密钥。
/// - **原错误优先**：续期失败时把原请求的错误交出去，`check_session` 的清会话
///   判读（`is_confirmed_auth_failure`）与从前完全一致 —— refresh token 真死了
///   仍然清，不会把死 lineage 无限期留着。
/// - **只重试一次**，不进循环：重试后仍 401 就交错误。
/// - NewAPI 的 401 文案是「登录态已失效」一类，不匹配 [`backend::is_token_expiry_failure`]，
///   天然不进这条路径 —— 它的 30 秒 reuse 判定下，拿可能已被消费的 cookie 盲目
///   重试会吊销整个会话族。
async fn relay_read_with_refresh_retry<R, F, Fut, T>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
    run: F,
) -> Result<T, AppError>
where
    R: tauri::Runtime,
    // Fn 而不是 FnOnce：原请求与续期后的重试各调一次。
    F: Fn(creds::Relay) -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    let op = usable_relay(app_handle, relay_id).await?;
    match run(op.clone()).await {
        Ok(value) => Ok(value),
        Err(original) => {
            let has_refresh_credential = op
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty());
            if !has_refresh_credential || !backend::is_token_expiry_failure(&original) {
                return Err(original);
            }
            match refresh_relay_session(app_handle, &op).await {
                Ok((renewed, _)) => run(renewed).await,
                Err(refresh_error) => {
                    log::warn!(
                        "中转站 {relay_id} 过期 401 后静默续期失败，按原错误处理：{refresh_error}"
                    );
                    Err(original)
                }
            }
        }
    }
}

async fn load_validated_relay<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
) -> Result<creds::Relay, AppError> {
    let op = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, |conn| creds::get(conn, relay_id))?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {relay_id} 的中转站")))?
    };

    match discovery::probe_site(&op.site_origin).await {
        Ok(detected) if detected.backend_kind == op.backend_kind => Ok(op),
        Ok(_) => {
            let state = app_handle.state::<AppState>();
            with_conn(&state, |conn| creds::clear_credentials(conn, relay_id))?;
            Err(AppError::Config(
                "站点协议已变化，已清除旧凭据，请重新添加或登录".into(),
            ))
        }
        Err(error) => match error.kind {
            discovery::DiscoveryErrorKind::Transport => Err(AppError::Config(format!(
                "连接站点失败，未改动已有凭据：{}",
                error.message
            ))),
            discovery::DiscoveryErrorKind::UnsupportedSite => {
                log::warn!(
                    "站点探针暂时无法识别 {}，沿用已保存的 {} 协议和凭据：{}",
                    op.site_origin,
                    op.backend_kind.as_str(),
                    error.message
                );
                Ok(op)
            }
            discovery::DiscoveryErrorKind::ProtocolConflict => Err(AppError::Config(format!(
                "站点协议识别结果冲突，未改动已有凭据：{}",
                error.message
            ))),
        },
    }
}

/// 打一次 profile，把账号身份写回库并更新手上这份 `op`。
///
/// 两个调用点、两种动机，但做的事完全一样，所以共用一个函数（各写一遍迟早分叉）：
///
/// 1. **token 够用但 `account_id` 为空** —— 补齐那个死局态（见 [`usable_relay`]
///    早退分支的注释）。
/// 2. **续期成功之后** —— 用户可能改了昵称/邮箱，而 `/auth/refresh` 不回账号信息。
///
/// ## 任何一步失败都只记日志
///
/// 调用方此刻的凭据**已经可用**（要么本来有效、要么刚续期成功）。账号标签陈旧或
/// `account_id` 还是空，都只影响显示与去重，不影响这一次请求 —— 为它把整个操作
/// 判失败会让用户在「明明能用」的时候被挡住。
async fn backfill_account_identity<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    mut op: creds::Relay,
) -> creds::Relay {
    let account = match backend::RuntimeBackend::for_relay(&op).account().await {
        Ok(a) => a,
        Err(e) => {
            log::warn!("读取账号信息失败（不影响使用）: {e}");
            return op;
        }
    };

    let state = app_handle.state::<AppState>();
    if let Err(e) = with_conn(&state, |conn| {
        creds::refresh_account_identity(conn, op.id, runtime_account_identity(&account))
    }) {
        log::warn!("刷新账号信息失败（不影响使用）: {e}");
        return op;
    }

    // 写库成功才更新手上这份 —— 否则返回的结构与库里不一致，
    // 调用方据此判断 `account_id` 已补上，而下次读库又是空的。
    apply_runtime_account_identity(&mut op, account);
    op
}

fn runtime_account_identity(account: &backend::RuntimeAccount) -> creds::AccountIdentity<'_> {
    creds::AccountIdentity {
        id: account.id,
        label: &account.label,
        login_identifier: &account.login_identifier,
    }
}

fn apply_runtime_account_identity(op: &mut creds::Relay, account: backend::RuntimeAccount) {
    op.account_id = Some(account.id);
    op.account_label = account.label;
    op.login_identifier = account.login_identifier;
}

fn should_clear_credentials_after_probe_error(error: &AppError) -> bool {
    backend::is_confirmed_auth_failure(error)
}

fn persist_refreshed_session(
    state: &AppState,
    current: &creds::Relay,
    refreshed: &backend::RefreshedSession,
) -> Result<creds::Relay, AppError> {
    persist_refreshed_session_with_identity_writer(
        state,
        current,
        refreshed,
        |state, relay_id, account| {
            with_conn(state, |conn| {
                creds::refresh_account_identity(conn, relay_id, runtime_account_identity(account))
            })
        },
    )
}

fn persist_refreshed_session_with_identity_writer(
    state: &AppState,
    current: &creds::Relay,
    refreshed: &backend::RefreshedSession,
    write_identity: impl FnOnce(&AppState, i64, &backend::RuntimeAccount) -> Result<(), AppError>,
) -> Result<creds::Relay, AppError> {
    let refresh_token = refreshed
        .refresh_credential
        .clone()
        .or_else(|| current.refresh_token.clone());
    // 走 update_tokens 而不是 save_credentials：续期是「同一个账号换一把新 token」，
    // 账号没变 ⇒ 没有重复可言，不该走那条会查重并可能合并行的路径。
    with_conn(state, |conn| {
        creds::update_tokens(
            conn,
            current.id,
            &refreshed.auth_token,
            refresh_token.as_deref(),
            refreshed.token_expires_at,
        )
    })?;

    let mut renewed = creds::Relay {
        auth_token: refreshed.auth_token.clone(),
        refresh_token,
        token_expires_at: refreshed.token_expires_at,
        ..current.clone()
    };

    if let Some(account) = refreshed.account.as_ref() {
        if let Err(e) = write_identity(state, current.id, account) {
            log::warn!("刷新账号信息失败（不影响使用）: {e}");
        } else {
            apply_runtime_account_identity(
                &mut renewed,
                backend::RuntimeAccount {
                    id: account.id,
                    label: account.label.clone(),
                    login_identifier: account.login_identifier.clone(),
                },
            );
        }
    }

    Ok(renewed)
}

/// 拉分组、为每组备好 sk、写成 provider 记录。
///
/// ## 一次探全部平台，各归各的 tab（2026-08-03 改）
///
/// **不吃 `app` 参数** —— 每个分组落到哪个 CLI 由它自己的 `platform` 决定
/// （`openai → codex`、`anthropic → claude`、`gemini → gemini`、`grok → grokbuild`），
/// 见 [`provision::provision`]。用户在任何一个 tab 登录一次，全部平台的档位都备好了。
///
/// ### 为什么去掉那个参数（它曾经是 bug 的根源）
///
/// 原来签名吃 `app`，于是「拉哪些分组」和「写成什么形状」都由调用方决定 ——
/// 在 claude tab 点「获取密钥」时**拉的是 openai 分组、却写成 claude 的配置形状**
/// （openai 的 sk 配在 `ANTHROPIC_BASE_URL` 上，调用必失败），
/// 而用户看到的是「claude 页出现了 chatgpt 的分组」。
///
/// 根因是把「分组属于哪个 CLI」的决定权交给了调用方，而那是分组自身的属性。
/// 现在 `provision` 返回 [`provision::TargetedTier`]（分组 + 它该落到的 app_type），
/// 调用方不需要知道 platform 映射规则 —— 那才是低耦合。
///
/// 认不出配置形状的 CLI（`settings_config_for` 返回 `None`）在循环里跳过并计入
/// `failures`，不让整批失败。
///
/// ## `relay_id`：显式指定作用于哪个中转站（2026-08-03 加）
///
/// 原来它只吃 `AppHandle`，靠 `creds::load()` 读「`is_current = 1` 的那一行」。
/// 于是多行并列的页面必须先 `set_current(id)` 才能让它作用到对的账号上
/// （前端那个 `focusRelay`）—— 而 `is_current` 是**全局单例状态**：
///
/// 两个中转站同时 provision 时，B 的 `set_current(B)` 会改掉 A 那次操作的目标，
/// A 后续的 balance / refresh 全串到 B 上。前端当时是用「任一操作进行中就禁用所有行」
/// 兜住的 —— **那是拿全局禁用换正确性，修的是症状**：中转站之间本来毫无依赖，
/// 用户点 A 的按钮却发现 B、C 的按钮全灰了。
///
/// 现在把目标变成参数，全局状态不再参与定位 ⇒ 各行真正独立、可并发。
/// 这也正是「中转站（登录态）一个模块、分组（sk）一个模块」该有的样子：
/// 分组操作显式说明「给哪个中转站」，而不是去读一个由 UI 顺手改掉的全局变量。
///
/// `None` 保留给单站流程（LoongPort 页首启引导，全程只有一个站）。
#[tauri::command]
pub async fn relay_provision(
    app_handle: tauri::AppHandle,
    relay_id: i64,
) -> Result<ProvisionSummary, String> {
    do_provision(&app_handle, relay_id)
        .await
        // ⚠️ **失败必须落日志**（维护者实测抓出）。
        //
        // 这条路径原来一个字都不记，而前端「刷新」那处又把 `Promise.allSettled` 的
        // `reason` 丢掉、只显示「<站名> 刷新失败」⇒ 两处一叠，**用户和维护者都拿不到
        // 真实错误** —— 定位一次要手工从 DB 里取 token、逐个端点 curl 一遍。
        //
        // 带上 `relay_id`：多行并列时「哪一行失败了」本身就是信息，
        // 而错误文案里未必有站名。
        .inspect_err(|e| {
            log::error!("provision 失败（relay_id={relay_id}）：{e}");
        })
        .map_err(|e| e.to_string())
}

#[derive(Clone)]
struct ManagedProvisionCandidate {
    provider_id: String,
    app_type: AppType,
    group_id: String,
    group_name: String,
    rate_multiplier: Option<f64>,
    api_key: String,
    model: String,
    models: Option<Vec<String>>,
    roles: Option<provision::ClaudeRoleModels>,
    allow_image_generation: Option<bool>,
    api_base_url: String,
}

#[derive(Default)]
struct ManagedProvisionBatch {
    account_id: Option<i64>,
    /// 登录/添加时按约定路径探测到的站点声明（relay/site_config.rs）。
    /// `None` = 站长没放文件或拉取失败（探测语义，静默降级）。
    site_declaration: Option<crate::relay::site_config::SiteDeclaredConfig>,
    candidates: Vec<ManagedProvisionCandidate>,
    /// Upstream-observed `(app_type, provider_id)` pairs that stale pruning must retain.
    observed_keep: std::collections::HashSet<(String, String)>,
    failures: Vec<FailureInfo>,
    keys_created: usize,
}

fn newapi_app_types() -> [AppType; 3] {
    [AppType::Claude, AppType::Codex, AppType::Gemini]
}

/// 把「一个 new-api 分组被观察到了」这个事实写进 keep 白名单，**按分类只保真实槽位**。
///
/// keep 是 `prune_stale_tiers` 的白名单：不在里面的托管档位会被清掉。
///
/// - 分类已知（`models = Some(非空目录)`）：只保候选真正会落的槽位 —— 纯生图分组
///   只占生图栏，它历史上被扇出到 claude/codex/gemini 的旧投影随这次 provision 一并
///   清掉（那是「生图模型被写成聊天模型」的必 404 档位）。
/// - 分类未知（`models = None`：目录拉不到、或分组对账没走完）：保全四个槽位 ——
///   keep 的语义是「观察到了就别删」，宁可留旧档也不误删。
///
/// ⚠️ `Some` 必须非空：空目录的 [`provision::is_pure_image_group`] 是真空真，
/// 会把「读不出目录」误判成「纯生图」（见该函数文档的闸说明）。
fn newapi_keep_insert(
    keep: &mut std::collections::HashSet<(String, String)>,
    site_origin: &str,
    account_id: i64,
    identity: &newapi::GroupIdentity,
    models: Option<&[String]>,
) {
    let provider_id = provision::newapi_provider_id_for(site_origin, account_id, &identity.0);
    let app_types: Vec<AppType> = match models {
        Some(models) if provision::is_pure_image_group(models) => vec![AppType::CodexImage],
        Some(_) => newapi_app_types().into(),
        None => vec![
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::CodexImage,
        ],
    };
    for app_type in app_types {
        keep.insert((app_type.as_str().to_string(), provider_id.clone()));
    }
}

fn newapi_candidates_for_group(
    site_origin: &str,
    account_id: i64,
    group: &newapi_provision::ReconciledGroup,
    models: &[String],
    tables: &provision::ModelSelectionTables,
) -> Vec<ManagedProvisionCandidate> {
    let provider_id = provision::newapi_provider_id_for(site_origin, account_id, &group.identity.0);
    // 纯生图分组只出生图候选，不扇出到三个聊天栏：把生图模型写成聊天模型是**调用必
    // 404** 的档位（claude 拿 nano-banana-2 当 ANTHROPIC_MODEL、gemini 拿 gpt-image-2
    // 当 GEMINI_MODEL —— 2026-09-05 真实 new-api 站点的生图分组实测踩中）。判据与 sub2api 路径的
    // `image_tier_app_type` 同源：[`provision::is_pure_image_group`]。
    let app_types: Vec<AppType> = if provision::is_pure_image_group(models) {
        vec![AppType::CodexImage]
    } else {
        newapi_app_types().into()
    };
    app_types
        .into_iter()
        .map(|app_type| {
            let picked = provision::pick_tier_models_with(&app_type, Some(models), tables);
            ManagedProvisionCandidate {
                provider_id: provider_id.clone(),
                app_type,
                group_id: group.identity.0.clone(),
                group_name: group.name.clone(),
                rate_multiplier: group.rate_multiplier,
                api_key: group.api_key.clone(),
                model: picked.main,
                models: Some(models.to_vec()),
                roles: picked.claude_roles,
                allow_image_generation: None,
                // NewAPI exposes one OpenAI-compatible root. Per-app suffixes are projected by
                // `api::base_url_for`, so no persisted sub2api base belongs here.
                api_base_url: String::new(),
            }
        })
        .collect()
}

fn normalize_newapi_model_catalog(models: Option<Vec<String>>) -> Option<Vec<String>> {
    models
        .map(provision::normalize_model_names)
        .filter(|models| !models.is_empty())
}

fn newapi_reconcile_stage(stage: newapi_provision::ReconcileStage) -> &'static str {
    match stage {
        newapi_provision::ReconcileStage::Create => "token_create",
        newapi_provision::ReconcileStage::Relist => "token_relist",
        newapi_provision::ReconcileStage::Reveal => "token_reveal",
        newapi_provision::ReconcileStage::DeleteStale => "token_delete_stale",
    }
}

async fn provision_backend(
    op: &creds::Relay,
    browser_fallback: Option<api::BrowserApiFallback>,
) -> Result<ManagedProvisionBatch, AppError> {
    // 一次 provision 解析一次选型表（内置 + 远端覆盖），两个 backend 共用 ——
    // sub2api 与 newapi 的选型纪律必须来自同一份数据（尺子 1.4：一个事实一个 owner）。
    let tables = provision::ModelSelectionTables::resolve();
    // 站点声明探测与 backend 无关（约定路径是 LoongPort 的约定，newapi 站长同样能放）：
    // 404/失败/格式不认都是 None，绝不打断登录主流程。
    let site_declaration = crate::relay::site_config::fetch_site_declaration(&op.site_origin).await;
    if site_declaration.is_some() {
        log::info!(
            "{}",
            crate::diagnostics::DiagnosticEvent::new("relay.site_config", "declaration_found")
                .field_display("site", crate::url_for_log(&op.site_origin))
        );
    }
    match op.backend_kind {
        discovery::BackendKind::Sub2Api => {
            let mut client = api::Client::new(
                &op.site_origin,
                &op.auth_token,
                op.account_id,
                op.user_agent.as_deref(),
                op.cf_clearance.as_deref(),
            )?;
            // 登录后自动备 key 时登录窗还开着：站点被指纹级防护拦下（403 HTML）时，
            // 由登录窗代拉（见 `browser_api_fallback`）。测试等无 UI 上下文传 `None`。
            if let Some(fallback) = browser_fallback {
                client = client.with_browser_fallback(fallback);
            }
            let mut result = provision::provision(&client, &tables).await?;
            provision::sort_tiers(&mut result.tiers);
            let keys_created = result
                .tiers
                .iter()
                .filter(|targeted| targeted.tier.key_was_created)
                .count();
            let candidates = result
                .tiers
                .into_iter()
                .map(|targeted| {
                    let tier = targeted.tier;
                    ManagedProvisionCandidate {
                        provider_id: provision::provider_id_for(
                            &op.site_origin,
                            op.account_id,
                            tier.group_id,
                        ),
                        app_type: targeted.app_type,
                        group_id: tier.group_id.to_string(),
                        group_name: tier.group_name,
                        rate_multiplier: Some(tier.rate_multiplier),
                        api_key: tier.api_key,
                        model: tier.model,
                        models: tier.models,
                        roles: tier.roles,
                        allow_image_generation: Some(tier.allow_image_generation),
                        api_base_url: op.api_base_url.clone(),
                    }
                })
                .collect();
            Ok(ManagedProvisionBatch {
                account_id: op.account_id,
                site_declaration,
                candidates,
                observed_keep: std::collections::HashSet::new(),
                failures: result
                    .failures
                    .into_iter()
                    .map(|(group_name, reason)| FailureInfo { group_name, reason })
                    .collect(),
                keys_created,
            })
        }
        discovery::BackendKind::NewApi => {
            let client = newapi::NewApiClient::with_optional_account_id(
                &op.site_origin,
                &op.auth_token,
                op.account_id,
            )?;
            let account = client.account().await?;
            if op.account_id.is_some() && op.account_id != Some(account.id) {
                return Err(AppError::Config(
                    "NewAPI 登录态所属账号与本地中转站账号不一致，请重新登录".into(),
                ));
            }
            let result = newapi_provision::reconcile_for_account(&client, account.id).await?;

            let mut batch = ManagedProvisionBatch {
                account_id: Some(result.account_id),
                site_declaration,
                candidates: Vec::new(),
                // 分类感知的槽位在下面的分组循环里逐个补（`newapi_keep_insert`），
                // 不再一次性保三个聊天栏 —— 纯生图分组只保生图栏。
                observed_keep: std::collections::HashSet::new(),
                failures: result
                    .failures
                    .into_iter()
                    .map(|failure| FailureInfo {
                        group_name: failure
                            .group_identity
                            .map(|identity| identity.0)
                            .unwrap_or_else(|| "NewAPI token cleanup".into()),
                        reason: format!(
                            "{}: {}",
                            newapi_reconcile_stage(failure.stage),
                            failure.reason
                        ),
                    })
                    .collect(),
                keys_created: result.tokens_created,
            };

            // 对账没走完的分组（建/列/揭示密钥失败，拿不到 sk 也拉不了目录）：
            // 分类未知，保全四个槽位 —— 观察到了就别删。
            let reconciled: std::collections::HashSet<newapi::GroupIdentity> = result
                .groups
                .iter()
                .map(|group| group.identity.clone())
                .collect();
            for identity in &result.observed_groups {
                if !reconciled.contains(identity) {
                    newapi_keep_insert(
                        &mut batch.observed_keep,
                        &op.site_origin,
                        result.account_id,
                        identity,
                        None,
                    );
                }
            }

            for group in result.groups {
                let models = match api::list_models(&op.site_origin, &group.api_key).await {
                    Ok(models) => match normalize_newapi_model_catalog(models) {
                        Some(models) => models,
                        None => {
                            batch.failures.push(FailureInfo {
                                group_name: group.name,
                                reason: "model_catalog: /v1/models 未返回可用模型目录".into(),
                            });
                            // 目录拉不到 = 分类未知：保全四个槽位，别把旧档误删。
                            newapi_keep_insert(
                                &mut batch.observed_keep,
                                &op.site_origin,
                                result.account_id,
                                &group.identity,
                                None,
                            );
                            continue;
                        }
                    },
                    Err(error) => {
                        batch.failures.push(FailureInfo {
                            group_name: group.name,
                            reason: format!("model_catalog: {error}"),
                        });
                        newapi_keep_insert(
                            &mut batch.observed_keep,
                            &op.site_origin,
                            result.account_id,
                            &group.identity,
                            None,
                        );
                        continue;
                    }
                };
                newapi_keep_insert(
                    &mut batch.observed_keep,
                    &op.site_origin,
                    result.account_id,
                    &group.identity,
                    Some(&models),
                );
                batch.candidates.extend(newapi_candidates_for_group(
                    &op.site_origin,
                    result.account_id,
                    &group,
                    &models,
                    &tables,
                ));
            }
            Ok(batch)
        }
    }
}

async fn do_provision(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
) -> Result<ProvisionSummary, AppError> {
    let op = usable_relay(app_handle, relay_id).await?;
    provision_relay(app_handle, &op).await
}

async fn refresh_relay_provision(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
) -> Result<ProvisionSummary, AppError> {
    let op = usable_relay(app_handle, relay_id).await?;
    let op = backfill_account_identity(app_handle, op).await;
    provision_relay(app_handle, &op).await
}

async fn provision_relay(
    app_handle: &tauri::AppHandle,
    op: &creds::Relay,
) -> Result<ProvisionSummary, AppError> {
    let batch = provision_backend(op, Some(browser_api_fallback(app_handle))).await?;
    let state = app_handle.state::<AppState>();
    let result = persist_provision_batch(state.inner(), op, batch);
    mark_pricing_after_success(state.inner(), op.id, chrono::Utc::now().timestamp(), result)
}

fn mark_pricing_after_success<T>(
    state: &AppState,
    relay_id: i64,
    synced_at: i64,
    result: Result<T, AppError>,
) -> Result<T, AppError> {
    let value = result?;
    with_conn(state, |conn| {
        creds::mark_pricing_synced(conn, relay_id, synced_at)
    })?;
    Ok(value)
}

fn persist_provision_batch(
    state: &AppState,
    op: &creds::Relay,
    mut batch: ManagedProvisionBatch,
) -> Result<ProvisionSummary, AppError> {
    let mut tiers = Vec::new();
    let mut merged_providers = Vec::new();
    // NewAPI fills this from the complete upstream inventory before reveal/model/write.
    // sub2api candidates are inserted before their local write for the same retention property.
    let mut keep = std::mem::take(&mut batch.observed_keep);
    let mut refresh_live: Vec<AppType> = Vec::new();
    for (idx, candidate) in batch.candidates.into_iter().enumerate() {
        let app_type = &candidate.app_type;
        let provider_id = candidate.provider_id.clone();
        // 先取出来：`candidate.group_name` 下面会被 move 进 failures，
        // 而倍率在那之后还要用。
        let rate_multiplier = candidate.rate_multiplier;
        let display_name = provision::provider_display_name(&op.site_name, &candidate.group_name);
        keep.insert((app_type.as_str().to_string(), provider_id.clone()));

        let base_url = api::base_url_for(app_type, &op.site_origin, &candidate.api_base_url);
        let defaults = if matches!(app_type, AppType::Claude) {
            provision::settings_config_with_roles_and_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                candidate.roles.clone(),
                candidate.models.as_deref(),
                provision::ProvisionStyle::default(),
            )
        } else if matches!(app_type, AppType::Codex) {
            provision::settings_config_with_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                candidate.models.as_deref(),
            )
        } else if matches!(app_type, AppType::Gemini) {
            // gemini 同样落模型目录（只收 gemini-* 家族，见生成侧注释）
            provision::settings_config_with_roles_and_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                None,
                candidate.models.as_deref(),
                provision::ProvisionStyle::default(),
            )
        } else if matches!(app_type, AppType::GrokBuild) {
            // grokbuild 同样落模型目录（收全部文本模型，见生成侧注释）——
            // 主界面「支持的模型」芯片与托盘「模型」子菜单都从它取
            provision::settings_config_with_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                candidate.models.as_deref(),
            )
        } else {
            provision::settings_config_for(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
            )
        };
        let Some(defaults) = defaults else {
            batch.failures.push(FailureInfo {
                group_name: candidate.group_name,
                reason: format!("{}: 还不能生成配置", app_type.as_str()),
            });
            continue;
        };

        let existing = state
            .db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .ok()
            .flatten();
        // 旧档位上已有的「站点推荐配置」标注：重放路径不重新应用声明（见下），
        // 但配置里还带着当初声明的参数，标注不能丢。
        let existing_site_declared = existing
            .as_ref()
            .and_then(|old| old.meta.as_ref())
            .and_then(|meta| meta.site_declared_origin.clone());
        let is_first_import = existing.is_none();
        let user_edited = match state.db.get_user_edited(app_type.as_str(), &provider_id) {
            Ok(user_edited) => user_edited,
            Err(error) => {
                batch.failures.push(FailureInfo {
                    group_name: candidate.group_name,
                    reason: format!("{}: 读取用户编辑标记失败: {error}", app_type.as_str()),
                });
                continue;
            }
        };

        let mut settings_config = match existing {
            Some(old) => {
                if user_edited {
                    let mut kept = old.settings_config;
                    if provision::patch_api_key(&mut kept, app_type, &candidate.api_key) {
                        kept
                    } else {
                        log::warn!("{display_name} 的配置里找不到放密钥的位置，已重置为默认配置");
                        defaults
                    }
                } else {
                    preserve_supported_model(app_type, defaults, &old.settings_config)
                }
            }
            None => defaults,
        };

        // 站点声明段（relay/site_config.rs，上游提案 Wei-Shaw/sub2api#6518 先行落地）：
        // **只在首次导入时自动应用**——开箱即站长版默认（deny-list 拦执行面，端点与
        // 凭证不开放覆盖）。重放路径（非用户编辑的 defaults 重算）刻意不应用：
        // `preserve_supported_model` 保住的可能是省心模式选下的模型，每次 provision
        // 都拿站长默认覆盖它会让自动选档失效；站长更新后的同步走手动输入命令
        // （用户显式动作）。同源校验失败按「没有声明」处理，不阻断建档。
        let mut site_declared_origin: Option<String> = None;
        if is_first_import {
            if let Some(declared) = &batch.site_declaration {
                if site_config::validate_same_origin(&declared.site_origin, &op.site_origin).is_ok()
                {
                    if let Some(platform) = platform_map::platform_for_app(app_type) {
                        if let Some(segment) = declared.segment_for(platform) {
                            match site_config::apply_segment_to_app(
                                app_type,
                                segment,
                                &mut settings_config,
                            ) {
                                Ok(true) => {
                                    site_declared_origin = Some(declared.site_origin.clone());
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    log::warn!(
                                        "{display_name} 的站点声明段应用失败（按内置默认继续）: {error}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();

        let provider = Provider {
            id: provider_id.clone(),
            name: display_name.clone(),
            settings_config,
            website_url: Some(op.site_origin.clone()),
            // aggregator 而不是 official：official 那条分类会触发一批只对官方订阅成立的
            // 逻辑（stale auth 清理、统一会话桶注入）。
            category: Some("aggregator".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: Some(idx),
            notes: None,
            meta: {
                let mut meta = managed_meta(
                    app_type,
                    batch.account_id,
                    Some(crate::provider::LoongportGroupIdentity {
                        id: candidate.group_id.clone(),
                        name: candidate.group_name.clone(),
                    }),
                );
                meta.site_declared_origin = site_declared_origin.or(existing_site_declared);
                Some(meta)
            },
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };

        if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
            batch.failures.push(FailureInfo {
                group_name: candidate.group_name,
                reason: format!(
                    "{}: 保存档位 {display_name} 失败: {error}",
                    app_type.as_str()
                ),
            });
            continue;
        }

        // 倍率落库。**这是它唯一的写入点** —— 「刷新倍率」= 重新 provision，
        // 界面上就是「顶部刷新 / 更新可用分组 / 登录成功」那几下。
        //
        // 曾经它一个字都不存（`list_tiers_impl` 恒返回 `None`），靠一条独立命令在
        // 每次 reload 后**每个档位打一次 HTTP** 去补 —— 而 reload 挂在每个动作后面，
        // 于是切一次档位就把全部档位的倍率重查一遍。倍率是服务端定价，不是实时量。
        //
        // 写失败只 warn：档位已经存对了，不该因为一个显示值让「获取密钥」整个报失败。
        if let Err(error) =
            state
                .db
                .set_tier_rate_multiplier(app_type.as_str(), &provider_id, rate_multiplier)
        {
            log::warn!("记录档位 {provider_id} 的倍率失败（只影响显示）: {error}");
        }

        let merged_current = match provider_fingerprint::remove_unmanaged_duplicates(
            state.db.as_ref(),
            app_type,
            &provider,
        ) {
            Ok(merged) => merged,
            Err(error) => {
                batch.failures.push(FailureInfo {
                    group_name: candidate.group_name.clone(),
                    reason: format!("{}: 收编重复 provider 失败: {error}", app_type.as_str()),
                });
                Vec::new()
            }
        };
        let mut is_current = current == provider_id;
        if !merged_current.is_empty() {
            log::info!(
                "收编 {} 个重复的 {} provider：{}",
                merged_current.len(),
                app_type.as_str(),
                merged_current
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            );
            merged_providers.extend(merged_current.iter().map(|merged| MergedProviderInfo {
                name: merged.name.clone(),
                app_id: app_type.as_str().to_string(),
            }));
            if merged_current.iter().any(|m| m.was_current) {
                is_current = true;
                if !refresh_live.contains(app_type) {
                    refresh_live.push(app_type.clone());
                }
            }
        }

        if is_current && !refresh_live.contains(app_type) {
            refresh_live.push(app_type.clone());
        }

        tiers.push(TierInfo {
            is_current,
            provider_id,
            // **这条分组自己的 app_type**，不是调用方给的 —— 这一整段循环的前提就是
            // 「一次 provision 探全部平台」，写错会让前端把别的平台的档位算成自己的。
            app_id: app_type.as_str().to_string(),
            group_name: candidate.group_name,
            display_name,
            model: provision::selected_model(app_type, &provider.settings_config)
                .unwrap_or_default(),
            models: models_from_settings(&provider.settings_config),
            rate_multiplier,
            can_verify_models: verification_target::supports_app_type(app_type),
            user_edited: Some(user_edited),
            allow_image_generation: candidate.allow_image_generation,
            // provision 刚建档/重放完，标注以库里的 meta 为准（首次导入应用过
            // 声明的档位这里立刻就有值，前端不用等下一次 listRelays）。
            site_declared_origin: provider
                .meta
                .as_ref()
                .and_then(|meta| meta.site_declared_origin.clone()),
        });
    }

    refresh_live_for_current_tiers(state, &refresh_live);

    let removed = prune_stale_tiers(state, &op.site_origin, batch.account_id, &keep)?;
    if removed > 0 {
        log::info!("清理了 {removed} 个不再存在的档位（{}）", op.site_origin);
    }

    // 生图工具跟着「生图栏里有没有档位」对齐一次。见 `sync_imagegen_mcp` 的文档。
    //
    // ⚠️ **必须在 `prune_stale_tiers` 之后** —— 判据是「生图栏里还有档位吗」，
    // 而清理正是让最后一条生图档位消失的那一步。反过来的话，中转站下架全部生图分组后
    // 那个工具会留到下一次 provision 才撤掉，期间它每次调用都报「档位已经不在了」。
    //
    // 失败只 warn：档位已经存对了，不该因为一个 MCP 记录写不下去就把「获取密钥」
    // 整个报成失败（用户会以为连密钥都没拿到）。
    if let Err(e) = imagegen_mcp::sync_registration(state) {
        log::warn!("同步生图工具记录失败（生图可能暂时用不了）: {e}");
    }

    let retained_observed = keep.iter().any(|(app_type, provider_id)| {
        state
            .db
            .get_provider_by_id(provider_id, app_type)
            .ok()
            .flatten()
            .is_some()
    });
    if tiers.is_empty() && !retained_observed {
        let detail = batch
            .failures
            .iter()
            .map(|failure| format!("{}: {}", failure.group_name, failure.reason))
            .collect::<Vec<_>>()
            .join("；");
        return Err(AppError::Config(if detail.is_empty() {
            "没有可写入或可保留的托管档位".into()
        } else {
            format!("所有分组都没能备好托管档位（{detail}）")
        }));
    }

    Ok(ProvisionSummary {
        keys_created: batch.keys_created,
        tiers,
        failures: batch.failures,
        merged_providers,
    })
}

/// 把这些 app 的**当前项**的配置刷到 live 文件上。失败只 warn，不中断调用方。
///
/// ## 为什么必须有这一步（用户实测的症状）
///
/// CLI 读的是落地文件（`~/.codex/config.toml` 等），**不是我们的 DB**。所以凡是「改了
/// 当前项的 `settings_config` 却只 `save_provider`」的路径，结果都是：界面提示成功、
/// 库里也确实是新内容，而 **codex / claude 仍在用旧的**。
///
/// 而且用户没有自救手段 —— UI 认为这个档位已经是当前项（`isCurrent` 为 true），
/// 前端 `if (tier.isCurrent) return;` 会让「再点它一次」什么也不做。
///
/// 两个调用方：[`do_provision`]（sk 被撤销后重建了一把）与
/// [`reset_tier_config_impl`]（把被改坏的配置恢复成默认）。
///
/// ## 为什么走 `sync_current_provider_for_app` 而不是 `switch`
///
/// 我们不是在**切换**当前项（它本来就是当前项），只是让落地配置追上 DB。那个 API 内部
/// 已处理代理接管（接管时更新备份而不是覆盖 live 文件）；而 `switch` 会跑一整套切换语义
/// ——接管态下走 `hot_switch_provider_inner`，还带「接管时不许切到官方供应商」那道拦，
/// 对一次「刷新密钥」是错的。
///
/// ## 失败只 warn
///
/// 记录已经存对了，用户手工切一次就能生效。不该因为落地文件写不下去（权限 / 文件被占）
/// 就把整次「获取密钥」报成失败 —— 那会让他以为连密钥都没拿到。
///
/// ⚠️ **收成一个函数是为了让命令层这一步可测**：两个调用方都吃 `&tauri::AppHandle`
/// （单测里造不出来），所以原来那两段内联代码**没有任何测试覆盖得到** ——
/// 第二路 review 实测：把它们注释掉，2578 条测试全绿。
fn refresh_live_for_current_tiers(state: &AppState, app_types: &[AppType]) {
    for app_type in app_types {
        if let Err(e) = ProviderService::sync_current_provider_for_app(state, app_type.clone()) {
            log::warn!(
                "刷新 {} 的当前配置失败（记录已保存，切换一次即生效）: {e}",
                app_type.as_str()
            );
        }
    }
}

/// 这条 provider 是不是「这个站 × 这个账号」名下的托管档位。
///
/// **收成一个函数是必要的，不是整理**：它同时被 [`prune_stale_tiers`]（要删哪些）与
/// [`apps_using_this_accounts_tiers`]（哪些不许删）消费 —— 两者必须对「归属」给出**同一个**
/// 答案。散成两份的后果是守卫与删除各认一套：守卫说「这条不是你的、不拦」，删除说
/// 「这条是你的、删了」⇒ 恰好绕过守卫删掉正在用的配置，而那正是守卫要防的事。
///
/// [`belongs_to_relay`]（严格版）才是余额 / 对账这些**归属**路径用的判据 ——
/// 两者 `(account_id, 档位标记)` 的 `(None, Some)` 那一格语义相反，见它那边的说明。
///
/// 三道判据缺一不可：
///
/// - `is_managed` —— 我们生成的（前缀 + 恰好 16 位小写 hex，即校验哈希形状）。用户手工加的 provider
///   一律不碰，错删它是不可挽回的。
/// - `website_url == site_origin` —— 只认这个站的。`provider_id` 是哈希，单向不可逆，
///   反推不出它属于谁。
/// - 账号维度 —— 同一个站可以挂多个账号。归属记在 `meta.loongportAccountId`，三种情况：
///   - **两边都有且不等** ⇒ 别人的，不是。
///   - **记录没有标记（`None`）** ⇒ 旧数据，只能靠站点判，**算是**。否则升级前生成的
///     孤儿档位永远清不掉（那正是 `prune_stale_tiers` 存在的理由），而它们必定 401。
///     代价是可能误伤同站另一账号的旧档位，但那些会在下次 provision 时重新生成并带上标记。
///   - **调用方不知道账号（`account_id` 为 `None`）** ⇒ 未登录的行（provision 不出档位）
///     或删站兜底路径，此时不按账号过滤。
pub(crate) fn belongs_to_account(
    provider: &Provider,
    site_origin: &str,
    account_id: Option<i64>,
) -> bool {
    if !is_managed(provider) {
        return false;
    }
    // 归属按注册域身份判：website_url 与 site_origin 都是 provision 时写入的
    // origin，同站不同子域拼写（面板域 vs API 域）也该认 —— 裸字符串相等会漏。
    if !same_site_identity(provider.website_url.as_deref(), Some(site_origin)) {
        return false;
    }
    match (
        account_id,
        provider.meta.as_ref().and_then(|m| m.loongport_account_id),
    ) {
        (Some(want), Some(owner)) => want == owner,
        _ => true,
    }
}

/// 这条 provider 是不是「这个 relay 行」名下的托管档位 —— **严格归属版**。
///
/// 与 [`belongs_to_account`] 只差一格：relay 行没登录（`account_id == None`）而档位
/// 记了**别人的**账号时，这里**不认**。两个方向语义相反，不能合并：
///
/// - [`belongs_to_account`] 服务清理 / 守卫 —— 宁可多认不误删（同站没记归属的旧档位
///   要能跟着清掉，`None` 一律「算是」）。
/// - 这里服务**把事实记到某一行头上**的路径（余额 sk 收集、扣费对账的成本归属）——
///   把别的账号的 sk / 成本算进未登录行，等于把 B 的消费记到 A 头上，比漏算更糟。
///   这也是 `apps_using_this_accounts_tiers` 当年要额外加 `account_id.is_some()` 的
///   同一类教训（见它那边的 ⚠️ 段）。
///
/// 判据即 `relay_balance_inputs` 原内联那份（`(None, Some(_)) => false`），收成函数
/// 供余额与 `relay_reconciliation`（`commands/reconcile.rs`）共用 —— 归属口径只有一份。
pub(crate) fn belongs_to_relay(
    provider: &Provider,
    site_origin: &str,
    account_id: Option<i64>,
) -> bool {
    if !is_managed(provider) {
        return false;
    }
    if !same_site_identity(provider.website_url.as_deref(), Some(site_origin)) {
        return false;
    }
    match (
        account_id,
        provider.meta.as_ref().and_then(|m| m.loongport_account_id),
    ) {
        (Some(want), Some(owner)) => want == owner,
        (_, None) => true,
        (None, Some(_)) => false,
    }
}

/// 两个可选 origin 是否指向**同一站点**（注册域身份，None 与任何值都不相等）。
///
/// 收拢 `website_url` ↔ `site_origin` 这类归属判据：两边都是持久化的 origin 字符串，
/// 裸相等依赖「写入时同拼写」这个脆弱不变量 —— 同站的面板域与 API 域拼写不同
/// 就静默失配。身份归一见 [`crate::relay::identity`]。
fn same_site_identity(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            crate::relay::identity::site_domain(left) == crate::relay::identity::site_domain(right)
        }
        _ => false,
    }
}

/// 这个账号名下的档位里，有哪些**正是某个 app 的当前项**。返回 `(app_type, 档位名)`。
///
/// 给 [`remove_site_impl`] 当闸用：非空就说明删下去会毁掉一份**还能用**的配置
/// （见那边关于「为什么不能靠前端按钮态」的说明）。
///
/// 扫 `AppType::all()` 而不是某一个 app —— 这条闸的全部意义就在于**跨 app**：
/// 前端那道判据只看当前 tab，而档位可以是别的 app 的当前项。
///
/// 读不出某个 app 的列表时**跳过它**：那是「配置文件坏了 / 没权限」，而这条闸的作用是
/// 拦住已知的破坏。因为读不出来就把删除整个拦死，会让用户卡在一个他无法处置的错误上。
///
/// ## ⚠️ 这一行还没有 `account_id`（`None`）⇒ 一律不拦（第二路 review 抓出）
///
/// [`belongs_to_account`] 对 `account_id: None` 返回 `true`（"不按账号过滤"）。那个语义
/// 对**删除方向**是对的：删这一行时，同站那些没记归属的旧档位该跟着清掉。但对**守卫
/// 方向**反过来就错了 —— 它会把「同站另一个账号正在用的档位」算成「你名下的」。
///
/// 那种行真实可达，不是理论情况：`clear_credentials` 会把 `account_id` 置 `NULL`
/// （站点换了后端协议时走这条，见 [`load_validated_relay`]），而唯一索引把 `NULL`
/// 视为互不相等 ⇒ 它与那个已登录的行并存。此时用户删这个空行会看到
/// 「这个账号名下还有档位正在使用中：B 的档位（codex）」—— 点名一个**它并不拥有**的档位，
/// 而这一行压根没有任何档位。他唯一的出路是去 codex 把 B 切走，才能删掉一个空行。
///
/// （**登录态失效不再走那条路**：那边现在用 `creds::clear_session`，账号身份留着 ——
/// 见它的文档。所以 `None` 的来源只剩「从没登录过」与「协议变更」两种。）
///
/// 所以这里额外要求 `account_id.is_some()`：**认不出归属就不拦**。漏拦的代价是什么？
/// 没有 —— 没有 `account_id` 的行派生不出 provider id（`provider_id_for` 要它），
/// 所以它名下本来就不可能有档位，`prune_stale_tiers` 那一步也就没什么可删的。
fn apps_using_this_accounts_tiers(
    state: &AppState,
    site_origin: &str,
    account_id: Option<i64>,
) -> Vec<(AppType, String)> {
    let mut in_use = Vec::new();
    for app_type in AppType::all() {
        let Ok(list) = ProviderService::list(state, app_type.clone()) else {
            log::warn!(
                "检查「档位是否在用」时读不出 {} 的 provider 列表，跳过",
                app_type.as_str()
            );
            continue;
        };
        let Ok(current) = ProviderService::current(state, app_type.clone()) else {
            continue;
        };
        if current.is_empty() {
            continue;
        }
        if let Some(provider) = list.get(&current) {
            if belongs_to_account(provider, site_origin, account_id) && account_id.is_some() {
                in_use.push((app_type.clone(), provider.name.clone()));
            }
        }
    }
    in_use
}

/// 删掉「这个站在本地留着、但这次 provision 没再生成」的档位。返回删了几条。
///
/// ## 为什么必须有这一步（2026-08-03 加，用户实测发现）
///
/// `provision` 原来只 `save_provider`（新增或更新），**从不删**。于是任何一次
/// 「这条档位不该再存在了」都无法被纠正：
///
/// 1. **旧版本写错的记录永久残留**。曾经有个 bug 把 openai 分组写进了 claude 下
///    （`is_usable_for(&AppType::Codex)` 写死而外层已改成多平台），修掉代码之后
///    那些脏记录**点多少次「刷新」都不会消失** —— 用户看到「claude 页还有 codex
///    的分组」，只能怀疑是没修好。
/// 2. **中转站在网页端删掉一个分组，本地那条会一直留着**。用户点它 ⇒ 用一把
///    已失效的 sk 发请求 ⇒ 报一个看不懂的 401。
///
/// ## 判据必须精确，宁可漏删不可错删
///
/// 删除条件是**三个都成立**：
///
/// - `is_managed(id)` —— 是我们生成的（**前缀 + 恰好 16 位小写 hex**），用户手工加的 provider
///   一律不碰。这是最重要的一道：错删用户自己配的 provider 是不可挽回的。
/// - `website_url == 这次的 site_origin` —— **只清这个中转站的**。别的站的档位这次
///   压根没查（`provision` 只拉当前这一个站的分组），凭「这次没生成」删它们是错的。
/// - `id` 不在这次生成的集合里 —— 真的不该存在了。
///
/// ## 为什么扫全部 app_type 而不是只扫参数指定的那个
///
/// ⚠️ **依赖 `AppType::all()` 被同步维护** —— 它是**手工数组**（`app_config.rs:412`），
/// 不是从 enum 自动派生的。上游加一个 CLI 而漏改它，那个 app 下的串台脏记录就
/// **永远不被清理**（静默失效）。已加闸：`app_type_all_covers_every_variant`。
///
/// `provision` 现在一次探全部平台（分组自己的 `platform` 决定落到哪个 CLI），所以
/// 「不该存在的记录」可能在任何一个 app_type 下 —— 上面那个 claude/codex 串台的
/// bug 正是这种。只扫一个 app 就清不掉它。
///
/// ## 当前项也删 —— 一条不该存在的档位不配当「当前」
///
/// `ProviderService::delete` 会在「删的是当前项」时返回 `Err`（"无法删除当前正在
/// 使用的供应商"）。那条约束对**用户手工删除**是对的（防误删正在用的配置），但对
/// 这里不成立：走到这一步说明这条档位**服务端已经没有了**，它的 sk 是死的 ——
/// 留着当「当前项」只会让 CLI 拿一把失效密钥去发请求，报一个看不懂的 401。
/// 用户重新选一个可用的就好。
///
/// 所以当前项那条**直连 `state.db.delete_provider`** 绕过那层保护。
/// 悬空的 `is_current` 指针不用手工清：`settings::get_effective_current_provider`
/// 会验证 id 在库里是否存在，不存在就自动清掉本地 settings 并回落
/// （它的文档明说了这一条，正是为云同步导入后失效的场景写的）。
///
/// 非当前项走 `ProviderService::delete` —— 它顺带处理 additive-mode app 的
/// live config 清理，那套逻辑不该在这里重写一遍。
///
/// `AppType::all()` 是穷尽的（上游维护），加新 CLI 时这里自动覆盖。
///
/// 归属判据本身收在 [`belongs_to_account`]（删档位与删账号前的守卫共用一份）。
fn prune_stale_tiers(
    state: &AppState,
    site_origin: &str,
    account_id: Option<i64>,
    keep: &std::collections::HashSet<(String, String)>,
) -> Result<usize, AppError> {
    let mut removed = 0usize;
    for app_type in AppType::all() {
        let Ok(list) = ProviderService::list(state, app_type.clone()) else {
            // 某个 app 的配置读不出来（文件坏了 / 权限）时跳过它，别让整次 provision 失败 ——
            // 清理是收尾动作，主线（档位已经写好了）不该被它拖垮。
            log::warn!(
                "清理档位时读不出 {} 的 provider 列表，跳过",
                app_type.as_str()
            );
            continue;
        };

        // 读一次当前项，用来选删除路径（当前项要绕过 ProviderService 的保护）。
        let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();

        for provider in list.values() {
            if !belongs_to_account(provider, site_origin, account_id) {
                continue;
            }
            // 判据是 **(app_type, id) 组合**，不是光看 id ——
            // 见 `do_provision` 里 `keep.insert` 那处的说明（同一个分组在两个 app
            // 下是同一个 id，只看 id 会让串台的脏记录永远删不掉）。
            if keep.contains(&(app_type.as_str().to_string(), provider.id.clone())) {
                continue;
            }

            let is_current = provider.id == current;
            let outcome = if is_current {
                // 绕过 ProviderService::delete 的「不许删当前项」保护（见上方说明）。
                state.db.delete_provider(app_type.as_str(), &provider.id)
            } else {
                ProviderService::delete(state, app_type.clone(), &provider.id)
            };

            match outcome {
                Ok(()) => {
                    log::info!(
                        "删除不再存在的档位：{} ({} / {}){}",
                        provider.name,
                        app_type.as_str(),
                        provider.id,
                        if is_current {
                            " —— 它曾是当前项，请重新选一个档位"
                        } else {
                            ""
                        }
                    );
                    removed += 1;
                }
                // 删不掉如实记录但不中断 —— 其余的照样该清。
                Err(e) => log::warn!("删除档位 {} 失败: {e}", provider.id),
            }
        }
    }
    Ok(removed)
}

/// 「中转站 × 分组」页的数据源：一次返回渲染整页所需的全部内容。
///
/// **只读本地，不发网络请求**（spec §三）—— 与 [`relay_status`] 的首屏契约一致。
/// 代价是 `rate_multiplier` 恒为 `None`，要等用户主动 provision 才有值；
/// 那是有意的取舍，首屏不该卡在网络上。
///
/// `app` 决定读哪个 app_type 下的 provider（前端本来就知道当前是哪个 tab）。
#[tauri::command]
pub fn relay_list_relays(state: State<'_, AppState>, app: String) -> Result<Vec<RelayRow>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    list_relays_impl(state.inner(), app_type).map_err(|e| e.to_string())
}

/// 「这一行能开带登录态的站点窗吗」——充值与查看用量共用同一判据
/// （配置了入口 + 登录态有效 + NewAPI 还要有可轮换的 refresh cookie）。
fn can_open_site_window(relay: &creds::Relay, logged_in: bool, configured_url: bool) -> bool {
    configured_url
        && logged_in
        && match relay.backend_kind {
            creds::BackendKind::Sub2Api => true,
            creds::BackendKind::NewApi => relay
                .refresh_token
                .as_deref()
                .is_some_and(|refresh_cookie| !refresh_cookie.trim().is_empty()),
        }
}

fn list_relays_impl(state: &AppState, app_type: AppType) -> Result<Vec<RelayRow>, AppError> {
    let relays = with_conn(state, creds::list)?;
    // 一次读全量再在内存里按站分组，而不是对每个站各查一次 —— 站点通常 1-5 个，
    // 而 ProviderService::list 每次都要解一遍 settings_config 的 JSON。
    // `app_type` 下面在闭环里要按站点各用一次（判「用户改过配置没有」），
    // 而它没派生 Copy（上游结构，别为此改它）⇒ 先 clone 一份给 `list_tiers_impl`。
    let tiers = list_tiers_impl(state, app_type.clone())?;
    // 签名目录只读一次；逐行只做纯解析，避免重复验签与磁盘读取。
    let signed_config = remote_config::load_cached().unwrap_or_default();
    let now = chrono::Utc::now().timestamp();

    relays
        .into_iter()
        .map(|op| -> Result<RelayRow, AppError> {
            let mine = tiers_of_site(state, &tiers, &op.site_origin, op.account_id, &app_type)?;
            let logged_in = op.token_looks_valid(now);
            let session_expired = op.session_expired(now);
            let has_balance_key = !relay_balance_inputs(state, &op).1.is_empty();
            let status = if session_expired {
                if has_balance_key {
                    RelayRowStatus::SessionExpiredUsable
                } else {
                    RelayRowStatus::SessionExpired
                }
            } else if !logged_in && !op.can_refresh(now) {
                RelayRowStatus::NotLoggedIn
            } else if mine.is_empty() {
                RelayRowStatus::NoTiers
            } else {
                RelayRowStatus::Ready
            };
            let configured_url =
                match remote_config::configured_purchase_url(&signed_config, &op.site_origin) {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(_) => {
                        let host = url::Url::parse(&op.site_origin)
                            .ok()
                            .and_then(|url| url.host_str().map(str::to_owned))
                            .unwrap_or_else(|| "<unknown>".into());
                        log::warn!("中转站 {host} 的购买入口配置无效，已禁用购买");
                        false
                    }
                };
            let usage_url_configured =
                match remote_config::configured_usage_url(&signed_config, &op.site_origin) {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(_) => {
                        let host = url::Url::parse(&op.site_origin)
                            .ok()
                            .and_then(|url| url.host_str().map(str::to_owned))
                            .unwrap_or_else(|| "<unknown>".into());
                        log::warn!("中转站 {host} 的用量入口配置无效，已禁用查看用量");
                        false
                    }
                };
            let usage_blockers =
                apps_using_this_accounts_tiers(state, &op.site_origin, op.account_id)
                    .into_iter()
                    .map(|(app_type, tier_name)| UsageBlocker {
                        app: app_type.as_str().to_string(),
                        tier_name,
                    })
                    .collect::<Vec<_>>();
            Ok(RelayRow {
                id: op.id,
                site_origin: op.site_origin.clone(),
                site_name: op.site_name.clone(),
                // 有 account_id 才算真的认得这个账号 —— email 可能被中转站留空。
                account_label: if op.account_id.is_some() {
                    op.account_label.clone()
                } else {
                    String::new()
                },
                status,
                is_current: mine.iter().any(|tier| tier.is_current),
                can_query_balance: logged_in || has_balance_key,
                can_purchase: can_open_site_window(&op, logged_in, configured_url),
                can_view_usage: can_open_site_window(&op, logged_in, usage_url_configured),
                can_refresh: op.can_refresh(now),
                usage_blockers,
                remove_confirmation: if op.account_id.is_some() {
                    RemoveConfirmation::Configured
                } else {
                    RemoveConfirmation::NeverLoggedIn
                },
                tiers: mine,
            })
        })
        .collect()
}

/// 把一个托管档位的配置**恢复成默认值**。
///
/// ## 为什么需要这个命令
///
/// 编辑走 cc-switch 现成的编辑页（那页支持全部字段，我们不重做）。代价是用户可能改坏 ——
/// 改错 base_url、删掉 `disable_response_storage`、把 `model_provider` 从 `custom` 改成
/// `OpenAI`（那会让会话历史分家）。这些改动都不会报错，只会让调用静默失败。
///
/// 所以给一条回头路。**它是唯一会重写用户编辑的入口** —— 重复 provision 不再覆盖
/// （见 `do_provision` 里那段），改配置的责任明确落在用户显式点这个按钮上。
///
/// **sk 保留不变**：从现有配置里读出来再塞回去。恢复默认是「修配置」不是「换密钥」，
/// 顺手换掉 sk 会让用户的其它设备上那把 key 失效（虽然认领逻辑会重新拿到，
/// 但那是多余的服务端写操作）。sk 读不出来时（配置被改得面目全非）**明确报错**，
/// 让用户走「获取密钥」重建 —— 不静默生成一份没有 sk 的配置。
#[tauri::command]
pub async fn relay_reset_tier_config(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    reset_tier_config_impl(&app_handle, &provider_id, app_type)
        .await
        .map_err(|e| e.to_string())
}

async fn reset_tier_config_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
) -> Result<(), AppError> {
    let state = app_handle.state::<AppState>();
    reset_tier_config_in_state(state.inner(), provider_id, app_type)
}

fn reset_tier_config_in_state(
    state: &AppState,
    provider_id: &str,
    app_type: AppType,
) -> Result<(), AppError> {
    // 只对托管档位有效 —— 用户自建的 provider 没有「默认配置」这个概念。
    // 用正向判据 `is_managed`，不要拿 `reject_if_managed` 的 Err 反着判 ——
    // 那个函数的语义是「撞到托管项就拦下」（给通用命令用），这里要的恰好相反
    // （只对托管项生效），借它的错误来表达「是托管的」会让代码反着读。
    if !crate::relay::is_managed(provider_id) {
        return Err(AppError::Config(
            "只有 LoongPort 托管的档位才能恢复默认配置".into(),
        ));
    }

    let verification_scope =
        crate::relay::model_verification::types::TargetScope::new(provider_id, app_type.as_str());
    state.model_verification.cancel_scope(&verification_scope);
    let existing = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())
        .map_err(|e| AppError::Database(format!("读取档位失败: {e}")))?
        .ok_or_else(|| AppError::Config("这个档位不存在".into()))?;

    // ⚠️ **中转站必须按这个档位自己的归属取，绝不能用 `creds::load()`**（review 抓出的 P0）。
    //
    // `creds::load` 返回的是**全局「当前站」**（`ORDER BY is_current DESC LIMIT 1`），而
    // 分组页把所有中转站并列显示 —— 用户展开 B 站那一行、点它某个档位的「恢复默认配置」时，
    // 拿到的会是 A 站的 `api_base_url`，于是那个档位被写成「B 的 sk + A 的端点」⇒
    // **每次调用都 401**，而界面显示恢复成功。「恢复默认」恰恰是用户在档位坏了时点的按钮，
    // 那等于让它把自己要修的问题弄得更糟。
    //
    // `website_url` 是档位归属的唯一可靠依据（provision 时写入，见 `:799`；
    // `prune_stale_tiers` 也是靠它认主人）—— `provider_id` 是
    // `sha256(site_origin + group_id)`，单向不可逆，反推不出属于哪个站。
    //
    // 这是 `b2400000`「中转站之间彻底解耦」那一轮的漏网之鱼：这条命令写在那之前
    // （`ea2a32b7`），保留了「靠全局当前站定位」的旧写法，而本轮给它接上 UI 入口
    // 才让这个潜在缺陷变得可达。
    let site_origin = existing
        .website_url
        .as_deref()
        .ok_or_else(|| {
            AppError::Config(
                "这个档位没有记录它属于哪个中转站，请用「获取密钥」重新生成它。".into(),
            )
        })?
        .to_string();

    // ⚠️ **必须连账号一起认**，不能只按站点取第一个匹配的行 ——
    // 同一个站可以挂多个账号，取错了就是「用 A 账号的凭据重建 B 账号的档位」，
    // 而那把 sk 属于 A ⇒ 用户拿到一条指向错误账号的配置（还会算错账）。
    //
    // 归属记在 `meta.loongportAccountId`（provision 时写入）。
    // `None` = 旧数据：那时只能按站点回落，且**只在该站只有一行时**才敢用 ——
    // 有多行还猜就是重演这个 bug。
    let account_id = existing.meta.as_ref().and_then(|m| m.loongport_account_id);
    let candidates: Vec<_> = with_conn(state, creds::list)?
        .into_iter()
        .filter(|candidate| {
            same_site_identity(Some(&candidate.site_origin), Some(site_origin.as_str()))
        })
        .collect();
    let op = match account_id {
        Some(want) => candidates
            .into_iter()
            .find(|candidate| candidate.account_id == Some(want))
            .ok_or_else(|| {
                AppError::Config(format!(
                    "这个档位属于 {site_origin} 上的某个账号，但那个账号已经不在列表里了。\
                     重新登录它、或者直接删掉这个档位。"
                ))
            })?,
        // 旧数据没有账号标记。
        None if candidates.len() == 1 => candidates.into_iter().next().expect("刚判过只有一个"),
        None if candidates.is_empty() => {
            return Err(AppError::Config(format!(
                "这个档位属于 {site_origin}，但那个中转站已经不在列表里了。\
                 重新添加它、或者直接删掉这个档位。"
            )))
        }
        // 该站有多个账号，而这条档位没记归属 ⇒ **不猜**。
        // 猜错的后果（用错账号的 sk 重建）比让用户重新生成一次糟得多。
        None => {
            return Err(AppError::Config(format!(
                "这个档位没有记录属于 {site_origin} 上的哪个账号，而那个站现在挂着多个账号。\
                 请用「获取密钥」重新生成它 —— 那会带上账号归属。"
            )))
        }
    };

    // sk 从现有配置里取。取不到就让用户走「获取密钥」重建 —— 生成一份没有 sk 的
    // 「默认配置」比保持现状更糟（那是一条必定 401 的记录）。
    let api_key =
        provision::extract_api_key(&existing.settings_config, &app_type).ok_or_else(|| {
            AppError::Config("这个档位的配置里读不出密钥了，请用「获取密钥」重新生成它。".into())
        })?;

    // Codex 的远端模型目录已经存进 `modelCatalog`：恢复默认时按同一份目录重新挑
    // 默认模型，并保留目录本身。否则这个动作会把刚外露的模型列表清空，还可能把
    // 不支持 `DEFAULT_MODEL` 的分组重置成一条选中即 404 的配置。
    let codex_models = if matches!(app_type, AppType::Codex) {
        models_from_settings(&existing.settings_config)
    } else {
        Vec::new()
    };

    // ⚠️ **生图档位要保住它自己的模型名**（review 抓出）。
    //
    // 这条路拿不到分组数据（手上只有本地 `settings_config`），所以原来无条件写
    // `DEFAULT_MODEL` —— 那会把纯生图档位重置成一个**必定 404** 的形状，
    // 即「恢复默认」这个专门用来救砖的按钮，反过来把生图档位弄砖。
    //
    // 判据用配置里现有的模型名：是 `gpt-image-*` 就留着（那个值本就是这个档位的正解，
    // 由 `provision::pick_model` 按服务端的模型列表定），否则回落 `DEFAULT_MODEL`。
    //
    // 这**不是**「保留用户改的模型名」—— 用户把模型改成任何文本模型时仍然会被重置成
    // 默认值，那正是这个按钮该做的事。
    let model = if codex_models.is_empty() {
        provision::extract_model(&existing.settings_config)
            .filter(|m| provision::is_image_model(m))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    } else {
        provision::pick_tier_models(&app_type, Some(&codex_models)).main
    };
    let base_url = api::base_url_for(&app_type, &op.site_origin, &op.api_base_url);

    let settings_config = if matches!(app_type, AppType::Codex) {
        provision::settings_config_with_models(
            &app_type,
            &api_key,
            &existing.name,
            &base_url,
            &model,
            Some(&codex_models),
        )
    } else {
        provision::settings_config_for(&app_type, &api_key, &existing.name, &base_url, &model)
    }
    .ok_or_else(|| {
        AppError::Config(format!(
            "还不能为 {} 生成默认配置（`settings_config_for` 里没有它的形状）。",
            app_type.as_str()
        ))
    })?;

    // 除 settings_config 外其余字段保持原样（sort_index / created_at 等都不该被重置）。
    //
    // `managed_meta` 传 `op.account_id` 而不是上面那个 `account_id` ——
    // 后者可能是 `None`（旧数据），而这次重建正好是**补上归属标记**的时机：
    // 我们刚刚确认了它属于 `op` 这一行。
    //
    // 分组身份同样趁重建补上——但这条路拿不到分组数据（手上只有本地
    // `settings_config`），只能保留旧值；旧值也是 `None` 时维持 `None`
    // （能力判定按 Unknown 处理，不排除任何模型）。
    let preserved_group = existing
        .meta
        .as_ref()
        .and_then(|meta| meta.loongport_group.clone());
    let restored = Provider {
        settings_config,
        meta: Some(managed_meta(&app_type, op.account_id, preserved_group)),
        ..existing
    };

    state
        .db
        .save_provider(app_type.as_str(), &restored)
        .map_err(|e| AppError::Database(format!("恢复默认配置失败: {e}")))?;

    // 「恢复默认配置」= 回到 LoongPort 的默认 ⇒ 清掉「已手工维护」标记。
    state
        .db
        .set_user_edited(app_type.as_str(), &restored.id, false)
        .map_err(|e| AppError::Database(format!("清除已手工维护标记失败: {e}")))?;

    state
        .model_verification
        .clear_scope(&verification_scope)
        .map_err(|_| AppError::Database("清除模型验证结果失败".into()))?;

    // 重置的正是当前项 ⇒ 把默认配置落到 live 文件上。
    //
    // ⚠️ **这一步不可省，否则这个命令对当前项整体无效**：这个按钮的全部意义就是
    // 「用户把配置改坏了，给他一条回头路」，而改坏的配置**就在 live 文件里**
    // （CLI 读那个文件，不读 DB）。只写 DB 的话，界面提示已恢复默认、库里也确实是
    // 默认配置，而 CLI 用的仍是那份坏配置 —— 且用户没有自救手段：UI 认为它已经是
    // 当前项，再点一次不会触发切换（前端 `if (tier.isCurrent) return;`）。
    //
    // 与 `do_provision` 同一条路（见那边关于为什么用 `sync_current_provider_for_app`
    // 而不是 `switch` 的说明）。失败只 warn：DB 已经是对的，切一次即生效，
    // 不该因为落地文件写不下去就报「恢复失败」。
    let is_current = ProviderService::current(state, app_type.clone())
        .map(|current| current == restored.id)
        .unwrap_or(false);
    if is_current {
        refresh_live_for_current_tiers(state, std::slice::from_ref(&app_type));
    }

    Ok(())
}

/// 保存中转站行的手工顺序。
///
/// `relay_ids` 是拖动后的完整顺序，下标即新的 `sort_index`。
///
/// ## 为什么行序要用户说了算
///
/// 原来 `creds::list` 排的是 `ORDER BY is_current DESC, id ASC` —— 「当前站」永远第一。
/// 而 `is_current` 会因为用户点某一行的登录/获取密钥而改变 ⇒ **行序跟着跳**。
/// 用户明确指出过：选一个档位不该重排中转站的顺序。
///
/// 现在改成按 `sort_index` 排，而这个命令是唯一会写它的地方 —— 只有用户拖动才改顺序。
#[tauri::command]
pub fn relay_reorder(state: State<'_, AppState>, relay_ids: Vec<i64>) -> Result<(), String> {
    with_conn(state.inner(), |conn| creds::reorder(conn, &relay_ids)).map_err(|e| e.to_string())
}

/// 一条档位 + 它的归属信息。
///
/// **打成结构体而不是 `(TierInfo, Option<String>, Option<i64>)`** —— 两个 `Option`
/// 并列时调换了编译器也不会报，而后果是把档位分给错的账号。带字段名就调不错。
#[derive(Debug, Clone)]
struct OwnedTier {
    tier: TierInfo,
    /// provision 时写下的 `site_origin`。`None` = 历史数据 / 手工造的。
    site_origin: Option<String>,
    /// provision 时写下的中转站账号 id（`meta.loongportAccountId`）。
    /// `None` = 升级前生成的档位（那时还没记账号）。
    account_id: Option<i64>,
}

/// 从档位列表里挑出属于某一行中转站（站点 × 账号）的那些，保持原有顺序。
///
/// ## 归属判据是「站点 + 账号」两项
///
/// provision 时把 `site_origin` 写进 `website_url`、把账号写进
/// `meta.loongportAccountId`（见本文件 provision 段）。
///
/// ⚠️ **只看站点是不够的** —— 同一个站可以挂多个账号，那样每一行都会显示该站的
/// **全部**档位（包括别的账号的），用户看到的档位数与他实际拥有的不符，
/// 点进去用的还是别人的 sk。
///
/// ⚠️ **不能靠 `provider_id` 反推**：它是 sha256 的前 16 位 hex
/// （`provision::provider_id_for`），单向不可逆 —— 那不是判据。
///
/// `website_url` 为 `None` 的档位**不归任何行**（历史数据或手工造的），
/// 宁可不显示也不能猜着塞给某个站 —— 塞错了用户会以为自己在 A 站买的档位属于 B 站。
///
/// `account_id` 为 `None` 的档位（升级前生成的）**按站点归属**：那时还没记账号，
/// 不显示它们等于让老档位在界面上凭空消失。它们在下次 provision 后就带上标记了。
/// `app_type` 只用来读「已手工维护」标记（存库，见 `providers.user_edited`）。
fn tiers_of_site(
    state: &AppState,
    tiers: &[OwnedTier],
    site_origin: &str,
    account_id: Option<i64>,
    app_type: &AppType,
) -> Result<Vec<TierInfo>, AppError> {
    tiers
        .iter()
        .filter(|owned| same_site_identity(owned.site_origin.as_deref(), Some(site_origin)))
        .filter(|owned| match (account_id, owned.account_id) {
            // 两边都知道账号 ⇒ 必须相等。
            (Some(want), Some(owner)) => want == owner,
            // 档位没记账号（旧数据）⇒ 只按站点归，见上面的文档。
            (_, None) => true,
            // 这一行还没登录（没有 account_id），而档位有主 ⇒ 不是它的。
            (None, Some(_)) => false,
        })
        .map(|owned| -> Result<TierInfo, AppError> {
            Ok(TierInfo {
                // ⚠️ **`app_id` 靠 `..owned.tier.clone()` 隐式继承**（来自
                // `list_tiers_impl`，那条路按 app 查所以值天然正确）。改成显式构造、
                // 或在中间插一层跨 app 的合并时**必须重新想清楚它** —— 那时它会静默
                // 变错（前端据它筛「属于当前那一屏的档位」），而没有测试会红。
                //
                // 「已手工维护」读存库标记（编辑页置位、恢复默认复位）。
                user_edited: Some(
                    state
                        .db
                        .get_user_edited(app_type.as_str(), &owned.tier.provider_id)?,
                ),
                ..owned.tier.clone()
            })
        })
        .collect()
}

// 曾经这里有个 `relay_list_tiers`（扁平列出全部档位，不按中转站分组）。
// 它在 `relay_list_relays` 上线后就没有调用方了 —— 界面按中转站分行显示，
// 拿一份不带归属的扁平列表没法渲染。2026-08-04 删掉命令壳，
// `list_tiers_impl` 留着（`list_relays_impl` 在用它）。

/// 内部版本额外带出每条档位的 `website_url`（= 所属站点的 origin），供
/// [`list_relays_impl`] 按站分组。命令层把它丢掉 —— 那是实现细节，不进对外契约。
///
/// `pub(crate)`：托盘的「模型」子菜单也从这里取目录（同一份 `modelCatalog` 两个消费者）。
pub(crate) fn models_from_settings(settings: &serde_json::Value) -> Vec<String> {
    settings
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("model").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect()
}

/// A LoongPort model-chip click is a managed preference, so refreshing the
/// tier should keep it while the newly fetched catalog still advertises it.
/// Once the upstream removes the model, the freshly computed default wins.
fn preserve_supported_codex_model(
    defaults: serde_json::Value,
    previous: &serde_json::Value,
) -> serde_json::Value {
    let Some(model) = provision::extract_model(previous) else {
        return defaults;
    };
    select_codex_model(&defaults, &model).unwrap_or(defaults)
}

/// [`preserve_supported_codex_model`] 的跨平台版：claude / gemini 走 env 形状
/// （剥掉 `[1M]` 声明后对新目录查成员资格），grokbuild 走 TOML 形状，其余平台
/// 没有「选模型」概念，新默认直接接管。
fn preserve_supported_model(
    app_type: &AppType,
    defaults: serde_json::Value,
    previous: &serde_json::Value,
) -> serde_json::Value {
    match app_type {
        AppType::Codex => preserve_supported_codex_model(defaults, previous),
        AppType::Claude | AppType::Gemini => {
            let catalog = models_from_settings(&defaults);
            provision::preserve_supported_env_model(app_type, defaults, previous, &catalog)
        }
        AppType::GrokBuild => preserve_supported_grok_model(defaults, previous),
        _ => defaults,
    }
}

/// [`preserve_supported_codex_model`] 的 grok 版：读旧选中值（TOML 选中模型表的
/// `model` 字段，[`provision::selected_model`]）对新目录查成员资格，命中就把新
/// 默认的该字段改回旧值 —— profile 名、端点、密钥都不动。
fn preserve_supported_grok_model(
    defaults: serde_json::Value,
    previous: &serde_json::Value,
) -> serde_json::Value {
    let Some(old) = provision::selected_model(&AppType::GrokBuild, previous) else {
        return defaults;
    };
    if !models_from_settings(&defaults).iter().any(|m| m == &old) {
        return defaults;
    }
    select_grok_model(&defaults, &old).unwrap_or(defaults)
}

fn select_codex_model(
    settings: &serde_json::Value,
    model: &str,
) -> Result<serde_json::Value, AppError> {
    let model = model.trim();
    if model.is_empty()
        || !models_from_settings(settings)
            .iter()
            .any(|candidate| candidate == model)
    {
        return Err(AppError::Config(format!(
            "模型 {model:?} 不在这个档位支持的模型列表中"
        )));
    }

    let config = settings
        .get("config")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Config("这个 Codex 档位缺少 config.toml".to_string()))?;
    let updated_config = crate::codex_config::update_codex_toml_field(config, "model", model)
        .map_err(AppError::Config)?;
    let mut updated = settings.clone();
    updated["config"] = serde_json::Value::String(updated_config);
    Ok(updated)
}

/// [`select_codex_model`] 的 grok 版（成员资格校验由调用方对着目录做，与 env 分支
/// 对齐）：改写 config TOML 里选中模型表的 `model` 字段。profile 名
/// （`models.default` 指向的表名）、端点、密钥、上下文窗口都与模型无关，不动；
/// toml_edit 保格式，用户的手工编辑不丢。
fn select_grok_model(
    settings: &serde_json::Value,
    model: &str,
) -> Result<serde_json::Value, AppError> {
    let config = settings
        .get("config")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Config("这个 Grok 档位缺少 config.toml".to_string()))?;
    let updated_config = crate::grok_config::update_selected_model_string(config, "model", model)?;
    let mut updated = settings.clone();
    updated["config"] = serde_json::Value::String(updated_config);
    Ok(updated)
}

fn list_tiers_impl(state: &AppState, app_type: AppType) -> Result<Vec<OwnedTier>, AppError> {
    // AppType 没派生 Copy（上游结构，别为此改它），所以 clone 一份给第二个调用点。
    let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();
    // 这条路按 app 查，所以结果天然同质 —— 每条档位的 `app_id` 就是被查的那个。
    // 先取出来：`app_type` 下一行就被 move 进 `list` 了。
    let app_id = app_type.as_str().to_string();
    let can_verify_models = verification_target::supports_app_type(&app_type);
    let providers = ProviderService::list(state, app_type.clone())?;

    let mut tiers: Vec<OwnedTier> = providers
        .values()
        .filter(|p| is_managed(p))
        .map(|p| OwnedTier {
            tier: TierInfo {
                provider_id: p.id.clone(),
                app_id: app_id.clone(),
                // 倍率读**上次 provision 写下的那个值**（`providers.tier_rate_multiplier`）。
                //
                // 它是服务端定价，不是实时量 —— 所以「刷新倍率」就等于「重新拉分组」，
                // 界面上是顶部刷新 / 更新可用分组 / 登录成功那几下。这条命令仍然
                // **只读本地不发网络**，首屏契约不变，但首屏现在就有倍率可显示了。
                //
                // 读不出来（旧库、行刚被别处删掉）⇒ `None`，UI 显示「倍率未知」。
                // **绝不能退化成 0** —— 那会让用户以为这是最便宜的一档。
                rate_multiplier: state
                    .db
                    .get_tier_rate_multiplier(&app_id, &p.id)
                    .unwrap_or(None),
                group_name: p.name.clone(),
                display_name: p.name.clone(),
                model: provision::selected_model(&app_type, &p.settings_config).unwrap_or_default(),
                // 目录没有就返回空 —— UI/托盘按「无目录」处理，不用按 app 分支
                models: models_from_settings(&p.settings_config),
                is_current: current == p.id,
                can_verify_models,
                // 判据要 `api_base_url`（按站点存），这里拿不到 ⇒ 留 None，
                // 由 `tiers_of_site` 在按站分组时填。见该字段的文档。
                user_edited: None,
                // **这个在本地就能算**（判据是配置里的 `model`），所以首屏就有真值 ——
                // 不像倍率那样留 None 等异步填。见该字段的文档：入口忽隐忽现是有害的。
                // 纯服务端信息，本地推不出来 ⇒ None（UI 不显示标记）。
                allow_image_generation: None,
                site_declared_origin: p
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.site_declared_origin.clone()),
            },
            site_origin: p.website_url.clone(),
            account_id: p.meta.as_ref().and_then(|m| m.loongport_account_id),
        })
        .collect();

    // 按 provision 时写下的 sort_index 排（倍率低的在前）。provider_id 是哈希，
    // 按它排等于随机顺序。
    let order: std::collections::HashMap<&str, usize> = providers
        .values()
        .map(|p| (p.id.as_str(), p.sort_index.unwrap_or(usize::MAX)))
        .collect();
    tiers.sort_by_key(|owned| {
        (
            order
                .get(owned.tier.provider_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
            owned.tier.provider_id.clone(),
        )
    });
    Ok(tiers)
}

/// 切换档位：退 ChatGPT → 切换 → 重开。
///
/// `quit_chatgpt` 由前端在用户确认弹窗后传 true。传 false 则只切换（用户自己管重启）。
///
/// `app` 是**必需参数**，不能从 `provider_id` 反推（spec §三）：
/// `provider_id_for(site_origin, group_id)` 不含 platform，而四段 Key 契约恰恰写明
/// 「分组 id 只在平台内唯一，跨平台会撞号」—— 所以同一个 `loongport-<hash>` 可以合法地
/// 存在于两个 app_type 行下（`providers` 主键是 `(id, app_type)`），哈希单向反解不出来。
/// 调用方（前端）本来就知道当前是哪个 tab。
#[tauri::command]
pub async fn relay_switch_tier(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchTierCommandResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_command(&app_handle, &provider_id, app_type, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}

/// Select a supported model from a managed Codex tier and activate that tier.
///
/// The model catalog stored with the provider is the authority for validation;
/// this keeps a stale frontend from writing an arbitrary model into
/// `config.toml`. Updating the provider before the normal switch flow also
/// means ChatGPT is restarted with the selected model already in place.
#[tauri::command]
pub async fn relay_switch_tier_model(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    model: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchTierCommandResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_model_command(&app_handle, &provider_id, app_type, &model, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}

/// [`relay_switch_tier_model`] 的命令层实现，托盘的模型子菜单也走这里 ——
/// 与 [`switch_tier_command`] 同样的「一个编排、多个入口」约定（见它的文档）。
pub(crate) async fn switch_tier_model_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: &str,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    if should_request_switch_confirmation(
        &app_type,
        user_choice,
        chatgpt_app::needs_user_attention(),
    ) {
        let state = app_handle.state::<AppState>();
        let target_name = state
            .db
            .get_provider_by_id(provider_id, app_type.as_str())?
            .map(|provider| format!("{} · {model}", provider.name))
            .unwrap_or_else(|| model.to_string());
        return Ok(SwitchTierCommandResult::ConfirmationRequired { target_name });
    }
    select_tier_model_impl(
        app_handle,
        provider_id,
        app_type,
        model,
        user_choice.unwrap_or(false),
    )
    .await
    .map(|result| SwitchTierCommandResult::Switched { result })
}

fn should_request_switch_confirmation(
    app_type: &AppType,
    user_choice: Option<bool>,
    needs_attention: bool,
) -> bool {
    matches!(app_type, AppType::Codex) && user_choice.is_none() && needs_attention
}

pub(crate) async fn switch_tier_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    let state = app_handle.state::<AppState>();
    let provider = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())?
        .ok_or_else(|| AppError::Config("这个接入配置不存在".to_string()))?;
    if should_request_switch_confirmation(
        &app_type,
        user_choice,
        chatgpt_app::needs_user_attention(),
    ) {
        return Ok(SwitchTierCommandResult::ConfirmationRequired {
            target_name: provider.name,
        });
    }
    switch_tier_impl(
        app_handle,
        provider_id,
        app_type,
        user_choice.unwrap_or(false),
    )
    .await
    .map(|result| SwitchTierCommandResult::Switched { result })
}

async fn select_tier_model_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: &str,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, AppError> {
    // 支持选模型的平台：codex（config TOML）/ claude（env.ANTHROPIC_MODEL）/
    // gemini（env.GEMINI_MODEL）/ grokbuild（config TOML 选中模型表的 model 字段）。
    // 目录（modelCatalog）各平台同一份形状。
    if !matches!(
        app_type,
        AppType::Codex | AppType::Claude | AppType::Gemini | AppType::GrokBuild
    ) {
        return Err(AppError::Config(
            "模型选择目前只支持 Codex / Claude / Gemini / Grok 档位".to_string(),
        ));
    }
    if !crate::relay::is_managed(provider_id) {
        return Err(AppError::Config(
            "只有 LoongPort 托管的档位才能从模型列表切换".to_string(),
        ));
    }

    let state = app_handle.state::<AppState>();
    let original_settings = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())?
        .ok_or_else(|| AppError::Config("这个档位不存在".to_string()))?
        .settings_config;
    let settings = match &app_type {
        AppType::Codex => select_codex_model(&original_settings, model)?,
        AppType::GrokBuild => {
            let bare = model.trim();
            if bare.is_empty()
                || !models_from_settings(&original_settings)
                    .iter()
                    .any(|candidate| candidate == bare)
            {
                return Err(AppError::Config(format!(
                    "模型 {bare:?} 不在这个档位支持的模型列表中"
                )));
            }
            select_grok_model(&original_settings, bare)?
        }
        // env 形状：成员资格对着目录校验（select_codex_model 内置，这里对齐）
        _ => {
            let bare = model.trim();
            if bare.is_empty()
                || !models_from_settings(&original_settings)
                    .iter()
                    .any(|candidate| candidate == bare)
            {
                return Err(AppError::Config(format!(
                    "模型 {bare:?} 不在这个档位支持的模型列表中"
                )));
            }
            provision::select_env_model(&app_type, &original_settings, bare)?
        }
    };

    state
        .db
        .update_provider_settings_config(app_type.as_str(), provider_id, &settings)?;

    match switch_tier_impl(app_handle, provider_id, app_type.clone(), quit_chatgpt).await {
        Ok(result) => Ok(result),
        Err(error) => {
            // Model selection is a managed preference, not a manual provider
            // edit. If the guarded switch fails, restore the DB value so a
            // later refresh cannot silently apply a model the user never
            // successfully switched to.
            if let Err(rollback_error) = state.db.update_provider_settings_config(
                app_type.as_str(),
                provider_id,
                &original_settings,
            ) {
                return Err(AppError::Config(format!(
                    "{error}；模型配置回滚失败：{rollback_error}"
                )));
            }
            Err(error)
        }
    }
}

/// 这次切换要不要退 ChatGPT。
///
/// 两个条件都得成立：**用户同意了**（`user_agreed`，来自确认弹窗），
/// **且切的是 codex**（`app_type`）。
///
/// ## 为什么 codex 之外不退
///
/// 不是「其它平台不支持」，是**不需要** —— `chatgpt_app` 管的是 ChatGPT 桌面版
/// （bundle id `com.openai.codex`），它只读 `~/.codex`。切 claude/gemini 的档位时
/// 它压根不涉及，去退它是扰民：关掉用户正开着的、与本次切换毫无关系的对话。
///
/// 判据放后端而不是让前端决定：前端传的 `user_agreed` 表达「用户同意了退出」，
/// 而「这个平台要不要退」是后端事实 —— 两件事别混在一个布尔里。
fn should_quit_chatgpt(user_agreed: bool, app_type: &AppType) -> bool {
    user_agreed && matches!(app_type, AppType::Codex)
}

async fn switch_tier_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, AppError> {
    let quit_chatgpt = should_quit_chatgpt(quit_chatgpt, &app_type);
    // `AppType` 没派生 Copy（上游结构，别为此改它），而下面 `ProviderService::list`
    // 会把它 move 掉 —— 事件那一步要用，先留一份。
    let app_type_for_event = app_type.clone();

    // 编排（退 → 切 → 重开，失败要把 ChatGPT 开回去）走 `chatgpt_app::around`。
    // 那套四分支处理原来内联在这里，抽出去是为了让**上游那条通用 provider 切换**
    // 也走同一份（`switch_provider` 里那处）—— 复制第二遍的必然结局是两份分叉，
    // 而分叉的表现是「从 LoongPort 页切没问题、从 provider 页切就静默用错配置」。
    //
    // `abort_on_unconfirmed_exit = false`：切档位只写 `config.toml`，退不掉也能照常切
    // （配置写进去就生效了），提示用户手动重启即可。这与「切回官方登录」相反 ——
    // 那条要删 `auth.json`，而 ChatGPT 退出时会重写它。
    let switch_once = || {
        let state = app_handle.state::<AppState>();
        ProviderService::switch(&state, app_type.clone(), provider_id)
            .map_err(|e| AppError::Config(format!("切换失败：{e}。配置未改动")))
    };

    let (switched, chatgpt) = if quit_chatgpt {
        chatgpt_app::around(false, switch_once)?
    } else {
        // 不需要碰 ChatGPT（非 codex，或用户选了「只切换」）：直接切，
        // outcome 全默认（没关过 ⇒ 不重开）。
        (switch_once()?, chatgpt_app::AroundOutcome::default())
    };

    let mut warnings = chatgpt.warnings;
    warnings.extend(switched.warnings);

    let provider_name = {
        let state = app_handle.state::<AppState>();
        ProviderService::list(&state, app_type)
            .ok()
            .and_then(|list| list.get(provider_id).map(|p| p.name.clone()))
            .unwrap_or_else(|| provider_id.to_string())
    };

    // 广播「当前供应商变了」—— **镜像方向**：切档位之后 provider 页那份列表
    // （react-query 的 `["providers", app]`）也陈旧了，而它的刷新靠的正是这个事件
    // （`App.tsx` 的 `providersApi.onSwitched`）。
    //
    // 两条切换路径都发，共用 `commands::provider::emit_provider_switched` 那一份实现 ——
    // payload 形状复制第二遍的必然结局是两份分叉（那边的文档写了完整理由）。
    emit_provider_switched(app_handle, &app_type_for_event, provider_id);

    // 托盘也要跟上：这里不刷，用户从主界面切完档位、再看托盘标题还是旧的
    // （前端 relay 那条路不会替我们调 `update_tray_menu`）。放在 `switch_tier_impl`
    // 而不是各命令壳里 —— relay / vendor / 托盘三个入口一次全修，后来者免接。
    crate::tray::refresh_tray_menu(app_handle);

    Ok(SwitchTierResult {
        provider_name,
        chatgpt_was_running: chatgpt.was_running,
        chatgpt_relaunched: chatgpt.relaunched,
        warnings,
    })
}

/// 列出全部已添加的站点。
#[tauri::command]
pub fn relay_list_sites(state: State<'_, AppState>) -> Result<Vec<SiteInfo>, String> {
    list_sites_impl(state.inner()).map_err(|e| e.to_string())
}

fn list_sites_impl(state: &AppState) -> Result<Vec<SiteInfo>, AppError> {
    with_conn(state, |conn| {
        let mut summaries = Vec::<SiteInfo>::new();
        for relay in creds::list(conn)? {
            // 按注册域身份聚行：同一站的 www/apex/api 拼写变体算一家，不再拆行。
            if let Some(summary) = summaries.iter_mut().find(|summary| {
                same_site_identity(Some(&summary.site_origin), Some(&relay.site_origin))
            }) {
                summary.account_count += usize::from(relay.account_id.is_some());
            } else {
                summaries.push(SiteInfo {
                    site_origin: relay.site_origin,
                    account_count: usize::from(relay.account_id.is_some()),
                });
            }
        }
        Ok(summaries)
    })
}

/// 删掉一个站点，**连带它已生成的托管档位**。
///
/// ## 为什么连带删（2026-08-03 改，原来是只删站点）
///
/// 原来的行为是「不删 provider 记录，要清理走 provider 列表」。那条在只有一个入口
/// （站点切换器上的小叉）时说得通，但中转站行现在有了自己的删除按钮，而用户对那个
/// 按钮的预期是「这一行连它下面那几个档位一起没了」—— 留下一堆没有主人的托管档位
/// （登录态已经删了 ⇒ 它们必定 401），比删干净糟。
///
/// **只删这个站的托管档位**，判据是 `website_url == site_origin`（`prune_stale_tiers`
/// 里写清了为什么 `provider_id` 反推不出归属）。用户自建的 provider 一律不碰。
///
/// ## ⚠️ 「在用的档位」有两条出路：默认拦下；`force` 才放行 —— 闸始终在后端
///
/// 前端的判据只看当前 tab（`RelayRow::usageBlockers` 虽是后端跨 app 算的，但按钮态
/// 仍可能被绕过 / 旧版本前端没有它）。于是：
///
/// 1. 用户在 **Claude** tab 上看某一行 —— 它在 claude 下没有当前项；
/// 2. 而同一个账号在 **Codex** 下的档位正是 codex 的当前项；
/// 3. 删下去 ⇒ codex 的当前 provider 记录没了，而 `~/.codex/config.toml` 还指着它。
///
/// 「删 Claude 页的账号，把 Codex 正在用的配置删掉了」对用户是不可预期的，所以这里
/// **必须自己查一遍全部 app**。查到在用时的两条出路：
///
/// - **默认（`force == false`）**：报错拦下，文案点名平台与档位，让用户先去切走。
/// - **`force == true`**：放行。它只该来自前端那道**点名了在用 app 的确认弹窗**
///   （`RelayRow::usage_blockers` 与这道闸同一次扫描，弹窗文案由它驱动 —— 用户
///   是看着「Codex 正在用 BestApi · Pro」这句话按的确认），即知情删除。
///
/// ## 与 `prune_stale_tiers` 「当前项也删」的区别（两者都对，因为前提不同）
///
/// `prune_stale_tiers` 在 provision 路径上会删当前项，理由是走到那一步说明**服务端已经
/// 没有那个分组了** ⇒ 它的 sk 是死的，留着当当前项只会让 CLI 拿废密钥去 401（见它的文档）。
///
/// 删账号这条路的前提相反：那些档位**还是好的**。默认路径因此拦下（不忍心毁一份能用的
/// 配置）；`force` 路径是用户在弹窗里**看着点名**做出的选择 —— 删掉一条还活着的当前项
/// 正是他要的（「我不想再用这个站了」），与那条裁决不矛盾 —— 判据从「档位还活着吗」
/// 变成「用户知道自己在删什么吗」。
///
/// 顺序：**（force 时）先切官方 → 先删档位再删站点**。反过来的话，站点行没了而
/// `site_origin` 是档位归属的唯一依据 —— 删站点之后就再也认不出哪些档位属于它，
/// 那些记录会永久留在 provider 列表里。切官方放在删档位之前，则是让被安置的档位
/// 先卸下「当前项」身份、删除全走常规路径（见 [`switch_affected_apps_to_official`]）。
#[tauri::command]
pub fn relay_remove_site(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
    force: Option<bool>,
) -> Result<(), String> {
    remove_site_impl(state.inner(), id, force.unwrap_or(false), Some(&app))
        .map_err(|e| e.to_string())
}

fn remove_site_impl(
    state: &AppState,
    id: i64,
    force: bool,
    // `None` = 单测（没有 AppHandle）：切官方照常执行，只是不发事件、不刷托盘。
    app: Option<&tauri::AppHandle>,
) -> Result<(), AppError> {
    // 先取归属信息 —— 删掉那行之后就没法知道该清哪些档位了。
    //
    // ⚠️ **`account_id` 与 `site_origin` 一样必须取**：删的是**一个账号**（一行），
    // 不是「这个站的全部」。同站另一个账号的档位不该被连带清掉 ——
    // 那正是 `prune_stale_tiers` 加账号维度要挡的事（见它的文档）。
    let op = with_conn(state, |conn| creds::get(conn, id))?
        .ok_or_else(|| AppError::Config("这个站点已经不存在了".into()))?;
    let site_origin = op.site_origin;
    let account_id = op.account_id;

    // ⚠️ 闸：这个账号名下有档位正被某个 app 用着 ⇒ 默认**一条都不删，直接报错**；
    // `force == true` 才放行（只该来自点名了在用 app 的前端确认弹窗，见命令文档）。
    //
    // 全有或全无（在 `prune_stale_tiers` 之前拦，而不是让它逐条跳过正在用的那些）：
    // 半删的结果是「账号行没了，名下还剩一条孤儿档位」—— 而托管档位在 provider 列表里
    // 被前端过滤、被通用删除命令拒绝 ⇒ 那条记录用户再也处置不了。
    //
    // 文案点名**哪个平台、哪个档位**：只说「有档位在使用中」的话，用户得自己去六个 tab
    // 里翻是哪一个。他要做的处置（切走 / 或回弹窗里选强制删）完全取决于这个信息。
    //
    let in_use = apps_using_this_accounts_tiers(state, &site_origin, account_id);
    if !in_use.is_empty() && !force {
        let detail = in_use
            .iter()
            .map(|(app_type, name)| format!("{}（{}）", name, app_type.as_str()))
            .collect::<Vec<_>>()
            .join("、");
        return Err(AppError::Config(format!(
            "这个账号名下还有档位正在使用中：{detail}。请先在对应平台切换到别的供应商，再删除这个账号。"
        )));
    }

    // force 路径对「被删掉的当前项」的安置：**先切回官方 seed provider，再删档位**
    // （[`switch_affected_apps_to_official`] 的文档写了完整理由 —— 删本地登录态不会
    // 吊销服务端 sk，留旧配置等于 CLI 继续把请求打向一个用户明确要删掉的站）。
    // 切过去之后档位不再是当前项，`prune_stale_tiers` 全走常规删除路径。
    //
    // 切不动的（codex-image 无官方 seed / seed 被删 / 代理接管禁止切官方）降级成
    // 悬空自愈（`get_effective_current_provider`）—— 宁可降级也不让「删账号」失败。
    if force && !in_use.is_empty() {
        let switched = switch_affected_apps_to_official(state, &in_use);
        if let Some(app) = app {
            for (app_type, provider_id) in &switched {
                // 背后切了 current 就必须广播（2026-08-04 的教训：不发事件的症状是
                // provider 页 / 中转站页静默陈旧，用户以为删除功能坏了）。
                emit_provider_switched(app, app_type, provider_id);
            }
            if !switched.is_empty() {
                crate::tray::refresh_tray_menu(app);
            }
        }
    }

    // 空 `keep` = 这一行名下的托管档位全不保留。
    //
    // 清理失败（`delete_provider` 报错）**不阻止删站点**：那会让用户卡在「删不掉这一行」
    // 上，而他要的是把这个账号清走，站点行也只有这一个入口。
    //
    // ⚠️ **代价要说清：残留的档位用户自己处置不了**。托管档位在 provider 列表里被前端
    // 按 id 前缀过滤掉（`ProviderList.tsx`），`delete_provider` / `update_provider` 也会
    // 被 `reject_if_managed` 拦下 ⇒ 那条记录既看不见也删不掉，只能靠下一次对这个站
    // provision 时的 `prune_stale_tiers` 顺手清（而账号行已经删了 ⇒ 不会再有那一次）。
    //
    // 仍选「不阻断」是因为走到这里的前提已经很窄：上面那道闸保证了它不是任何平台的当前项，
    // 所以残留的是一条**没人在用**的死记录 —— 它不会让任何 CLI 出错，只是脏。
    // 而阻断的代价是用户永久删不掉这个账号。两害相权取其轻，但**别把它写成没有代价**。
    match prune_stale_tiers(
        state,
        &site_origin,
        account_id,
        &std::collections::HashSet::new(),
    ) {
        Ok(removed) => log::info!("删除站点 {site_origin} 时清掉了 {removed} 个托管档位"),
        Err(e) => log::warn!("删除站点 {site_origin} 时清理档位失败（站点仍会删掉）: {e}"),
    }

    // 生图工具跟着对齐一次 —— **这条路必须自己调**（review 抓出）。
    //
    // 另一个调用点在 `do_provision` 收尾，但那条路依赖「还会再 provision 一次」。
    // 删掉的正是拥有生图档位的那个账号时，**不会再有下一次** ⇒ `loongport-imagegen`
    // 这条 MCP 记录永久留在用户的 CLI 配置里，而它每次被调用都报「还没有选定用哪个
    // 档位生图」—— 一个删不掉的坏工具。
    //
    // 失败只 warn：站点记录马上就删了，不该因为一个 MCP 记录撤不掉而让「删站点」失败
    // （与上面那段清理档位同一条原则）。
    if let Err(e) = imagegen_mcp::sync_registration(state) {
        log::warn!("删除站点 {site_origin} 后同步生图工具记录失败: {e}");
    }

    with_conn(state, |conn| creds::remove(conn, id))
}

/// force 删的收尾一步：把受影响的 app 切回各自的官方 seed provider。
///
/// ## 为什么是「切官方」而不是留着旧配置
///
/// 删登录态只是本地动作，**服务端的 sk 并没有被吊销** —— 留着 live 旧配置等于
/// CLI 继续把请求（和扣费）打向一个用户刚刚明确删掉的站，那才是违背「我不想再
/// 用这个站」的意图。官方 seed 的「空 env / 空 config」正是该 CLI 刚装好时的
/// 默认认证状态（seed 注释原文）：纯中转站用户下次运行会被引导登录；本来就有
/// 官方登录的直接用回它（那份登录与中转站无关，保留正是预期）。
///
/// ## 复用与边界
///
/// live 配置怎么清、current 怎么写、MCP 怎么同步全是 `ProviderService::switch`
/// 现成的，这里只编排「谁该被切」。没有官方 seed 的 app（codex-image 生图）跳过；
/// 切失败（seed 被用户删了 / 代理接管下 switch 主动拒绝切官方 —— 防封号）记日志
/// 降级成悬空自愈，**不让删账号失败** —— 安置是收尾，不是闸。
///
/// 返回成功切到官方的 `(app_type, official_id)`，调用方拿它发 `PROVIDER_SWITCHED`
/// 与刷托盘。
fn switch_affected_apps_to_official(
    state: &AppState,
    affected: &[(AppType, String)],
) -> Vec<(AppType, String)> {
    let mut switched = Vec::new();
    for (app_type, tier_name) in affected {
        let Some(official_id) = crate::database::official_seed_id(app_type) else {
            log::info!(
                "{} 没有官方 seed 可回落（原在用档位「{tier_name}」随删除悬空自愈）",
                app_type.as_str()
            );
            continue;
        };
        match ProviderService::switch(state, app_type.clone(), official_id) {
            Ok(result) => {
                for warning in &result.warnings {
                    log::info!("强删后 {} 切回官方的提示：{warning}", app_type.as_str());
                }
                log::info!(
                    "强删后把 {} 切回官方 provider（原在用档位：{tier_name}）",
                    app_type.as_str()
                );
                switched.push((app_type.clone(), official_id.to_string()));
            }
            Err(e) => {
                log::warn!(
                    "强删后把 {} 切回官方失败（current 将悬空自愈，不影响删除）：{e}",
                    app_type.as_str()
                );
            }
        }
    }
    switched
}

/// 一行名下**全部托管档位**里的 base_url 与 sk。
///
/// 归属判据走 [`belongs_to_relay`]（严格版：未登录的行不认别人账号的档位），但
/// **跨全部 app 扫** —— 同一行的档位可能只挂在某一个 CLI 下（用户只给 codex 生成过 sk），
/// 按单个 app 查会在别的 app 上空手而归，让余额白白落到下一条路。
///
/// 顺序不稳定不要紧：调用方（[`crate::relay::balance::resolve`]）是并发试完取第一个
/// 拿到结果的，同一行的每把 sk 问出的钱包余额是同一个账户的同一个数。
fn relay_balance_inputs(state: &AppState, relay: &creds::Relay) -> (String, Vec<String>) {
    let mut base_url = None;
    let mut keys: Vec<String> = Vec::new();
    for app_type in AppType::all() {
        let Ok(providers) = ProviderService::list(state, app_type.clone()) else {
            continue;
        };
        for provider in providers.values() {
            if !belongs_to_relay(provider, &relay.site_origin, relay.account_id) {
                continue;
            }
            if let Some(sk) = provision::extract_api_key(&provider.settings_config, &app_type) {
                if base_url.is_none() {
                    base_url = crate::proxy::providers::get_adapter(&app_type)
                        .and_then(|adapter| adapter.extract_base_url(provider).ok());
                }
                if !sk.trim().is_empty() && !keys.contains(&sk) {
                    keys.push(sk);
                }
            }
        }
    }
    (base_url.unwrap_or_default(), keys)
}

/// 余额。`relay_id` 指定查**哪一行**的。
///
/// 与 [`relay_login`] / [`relay_provision`] 同一套纪律：显式指定查不到就报错，绝不
/// 回落到其它站点 —— 那会把 B 的余额显示在 A 那一行上，比报错更糟。
///
/// ## 一行一次请求是安全的
///
/// `/user/profile` **没挂 `Heavy()`**，只吃 `panelRateLimiter.Global()`
/// （sub2api 默认 `UserRPM = 240/分钟`，按 user_id 计数）—— 而且不同中转站行往往是
/// **不同用户**，各记各的额度。N 行各打一次远远碰不到限流。
///
/// ## 为什么返回 [`UsageResult`] 而不是 `api::Balance`
///
/// 这是本轮最主要的收敛（全局准则 §1.4）。原来中转站行回 `{balance, frozenBalance}`
/// 数字、官网行回**后端已格式化好的字符串** `"¥547.08"` —— 同一个事实两套契约，
/// 于是前端也就有两份余额 state、两个 effect、两处渲染。改成两类行都回上游那个
/// [`UsageResult`] 之后，前端只剩一个 hook 一个组件，还顺带白拿了 provider 页
/// 那套用量条（上次查询时间 + 手动刷新按钮）。
///
/// ## **不走 [`usable_relay`]，因此登录态过期也能查**
///
/// 这是本轮的目的本身。`usable_relay` 会校验登录态、过期就报错 —— 而 sk 是独立凭据，
/// 登录态过期时它照样能调用。所以这里只读那一行的记录（[`creds::get`]），把
/// 「有没有可用登录态」交给 [`crate::relay::balance::resolve`] 的第 3 步自己判：
/// 前两步不需要登录态，第 3 步需要，语义落在那一步里而不是拦在门口。
///
/// **不返回 `Err`**（除了「这一行不存在」）：三条路都失败时回 `success:false`，
/// 前端才有失败态可渲染、有刷新按钮可点。见 [`crate::relay::balance`] 模块文档。
#[tauri::command]
pub async fn relay_balance(
    app_handle: tauri::AppHandle,
    relay_id: i64,
) -> Result<balance::RowBalanceResult, String> {
    relay_balance_impl(&app_handle, relay_id)
        .await
        .map_err(|error| error.to_string())
}

async fn relay_balance_impl<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
) -> Result<balance::RowBalanceResult, AppError> {
    let (relay, base_url, api_keys) = {
        let state = app_handle.state::<AppState>();
        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {relay_id} 的中转站")))?;
        let (base_url, api_keys) = relay_balance_inputs(&state, &relay);
        (relay, base_url, api_keys)
    };

    let usage = balance::resolve(
        balance::BalanceQuery {
            site_origin: &relay.site_origin,
            base_url: &base_url,
            api_keys: &api_keys,
        },
        // 登录态还在才给第 3 步（JWT 路）。空 token 就别让它白打一个必定 401 的请求。
        if relay.auth_token.trim().is_empty() {
            balance::SessionFallback::None
        } else {
            balance::SessionFallback::Relay(&relay)
        },
    )
    .await
    .usage;

    // 余额快照是扣费对账的旁路采样，挂在成功解析之后：写入条件与失败兜底都在
    // `reconcile::capture_balance_snapshot` 里，这里只出一条调用，不拖垮余额显示。
    reconcile::capture_balance_snapshot(&app_handle.state::<AppState>().db, relay_id, &usage);

    Ok(balance::row_balance_result(usage, true))
}

/// 带登录态打开某个中转站的充值页。
///
/// `relay_id` 指定给**哪一行**充值。与 [`relay_login`] / [`relay_balance`] 同形
/// 同纪律：显式指定查不到就报错、绝不回落到当前站 —— 那会让用户在 B 行点充值、
/// 钱充进 A 账号。
///
/// 返回 `Ok(())` 只表示**窗口开出来了**，不表示用户付了钱。
/// 我们**有意不做支付成功感知**（维护者裁决）：关窗时刷一次余额就够，
/// 充完钱余额自然会涨。
#[tauri::command]
pub async fn relay_purchase(app_handle: tauri::AppHandle, relay_id: i64) -> Result<(), String> {
    open_purchase_window(&app_handle, relay_id)
        .await
        .map_err(|e| e.to_string())
}

/// 带登录态开这一行的**用量页**（「查看用量」）。
///
/// 与充值窗同一套机制与纪律（[`relay_purchase`]），只有两处不同：入口由签名配置的
/// `usage_url` 决定、窗口是独立的用量窗（付钱页不会被顶掉）。用途见 design TODO
/// 「查看用量外链」：站点自己的用量页比客户端逐条对账更全，也避免重算口径
/// 与账单不一致造成的纠纷。
#[tauri::command]
pub async fn relay_open_usage(app_handle: tauri::AppHandle, relay_id: i64) -> Result<(), String> {
    open_usage_window(&app_handle, relay_id)
        .await
        .map_err(|e| e.to_string())
}

/// 应用站长自报调用配置（`relay/site_config.rs`）的摘要：哪些档位吃到了声明段。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteConfigAppliedTier {
    pub app_id: String,
    pub provider_id: String,
    pub display_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteConfigApplySummary {
    pub site_origin: String,
    pub declared_origin: String,
    pub applied: Vec<SiteConfigAppliedTier>,
}

/// 应用站长自报的调用配置：用户贴入站长给的 URL / JSON / base64，把站长维护的
/// 各平台默认配置同步到该站点的托管档位上。
///
/// 与自动路径（首次导入探测，见 `persist_provision_batch`）的语义差异：这是用户
/// **显式动作**，允许覆盖用户编辑过的配置——用户明确要站长的这套；自动路径永远
/// 不碰用户编辑。双重同源校验：手输 URL 先拦（不发往第三方域），内容里声明的
/// `site_origin` 再拦（防内容冒名）。只对**该站点的托管档位**生效（`website_url`
/// 等值匹配，宁漏配不误配）。
#[tauri::command]
pub async fn relay_apply_site_config(
    app_handle: tauri::AppHandle,
    relay_id: i64,
    input: String,
) -> Result<SiteConfigApplySummary, AppError> {
    let op = usable_relay(&app_handle, relay_id).await?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "请粘贴站长提供的配置链接或内容".into(),
        ));
    }
    let declared = if trimmed.starts_with("https://") {
        site_config::validate_same_origin(trimmed, &op.site_origin)?;
        site_config::fetch_declaration_from_url(trimmed).await?
    } else {
        site_config::parse_site_config(trimmed)?
    };
    site_config::validate_same_origin(&declared.site_origin, &op.site_origin)?;

    let state = app_handle.state::<crate::store::AppState>();
    let mut applied = Vec::new();
    // 声明里出现的每个平台 → 对应 app 的该站点托管档位逐个应用。
    // platform 键来自 parse_platform（schema 合法键集），app_type() 拿 Mapped 目标。
    let platforms: Vec<_> = declared
        .platforms
        .keys()
        .filter_map(|key| platform_map::parse_platform(key))
        .collect();
    for platform in platforms {
        let Some(app_type) = platform.app_type() else {
            continue;
        };
        let Some(segment) = declared.segment_for(platform) else {
            continue;
        };
        let providers = ProviderService::list(&state, app_type.clone())?;
        for mut provider in providers.into_values() {
            if !is_managed(&provider) {
                continue;
            }
            if provider.website_url.as_deref() != Some(op.site_origin.as_str()) {
                continue;
            }
            if !site_config::apply_segment_to_app(
                &app_type,
                segment,
                &mut provider.settings_config,
            )? {
                continue;
            }
            let meta = provider.meta.get_or_insert_with(Default::default);
            meta.site_declared_origin = Some(declared.site_origin.clone());
            state
                .db
                .save_provider(app_type.as_str(), &provider)
                .map_err(|e| AppError::Config(format!("保存档位 {} 失败: {e}", provider.name)))?;
            applied.push(SiteConfigAppliedTier {
                app_id: app_type.as_str().to_string(),
                provider_id: provider.id.clone(),
                display_name: provider.name.clone(),
            });
        }
    }
    if applied.is_empty() {
        return Err(AppError::Config(
            "该站点下没有可应用声明的托管档位——先登录或获取密钥建立档位".into(),
        ));
    }
    Ok(SiteConfigApplySummary {
        site_origin: op.site_origin,
        declared_origin: declared.site_origin,
        applied,
    })
}

/// 从现有 settings_config 提取重建默认所需的 (api_key, base_url, model)。
///
/// 「恢复内置默认」要把档位重建回 `settings_config_for` 的形状，但 sk 与端点
/// 必须原样保留（它们来自用户登录，重建不是换钥匙）。四个 app 的读取位置与
/// `persist_provision_batch` 的写入位置一一对应；读不出任何一项就返回 `None`
///（调用方跳过该档位——重建出一把没有钥匙的默认配置比不重建更糟）。
fn rebuild_inputs_from_settings(
    app_type: &AppType,
    settings: &serde_json::Value,
) -> Option<(String, String, String)> {
    let env = settings.get("env").and_then(|v| v.as_object());
    match app_type {
        AppType::Claude | AppType::ClaudeDesktop => {
            let env = env?;
            let api_key = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))?
                .as_str()?;
            let base_url = env.get("ANTHROPIC_BASE_URL")?.as_str()?;
            let model = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str())?;
            Some((api_key.to_string(), base_url.to_string(), model.to_string()))
        }
        AppType::Gemini => {
            let env = env?;
            let api_key = env.get("GEMINI_API_KEY")?.as_str()?;
            let base_url = env.get("GOOGLE_GEMINI_BASE_URL")?.as_str()?;
            let model = env.get("GEMINI_MODEL").and_then(|v| v.as_str())?;
            Some((api_key.to_string(), base_url.to_string(), model.to_string()))
        }
        AppType::Codex | AppType::CodexImage => {
            let api_key = settings
                .get("auth")?
                .get("OPENAI_API_KEY")?
                .as_str()?
                .to_string();
            let toml_text = settings.get("config")?.as_str()?;
            let toml_value: toml::Value = toml::from_str(toml_text).ok()?;
            let base_url = toml_value
                .get("model_providers")?
                .get("custom")?
                .get("base_url")?
                .as_str()?
                .to_string();
            let model = toml_value.get("model")?.as_str()?.to_string();
            Some((api_key, base_url, model))
        }
        AppType::GrokBuild => {
            let inner: serde_json::Value =
                serde_json::from_str(settings.get("config")?.as_str()?).ok()?;
            let api_key = inner.get("apiKey")?.as_str()?.to_string();
            let base_url = inner.get("baseUrl")?.as_str()?.to_string();
            let model = provision::selected_model(app_type, settings).unwrap_or_default();
            Some((api_key, base_url, model))
        }
        _ => None,
    }
}

/// 恢复内置默认：把该站点的全部托管档位重建回 `settings_config_for` 的形状
/// （sk / 端点 / 当前模型选择原样保留），并清掉「站点推荐配置」标注。
///
/// 「一键回退」的数据面（spec：站点声明的值可发现、可回退）。与
/// [`relay_apply_site_config`] 对称：一个把站长声明合进来，一个退回去。
#[tauri::command]
pub async fn relay_reset_site_config(
    app_handle: tauri::AppHandle,
    relay_id: i64,
) -> Result<SiteConfigApplySummary, AppError> {
    let op = usable_relay(&app_handle, relay_id).await?;
    let state = app_handle.state::<crate::store::AppState>();

    let mut applied = Vec::new();
    for app_id in ["codex", "claude", "gemini", "grokbuild"] {
        let app_type = AppType::from_str(app_id)
            .map_err(|e| AppError::Config(format!("app 类型不认: {e}")))?;
        let providers = ProviderService::list(&state, app_type.clone())?;
        for mut provider in providers.into_values() {
            if !is_managed(&provider) {
                continue;
            }
            if provider.website_url.as_deref() != Some(op.site_origin.as_str()) {
                continue;
            }
            let Some((api_key, base_url, model)) =
                rebuild_inputs_from_settings(&app_type, &provider.settings_config)
            else {
                log::warn!(
                    "{} 的配置读不出重建要素（sk/端点/模型），跳过恢复默认",
                    provider.name
                );
                continue;
            };
            let Some(defaults) = provision::settings_config_for(
                &app_type,
                &api_key,
                &provider.name,
                &base_url,
                &model,
            ) else {
                continue;
            };
            provider.settings_config = defaults;
            if let Some(meta) = provider.meta.as_mut() {
                meta.site_declared_origin = None;
            }
            state
                .db
                .save_provider(app_type.as_str(), &provider)
                .map_err(|e| AppError::Config(format!("保存档位 {} 失败: {e}", provider.name)))?;
            applied.push(SiteConfigAppliedTier {
                app_id: app_type.as_str().to_string(),
                provider_id: provider.id.clone(),
                display_name: provider.name.clone(),
            });
        }
    }
    if applied.is_empty() {
        return Err(AppError::Config("该站点下没有可恢复默认的托管档位".into()));
    }
    Ok(SiteConfigApplySummary {
        site_origin: op.site_origin,
        declared_origin: String::new(),
        applied,
    })
}

async fn open_purchase_window<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
) -> Result<(), AppError> {
    let op = usable_relay(app_handle, relay_id).await?;

    // 充值页直接承载付款动作，它指向哪由**签名配置**说了算 —— 客户端不再读站点
    // 公开设置的支付开关去推测 `/purchase` 还是 `/redeem`（那是在替站长决定入口）。
    // 配置没加载 / 这个站没配入口都明确报错，绝不回落到猜测的路由。
    let config = remote_config::load_cached()
        .ok_or_else(|| AppError::Config("中转站配置尚未加载，暂时无法打开充值入口".into()))?;
    let purchase_url = remote_config::configured_purchase_url(&config, &op.site_origin)?
        .ok_or_else(|| AppError::Config("该中转站尚未配置充值入口".into()))?;

    let window = purchase::purchase_window(relay_id, &op.site_origin);
    dispatch_site_window(app_handle, op, window, purchase_url).await
}

async fn open_usage_window<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
) -> Result<(), AppError> {
    let op = usable_relay(app_handle, relay_id).await?;

    // 用量页入口同样由签名配置拥有（sub2api 是 /usage，New API 是各自 console
    // 路由）——路由事实在站方，客户端不猜。
    let config = remote_config::load_cached()
        .ok_or_else(|| AppError::Config("中转站配置尚未加载，暂时无法打开用量入口".into()))?;
    let usage_url = remote_config::configured_usage_url(&config, &op.site_origin)?
        .ok_or_else(|| AppError::Config("该中转站尚未配置用量入口".into()))?;

    let window = purchase::usage_window(relay_id, &op.site_origin);
    dispatch_site_window(app_handle, op, window, usage_url).await
}

/// 按协议分派**站点页面窗**开窗；窗口身份（label/标题）与目标 URL 都必须由
/// 调用方解析后传入（充值 / 用量两个入口各自的 `open_*_window` 负责）。
///
/// 拆出这个接缝与 `open_sub2api_site_window` 的「参数化只为可测」同一惯例：
/// 生产 `load_cached()` 用生产公钥验签，测试无法（也不该）伪造一份能过验签的缓存，
/// 所以协议分派的回归测试直接驱动本函数、自己构造内存里的 `RemoteConfig`。
async fn dispatch_site_window<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    op: creds::Relay,
    window: purchase::SiteWindow,
    target_url: url::Url,
) -> Result<(), AppError> {
    // ⭐ 同一行第二击：聚焦现有窗口，不做任何协议相关工作 —— 不发 HTTP、不取 lease。
    //
    // 这段检查原先在 `open_sub2api_purchase_window` 内部（协议分派之后才跑），上移到
    // 分派层有两个理由：
    // 1. NewAPI 与 sub2api 共用同一个 label 空间（`purchase::window_label`），聚焦
    //    检查对两种协议同样必要，放两处迟早分叉；
    // 2. 原顺序下第二击会先打「续期 + 档案」两个 sub2api 请求才聚焦 —— 白白发 HTTP，
    //    NewAPI 那边更糟：续期会轮换 refresh cookie，正是 lease 闸要防的那类冲突。
    //
    // 为什么聚焦而不是销毁重开：充值窗背后是**已经发生的钱**（见
    // `open_sub2api_site_window` 里那条注释）；用量窗同理——用户可能正读到一半。
    if let Some(existing) = app_handle.get_webview_window(&window.label) {
        log::info!(
            "这一行的站点窗（{}）已经开着，聚焦它而不是重开",
            window.label
        );
        // 可能被用户最小化或藏到别的 Space 了，先 show 再 focus ——
        // `set_focus` 对不可见窗口是 no-op。
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
        return Ok(());
    }

    match op.backend_kind {
        creds::BackendKind::Sub2Api => {
            open_sub2api_site_window(app_handle, op, window, target_url).await
        }
        // NewAPI 的站点窗是「cookie 形态登录态 + 轮换跟踪」的另一套实现
        // （`relay::newapi_purchase`，接线顺序的理由见它的模块文档）。
        // 空白 refresh credential 在建窗前由 `newapi_purchase::open` 拒绝
        // （含「重新登录」文案）—— lease 在那之后才被消费。
        creds::BackendKind::NewApi => {
            let state = app_handle.state::<AppState>();
            let lease = state.purchase_sessions.try_acquire(op.id)?;
            newapi_purchase::open(app_handle, op, window, target_url, lease).await
        }
    }
}

/// 打开某个 sub2api 中转站的站点页面窗（登录态注入版）。
///
/// 窗口身份与目标 URL 必须由调用方解析后传入 —— 本函数**不做路由选择**。
/// 拆出这个接缝与 `remote_config::load_cached_with` 的「参数化只为可测」同构：
/// 生产 `load_cached()` 用生产公钥验签，测试无法（也不该）伪造一份能过验签的缓存，
/// 所以回归测试直接驱动本函数、自己构造内存里的 `RemoteConfig`。
async fn open_sub2api_site_window<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    op: creds::Relay,
    window: purchase::SiteWindow,
    target_url: url::Url,
) -> Result<(), AppError> {
    // ⚠️ **充值是长会话，`usable_relay` 的余量对它不够**（review 抓出）。
    //
    // 那个函数的判据是「还剩 > 60 秒」—— 对「发一次请求」够用，但充值页会挂着几分钟
    // 到几十分钟（等用户扫码转账、等网关回调），期间它每隔几秒轮询一次订单状态。
    // 而我们**有意不注入 refresh_token**（见 `purchase.rs` 模块文档第 2 条）⇒
    // 那个页面自己没有续期能力，access token 一到期就会被 401 拦截器清掉登录态、
    // 打断付款流程，而钱可能已经付出去了。
    //
    // 所以在开窗前主动要一次续期：不看「现在还能不能用」，看「够不够撑完一次付款」。
    // 拿不到更长的 token 也照样开窗 —— 那时用户至少还能完成一笔快的（扫码即付），
    // 硬拦住他反而是把「可能不够」当成「一定不行」。
    let op = ensure_token_outlasts_a_payment(app_handle, op).await;

    // 先取账号档案。**必须在开窗之前** —— 站点的 router 守卫在页面启动那一刻就读
    // localStorage，注入脚本必须在那之前就带着完整的值。拿不到就别开窗：
    // 开一个注定落到登录页的窗口，用户只会以为「点了充值却要我重新登录」。
    let client = api::Client::new(
        &op.site_origin,
        &op.auth_token,
        op.account_id,
        op.user_agent.as_deref(),
        op.cf_clearance.as_deref(),
    )?;
    let auth_user = purchase::auth_user_from_profile(client.profile_raw().await?)?;

    // 「同一行第二击聚焦现有窗口」的检查在 `dispatch_site_window`（分派层）—— 两种协议
    // 共用同一 label 空间，检查只有一份。本函数假定调用时没有同 label 窗口存在。

    // 关窗事件要带上是哪一行 —— 前端据此只刷那一行的余额。
    let handle_for_close = app_handle.clone();
    let closed_relay_id = op.id;

    let built = tauri::WebviewWindowBuilder::new(
        app_handle,
        window.label,
        tauri::WebviewUrl::External(target_url),
    )
    .title(window.title)
    // 尺寸比登录窗宽得多，而且**这是安全要求不是体验偏好**：USDT 充值页有一段
    // 「转错网络资产不可找回」的警告，窗口太窄会把它挤到要滚动才看得见的地方。
    // 可缩放 + 足够高，让那段话一屏内可读。
    .inner_size(1000.0, 800.0)
    .resizable(true)
    // 防止在小屏上超出可用区域（框架原生实现就是 `work_area - margin` 再 clamp，
    // 比自己查 monitor 再算术安全 —— 后者容易把 PhysicalSize 当逻辑像素用，
    // 那正是 Retina 上「窗口大一倍」的成因）。
    .prevent_overflow_with_margin(tauri::LogicalSize::new(40.0, 40.0))
    .center()
    // ⚠️ **必须 incognito**，理由见 `purchase.rs` 模块文档第 1 条。
    // 一句话：持久 profile 是全 app 共享的，不隔离的话这个窗口会读到**别的账号**
    // 残留的 refresh_token，站点的 401 拦截器拿它续期后覆盖 auth_token
    // ⇒ 用户在 B 行点充值、钱充进 A 账号（已实测复现）。
    //
    // 它**不影响**注入：`initialization_script` 是 WKUserScript(AtDocumentStart)、
    // 与页面同一个 JS 世界，而 incognito 只决定这份 localStorage 落不落盘。
    .incognito(true)
    // 放行 window.open 弹窗：充值页的支付二维码 / 收银台弹窗默认会被 wry 静默
    // 吞掉（「点了支付没反应」），理由与 `browser_import` 那段逐条相同。
    .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Allow)
    .initialization_script(purchase::inject_script(
        &op.site_origin,
        &op.auth_token,
        &auth_user,
    ))
    .build()
    .map_err(|e| AppError::Config(format!("打开站点窗口失败: {e}")))?;

    // 关窗刷余额。认 `Destroyed`（窗口真的没了）而不是 `CloseRequested`
    // （可被拦下的关闭请求，某些平台上会先于实际销毁触发、甚至可能被取消）。
    //
    // 只 emit 事件、不在这里查余额：查余额要发 HTTP，而这个回调不能 await。
    built.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = handle_for_close.emit(PURCHASE_CLOSED, closed_relay_id);
        }
    });

    Ok(())
}

/// 开充值窗前**无条件**换一把新 token，让它以完整 TTL 起步。
///
/// ## 为什么是无条件，而不是「剩得不多才续」
///
/// 初版是「剩余寿命 < 20 分钟才续」。那样最好的情况也只能保证 20 分钟 ——
/// 而一次 USDT 充值要用户切到钱包 app、转账、等链上确认，站点的支付页还挂着
/// 每几秒一次的订单轮询。**无条件续则每次都从完整 TTL 起步**（sub2api 默认
/// `jwt.expire_hour = 24`），代价只是多一次 HTTP 请求 —— 而这是用户点了「充值」
/// 之后的一次交互，本来就要等开窗。
///
/// 这条与「不注入 refresh_token」是配套的：那个决定让充值页**自己没有续期能力**
/// （见 `purchase.rs` 模块文档第 2 条），所以我们必须在交出 token 之前把它做长。
/// 续期用的是**我们自己**那把 refresh token、续完写回库，不存在被站点抢走的问题。
///
/// **失败不算错误** —— 原样返回传进来的凭据（`usable_relay` 已经保证它现在可用），
/// 让用户至少能完成一笔快的；把「可能不够」当成「一定不行」去拦住他更糟。
async fn ensure_token_outlasts_a_payment<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    op: creds::Relay,
) -> creds::Relay {
    let Some(refresh) = op.refresh_token.clone() else {
        log::info!("充值前想续期但没有 refresh token，用现有凭据开窗");
        return op;
    };

    match api::refresh_token(&op.site_origin, &refresh).await {
        Ok(fresh) => {
            let state = app_handle.state::<AppState>();
            if let Err(e) = with_conn(&state, |conn| {
                creds::update_tokens(
                    conn,
                    op.id,
                    &fresh.auth_token,
                    // 服务端没轮换时沿用旧的 —— 覆写成 None 会让下次过期时无法续期。
                    fresh.refresh_token.as_deref().or(Some(refresh.as_str())),
                    fresh.token_expires_at,
                )
            }) {
                // 库没写进去但 token 是新的：**仍然用它开窗**（这一次付款能撑住），
                // 只是下次还会再续一遍。
                log::warn!("充值前续期成功但写库失败（不影响本次开窗）: {e}");
            }
            creds::Relay {
                auth_token: fresh.auth_token,
                refresh_token: fresh.refresh_token.or(Some(refresh)),
                token_expires_at: fresh.token_expires_at,
                ..op
            }
        }
        Err(e) => {
            // 续期失败不拦：现有 token 还没过期（`usable_relay` 已经保证了），
            // 只是可能撑不完一次慢付款。
            log::warn!("充值前续期失败，用现有凭据开窗: {e}");
            op
        }
    }
}

/// 「切回官方登录」的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOfficialLoginResult {
    /// 备份文件的完整路径。`None` 表示本来就没有 `auth.json`（没登录过 ChatGPT）。
    ///
    /// 要送给前端显示：那里面是 OAuth refresh token，用户手滑点了确认时得知道去哪儿捞回来。
    pub backup_path: Option<String>,
    /// 我们把 ChatGPT 关掉了吗（关了才会去重开它）。
    pub chatgpt_was_running: bool,
    pub warnings: Vec<String>,
}

/// 一键「切回官方登录」：清 codex 的第三方路由与登录态，让用户自己重新登录 ChatGPT。
///
/// ## 为什么需要它
///
/// LoongPort 把 codex 配成 **provider auth 模式**（`experimental_bearer_token` 写在
/// `config.toml` 里），鉴权压根不看 `auth.json` ⇒ 用户在 ChatGPT / codex 里点「注销」
/// **没有任何反应**，请求照样带着中转站的 sk 打出去。这不是 bug，但对用户是困惑 ——
/// 他以为自己退出了，实际没有。这条命令是那个「退出」真正的开关。
///
/// ## 四步的顺序不能反
///
/// ```text
/// 1. 退 ChatGPT      它持有 ~/.codex，且**退出时会回写 auth.json**
/// 2. 备份 auth.json  里面是 OAuth refresh token，删了要重走浏览器登录
/// 3. 切 codex-official  它的空 config 会自然清掉 experimental_bearer_token
/// 4. 删 auth.json    让 codex 回到「未登录」，用户自己登
/// ```
///
/// **1 在 2 之前**：不先退它，我们删完它退出时又把 `auth.json` 写回来 —— 用户看到的是
/// 「点了切回官方，但 codex 还是登录着的」。
///
/// **3 在 4 之前**：反过来的话中间有个窗口既没 bearer token 也没登录态。只做一半的后果
/// 各不相同且都很糟：只删 `auth.json` ⇒ 仍走中转站（token 还在 `config.toml` 里）；
/// 只切 provider ⇒ 走 ChatGPT auth 模式但没登录态 ⇒ codex 报 credentials incomplete。
///
/// **2 不可省**：`ProviderService::switch` 自己那套清理（`clear_stale_codex_live_auth_after_official_switch`）
/// **有意不删带 OAuth 的 auth.json**（见 `codex_config::codex_auth_has_credential_login_material`）——
/// 用户的 ChatGPT 登录正是它拒绝碰的那一类，所以第 4 步必须自己动手，而动手之前必须留后路。
#[tauri::command]
pub async fn relay_restore_official_login(
    app_handle: tauri::AppHandle,
) -> Result<RestoreOfficialLoginResult, String> {
    restore_official_login_impl(&app_handle)
        .await
        .map_err(|e| e.to_string())
}

async fn restore_official_login_impl(
    app_handle: &tauri::AppHandle,
) -> Result<RestoreOfficialLoginResult, AppError> {
    let auth_path = crate::codex_config::get_codex_auth_path();

    // 编排走 `chatgpt_app::around`（与切档位共用同一份，见那边的说明）。
    //
    // ⚠️ **`abort_on_unconfirmed_exit = true`，与切档位相反 —— 这个差异是有意的。**
    // 那条只写 `config.toml`（ChatGPT 不碰它）⇒ 退不掉也能照常切。而这条要删
    // `auth.json`，**macOS 上 ChatGPT 退出时会回写它** ⇒ 没确认它退出就动手等于白删：
    // 用户看到「已切回官方登录」，实际它一退出就把登录态写回来。
    //
    // 这个标志**只对 macOS 生效**（`around` 里那个 `cfg!`）：Windows 实测不回写
    // `~/.codex`（`auth.json` 整个启停周期 mtime 未变），那边理由不成立 ⇒ 不中止。
    // 见 `chatgpt_app::around` 的文档。
    let (outcome, chatgpt) = chatgpt_app::around(true, || {
        // ── 备份 auth.json ──
        //
        // 在切换之前做，且失败就**中止**（`?`）—— 这一步的全部意义是「删之前留后路」，
        // 留不下后路就不该往下走，那是拿用户的 OAuth 登录去赌。
        // 没有这个文件是正常状态（从没登录过 ChatGPT），不是错误。
        let backup_path = backup_codex_auth(&auth_path)?;

        // ── 切到 codex-official ──
        //
        // 走 cc-switch 既有链路，不另写落盘逻辑。失败时 `around` 会把 ChatGPT 开回去，
        // 而配置没动、`auth.json` 还在原处（备份是拷贝不是移动）⇒ 用户手上的状态与
        // 操作前完全一样。
        let switched = {
            let state = app_handle.state::<AppState>();
            ProviderService::switch(
                &state,
                AppType::Codex,
                crate::database::CODEX_OFFICIAL_PROVIDER_ID,
            )
            .map_err(|e| AppError::Config(format!("切回官方登录失败：{e}。配置未改动")))?
        };

        let mut warnings = switched.warnings;

        // ── 删 auth.json ──**必须在切 provider 之后** ──
        //
        // 反过来的话中间有个窗口既没 bearer token 也没登录态。只做一半的后果各不相同
        // 且都很糟：只删 `auth.json` ⇒ 仍走中转站（token 还在 `config.toml` 里）；
        // 只切 provider ⇒ 走 ChatGPT auth 模式但没登录态 ⇒ 报 credentials incomplete。
        //
        // 删失败**不回滚切换**：那时 codex 已经是官方 provider（没有 bearer token 了），
        // 回滚等于把用户送回中转站路由 —— 而他刚刚明确要求离开那里。如实报出来让他
        // 手动删，比擅自撤销他的决定好。
        if auth_path.exists() {
            if let Err(e) = crate::config::delete_file(&auth_path) {
                warnings.push(format!(
                    "已切到官方 provider，但删除登录态失败：{e}。请手动删除 {} 后重新登录。",
                    auth_path.display()
                ));
            }
        }

        Ok((backup_path, warnings))
    })?;

    let (backup_path, mut warnings) = outcome;
    warnings.extend(chatgpt.warnings);

    // 广播「当前供应商变了」—— 这条命令内部调了 `ProviderService::switch`（切到
    // `codex-official`），所以它跟 `relay_switch_tier` / `switch_provider` 一样
    // 是一条**切换路径**，必须发。
    //
    // 漏了它的症状（2026-08-04 review 抓出）：用户在设置页点「切回官方登录」成功后
    // 回到供应商页，中转站区里原来那个托管档位**仍高亮「当前使用中」**、
    // 中转站行的删除按钮仍是灰的、title 还写着「要先切走」—— 而他已经切走了。
    // 后端是对的，坏的只有「不重开窗口就看不到」这一段（静默的界面陈旧）。
    //
    // 补在后端而不是让 `RestoreOfficialLoginButton` 自己刷：那个按钮在设置页，
    // 与中转站区互不相识；发事件是上游已有的机制，一处发射喂到全部监听者
    // （`provider.rs::emit_provider_switched` 的文档写了完整论证）。
    emit_provider_switched(
        app_handle,
        &AppType::Codex,
        crate::database::CODEX_OFFICIAL_PROVIDER_ID,
    );

    Ok(RestoreOfficialLoginResult {
        backup_path,
        chatgpt_was_running: chatgpt.was_running,
        warnings,
    })
}

/// 把 codex 的 `auth.json` 备份到 `~/.cc-switch/backups/codex-auth-<时间戳>.json`。
///
/// 返回 `None` 表示**源文件不存在**（用户从没登录过 ChatGPT）—— 那是正常状态，
/// 不是错误：没什么可备份，调用方那边也没什么可删。
///
/// 抽成独立函数是为了可测：它是这条链路上唯一碰用户文件的一步，
/// 而 `restore_official_login_impl` 需要 `AppHandle` 才能跑（测不了）。
fn backup_codex_auth(auth_path: &std::path::Path) -> Result<Option<String>, AppError> {
    if !auth_path.exists() {
        return Ok(None);
    }
    // 沿用仓库既有的 backups 目录惯例（`~/.cc-switch/backups/<用途>`），
    // 与 hermes / openclaw / codex-history 那几处同一个根。
    let dir = crate::config::get_app_config_dir().join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| AppError::io(&dir, e))?;
    let dest = dir.join(format!(
        "codex-auth-{}.json",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ));
    crate::config::copy_file(auth_path, &dest)?;
    Ok(Some(dest.to_string_lossy().to_string()))
}

/// 这条 provider 是不是 LoongPort 管的。
///
/// 判据本身在 [`crate::relay::managed`]（唯一来源，托盘与命令层守卫也用它）；这里只是
/// 把「按 `&Provider` 判」这个便利形状留在本地，别在这儿重写前缀。
fn is_managed(p: &Provider) -> bool {
    crate::relay::is_managed(&p.id)
}

/// 托管 provider 的 meta。
///
/// **`apiFormat` 必须显式写 `openai_responses`**：不写它会落到 `ProxyChat` profile，而那是
/// 唯一会去 spawn `codex debug models --bundled` 子进程的分支。sub2api 的 openai 网关原生走
/// Responses，写对了就永远走内嵌模板、不起子进程。
/// 托管档位的 `meta`。
///
/// `account_id` 是**归属依据**，不是可选的装饰：同一个站可以挂多个账号，而
/// `website_url` 只记站点 ⇒ 少了它，清理 / 重建 / 删站三处都会误伤同站另一个账号的
/// 档位（见 [`crate::provider::ProviderMeta::loongport_account_id`] 的文档）。
fn managed_meta(
    app_type: &AppType,
    account_id: Option<i64>,
    group: Option<crate::provider::LoongportGroupIdentity>,
) -> crate::provider::ProviderMeta {
    crate::provider::ProviderMeta {
        // `api_format` **只被 `codex_config.rs` 消费**（`CodexCatalogToolProfile::from_api_format`），
        // 对 claude / gemini 无意义 —— 给它们填值不会有人读，反而让人以为那里有语义。
        api_format: match app_type {
            AppType::Codex => Some("openai_responses".to_string()),
            _ => None,
        },
        loongport_account_id: account_id,
        loongport_group: group,
        ..Default::default()
    }
}

fn with_conn<T>(
    state: &AppState,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let conn = state
        .db
        .conn
        .lock()
        .map_err(|e| AppError::Database(format!("获取数据库连接失败: {e}")))?;
    f(&conn)
}

// ============================================================================
// 生图工具（MCP）
// ============================================================================

/// 重新对齐生图 MCP。进入生图页时调用，用来修复应用运行期间被外部改掉的投影。
#[tauri::command]
pub fn relay_sync_imagegen_mcp(state: State<'_, AppState>) -> Result<(), AppError> {
    imagegen_mcp::sync_registration(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约闸：前端 TS 类型手写断言了这份 wire 形状（`src/lib/api/relay.ts` 的
    /// `SwitchTierCommandResult`），serde 的 enum 级 `rename_all` 只转变体名、
    /// 不转变体字段 —— 没有这条闸的话 casing 分叉编译期完全静默
    /// （2026-08-16 线上事故：`target_name` 蛇形下发，确认弹窗永不打开）。
    #[test]
    fn switch_tier_command_result_wire_contract_is_camel_case() {
        let confirmation = serde_json::to_value(SwitchTierCommandResult::ConfirmationRequired {
            target_name: "站点 · 分组".into(),
        })
        .unwrap();
        assert_eq!(confirmation["status"], "confirmationRequired");
        assert!(
            confirmation.get("targetName").is_some(),
            "变体字段必须驼峰下发：{confirmation}"
        );
        assert!(
            confirmation.get("target_name").is_none(),
            "蛇形键意味着前端读到 undefined：{confirmation}"
        );

        let switched = serde_json::to_value(SwitchTierCommandResult::Switched {
            result: SwitchTierResult {
                provider_name: "p".into(),
                chatgpt_was_running: false,
                chatgpt_relaunched: false,
                warnings: vec![],
            },
        })
        .unwrap();
        assert_eq!(switched["status"], "switched");
        assert!(
            switched.get("providerName").is_some(),
            "flatten 的结构体字段同样是驼峰契约：{switched}"
        );
    }

    fn purchase_capability_relay(backend_kind: creds::BackendKind) -> creds::Relay {
        creds::Relay {
            id: 1,
            site_origin: "https://relay.example".into(),
            site_name: "Relay".into(),
            backend_kind,
            api_base_url: "https://relay.example/v1".into(),
            account_id: Some(1),
            account_label: "account".into(),
            login_identifier: "account".into(),
            auth_token: "session".into(),
            refresh_token: None,
            token_expires_at: None,
            user_agent: None,
            cf_clearance: None,
            pricing_synced_at: None,
            sort_index: 0,
        }
    }

    fn sub2api_with_session() -> creds::Relay {
        purchase_capability_relay(creds::BackendKind::Sub2Api)
    }

    fn newapi_with_refresh_cookie() -> creds::Relay {
        creds::Relay {
            refresh_token: Some("refresh-cookie".into()),
            ..purchase_capability_relay(creds::BackendKind::NewApi)
        }
    }

    fn newapi_without_refresh_cookie() -> creds::Relay {
        purchase_capability_relay(creds::BackendKind::NewApi)
    }

    #[test]
    fn purchase_capability_requires_login_config_and_backend_credentials() {
        assert!(can_open_site_window(&sub2api_with_session(), true, true));
        assert!(can_open_site_window(
            &newapi_with_refresh_cookie(),
            true,
            true
        ));
        assert!(!can_open_site_window(
            &newapi_without_refresh_cookie(),
            true,
            true
        ));
        assert!(!can_open_site_window(&sub2api_with_session(), false, true));
        assert!(!can_open_site_window(&sub2api_with_session(), true, false));

        let newapi_with_blank_refresh_cookie = creds::Relay {
            refresh_token: Some("   ".into()),
            ..newapi_with_refresh_cookie()
        };
        assert!(!can_open_site_window(
            &newapi_with_blank_refresh_cookie,
            true,
            true
        ));
    }

    #[test]
    fn directory_update_event_matches_the_frontend_constant() {
        let frontend = include_str!("../../../src/config/constants.ts");

        assert!(frontend.contains(RELAY_DIRECTORY_UPDATED_EVENT));
    }
    use std::{
        collections::HashMap,
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
    };

    use futures::channel::oneshot;

    use crate::relay::model_verification::{
        coordinator::{
            ActiveVerifier, ModelVerificationCoordinator, PreparedVerification, ProbeProgress,
        },
        types::{
            EvidenceLevel, RunFailureKind, TargetKey, Verdict, VerificationReport, RULES_VERSION,
        },
    };

    #[test]
    fn browser_entry_url_preserves_user_path_and_query_but_forces_https() {
        let url = browser_entry_url("http://api.example.com/register?aff=ABC123")
            .expect("valid browser entry URL");

        assert_eq!(url.as_str(), "https://api.example.com/register?aff=ABC123");
    }

    #[test]
    fn browser_entry_url_accepts_bare_hosts_with_paths() {
        let url = browser_entry_url("api.example.com/login?next=%2Fdashboard")
            .expect("valid browser entry URL");

        assert_eq!(
            url.as_str(),
            "https://api.example.com/login?next=%2Fdashboard"
        );
    }

    #[test]
    fn directory_entry_source_accepts_only_policy_owned_entries() {
        let config = crate::relay::remote_config::RemoteConfig {
            relay_directory: crate::relay::remote_config::RelayDirectoryPolicy {
                blocked_hosts: vec![],
                sites: std::collections::BTreeMap::from([
                    (
                        "790053500.com".into(),
                        crate::relay::remote_config::RelayDirectorySite {
                            veridrop_host: Some("api.790053500.com".into()),
                            entry_url: Some("https://790053500.com/keys".into()),
                            purchase_url: None,
                            usage_url: None,
                            display_name: Some("鑫旺".into()),
                        },
                    ),
                    (
                        "plain.example".into(),
                        crate::relay::remote_config::RelayDirectorySite::default(),
                    ),
                    (
                        "broken.example".into(),
                        crate::relay::remote_config::RelayDirectorySite {
                            veridrop_host: None,
                            entry_url: Some("http://broken.example/keys".into()),
                            purchase_url: None,
                            usage_url: None,
                            display_name: None,
                        },
                    ),
                ]),
            },
            ..crate::relay::remote_config::RemoteConfig::default()
        };

        assert_eq!(
            directory_entry_source(&config, "https://790053500.com/keys"),
            Some(BrowserEntrySource::SignedDirectory)
        );
        assert_eq!(
            directory_entry_source(&config, "https://plain.example"),
            Some(BrowserEntrySource::Manual)
        );
        assert_eq!(
            directory_entry_source(&config, "https://unknown.example"),
            None
        );
        assert_eq!(
            directory_entry_source(&config, "https://790053500.com/other"),
            None
        );
        assert_eq!(
            directory_entry_source(&config, "https://broken.example"),
            Some(BrowserEntrySource::Manual)
        );
        assert_eq!(
            directory_entry_source(&config, "https://broken.example/keys"),
            None
        );
    }

    /// wawapi.top 实测踩出的洞：aff / sponsor 名单里的站进了广场（曝光集合），
    /// 导入闸却只认 relay_directory ⇒ 点「接入」被 NotInDirectory 拒。第二段回落
    /// 补上：受管全集按 Manual 放行裸 origin；带 path / blocked / 名单外仍拒。
    #[test]
    fn directory_entry_source_falls_back_to_the_full_managed_set() {
        let config = crate::relay::remote_config::RemoteConfig {
            relay_directory: crate::relay::remote_config::RelayDirectoryPolicy {
                blocked_hosts: vec!["blocked.example".into()],
                sites: std::collections::BTreeMap::new(),
            },
            sponsors: vec![crate::relay::remote_config::Sponsor {
                site_origin: "https://www.WawAPII.com".into(),
                display_name: "WawAPI".into(),
                tagline: String::new(),
            }],
            aff_codes: std::collections::BTreeMap::from([
                ("wawapi.top".to_string(), "AFF".to_string()),
                ("blocked.example".to_string(), "AFF".to_string()),
            ]),
            ..crate::relay::remote_config::RemoteConfig::default()
        };

        assert_eq!(
            directory_entry_source(&config, "https://wawapi.top"),
            Some(BrowserEntrySource::Manual)
        );
        // sponsor 的 www / 大小写变体归一后也要命中
        assert_eq!(
            directory_entry_source(&config, "https://wawapii.com"),
            Some(BrowserEntrySource::Manual)
        );
        // Manual 回落只认裸 origin —— 带 path 的输入不给受信处理
        assert_eq!(
            directory_entry_source(&config, "https://wawapi.top/register"),
            None
        );
        assert_eq!(
            directory_entry_source(&config, "https://blocked.example"),
            None
        );
        assert_eq!(
            directory_entry_source(&config, "https://unknown.example"),
            None
        );
    }

    fn detected_sub2api() -> discovery::DetectedSite {
        discovery::DetectedSite {
            backend_kind: discovery::BackendKind::Sub2Api,
            site_name: "Example".into(),
            api_base_url: String::new(),
            final_origin: None,
        }
    }

    fn detected_newapi() -> discovery::DetectedSite {
        discovery::DetectedSite {
            backend_kind: discovery::BackendKind::NewApi,
            site_name: "NewAPI".into(),
            api_base_url: String::new(),
            final_origin: None,
        }
    }

    #[test]
    fn import_anchor_origin_prefers_the_probe_final_origin() {
        let redirected = discovery::DetectedSite {
            final_origin: Some("https://panel.example".into()),
            ..detected_sub2api()
        };
        assert_eq!(
            import_anchor_origin("https://apex.example".into(), Some(&redirected)),
            "https://panel.example"
        );
        // 浏览器路径的回传不带 final_origin（守卫保证同源），探针也没跑成时同理：
        // 都保持用户输入的 origin。
        assert_eq!(
            import_anchor_origin("https://apex.example".into(), Some(&detected_sub2api())),
            "https://apex.example"
        );
        assert_eq!(
            import_anchor_origin("https://apex.example".into(), None),
            "https://apex.example"
        );
        let same_origin = discovery::DetectedSite {
            final_origin: Some("https://apex.example".into()),
            ..detected_sub2api()
        };
        assert_eq!(
            import_anchor_origin("https://apex.example".into(), Some(&same_origin)),
            "https://apex.example"
        );
    }

    #[test]
    fn browser_start_url_uses_origin_when_protocol_is_unknown_even_for_non_page_path() {
        let url = browser_start_url(
            "https://api.example.com/custom/subscription-token",
            "https://api.example.com",
            None,
            BrowserEntrySource::Manual,
        )
        .expect("valid browser start URL");

        assert_eq!(url.as_str(), "https://api.example.com/");
    }

    #[test]
    fn browser_start_url_preserves_auth_link_while_protocol_is_unknown() {
        let url = browser_start_url(
            "https://api.example.com/register?aff=ABC123",
            "https://api.example.com",
            None,
            BrowserEntrySource::Manual,
        )
        .expect("valid browser start URL");

        assert_eq!(url.as_str(), "https://api.example.com/register?aff=ABC123");
    }

    #[test]
    fn browser_start_url_replaces_non_page_path_after_native_detection() {
        let detected = detected_sub2api();
        let url = browser_start_url(
            "https://api.example.com/custom/subscription-token",
            "https://api.example.com",
            Some(&detected),
            BrowserEntrySource::Manual,
        )
        .expect("valid browser start URL");

        assert_eq!(url.as_str(), "https://api.example.com/register");
    }

    #[test]
    fn browser_start_url_preserves_a_signed_directory_entry_path() {
        let detected = detected_sub2api();
        let url = browser_start_url(
            "https://790053500.com/keys",
            "https://790053500.com",
            Some(&detected),
            BrowserEntrySource::SignedDirectory,
        )
        .expect("valid signed directory entry URL");

        assert_eq!(url.as_str(), "https://790053500.com/keys");
    }

    #[test]
    fn browser_start_url_preserves_invitation_link_after_native_detection() {
        let detected = detected_sub2api();
        let url = browser_start_url(
            "http://api.example.com/register?aff=ABC123",
            "https://api.example.com",
            Some(&detected),
            BrowserEntrySource::Manual,
        )
        .expect("valid browser start URL");

        assert_eq!(url.as_str(), "https://api.example.com/register?aff=ABC123");
    }

    #[test]
    fn browser_start_url_uses_protocol_registration_page_for_known_bare_origin() {
        let detected = detected_sub2api();
        let url = browser_start_url(
            "api.example.com",
            "https://api.example.com",
            Some(&detected),
            BrowserEntrySource::Manual,
        )
        .expect("valid browser start URL");

        assert_eq!(url.as_str(), "https://api.example.com/register");
    }

    #[test]
    fn browser_start_url_uses_newapi_legacy_registration_page_for_known_bare_origin() {
        let detected = detected_newapi();
        let url = browser_start_url(
            "api.example.com",
            "https://api.example.com",
            Some(&detected),
            BrowserEntrySource::Manual,
        )
        .expect("valid browser start URL");

        assert_eq!(
            url.as_str(),
            backend::browser_login_url(
                "https://api.example.com",
                discovery::BackendKind::NewApi,
                ""
            )
        );
    }

    #[test]
    fn native_protocol_conflict_is_terminal_while_unsupported_site_can_fall_back() {
        let conflict = recoverable_native_discovery_error(discovery::DiscoveryError {
            kind: discovery::DiscoveryErrorKind::ProtocolConflict,
            message: "conflict".into(),
        });
        assert_eq!(
            conflict
                .expect_err("conflict must not open browser fallback")
                .message,
            "conflict"
        );

        let unsupported = recoverable_native_discovery_error(discovery::DiscoveryError {
            kind: discovery::DiscoveryErrorKind::UnsupportedSite,
            message: "unsupported".into(),
        })
        .expect("unsupported site can use browser fallback");
        assert_eq!(unsupported.to_string(), "unsupported");
    }

    #[test]
    fn new_site_import_close_is_a_typed_cancellation() {
        let error = incomplete_new_site_import_error(IncompleteImportReason::Closed);

        assert_eq!(error.kind, Some(RelayImportErrorKind::Cancelled));
        assert_eq!(error.message, "注册或登录尚未完成");
    }

    fn bare_mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock app")
    }

    #[tokio::test]
    async fn stale_login_window_destroy_returns_without_waiting_when_none_exists() {
        // 没有残留窗口是绝大多数路径（上一轮窗口早就关了）：helper 必须立即返回，
        // 不进等待循环 —— 否则每次导入/重登都白等一个轮询周期。
        let app = bare_mock_app();
        let start = std::time::Instant::now();
        destroy_stale_login_window_with_timeout(app.handle(), std::time::Duration::from_secs(2))
            .await;
        assert!(
            start.elapsed() < std::time::Duration::from_millis(500),
            "没有残留窗口时不该等待，实际等了 {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn stale_login_window_destroy_is_bounded_when_the_label_never_frees() {
        // MockRuntime 不驱动事件循环 ⇒ destroy 后 manager 注册表里的 label 永不释放
        // （与 newapi_purchase 超时用例同款不可观测性）。能钉住的不变量是「有界返回」：
        // 到点必须继续走，否则真实运行时里事件循环卡一下，导入就永久卡死在等待里。
        let app = bare_mock_app();
        tauri::WebviewWindowBuilder::new(
            app.handle(),
            login::LOGIN_WINDOW_LABEL,
            tauri::WebviewUrl::External(url::Url::parse("about:blank").unwrap()),
        )
        .build()
        .expect("预建残留登录窗");

        let start = std::time::Instant::now();
        destroy_stale_login_window_with_timeout(
            app.handle(),
            std::time::Duration::from_millis(150),
        )
        .await;
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "label 永不释放时也必须有界返回，实际等了 {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn new_site_import_timeout_is_a_typed_cancellation() {
        let error = incomplete_new_site_import_error(IncompleteImportReason::TimedOut);

        assert_eq!(error.kind, Some(RelayImportErrorKind::Cancelled));
        assert_eq!(error.message, "注册或登录等待超时，请重试");
    }

    /// ⭐ **kind 的线上格式必须与前端 union 逐字一致。**
    ///
    /// 前端 `src/lib/api/relay.ts` 的 `ImportErrorKind` 按这些字面量匹配（switch
    /// 的是字符串，不是共享类型）。serde enum 的 rename 只在结构体字段上踩过 casing
    /// 雷（PR #153 的 `target_name`），枚举变体同理 —— 这里把每个变体的线上值钉死。
    #[test]
    fn import_error_kinds_serialize_to_the_wire_names_the_frontend_matches() {
        for (kind, wire) in [
            (
                RelayImportErrorKind::UnsupportedSite,
                "\"unsupported_site\"",
            ),
            (RelayImportErrorKind::NotInDirectory, "\"not_in_directory\""),
            (
                RelayImportErrorKind::ProtocolConflict,
                "\"protocol_conflict\"",
            ),
            (RelayImportErrorKind::Transport, "\"transport\""),
            (RelayImportErrorKind::Cancelled, "\"cancelled\""),
        ] {
            assert_eq!(
                serde_json::to_string(&kind).expect("kind 可序列化"),
                wire,
                "{kind:?} 的线上名变了，前端 union 要跟着改"
            );
        }
    }

    #[test]
    fn new_site_import_discovery_stays_in_memory_until_authentication() {
        let context = browser_login_context(
            "https://api.example.com",
            detected_sub2api(),
            Some("invite"),
            None,
        );

        assert_eq!(context.site.site_origin, "https://api.example.com");
        assert_eq!(context.site.site_name, "Example");
        assert_eq!(context.site.api_base_url, "https://api.example.com");
        assert_eq!(context.site.backend_kind, discovery::BackendKind::Sub2Api);
    }

    #[tokio::test]
    async fn completed_refresh_wins_when_close_and_refresh_are_ready_together() {
        let outcome =
            await_refresh_preserving_rotation(async { Ok::<_, AppError>("refreshed") }, async {
                "closed"
            })
            .await;

        assert!(matches!(outcome, RefreshWait::Refreshed(Ok("refreshed"))));
    }

    #[tokio::test]
    async fn refresh_started_before_close_is_drained_to_preserve_rotation() {
        let (release_refresh, wait_for_release) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(await_refresh_preserving_rotation(
            async {
                wait_for_release
                    .await
                    .expect("test releases the refresh response");
                Ok::<_, AppError>("rotated")
            },
            async { "closed" },
        ));

        tokio::task::yield_now().await;
        release_refresh
            .send(())
            .expect("refresh waiter remains alive after close");
        let outcome = tokio::time::timeout(std::time::Duration::from_millis(50), task)
            .await
            .expect("bounded refresh completes")
            .expect("refresh task does not panic");

        assert!(matches!(outcome, RefreshWait::Refreshed(Ok("rotated"))));
    }

    #[test]
    fn persisting_newapi_login_session_stores_tokens_and_native_account_identity() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);
        let relay_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                "https://newapi.example",
                "NewAPI",
                "https://newapi.example",
                discovery::BackendKind::NewApi,
            )
        })
        .expect("save site");
        let refreshed = crate::relay::newapi::RefreshedSession {
            access_token: "new-access-token".into(),
            access_expires_at: Some(1_900_000_000),
            session_id: "session-id".into(),
            account: crate::relay::newapi::SelfAccount {
                id: 84,
                username: "newapi-login".into(),
                display_name: "NewAPI Display".into(),
                email: "newapi@example.com".into(),
                group: "default".into(),
                quota: 0,
                used_quota: 0,
            },
            refresh_cookie: "rotated-refresh-cookie".into(),
        };

        let (final_relay_id, account_id) =
            persist_newapi_login_session(&state, relay_id, &refreshed).expect("persist login");
        let persisted = with_conn(&state, |conn| creds::get(conn, final_relay_id))
            .expect("load relay")
            .expect("relay exists");

        assert_eq!(account_id, 84);
        assert_eq!(persisted.auth_token, "new-access-token");
        assert_eq!(
            persisted.refresh_token.as_deref(),
            Some("rotated-refresh-cookie")
        );
        assert_eq!(persisted.token_expires_at, Some(1_900_000_000));
        assert_eq!(persisted.account_id, Some(84));
        assert_eq!(persisted.account_label, "NewAPI Display");
        assert_eq!(persisted.login_identifier, "newapi-login");
    }

    #[test]
    fn persisting_legacy_newapi_session_keeps_refresh_fields_absent() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);
        let relay_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                "https://legacy-newapi.example",
                "Legacy NewAPI",
                "https://legacy-newapi.example",
                discovery::BackendKind::NewApi,
            )
        })
        .expect("save site");
        let session = crate::relay::newapi::RefreshedSession {
            access_token: "long-lived-access-token".into(),
            access_expires_at: None,
            session_id: String::new(),
            account: crate::relay::newapi::SelfAccount {
                id: 42,
                username: "legacy-login".into(),
                display_name: "Legacy User".into(),
                email: String::new(),
                group: "default".into(),
                quota: 0,
                used_quota: 0,
            },
            refresh_cookie: String::new(),
        };

        let (final_relay_id, account_id) =
            persist_newapi_login_session(&state, relay_id, &session).expect("persist login");
        let persisted = with_conn(&state, |conn| creds::get(conn, final_relay_id))
            .expect("load relay")
            .expect("relay exists");

        assert_eq!(account_id, 42);
        assert_eq!(persisted.auth_token, "long-lived-access-token");
        assert_eq!(persisted.refresh_token, None);
        assert_eq!(persisted.token_expires_at, None);
        assert_eq!(persisted.account_id, Some(42));
    }

    #[test]
    fn import_result_uses_the_final_relay_id_after_account_merge() {
        let result = ImportResult::authenticated(
            DiscoveredRelaySite {
                site_origin: "https://api.example.com".into(),
                site_name: "Example".into(),
                api_base_url: "https://api.example.com".into(),
                backend_kind: discovery::BackendKind::NewApi,
            },
            11,
        );

        assert_eq!(result.relay_id, 11);
    }

    #[test]
    fn relay_row_serializes_backend_owned_remove_confirmation() {
        let row = RelayRow {
            id: 7,
            site_origin: "https://api.example.com".into(),
            site_name: "Example".into(),
            account_label: String::new(),
            status: RelayRowStatus::NotLoggedIn,
            is_current: false,
            can_query_balance: false,
            can_purchase: true,
            can_view_usage: false,
            can_refresh: false,
            usage_blockers: Vec::new(),
            remove_confirmation: RemoveConfirmation::NeverLoggedIn,
            tiers: Vec::new(),
        };

        let json = serde_json::to_value(row).expect("serialize relay row");
        assert_eq!(json["canPurchase"], true);
        assert_eq!(json["canViewUsage"], false);
        assert_eq!(json["removeConfirmation"], "neverLoggedIn");
        assert_eq!(json["usageBlockers"], serde_json::json!([]));
    }

    #[test]
    fn relay_status_owns_the_global_add_site_prompt_decision() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);

        assert!(
            relay_status_impl(&state)
                .expect("empty status")
                .should_prompt_add_site
        );

        with_conn(&state, |conn| {
            crate::vendor::creds::save_account(
                conn,
                crate::vendor::Vendor::DeepSeek,
                "token",
                &crate::vendor::VendorAccount {
                    account_id: "account-7".into(),
                    label: "DeepSeek user".into(),
                    login_identifier: "13800000000".into(),
                },
            )?;
            Ok(())
        })
        .expect("save vendor account");

        assert!(
            !relay_status_impl(&state)
                .expect("configured status")
                .should_prompt_add_site
        );
    }

    #[test]
    fn refresh_summary_counts_vendor_config_for_the_current_app() {
        let summary = refresh_summary(
            &AppType::Codex,
            Vec::new(),
            vec![(
                "DeepSeek".into(),
                Ok(super::super::vendor::VendorProvisionSummary {
                    provider_id: "managed-deepseek".into(),
                    platforms: vec![
                        AppType::Codex.as_str().into(),
                        AppType::Claude.as_str().into(),
                    ],
                    key_created: true,
                    merged_providers: vec!["Imported DeepSeek".into()],
                }),
            )],
        );

        assert_eq!(summary.refreshed_accounts, 1);
        assert_eq!(summary.tiers, 1);
        assert_eq!(summary.other_platform_tiers, 0);
        assert_eq!(summary.keys_created, 1);
        assert_eq!(summary.merged_providers, 1);
        assert!(matches!(summary.notice, RefreshNotice::UpdatedWithKeys));
    }

    #[test]
    fn refresh_result_is_cloneable() {
        let result = RefreshResult {
            summary: RefreshSummary {
                notice: RefreshNotice::None,
                refreshed_accounts: 0,
                tiers: 0,
                keys_created: 0,
                other_platform_tiers: 0,
                merged_providers: 0,
                failures: Vec::new(),
            },
            balances: Vec::new(),
        };

        let cloned = result.clone();
        assert!(matches!(cloned.summary.notice, RefreshNotice::None));
    }

    fn codex_settings(model: &str, models: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "auth": { "OPENAI_API_KEY": "sk-test" },
            "config": format!(
                "model_provider = \"custom\"\nmodel = {model:?}\n\n[model_providers.custom]\nname = \"Test\"\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
            ),
            "modelCatalog": {
                "models": models.iter().map(|model| serde_json::json!({ "model": model })).collect::<Vec<_>>()
            }
        })
    }

    fn provider_with_id(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: "t".into(),
            settings_config: serde_json::json!({}),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn codex_model_list_requires_a_real_catalog() {
        let settings = serde_json::json!({
            "config": "model_provider = \"custom\"\nmodel = \"gpt-current\"\n"
        });

        assert!(
            models_from_settings(&settings).is_empty(),
            "旧 provider 只有当前模型时，不能把它冒充成完整支持列表"
        );
    }

    #[test]
    fn selecting_a_codex_model_validates_and_only_updates_the_model_field() {
        let settings = codex_settings("gpt-a", &["gpt-a", "gpt-b"]);

        let selected = select_codex_model(&settings, " gpt-b ").expect("supported model");
        assert_eq!(
            provision::extract_model(&selected).as_deref(),
            Some("gpt-b")
        );
        assert_eq!(selected["modelCatalog"], settings["modelCatalog"]);
        assert_eq!(selected["auth"], settings["auth"]);

        assert!(select_codex_model(&settings, "gpt-unknown").is_err());
    }

    #[test]
    fn refreshing_a_managed_codex_tier_keeps_only_a_still_supported_selection() {
        let defaults = codex_settings("gpt-a", &["gpt-a", "gpt-b"]);
        let previous = codex_settings("gpt-b", &["gpt-a", "gpt-b"]);
        let kept = preserve_supported_codex_model(defaults.clone(), &previous);
        assert_eq!(provision::extract_model(&kept).as_deref(), Some("gpt-b"));

        let removed = codex_settings("gpt-removed", &["gpt-removed"]);
        let reset = preserve_supported_codex_model(defaults, &removed);
        assert_eq!(provision::extract_model(&reset).as_deref(), Some("gpt-a"));
    }

    /// 形状对齐 [`relay::provision`] 生成侧（`deeplink::build_grokbuild_settings`
    /// + `modelCatalog`）：选中模型在 `[model."<default>"]` 表的 `model` 字段。
    fn grok_settings(model: &str, models: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "config": format!(
                "[models]\ndefault = \"{model}\"\n\n[model.\"{model}\"]\nmodel = \"{model}\"\nbase_url = \"https://api.example.com\"\nname = \"Test\"\napi_key = \"sk-test\"\napi_backend = \"responses\"\ncontext_window = 500000\n"
            ),
            "modelCatalog": {
                "models": models.iter().map(|model| serde_json::json!({ "model": model })).collect::<Vec<_>>()
            }
        })
    }

    #[test]
    fn selecting_a_grok_model_only_updates_the_model_field() {
        let settings = grok_settings("grok-4.5", &["grok-4.5", "grok-4.6"]);

        let selected = select_grok_model(&settings, "grok-4.6").expect("supported model");
        assert_eq!(
            provision::selected_model(&AppType::GrokBuild, &selected).as_deref(),
            Some("grok-4.6")
        );
        // profile 名（models.default 指向的表）、端点、密钥、目录都不动 ——
        // 它们与模型无关
        let config = selected["config"].as_str().expect("config text");
        assert!(config.contains("[model.\"grok-4.5\"]"), "{config}");
        assert!(config.contains("base_url = \"https://api.example.com\""));
        assert!(config.contains("api_key = \"sk-test\""));
        assert_eq!(selected["modelCatalog"], settings["modelCatalog"]);
    }

    #[test]
    fn refreshing_a_managed_grok_tier_keeps_only_a_still_supported_selection() {
        let defaults = grok_settings("grok-4.5", &["grok-4.5", "grok-4.6"]);
        let previous = grok_settings("grok-4.6", &["grok-4.5", "grok-4.6"]);
        let kept = preserve_supported_grok_model(defaults.clone(), &previous);
        assert_eq!(
            provision::selected_model(&AppType::GrokBuild, &kept).as_deref(),
            Some("grok-4.6")
        );

        let removed = grok_settings("grok-gone", &["grok-gone"]);
        let reset = preserve_supported_grok_model(defaults, &removed);
        assert_eq!(
            provision::selected_model(&AppType::GrokBuild, &reset).as_deref(),
            Some("grok-4.5")
        );
    }

    /// ⭐ `relay_list_sponsors` 发给前端的**键名**必须是 camelCase。
    ///
    /// 这条守的是一个跨语言的静默失效：`Sponsor` 的 `Deserialize` 用 snake_case
    /// （签名覆盖的配置契约，动不了），`Serialize` 用 camelCase（TS 侧惯例）。
    /// 两者不一致看起来像疏漏，**很可能被人顺手统一** —— 而统一到 snake_case 时
    /// 编译器一声不响，前端拿到的每个字段都是 `undefined` ⇒
    /// **首启屏卡片全是空白按钮**（`displayName` 为 undefined、React 什么都不渲染）。
    ///
    /// 断言的是序列化后的键，不是结构体字段名 —— 后者与前端无关。
    /// （`remote_config` 那边也有一条同向的闸，两处各守一端：
    /// 那条管「结构体的两个方向」，这条管「命令实际吐出去的东西」。）
    #[test]
    fn list_sponsors_emits_camel_case_keys_for_the_frontend() {
        let sponsor = crate::relay::remote_config::Sponsor {
            site_origin: "https://x.com".into(),
            display_name: "X".into(),
            tagline: "T".into(),
        };
        // 命令的返回类型是 `Vec<Sponsor>`，所以按它实际的序列化形态断言。
        let json = serde_json::to_value(vec![sponsor]).expect("要能序列化");
        let first = json[0].as_object().expect("是个对象");

        for key in ["siteOrigin", "displayName", "tagline"] {
            assert!(
                first.contains_key(key),
                "前端要的键 {key} 不在返回里，实际：{:?}",
                first.keys().collect::<Vec<_>>()
            );
        }
        assert!(
            !first.contains_key("site_origin") && !first.contains_key("display_name"),
            "别把 snake_case 键发给前端（TS 那边按 camelCase 读）"
        );
    }

    /// ⭐ **`TierInfo` 必须说清自己落在哪个 CLI 上。**
    ///
    /// ## 它守的是什么缺陷（TODO 债 11）
    ///
    /// [`do_provision`] 一次探**全部平台**，`tiers` 收的是全平台的结果，而 UI 那一行
    /// 只显示**当前 app** 的档位。于是「这个站没有 anthropic 分组」与「拉取失败」
    /// 在界面上长得一样（都是零档位 + 「该账号在此平台下没有可用分组」）——
    /// 而前者重试一百次也不会有，后者重试有意义。
    ///
    /// 区分它们所需的信息 provision 时**本来就在手上**（每个分组的 `app_type`），
    /// 少的只是把它发给前端。没有这个字段，前端拿到一堆 tiers 却分不出哪条是自己的。
    ///
    /// ## 为什么键名是 `appId`
    ///
    /// 前端那边这个概念叫 `AppId`（`lib/api/types.ts`），命令层签名也一直吃
    /// `app_id`。发 `appType` 会让同一个东西在两侧各有一个名字。
    #[test]
    fn tier_info_tells_the_frontend_which_cli_it_landed_on() {
        let tier = TierInfo {
            provider_id: "loongport-0123456789abcdef".into(),
            app_id: AppType::Claude.as_str().to_string(),
            group_name: "pro池".into(),
            display_name: "站 · pro池".into(),
            model: "claude-sonnet-5".into(),
            models: vec!["claude-sonnet-5".into()],
            rate_multiplier: Some(1.0),
            is_current: false,
            can_verify_models: true,
            user_edited: None,
            allow_image_generation: None,
            site_declared_origin: None,
        };

        let json = serde_json::to_value(&tier).expect("要能序列化");
        let obj = json.as_object().expect("是个对象");

        assert_eq!(
            obj.get("appId").and_then(|v| v.as_str()),
            Some("claude"),
            "前端要靠 appId 判断这条档位是不是属于它当前那一屏，实际：{:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            obj.get("canVerifyModels").and_then(|value| value.as_bool()),
            Some(true),
            "模型验证支持资格必须由后端随档位返回"
        );
        assert!(
            !obj.contains_key("app_id"),
            "别把 snake_case 键发给前端（TS 那边按 camelCase 读）"
        );
    }

    #[test]
    fn managed_detection_matches_generated_ids_only() {
        // 正面：provision 生成的 id 必须被认出来。
        let real = provision::provider_id_for("https://bestapi.store", Some(1), 42);
        assert!(is_managed(&provider_with_id(&real)));

        // 反面：用户自己加的 provider 不能被当成托管的（否则会被 provision 覆盖）。
        for id in ["custom-1", "codex-official", "", "LoongPort-1"] {
            assert!(!is_managed(&provider_with_id(id)), "id: {id}");
        }
    }

    #[test]
    fn provision_merge_removes_only_same_app_unmanaged_duplicate() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let site = "https://relay.example";
        let key = "sk-same";
        let settings = provision::settings_config_for(
            &app_type,
            key,
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");

        let duplicate = Provider {
            id: "cc-switch-duplicate".into(),
            name: "Imported duplicate".into(),
            settings_config: settings.clone(),
            website_url: Some(site.into()),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        let different_key = Provider {
            id: "cc-switch-different-key".into(),
            name: "Keep different key".into(),
            settings_config: provision::settings_config_for(
                &app_type,
                "sk-other",
                "Other",
                "https://relay.example/v1",
                "model-a",
            )
            .expect("codex 配置"),
            ..duplicate.clone()
        };
        let managed_duplicate = Provider {
            id: provision::provider_id_for(site, Some(1), 42),
            name: "Managed duplicate".into(),
            meta: Some(managed_meta(&app_type, Some(1), None)),
            ..duplicate.clone()
        };
        db.save_provider(app_type.as_str(), &duplicate)
            .expect("写入重复项");
        db.save_provider(app_type.as_str(), &different_key)
            .expect("写入不同 key");
        db.save_provider(app_type.as_str(), &managed_duplicate)
            .expect("写入托管项");

        let merged =
            provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed_duplicate)
                .expect("收编不该失败");

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "Imported duplicate");
        assert!(db
            .get_provider_by_id("cc-switch-duplicate", app_type.as_str())
            .expect("查询")
            .is_none());
        assert!(db
            .get_provider_by_id("cc-switch-different-key", app_type.as_str())
            .expect("查询")
            .is_some());
        assert!(db
            .get_provider_by_id(&managed_duplicate.id, app_type.as_str())
            .expect("查询")
            .is_some());
    }

    #[test]
    fn provision_merge_reports_when_duplicate_was_current() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let settings = provision::settings_config_for(
            &app_type,
            "sk-current",
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");
        let duplicate = Provider {
            id: "cc-switch-current".into(),
            name: "Current imported duplicate".into(),
            settings_config: settings,
            website_url: Some("https://relay.example".into()),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        db.save_provider(app_type.as_str(), &duplicate)
            .expect("写入当前项");
        db.set_current_provider(app_type.as_str(), &duplicate.id)
            .expect("设为当前");

        let managed = Provider {
            id: provision::provider_id_for("https://relay.example", Some(1), 99),
            name: "Managed replacement".into(),
            meta: Some(managed_meta(&app_type, Some(1), None)),
            ..duplicate.clone()
        };
        db.save_provider(app_type.as_str(), &managed)
            .expect("写入托管替代项");

        let merged = provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed)
            .expect("收编不该失败");

        assert_eq!(merged.len(), 1);
        assert!(merged[0].was_current);
        assert_eq!(
            db.get_current_provider(app_type.as_str())
                .expect("读取收编后的当前项")
                .as_deref(),
            Some(managed.id.as_str())
        );
    }

    #[test]
    fn provision_merge_rolls_back_duplicate_deletion_when_current_transfer_fails() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let settings = provision::settings_config_for(
            &app_type,
            "sk-current",
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");
        let duplicate = Provider {
            id: "cc-switch-current".into(),
            name: "Current imported duplicate".into(),
            settings_config: settings,
            website_url: Some("https://relay.example".into()),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        db.save_provider(app_type.as_str(), &duplicate)
            .expect("写入当前项");
        db.set_current_provider(app_type.as_str(), &duplicate.id)
            .expect("设为当前");

        let managed = Provider {
            id: provision::provider_id_for("https://relay.example", Some(1), 99),
            name: "Managed replacement".into(),
            meta: Some(managed_meta(&app_type, Some(1), None)),
            ..duplicate.clone()
        };
        db.save_provider(app_type.as_str(), &managed)
            .expect("写入托管替代项");
        {
            let conn = db.conn.lock().expect("lock db");
            conn.execute_batch(&format!(
                "CREATE TRIGGER fail_managed_current
                 BEFORE UPDATE OF is_current ON providers
                 WHEN NEW.id = '{}' AND NEW.is_current = 1
                 BEGIN
                   SELECT RAISE(FAIL, 'injected current transfer failure');
                 END;",
                managed.id
            ))
            .expect("install current-transfer failure");
        }

        let error = provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed)
            .expect_err("current transfer failure must roll back adoption")
            .to_string();

        assert!(error.contains("injected current transfer failure"));
        assert!(db
            .get_provider_by_id(&duplicate.id, app_type.as_str())
            .expect("read duplicate")
            .is_some());
        assert_eq!(
            db.get_current_provider(app_type.as_str())
                .expect("read current after rollback")
                .as_deref(),
            Some(duplicate.id.as_str())
        );
    }

    #[test]
    fn provision_merge_never_uses_an_unmanaged_provider_as_the_owner() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let settings = provision::settings_config_for(
            &app_type,
            "sk-shared",
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");
        let imported = Provider {
            id: "cc-switch-imported".into(),
            name: "Imported".into(),
            settings_config: settings.clone(),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        let non_managed_candidate = Provider {
            id: "manual-provider".into(),
            name: "Manual".into(),
            settings_config: settings,
            ..imported.clone()
        };
        db.save_provider(app_type.as_str(), &imported)
            .expect("写入导入项");
        db.save_provider(app_type.as_str(), &non_managed_candidate)
            .expect("写入手工项");

        assert!(provider_fingerprint::remove_unmanaged_duplicates(
            &db,
            &app_type,
            &non_managed_candidate,
        )
        .expect("不该失败")
        .is_empty());
        assert!(db
            .get_provider_by_id(&imported.id, app_type.as_str())
            .expect("查询")
            .is_some());
    }

    #[test]
    fn provision_summary_reports_adopted_providers_to_the_frontend() {
        let summary = ProvisionSummary {
            tiers: Vec::new(),
            failures: Vec::new(),
            keys_created: 0,
            merged_providers: vec![MergedProviderInfo {
                name: "Imported duplicate".into(),
                app_id: AppType::Codex.as_str().to_string(),
            }],
        };

        let json = serde_json::to_value(summary).expect("应能序列化");
        assert_eq!(json["mergedProviders"][0]["name"], "Imported duplicate");
        assert_eq!(json["mergedProviders"][0]["appId"], "codex");
    }

    fn tier(id: &str) -> TierInfo {
        TierInfo {
            provider_id: id.into(),
            // 归属测试只关心「哪条属于哪个站/账号」，与落在哪个 CLI 无关。
            app_id: AppType::Codex.as_str().to_string(),
            group_name: id.into(),
            display_name: id.into(),
            model: "gpt-5.6-sol".into(),
            models: vec!["gpt-5.6-sol".into()],
            rate_multiplier: None,
            is_current: false,
            can_verify_models: true,
            // 归属测试不关心它 —— `tiers_of_site` 会自己算出来覆盖掉这个值。
            user_edited: None,
            // 同上：归属判定与生图无关。
            allow_image_generation: None,
            site_declared_origin: None,
        }
    }

    fn test_app() -> AppType {
        AppType::Codex
    }

    fn test_newapi_relay(account_id: i64) -> creds::Relay {
        creds::Relay {
            id: account_id,
            site_origin: "https://newapi.example".into(),
            site_name: "NewAPI".into(),
            backend_kind: discovery::BackendKind::NewApi,
            api_base_url: String::new(),
            account_id: Some(account_id),
            account_label: format!("account-{account_id}"),
            login_identifier: format!("account-{account_id}"),
            auth_token: "access-token".into(),
            refresh_token: None,
            token_expires_at: None,
            user_agent: None,
            cf_clearance: None,
            pricing_synced_at: None,
            sort_index: 0,
        }
    }

    async fn spawn_discovery_server(
        sub2api_body: Option<serde_json::Value>,
        newapi_body: Option<serde_json::Value>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{routing::get, Json, Router};

        let mut app = Router::new();
        if let Some(body) = sub2api_body {
            app = app.route(
                "/api/v1/settings/public",
                get(move || {
                    let body = body.clone();
                    async move { Json(body) }
                }),
            );
        }
        if let Some(body) = newapi_body {
            app = app.route(
                "/api/status",
                get(move || {
                    let body = body.clone();
                    async move { Json(body) }
                }),
            );
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind discovery test server");
        let origin = format!("http://{}", listener.local_addr().expect("server address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve discovery app");
        });
        (origin, server)
    }

    fn newapi_discovery_body() -> serde_json::Value {
        serde_json::json!({
            "success": true,
            "data": {
                "version": "1.0.0",
                "system_name": "NewAPI",
                "theme": "default",
                "register_enabled": true,
                "password_login_enabled": true
            }
        })
    }

    fn sub2api_discovery_body() -> serde_json::Value {
        serde_json::json!({
            "code": 0,
            "message": "success",
            "data": {
                "site_name": "Sub2API",
                "version": "1.0.0",
                "api_base_url": "",
                "registration_enabled": true,
                "promo_code_enabled": false,
                "invitation_code_enabled": false
            }
        })
    }

    fn saved_relay_app(
        site_origin: &str,
        backend_kind: discovery::BackendKind,
    ) -> (tauri::App<tauri::test::MockRuntime>, i64) {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let relay_id = {
            let conn = db.conn.lock().expect("lock memory database");
            let relay_id = creds::save_site_with_backend(
                &conn,
                site_origin,
                "Saved relay",
                site_origin,
                backend_kind,
            )
            .expect("save relay");
            creds::save_credentials(
                &conn,
                relay_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "Saved Account",
                    login_identifier: "saved-account",
                },
                "saved-access-token",
                Some("saved-refresh-token"),
                None,
                creds::SessionEnvironment::default(),
            )
            .expect("save relay credentials");
            relay_id
        };
        let app = tauri::test::mock_builder()
            .manage(AppState::new(db))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock app");
        (app, relay_id)
    }

    fn relay_credentials(
        app: &tauri::App<tauri::test::MockRuntime>,
        relay_id: i64,
    ) -> creds::Relay {
        let state = app.state::<AppState>();
        with_conn(&state, |conn| creds::get(conn, relay_id))
            .expect("read saved relay")
            .expect("saved relay exists")
    }

    /// 把 `saved_relay_app` 存好的 relay 的 access token 显式置为已过期。
    ///
    /// 必须给一个**过去**的 `token_expires_at`：`saved_relay_app` 存的是 `None`，而
    /// `token_looks_valid` 对 `None` 有意乐观降级（返回 true），`usable_relay` 会走
    /// token 早退分支，永远到不了要测的续期路径。
    fn expire_saved_relay_token(app: &tauri::App<tauri::test::MockRuntime>, relay_id: i64) {
        let state = app.state::<AppState>();
        let conn = state.db.conn.lock().expect("lock memory database");
        conn.execute(
            "UPDATE loongport_relay SET token_expires_at = ?1 WHERE id = ?2",
            rusqlite::params![chrono::Utc::now().timestamp() - 3600, relay_id],
        )
        .expect("expire saved relay token");
    }

    /// 起一个只回余额相关端点的本地 server（手法照 `relay/backend.rs` 的先例）。
    async fn spawn_balance_server(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (origin, task)
    }

    fn profile_router(balance: serde_json::Value) -> axum::Router {
        use axum::{routing::get, Json, Router};
        Router::new().route(
            "/api/v1/user/profile",
            get(move || async move { Json(balance) }),
        )
    }

    #[tokio::test]
    async fn relay_balance_writes_one_snapshot_on_successful_resolve() {
        let (origin, server) = spawn_balance_server(profile_router(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": {
                "id": 7,
                "username": "Sub User",
                "email": "sub@example.com",
                "balance": 12.5,
                "frozen_balance": 0.0
            }
        })))
        .await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let result = relay_balance_impl(app.handle(), relay_id)
            .await
            .expect("成功解析必须 Ok");

        assert!(result.usage.success, "{:?}", result.usage.error);
        let state = app.state::<AppState>();
        let rows = state
            .db
            .list_balance_snapshots(relay_id, None)
            .expect("读快照");
        assert_eq!(rows.len(), 1, "成功路径恰好落一条快照");
        assert_eq!(rows[0].balance_usd, 12.5);
        assert_eq!(rows[0].source, "balance_query");
        assert_eq!(rows[0].relay_id, relay_id);
        server.abort();
    }

    #[tokio::test]
    async fn relay_balance_writes_no_snapshot_when_resolve_fails() {
        use axum::{http::StatusCode, routing::get, Router};
        let router = Router::new().route(
            "/api/v1/user/profile",
            get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let (origin, server) = spawn_balance_server(router).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let result = relay_balance_impl(app.handle(), relay_id)
            .await
            .expect("失败路回 success:false，仍是 Ok");

        assert!(!result.usage.success);
        let state = app.state::<AppState>();
        assert_eq!(
            state
                .db
                .list_balance_snapshots(relay_id, None)
                .expect("读快照")
                .len(),
            0,
            "三路全败不能落快照"
        );
        server.abort();
    }

    /// ⭐ 快照写入失败绝不能拖垮余额显示（对账是旁路能力，plan §三.2）。
    #[tokio::test]
    async fn relay_balance_returns_balance_even_when_snapshot_insert_fails() {
        let (origin, server) = spawn_balance_server(profile_router(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": {
                "id": 7,
                "username": "Sub User",
                "email": "sub@example.com",
                "balance": 9.9,
                "frozen_balance": 0.0
            }
        })))
        .await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);
        {
            let state = app.state::<AppState>();
            let conn = state.db.conn.lock().expect("lock memory database");
            conn.execute("DROP TABLE relay_balance_snapshots", [])
                .expect("删表制造写入失败");
        }

        let result = relay_balance_impl(app.handle(), relay_id)
            .await
            .expect("快照写入失败时余额命令仍必须 Ok");
        assert!(result.usage.success);
        assert_eq!(
            result
                .usage
                .data
                .as_ref()
                .and_then(|items| items.first())
                .and_then(|item| item.remaining),
            Some(9.9)
        );
        server.abort();
    }

    #[tokio::test]
    async fn saved_relay_validation_accepts_the_same_detected_backend() {
        let (origin, server) = spawn_discovery_server(None, Some(newapi_discovery_body())).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

        let relay = usable_relay(app.handle(), relay_id)
            .await
            .expect("same backend remains usable");

        assert_eq!(relay.backend_kind, discovery::BackendKind::NewApi);
        assert_eq!(relay.auth_token, "saved-access-token");
        server.abort();
    }

    #[tokio::test]
    async fn saved_relay_validation_clears_credentials_on_detected_backend_mismatch() {
        let (origin, server) = spawn_discovery_server(Some(sub2api_discovery_body()), None).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

        let error = usable_relay(app.handle(), relay_id)
            .await
            .expect_err("backend mismatch must stop runtime dispatch");

        assert!(error.to_string().contains("协议"), "{error}");
        let relay = relay_credentials(&app, relay_id);
        assert!(relay.auth_token.is_empty());
        assert!(relay.refresh_token.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn saved_relay_validation_uses_saved_backend_when_probe_is_unsupported() {
        let (origin, server) = spawn_discovery_server(
            Some(serde_json::json!({ "unknown": "sub" })),
            Some(serde_json::json!({ "unknown": "new" })),
        )
        .await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

        let relay = usable_relay(app.handle(), relay_id)
            .await
            .expect("unsupported probe should fall back to the saved backend");

        assert_eq!(relay.backend_kind, discovery::BackendKind::NewApi);
        assert_eq!(relay.auth_token, "saved-access-token");
        assert_eq!(relay.refresh_token.as_deref(), Some("saved-refresh-token"));
        server.abort();
    }

    #[tokio::test]
    async fn saved_relay_validation_preserves_credentials_on_conflicting_protocol() {
        let (origin, server) = spawn_discovery_server(
            Some(sub2api_discovery_body()),
            Some(newapi_discovery_body()),
        )
        .await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

        usable_relay(app.handle(), relay_id)
            .await
            .expect_err("conflicting probe must stop runtime dispatch");

        let relay = relay_credentials(&app, relay_id);
        assert_eq!(relay.auth_token, "saved-access-token");
        assert_eq!(relay.refresh_token.as_deref(), Some("saved-refresh-token"));
        server.abort();
    }

    #[tokio::test]
    async fn saved_relay_validation_preserves_credentials_on_transport_only_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind connection-drop server");
        let origin = format!("http://{}", listener.local_addr().expect("server address"));
        let server = tokio::spawn(async move {
            for _ in 0..discovery::PROBE_CANDIDATES.len() {
                let (stream, _) = listener.accept().await.expect("accept probe request");
                drop(stream);
            }
        });
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

        let error = usable_relay(app.handle(), relay_id)
            .await
            .expect_err("transport failure must stop dispatch");

        assert!(error.to_string().contains("连接"), "{error}");
        let relay = relay_credentials(&app, relay_id);
        assert_eq!(relay.auth_token, "saved-access-token");
        assert_eq!(relay.refresh_token.as_deref(), Some("saved-refresh-token"));
        server.await.expect("connection-drop server completes");
    }

    /// mock 站点轮换后回的假凭据：沿用 `saved_*` 前缀家族，仅供断言对得上号，
    /// 不是任何真实站点的密钥。
    const RENEWED_ACCESS: &str = "saved-access-token-renewed";
    const RENEWED_ROTATION: &str = "saved-refresh-token-renewed";

    /// ⭐ 回归闸（2026-08-17 bestapi.store 线上事故）：登录快照没带回过期时间
    /// （`token_expires_at = NULL`）的行，`token_looks_valid` 永远乐观为真，
    /// `usable_relay` 的主动续期永不触发；access token 在服务端到 24h 过期后，
    /// 探活撞上 401「登录已过期」直接清会话 —— refresh token 一次没用过就被连坐。
    /// 钉住：过期类 401 + 手里有 refresh token ⇒ 先静默续期一次、用新凭据重跑原请求。
    #[tokio::test]
    async fn relay_read_refreshes_once_and_retries_when_token_expires_server_side() {
        use axum::{
            http::{header, HeaderMap, StatusCode},
            routing::{get, post},
            Json, Router,
        };
        let router = Router::new()
            .route(
                "/api/v1/user/profile",
                get(|headers: HeaderMap| async move {
                    let stale = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| value.ends_with("saved-access-token"));
                    if stale {
                        return (
                            StatusCode::UNAUTHORIZED,
                            Json(serde_json::json!({
                                "code": "TOKEN_EXPIRED",
                                "message": "登录已过期，请重新登录"
                            })),
                        );
                    }
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": {
                                "id": 7,
                                "username": "Sub User",
                                "email": "sub@example.com",
                                "balance": 12.5,
                                "frozen_balance": 0.0
                            }
                        })),
                    )
                }),
            )
            .route(
                "/api/v1/auth/refresh",
                post(|Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(body["refresh_token"], "saved-refresh-token", "{body}");
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": {
                                "access_token": RENEWED_ACCESS,
                                "refresh_token": RENEWED_ROTATION,
                                "expires_at": 4_102_444_800_000_i64
                            }
                        })),
                    )
                }),
            );
        let (origin, server) = spawn_balance_server(router).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let balance = relay_read_with_refresh_retry(app.handle(), relay_id, |op| async move {
            backend::RuntimeBackend::for_relay(&op).balance().await
        })
        .await
        .expect("服务端 401 过期必须被静默续期救回");

        assert_eq!(balance.balance, 12.5);
        let creds = relay_credentials(&app, relay_id);
        assert_eq!(creds.auth_token, RENEWED_ACCESS);
        assert_eq!(creds.refresh_token.as_deref(), Some(RENEWED_ROTATION));
        assert!(
            creds.token_expires_at.is_some(),
            "续期响应带回的过期时间必须落库 —— 为 NULL 正是这起事故的起点"
        );
        server.abort();
    }

    /// 续期救不回来时（refresh token 也被服务端拒了），必须把**原错误**交回去：
    /// `check_session` 靠错误分类决定清会话，换成续期那条报错会悄悄改变判读。
    #[tokio::test]
    async fn relay_read_returns_original_error_when_refresh_cannot_rescue() {
        use axum::{
            http::StatusCode,
            routing::{get, post},
            Json, Router,
        };
        let router = Router::new()
            .route(
                "/api/v1/user/profile",
                get(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "code": "TOKEN_EXPIRED",
                            "message": "登录已过期，请重新登录"
                        })),
                    )
                }),
            )
            .route(
                "/api/v1/auth/refresh",
                post(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "code": "REFRESH_TOKEN_INVALID",
                            "message": "refresh token 已失效"
                        })),
                    )
                }),
            );
        let (origin, server) = spawn_balance_server(router).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let error = relay_read_with_refresh_retry(app.handle(), relay_id, |op| async move {
            backend::RuntimeBackend::for_relay(&op).balance().await
        })
        .await
        .expect_err("续期失败时原请求的错误必须往外抛");

        assert!(error.to_string().contains("登录已过期"), "{error}");
        assert!(!error.to_string().contains("续期失败"), "{error}");
        let creds = relay_credentials(&app, relay_id);
        assert_eq!(
            creds.auth_token, "saved-access-token",
            "续期失败不得改动库里的凭据"
        );
        server.abort();
    }

    /// ⭐ 回归闸：充值窗口持有 NewAPI 账号的 refresh 轮换独占权时，`usable_relay`
    /// 的静默续期必须被拦下 —— NewAPI 的 refresh cookie 一次性轮换，后台并发续期会把
    /// 充值窗口里种着的那颗 cookie 立刻作废（用户充值到一半被踢回登录页）。
    ///
    /// 两个断言互相补充：
    /// 1. 持 lease 时：报「充值窗口」错误，且 fake 站点收到 **0** 个 refresh 请求
    ///    （`newapi::refresh_url` 指向的端点）—— 闸必须挡在发请求之前，不是发完再补救。
    /// 2. drop lease 后：同一 relay 的 `usable_relay` 正常走续期（端点收到请求、拿到
    ///    新 token）—— 证明闸只认 lease，不是无条件挡路。
    ///
    /// 协议细节（refresh 端点路径、cookie 名）全部从 `newapi` owner 派生，本文件
    /// 不写字面量 —— `browser_login_dispatch_keeps_protocol_details_out_of_commands`
    /// 闸钉着 commands 层不得拥有这些细节。探测阶段会打 `/api/status`（还可能探别的
    /// 候选端点吃 404），与断言无关 —— 请求日志里只数 refresh 端点的个数。
    #[tokio::test]
    async fn active_purchase_session_blocks_newapi_refresh() {
        use axum::{
            extract::Request,
            http::{header, HeaderValue},
            middleware,
            middleware::Next,
            response::IntoResponse,
            routing::get,
            routing::post,
            Json, Router,
        };

        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let every_request = Arc::clone(&requests);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind refresh-block test server");
        let origin = format!("http://{}", listener.local_addr().expect("server address"));
        // refresh 端点路径从 owner 派生：fake 路由和下面的请求计数共用它，两边
        // 永远指向同一个端点（写两份字面量迟早分叉，而且这份文件不许有字面量）。
        let refresh_path = newapi::refresh_url(&origin)
            .expect("newapi refresh url")
            .path()
            .to_string();

        let site = Router::new()
            .route(
                "/api/status",
                get(move || {
                    let body = newapi_discovery_body();
                    async move { Json(body) }
                }),
            )
            // 不持 lease 的那次 `usable_relay` 要真的续期成功，回一个完整的 NewAPI
            // refresh 信封（parser 要求带轮换后的 Set-Cookie，名字同样取自 owner）。
            .route(
                refresh_path.as_str(),
                post(move || {
                    async move {
                        let set_cookie = format!(
                            "{}=rotated-secret; Path=/; HttpOnly",
                            newapi::REFRESH_COOKIE_NAME
                        );
                        (
                            [(
                                header::SET_COOKIE,
                                // HeaderValue 拥有自己的字节：cookie 值是运行期拼出来的
                                // （名字来自 owner 常量），借用拼不出 'static 响应。
                                HeaderValue::from_str(&set_cookie).expect("合法 set-cookie"),
                            )],
                            Json(serde_json::json!({
                                "success": true,
                                "data": {
                                    "access_token": "refreshed-access-token",
                                    "access_expires_at": 4_102_444_800_i64,
                                    "user": {
                                        "id": 7,
                                        "username": "saved-account",
                                        "display_name": "Saved Account",
                                        "email": "saved@example.test",
                                        "group": "default",
                                        "quota": 1,
                                        "used_quota": 0
                                    },
                                    "session": { "sid": "sid-refreshed" }
                                }
                            })),
                        )
                            .into_response()
                    }
                }),
            )
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let requests = Arc::clone(&every_request);
                async move {
                    requests.lock().unwrap().push(req.uri().path().to_string());
                    next.run(req).await
                }
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, site)
                .await
                .expect("serve refresh-block app");
        });

        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);
        expire_saved_relay_token(&app, relay_id);

        let refresh_count = || {
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|path| path.as_str() == refresh_path)
                .count()
        };

        let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
        let lease = coordinator.try_acquire(relay_id).expect("acquire lease");

        let error = usable_relay(app.handle(), relay_id)
            .await
            .expect_err("持 lease 时后台续期必须被拦下");
        assert!(error.to_string().contains("充值窗口"), "{error}");
        assert_eq!(
            refresh_count(),
            0,
            "闸必须挡在发请求之前：fake 站点不该收到任何 refresh 请求"
        );

        drop(lease);
        let relay = usable_relay(app.handle(), relay_id)
            .await
            .expect("lease 释放后同一 relay 必须恢复续期");
        assert_eq!(relay.auth_token, "refreshed-access-token");
        assert_eq!(
            refresh_count(),
            1,
            "不持 lease 时同一 relay 的 usable_relay 会正常尝试续期 —— 闸不是无条件挡路"
        );

        server.abort();
    }

    /// ⭐ 回归闸：sub2api 充值页必须打开**签名配置的 URL**，不再读站点公开设置的
    /// 支付开关去猜 `/purchase` 还是 `/redeem`。
    ///
    /// 三个断言互相补充：
    /// 1. 请求日志**不含** `/api/v1/settings/public` —— 路由事实已归签名目录，
    ///    生产充值路径不该再读站点公开设置。
    /// 2. 请求日志恰好是「开窗前续期 + 取账号档案」两个请求 —— 证明 token 寿命续期
    ///    与登录态注入这些既有行为没有被这次改动顺带丢掉。
    /// 3. 窗口打开的 URL **逐字符等于**配置值 —— 配置里故意用了不可推导的路径
    ///    （`/topup-center?flow=card`），推导逻辑造不出它。
    ///
    /// 用 middleware 记录**所有**请求的 path（含未注册路由的 404）：逐 handler 记录会漏掉
    /// 「代码打了但我们没 serve 的路径」，那种漏记正好把要抓的回归放跑。
    #[tokio::test]
    async fn sub2api_purchase_uses_signed_url() {
        use axum::{
            extract::Request, middleware, middleware::Next, routing::get, routing::post, Json,
            Router,
        };

        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let every_request = Arc::clone(&requests);

        let app = Router::new()
            // `ensure_token_outlasts_a_payment` 开窗前无条件续期要打的端点。
            .route(
                "/api/v1/auth/refresh",
                post(|| async move {
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": {
                            "access_token": "fresh-access-token",
                            "refresh_token": "fresh-refresh-token",
                            "expires_at": 4_102_444_800_000_i64
                        }
                    }))
                }),
            )
            // `auth_user_from_profile` 要的账号档案（信封 `data` 里必须有 `id`）。
            .route(
                "/api/v1/user/profile",
                get(|| async move {
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": { "id": 7, "email": "saved-account", "username": "Saved Account" }
                    }))
                }),
            )
            // ⚠️ 陷阱端点：旧实现靠它读站点公开设置的支付开关猜路由。这里故意把它
            // 配成一个能正常解析的 sub2api 响应 —— 只要充值流程还来问它，下面的
            // 断言当场红。
            .route(
                "/api/v1/settings/public",
                get(|| async move { Json(sub2api_discovery_body()) }),
            )
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let requests = Arc::clone(&every_request);
                async move {
                    requests.lock().unwrap().push(req.uri().path().to_string());
                    next.run(req).await
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind purchase test server");
        let addr = listener.local_addr().expect("server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve purchase app");
        });

        // relay 行存 http origin（测试站是本机 http 服务）；签名目录的 `purchase_url`
        // 用同 host:port 的 https 形态 —— `normalize_site_origin` 强制 https 且保留端口，
        // 所以两者恰好同源、`configured_purchase_url` 解析成功。
        let (app, relay_id) =
            saved_relay_app(&format!("http://{addr}"), discovery::BackendKind::Sub2Api);
        let op = relay_credentials(&app, relay_id);

        let configured = format!("https://{addr}/topup-center?flow=card");
        let config = remote_config::RemoteConfig {
            relay_directory: remote_config::RelayDirectoryPolicy {
                blocked_hosts: vec![],
                sites: std::collections::BTreeMap::from([(
                    "127.0.0.1".to_string(),
                    remote_config::RelayDirectorySite {
                        veridrop_host: None,
                        entry_url: None,
                        purchase_url: Some(configured.clone()),
                        usage_url: None,
                        display_name: None,
                    },
                )]),
            },
            ..remote_config::RemoteConfig::default()
        };
        let purchase_url = remote_config::configured_purchase_url(&config, &op.site_origin)
            .expect("签名目录解析不该报错")
            .expect("这个站在目录里配了购买入口");

        let window = purchase::purchase_window(relay_id, &op.site_origin);
        open_sub2api_site_window(app.handle(), op, window, purchase_url)
            .await
            .expect("开充值窗");

        let paths = requests.lock().unwrap().clone();
        assert_eq!(
            paths,
            vec!["/api/v1/auth/refresh", "/api/v1/user/profile"],
            "充值流程只该打「开窗前续期 + 取账号档案」两个请求；\
             出现 /api/v1/settings/public 说明又回去按公开设置猜路由了"
        );

        let window = app
            .get_webview_window(&purchase::window_label(relay_id))
            .expect("充值窗应该开出来了");
        let opened = window
            .url()
            .expect("mock 窗口能读回创建时的 URL")
            .to_string();
        assert_eq!(
            opened, configured,
            "打开的外部 URL 必须恰好是签名配置值，不是推导出的 /purchase 或 /redeem"
        );

        server.abort();
    }

    // ======================================================================
    // NewAPI 充值分派（Task 7）。全部直接驱动 `dispatch_purchase` —— 生产
    // `load_cached()` 用生产公钥验签，测试无法（也不该）伪造缓存，这个接缝与
    // `open_sub2api_purchase_window` 的「参数化只为可测」是同一惯例。
    // 协议字面量一律从 `newapi` owner 派生（backend.rs 的架构闸钉着）。
    // ======================================================================

    /// 起一个「记录全部请求 path」的哨兵站点：只应答 NewAPI 探测端点的形状
    /// （`/api/status`），其余 404 —— 任何路径都会进日志（middleware 不挑路由）。
    async fn recording_newapi_sentinel(
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        use axum::{extract::Request, middleware, middleware::Next, routing::get, Json, Router};

        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let every_request = Arc::clone(&requests);
        let app = Router::new()
            .route(
                "/api/status",
                get(|| async move { Json(newapi_discovery_body()) }),
            )
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let requests = Arc::clone(&every_request);
                async move {
                    requests.lock().unwrap().push(req.uri().path().to_string());
                    next.run(req).await
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind newapi purchase sentinel");
        let addr = listener.local_addr().expect("sentinel address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve newapi purchase sentinel");
        });
        (format!("http://{addr}"), requests, server)
    }

    /// 等 monitor 任务收场把 lease 还回去（`open` 返回与后台任务 drop lease 之间
    /// 有毫秒级竞态，轮询等待而不是睡固定时长）。
    async fn until_purchase_lease_released(
        app: &tauri::App<tauri::test::MockRuntime>,
        relay_id: i64,
    ) {
        let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while coordinator.is_active(relay_id) {
            assert!(
                std::time::Instant::now() < deadline,
                "命令返回后 lease 必须随即释放"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// 把 `saved_relay_app` 存好的 refresh credential 覆写成空白（模拟凭据缺失的行）。
    fn blank_saved_refresh_credential(app: &tauri::App<tauri::test::MockRuntime>, relay_id: i64) {
        let state = app.state::<AppState>();
        let conn = state.db.conn.lock().expect("lock memory database");
        conn.execute(
            "UPDATE loongport_relay SET refresh_token = ?1 WHERE id = ?2",
            rusqlite::params!["   ", relay_id],
        )
        .expect("blank refresh credential");
    }

    #[tokio::test]
    async fn newapi_purchase_dispatch_times_out_without_touching_sub2api_endpoints() {
        let (origin, requests, server) = recording_newapi_sentinel().await;
        // relay 行存 https 形态的哨兵 origin（生产行的 origin 在导入时就归一成 https），
        // purchase_url 用同 host:port 的 https 形态 —— `configured_purchase_url` 才解析得出。
        let (app, relay_id) = saved_relay_app(
            &origin.replacen("http://", "https://", 1),
            discovery::BackendKind::NewApi,
        );
        let op = relay_credentials(&app, relay_id);
        assert_eq!(
            op.refresh_token.as_deref(),
            Some("saved-refresh-token"),
            "前提：这行有非空 refresh credential"
        );

        let configured = format!(
            "{}{}",
            origin.replacen("http://", "https://", 1),
            "/console/topup"
        );
        let config = remote_config::RemoteConfig {
            relay_directory: remote_config::RelayDirectoryPolicy {
                blocked_hosts: vec![],
                sites: std::collections::BTreeMap::from([(
                    "127.0.0.1".to_string(),
                    remote_config::RelayDirectorySite {
                        veridrop_host: None,
                        entry_url: None,
                        purchase_url: Some(configured.clone()),
                        usage_url: None,
                        display_name: None,
                    },
                )]),
            },
            ..remote_config::RemoteConfig::default()
        };
        let purchase_url = remote_config::configured_purchase_url(&config, &op.site_origin)
            .expect("签名目录解析不该报错")
            .expect("这个站在目录里配了购买入口");

        // MockRuntime 的 cookies_for_url 恒返回空 ⇒ 走 300ms 启动超时路径（生产 20s）。
        let window = purchase::purchase_window(relay_id, &op.site_origin);
        let error = dispatch_site_window(app.handle(), op, window, purchase_url)
            .await
            .expect_err("mock 下观察不到轮换，必须按超时收场");

        assert!(
            error.to_string().contains("重新登录"),
            "超时错误要有「重新登录」语义：{error}"
        );

        // 协议隔离：NewAPI 的充值分派不得打任何 sub2api 端点（协议字面量属于
        // api.rs / 既有测试形状，这里只对照黑名单）。
        let paths = requests.lock().unwrap().clone();
        for forbidden in [
            "/api/v1/settings/public",
            "/api/v1/user/profile",
            "/api/v1/auth/refresh",
        ] {
            assert!(
                !paths.iter().any(|path| path == forbidden),
                "NewAPI 充值分派不该打 sub2api 端点 {forbidden}：{paths:?}"
            );
        }

        until_purchase_lease_released(&app, relay_id).await;
        // 「窗口已销毁」在 MockRuntime 上不可观察：destroy 只清运行时自己的窗口表，
        // manager 那份（get_webview_window 读的）要等事件循环处理 Destroyed 才清，
        // 而 mock 的 run_iteration 是 no-op、测试也不驱动 run。销毁动作本身钉在
        // `newapi_purchase::open` 的 ready-Err 路径（返回前 destroy）与 monitor 兜底；
        // 这里能观察到的等价不变量是「lease 已还」—— 没有挂着 lease 却无人管理的窗口。
        assert!(
            !app.state::<AppState>()
                .purchase_sessions
                .is_active(relay_id),
            "超时收场后 lease 必须已释放"
        );

        server.abort();
    }

    #[tokio::test]
    async fn newapi_purchase_blank_refresh_credential_is_rejected_before_a_window() {
        let (app, relay_id) =
            saved_relay_app("https://newapi.example", discovery::BackendKind::NewApi);
        blank_saved_refresh_credential(&app, relay_id);
        let op = relay_credentials(&app, relay_id);

        let window = purchase::purchase_window(relay_id, &op.site_origin);
        let error = dispatch_site_window(
            app.handle(),
            op,
            window,
            url::Url::parse("https://newapi.example/console/topup").unwrap(),
        )
        .await
        .expect_err("空白 refresh credential 必须在建窗前被拒绝");

        assert!(
            error.to_string().contains("重新登录"),
            "错误要指明出路：{error}"
        );
        assert!(
            app.get_webview_window(&purchase::window_label(relay_id))
                .is_none(),
            "被拒绝的调用不得留下窗口"
        );
        assert!(
            !app.state::<AppState>()
                .purchase_sessions
                .is_active(relay_id),
            "失败路径的 lease 必须已释放"
        );
    }

    #[tokio::test]
    async fn newapi_purchase_focuses_an_existing_window_without_http_or_lease() {
        // relay 行故意存 http origin：任何回归（比如有人把续期挪到聚焦检查之前）都会
        // 真的打上这个哨兵，日志就不再是空的。
        let (origin, requests, server) = recording_newapi_sentinel().await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

        // 预建同 label 窗口，模拟「这一行的充值窗已经开着」。
        let label = purchase::window_label(relay_id);
        tauri::WebviewWindowBuilder::new(
            app.handle(),
            &label,
            tauri::WebviewUrl::External(url::Url::parse("about:blank").unwrap()),
        )
        .build()
        .expect("预建同 label 窗口");

        let op = relay_credentials(&app, relay_id);
        let window = purchase::purchase_window(relay_id, &op.site_origin);
        dispatch_site_window(
            app.handle(),
            op,
            window,
            url::Url::parse(&format!("{origin}/console/topup")).unwrap(),
        )
        .await
        .expect("聚焦现有窗口是 Ok");

        assert!(
            requests.lock().unwrap().is_empty(),
            "同一行第二击不得发任何 HTTP：{:?}",
            requests.lock().unwrap()
        );
        assert!(
            !app.state::<AppState>()
                .purchase_sessions
                .is_active(relay_id),
            "聚焦路径不得取 lease"
        );

        server.abort();
    }

    #[tokio::test]
    async fn newapi_purchase_ignores_another_relays_lease() {
        // relay1（NewAPI）的 lease 被占着，relay2（也是 NewAPI）的充值分派照样能拿到
        // **自己的** lease —— lease 按 relay 键控，一行占用不该拦住另一行。
        //
        // 第二腿特意用 NewApi 而不是 sub2api（review F3）：sub2api 分派根本不碰
        // lease 协调器（跨行隔离对它是平凡成立，走通开窗本身已由 Task 4 的测试覆盖）；
        // NewApi 腿会真的执行 `try_acquire(relay2)`，被 relay1 的 lease 挡住与否在这里
        // 才是可观察的。判据：错误是 mock 下的启动超时（「重新登录」）而不是 lease
        // 占用文案（「正在使用或正在关闭」）—— 后者出现说明 try_acquire 被**别人**
        // 的 lease 挡了（两条文案都含「充值窗口」，判别要认后者的专属措辞）。
        let (app, relay1) =
            saved_relay_app("https://newapi.example", discovery::BackendKind::NewApi);
        let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
        let held = coordinator
            .try_acquire(relay1)
            .expect("占用 relay1 的 lease");

        // 第二行：另一个 NewAPI 站点账号，自己的 id 与有效 refresh credential。
        let relay2 = {
            let state = app.state::<AppState>();
            let conn = state.db.conn.lock().expect("lock memory database");
            let id = creds::save_site_with_backend(
                &conn,
                "https://newapi2.example",
                "Second relay",
                "https://newapi2.example",
                discovery::BackendKind::NewApi,
            )
            .expect("save second relay");
            creds::save_credentials(
                &conn,
                id,
                creds::AccountIdentity {
                    id: 8,
                    label: "Second Account",
                    login_identifier: "second-account",
                },
                "second-access-token",
                Some("second-refresh-cookie"),
                None,
                creds::SessionEnvironment::default(),
            )
            .expect("save second credentials");
            id
        };

        let op = relay_credentials(&app, relay2);
        let window = purchase::purchase_window(relay2, &op.site_origin);
        let error = dispatch_site_window(
            app.handle(),
            op,
            window,
            url::Url::parse("https://newapi2.example/console/topup").unwrap(),
        )
        .await
        .expect_err("mock 下观察不到轮换，relay2 走自己的超时收场");

        let error = error.to_string();
        assert!(
            error.contains("重新登录"),
            "relay2 拿到了自己的 lease、走进了自己的开窗流程（mock 超时收场）：{error}"
        );
        assert!(
            !error.contains("正在使用或正在关闭"),
            "出现 lease 占用类错误说明 try_acquire 被别人（relay1）的 lease 挡了：{error}"
        );

        assert!(
            coordinator.is_active(relay1),
            "relay1 的 lease 不受 relay2 的分派影响"
        );
        until_purchase_lease_released(&app, relay2).await;

        drop(held);
    }

    #[tokio::test]
    async fn newapi_purchase_reports_when_its_own_lease_is_already_held() {
        let (app, relay_id) =
            saved_relay_app("https://newapi.example", discovery::BackendKind::NewApi);
        let op = relay_credentials(&app, relay_id);

        let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
        let held = coordinator.try_acquire(relay_id).expect("预占自己的 lease");

        let window = purchase::purchase_window(relay_id, &op.site_origin);
        let error = dispatch_site_window(
            app.handle(),
            op,
            window,
            url::Url::parse("https://newapi.example/console/topup").unwrap(),
        )
        .await
        .expect_err("自己的 lease 被占时必须明确报错");

        assert!(
            error.to_string().contains("充值窗口"),
            "错误要说清是充值窗口占用：{error}"
        );
        assert!(
            app.get_webview_window(&purchase::window_label(relay_id))
                .is_none(),
            "不得开第二个窗口"
        );
        drop(held);
    }

    #[tokio::test]
    async fn newapi_account_mismatch_stops_before_group_or_token_inventory() {
        use axum::{
            routing::{delete, get, post},
            Json, Router,
        };
        use serde_json::json;

        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let account_requests = Arc::clone(&requests);
        let group_requests = Arc::clone(&requests);
        let token_requests = Arc::clone(&requests);
        let create_requests = Arc::clone(&requests);
        let reveal_requests = Arc::clone(&requests);
        let delete_requests = Arc::clone(&requests);
        let app = Router::new()
            .route(
                "/api/user/self",
                get(move || {
                    let requests = Arc::clone(&account_requests);
                    async move {
                        requests.lock().unwrap().push("account".into());
                        Json(json!({
                            "success": true,
                            "data": {
                                "id": 99,
                                "username": "other-account",
                                "display_name": "Other Account",
                                "email": "other@example.test",
                                "group": "default",
                                "quota": 0,
                                "used_quota": 0
                            }
                        }))
                    }
                }),
            )
            .route(
                "/api/user/self/groups",
                get(move || {
                    let requests = Arc::clone(&group_requests);
                    async move {
                        requests.lock().unwrap().push("groups".into());
                        Json(json!({ "success": true, "data": {} }))
                    }
                }),
            )
            .route(
                "/api/token/",
                get(move || {
                    let requests = Arc::clone(&token_requests);
                    async move {
                        requests.lock().unwrap().push("tokens".into());
                        Json(json!({
                            "success": true,
                            "data": {
                                "page": 1,
                                "page_size": 100,
                                "total": 0,
                                "items": []
                            }
                        }))
                    }
                })
                .post(move || {
                    let requests = Arc::clone(&create_requests);
                    async move {
                        requests.lock().unwrap().push("create".into());
                        Json(json!({ "success": true }))
                    }
                }),
            )
            .route(
                "/api/token/{id}/key",
                post(move || {
                    let requests = Arc::clone(&reveal_requests);
                    async move {
                        requests.lock().unwrap().push("reveal".into());
                        Json(json!({ "success": true, "data": { "key": "unexpected" } }))
                    }
                }),
            )
            .route(
                "/api/token/{id}",
                delete(move || {
                    let requests = Arc::clone(&delete_requests);
                    async move {
                        requests.lock().unwrap().push("delete".into());
                        Json(json!({ "success": true }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind account-mismatch server");
        let origin = format!("http://{}", listener.local_addr().expect("server address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        let op = creds::Relay {
            site_origin: origin,
            ..test_newapi_relay(7)
        };

        let error = match provision_backend(&op, None).await {
            Ok(_) => panic!("persisted account mismatch must stop provisioning"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("账号不一致"), "{error}");
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            ["account"],
            "account preflight must be the only remote request; no group/token inventory or mutation may run"
        );
        server.abort();
    }

    fn test_newapi_group(
        identity: &str,
        api_key: &str,
    ) -> crate::relay::newapi_provision::ReconciledGroup {
        crate::relay::newapi_provision::ReconciledGroup {
            identity: crate::relay::newapi::GroupIdentity(identity.into()),
            name: identity.into(),
            rate_multiplier: Some(1.25),
            description: format!("{identity} description"),
            api_key: api_key.into(),
            token_was_created: false,
        }
    }

    fn newapi_models() -> Vec<String> {
        provision::normalize_model_names(vec![
            "gemini-2.5-pro".into(),
            "claude-haiku-4-5".into(),
            "gpt-5.4".into(),
            "claude-sonnet-4-5".into(),
            "gpt-5.4".into(),
        ])
    }

    #[test]
    fn newapi_model_catalog_requires_at_least_one_normalized_model() {
        assert!(normalize_newapi_model_catalog(None).is_none());
        assert!(normalize_newapi_model_catalog(Some(vec!["  ".into(), "\n".into()])).is_none());
        assert_eq!(
            normalize_newapi_model_catalog(Some(vec![
                " gpt-5.4 ".into(),
                "gemini-2.5-pro".into(),
                "gpt-5.4".into(),
            ])),
            Some(vec!["gemini-2.5-pro".into(), "gpt-5.4".into()])
        );
    }

    fn newapi_batch(
        op: &creds::Relay,
        groups: &[crate::relay::newapi_provision::ReconciledGroup],
    ) -> ManagedProvisionBatch {
        let account_id = op.account_id.expect("test relay has account id");
        // 与 provision_backend 同一条纪律：keep 槽位跟着分类走（混合目录 = 三个聊天栏）。
        let mut observed_keep = std::collections::HashSet::new();
        let candidates = groups
            .iter()
            .flat_map(|group| {
                let models = newapi_models();
                newapi_keep_insert(
                    &mut observed_keep,
                    &op.site_origin,
                    account_id,
                    &group.identity,
                    Some(&models),
                );
                newapi_candidates_for_group(
                    &op.site_origin,
                    account_id,
                    group,
                    &models,
                    // 测试钉内置表：选型断言不随本机真实远端缓存漂移。
                    &provision::ModelSelectionTables::builtin(),
                )
            })
            .collect();
        ManagedProvisionBatch {
            account_id: Some(account_id),
            site_declaration: None,
            candidates,
            observed_keep,
            failures: Vec::new(),
            keys_created: 0,
        }
    }

    /// **纯生图分组的 new-api 扇出只出生图候选**（2026-09-05 某 new-api 站点纯生图
    /// 分组的实测形状：`gpt-image-2 + nano-banana-2`）。
    ///
    /// 旧行为把每个分组无条件扇出到 claude/codex/gemini：生图模型被写成聊天模型
    /// （`ANTHROPIC_MODEL=nano-banana-2`、`GEMINI_MODEL=gpt-image-2`），切过去调用必
    /// 404 —— 生图栏则永远零档位（「此账号在当前平台没有可用分组」）。
    #[test]
    fn newapi_pure_image_group_lands_only_in_the_image_column_and_migrates_legacy_tiers() {
        let op = test_newapi_relay(7);
        let group = test_newapi_group("图", "sk-image");
        let image_models =
            provision::normalize_model_names(vec!["nano-banana-2".into(), "gpt-image-2".into()]);
        let account_id = 7;

        let candidates = newapi_candidates_for_group(
            &op.site_origin,
            account_id,
            &group,
            &image_models,
            &provision::ModelSelectionTables::builtin(),
        );
        assert_eq!(candidates.len(), 1, "纯生图分组不该再扇出到聊天栏");
        assert_eq!(candidates[0].app_type, AppType::CodexImage);
        // 默认模型 = gpt-image 家族优先（跨家族并存时表里靠前的家族胜出）。
        assert_eq!(candidates[0].model, "gpt-image-2");

        // 先按旧行为落三栏（等价于升级前 provision 过的存量），再按新分类
        // provision 一次：三个聊天栏的旧投影必须被清掉、生图栏出现新档位。
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());
        persist_provision_batch(&state, &op, newapi_batch(&op, std::slice::from_ref(&group)))
            .expect("seed legacy three-column projections");
        let provider_id =
            provision::newapi_provider_id_for(&op.site_origin, account_id, &group.identity.0);
        for app_type in newapi_app_types() {
            assert!(db
                .get_provider_by_id(&provider_id, app_type.as_str())
                .expect("read legacy projection")
                .is_some());
        }

        let mut keep = std::collections::HashSet::new();
        newapi_keep_insert(
            &mut keep,
            &op.site_origin,
            account_id,
            &group.identity,
            Some(&image_models),
        );
        let migrated = ManagedProvisionBatch {
            account_id: Some(account_id),
            site_declaration: None,
            candidates,
            observed_keep: keep,
            failures: Vec::new(),
            keys_created: 0,
        };
        let summary =
            persist_provision_batch(&state, &op, migrated).expect("migrate to image column");
        assert_eq!(summary.tiers.len(), 1);
        assert_eq!(summary.tiers[0].app_id, AppType::CodexImage.as_str());
        for app_type in newapi_app_types() {
            assert!(
                db.get_provider_by_id(&provider_id, app_type.as_str())
                    .expect("read pruned projection")
                    .is_none(),
                "{} 的旧投影没被清掉",
                app_type.as_str()
            );
        }
        // 生图栏新档位：codex 形状（生图 MCP 的读取契约）+ 家族优先选出的模型。
        let image_provider = db
            .get_provider_by_id(&provider_id, AppType::CodexImage.as_str())
            .expect("read image projection")
            .expect("image projection exists");
        assert_eq!(
            provision::extract_model(&image_provider.settings_config).as_deref(),
            Some("gpt-image-2")
        );
        assert_eq!(
            provision::extract_api_key(&image_provider.settings_config, &AppType::CodexImage)
                .as_deref(),
            Some("sk-image")
        );
    }

    #[test]
    fn newapi_group_expands_to_three_app_configs_with_one_provider_id() {
        let op = test_newapi_relay(7);
        let group = test_newapi_group(" vip/\u{4e2d}\u{6587} \u{1f680} ", "sk-shared");
        let batch = newapi_batch(&op, std::slice::from_ref(&group));

        assert_eq!(batch.candidates.len(), 3);
        assert_eq!(batch.observed_keep.len(), 3);
        let provider_ids = batch
            .candidates
            .iter()
            .map(|candidate| candidate.provider_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(provider_ids.len(), 1);
        assert_eq!(
            batch
                .candidates
                .iter()
                .map(|candidate| candidate.app_type.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "codex", "gemini"]
        );

        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());
        let summary = persist_provision_batch(&state, &op, batch).expect("persist projections");

        assert_eq!(summary.tiers.len(), 3);
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            let provider = db
                .get_provider_by_id(summary.tiers[0].provider_id.as_str(), app_type.as_str())
                .expect("read provider")
                .expect("projection exists");
            assert_eq!(
                provision::extract_api_key(&provider.settings_config, &app_type).as_deref(),
                Some("sk-shared")
            );
            assert_eq!(
                provider.website_url.as_deref(),
                Some(op.site_origin.as_str())
            );
            assert_eq!(
                provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.loongport_account_id),
                Some(7)
            );
        }
    }

    #[test]
    fn newapi_refresh_preserves_edited_config_but_recomputes_unedited_defaults() {
        let op = test_newapi_relay(7);
        let first_group = test_newapi_group("vip", "sk-first");
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());
        let first = persist_provision_batch(&state, &op, newapi_batch(&op, &[first_group]))
            .expect("initial provision");
        let provider_id = first.tiers[0].provider_id.clone();

        let mut edited = db
            .get_provider_by_id(&provider_id, AppType::Codex.as_str())
            .expect("read edited provider")
            .expect("edited provider exists");
        edited.settings_config = provision::settings_config_for(
            &AppType::Codex,
            "sk-first",
            "Custom Name",
            "https://custom.example/v1",
            "gpt-custom",
        )
        .expect("custom codex config");
        let mut expected_edited = edited.settings_config.clone();
        assert!(provision::patch_api_key(
            &mut expected_edited,
            &AppType::Codex,
            "sk-second"
        ));
        db.save_provider(AppType::Codex.as_str(), &edited)
            .expect("save edited provider");
        db.set_user_edited(AppType::Codex.as_str(), &provider_id, true)
            .expect("mark edited");

        let mut unedited = db
            .get_provider_by_id(&provider_id, AppType::Gemini.as_str())
            .expect("read unedited provider")
            .expect("unedited provider exists");
        unedited.settings_config["env"]["GEMINI_MODEL"] =
            serde_json::Value::String("gemini-stale".into());
        db.save_provider(AppType::Gemini.as_str(), &unedited)
            .expect("save stale unedited provider");

        let second_group = test_newapi_group("vip", "sk-second");
        let second_batch = newapi_batch(&op, &[second_group]);
        persist_provision_batch(&state, &op, second_batch).expect("refresh provision");

        let edited_after = db
            .get_provider_by_id(&provider_id, AppType::Codex.as_str())
            .expect("read refreshed edited provider")
            .expect("refreshed edited provider exists");
        assert_eq!(edited_after.settings_config, expected_edited);
        let unedited_after = db
            .get_provider_by_id(&provider_id, AppType::Gemini.as_str())
            .expect("read refreshed default provider")
            .expect("refreshed default provider exists");
        assert_eq!(
            provision::extract_api_key(&unedited_after.settings_config, &AppType::Gemini)
                .as_deref(),
            Some("sk-second")
        );
        assert_eq!(
            unedited_after
                .settings_config
                .pointer("/env/GEMINI_MODEL")
                .and_then(serde_json::Value::as_str),
            Some("gemini-2.5-pro")
        );
    }

    #[test]
    fn newapi_unclassified_keep_retains_failed_group_and_prunes_only_the_current_account() {
        let account_seven = test_newapi_relay(7);
        let account_eight = test_newapi_relay(8);
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        persist_provision_batch(
            &state,
            &account_seven,
            newapi_batch(
                &account_seven,
                &[
                    test_newapi_group("observed", "sk-seven-observed"),
                    test_newapi_group("removed", "sk-seven-removed"),
                ],
            ),
        )
        .expect("seed account seven");
        persist_provision_batch(
            &state,
            &account_eight,
            newapi_batch(
                &account_eight,
                &[
                    test_newapi_group("observed", "sk-eight-observed"),
                    test_newapi_group("removed", "sk-eight-removed"),
                ],
            ),
        )
        .expect("seed account eight");

        let observed = crate::relay::newapi::GroupIdentity("observed".into());
        let retained_id =
            provision::newapi_provider_id_for(&account_seven.site_origin, 7, &observed.0);
        let removed_id =
            provision::newapi_provider_id_for(&account_seven.site_origin, 7, "removed");
        // 对账没走完（拿到 observed 清单但没拿到 sk）：分类未知，保全四个槽位。
        let mut failure_keep = std::collections::HashSet::new();
        newapi_keep_insert(
            &mut failure_keep,
            &account_seven.site_origin,
            7,
            &observed,
            None,
        );
        let failure_batch = ManagedProvisionBatch {
            account_id: Some(7),
            site_declaration: None,
            candidates: Vec::new(),
            observed_keep: failure_keep,
            failures: vec![FailureInfo {
                group_name: "observed".into(),
                reason: "reveal: temporary failure".into(),
            }],
            keys_created: 0,
        };
        let summary = persist_provision_batch(&state, &account_seven, failure_batch)
            .expect("retained existing providers keep the refresh partial-successful");

        assert!(summary.tiers.is_empty());
        assert_eq!(summary.failures.len(), 1);
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            assert!(db
                .get_provider_by_id(&retained_id, app_type.as_str())
                .expect("read retained provider")
                .is_some());
            assert!(db
                .get_provider_by_id(&removed_id, app_type.as_str())
                .expect("read removed provider")
                .is_none());

            let other_account_id =
                provision::newapi_provider_id_for(&account_eight.site_origin, 8, "removed");
            assert!(db
                .get_provider_by_id(&other_account_id, app_type.as_str())
                .expect("read other account provider")
                .is_some());
        }
    }

    #[test]
    fn newapi_provider_write_failure_keeps_successful_apps_and_reports_the_failure() {
        let op = test_newapi_relay(7);
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        {
            let conn = db.conn.lock().expect("lock memory db");
            conn.execute_batch(
                "CREATE TRIGGER fail_newapi_claude_write
                 BEFORE INSERT ON providers
                 WHEN NEW.app_type = 'claude'
                 BEGIN
                   SELECT RAISE(FAIL, 'injected claude write failure');
                 END;",
            )
            .expect("install selective write failure");
        }
        let state = AppState::new(db.clone());

        let summary = persist_provision_batch(
            &state,
            &op,
            newapi_batch(&op, &[test_newapi_group("partial", "sk-partial")]),
        )
        .expect("two successful app projections keep the batch successful");

        assert_eq!(
            summary
                .tiers
                .iter()
                .map(|tier| tier.app_id.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "gemini"]
        );
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].group_name, "partial");
        assert!(summary.failures[0].reason.contains("claude"));
        assert!(summary.failures[0]
            .reason
            .contains("injected claude write failure"));
    }

    /// 构造一条带归属的档位。`account` 为 `None` 表示升级前生成的旧档位。
    fn owned(id: &str, site: Option<&str>, account: Option<i64>) -> OwnedTier {
        OwnedTier {
            tier: tier(id),
            site_origin: site.map(str::to_string),
            account_id: account,
        }
    }

    /// `tiers_of_site` 的归属参数在归属测试里恒定，包一层省得每处重复。
    /// 它内部造一个空内存库当 state（`tiers_of_site` 要读「已手工维护」标记；
    /// 这些归属测试不关心标记，空库读出来全是 false 即可）。
    fn tiers_of(tiers: &[OwnedTier], site: &str, account: Option<i64>) -> Vec<TierInfo> {
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().expect("内存库"),
        ));
        tiers_of_site(&state, tiers, site, account, &test_app()).expect("tiers_of_site 不该失败")
    }

    /// ⭐ **`tiers_of_site` 的 `user_edited` 来自存库标记，不是内容比对。**
    ///
    /// 旧实现靠比对 settings_config 与默认值算出「用户改过没有」；现在改为读
    /// `providers.user_edited`（编辑页置位、恢复默认复位）。这条钉住：分组时
    /// `user_edited` 如实反映库里标记，而不是原样透传 `None`。
    #[test]
    fn grouping_reads_the_user_edited_flag_from_the_database(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("内存库"));
        let state = AppState::new(db.clone());
        // 先造两条 provider 行（get_user_edited 读的是 providers 表，不是空表）。
        {
            let conn = crate::database::lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config) \
                 VALUES ('t-default','codex','t-default','{}'), ('t-edited','codex','t-edited','{}')",
                [],
            )
            .expect("插行");
        }
        // 库里只给 t-edited 置位；t-default 不置。
        db.set_user_edited(AppType::Codex.as_str(), "t-edited", true)
            .expect("置位");

        let tiers = vec![
            owned("t-default", Some(site), Some(1)),
            owned("t-edited", Some(site), Some(1)),
        ];
        let got = tiers_of_site(&state, &tiers, site, Some(1), &test_app()).expect("分组不该失败");
        let flags: Vec<_> = got.iter().map(|t| t.user_edited).collect();
        assert_eq!(
            flags,
            vec![Some(false), Some(true)],
            "user_edited 该读库里标记（t-default 没置位=false，t-edited 置位=true）"
        );
        Ok(())
    }

    #[test]
    fn tiers_are_grouped_by_site_origin_not_by_guessing() {
        let a = "https://bestapi.store";
        let b = "https://other.dev";
        let tiers = vec![
            owned("t-a1", Some(a), Some(1)),
            owned("t-b1", Some(b), Some(1)),
            owned("t-a2", Some(a), Some(1)),
            // 没有 website_url 的历史数据：不归任何站。
            owned("t-orphan", None, Some(1)),
        ];

        assert_eq!(
            tiers_of(&tiers, a, Some(1))
                .iter()
                .map(|t| t.provider_id.clone())
                .collect::<Vec<_>>(),
            vec!["t-a1", "t-a2"],
            "同站的档位要按原顺序全带上（顺序 = provision 时的 sort_index，倍率低的在前）"
        );
        assert_eq!(tiers_of(&tiers, b, Some(1)).len(), 1);

        // 孤儿档位不能被塞给任何站 —— 塞错了用户会以为在 A 站买的档位属于 B 站。
        let all: usize = [a, b]
            .iter()
            .map(|s| tiers_of(&tiers, s, Some(1)).len())
            .sum();
        assert_eq!(all, 3, "4 条里那条没有 website_url 的必须落空");
    }

    /// ⭐ **同一个站上的两个账号不能看到对方的档位。**
    ///
    /// 实测踩到的类：归属原本只判 `website_url`（站点），于是同站每一行都显示该站的
    /// **全部**档位 —— 用户看到的档位数与他实际拥有的不符，点进去用的还是别人的 sk
    /// （连账单都算到别人头上）。
    #[test]
    fn tiers_are_split_between_two_accounts_on_the_same_site() {
        let site = "https://bestapi.store";
        let tiers = vec![
            owned("t-acct7", Some(site), Some(7)),
            owned("t-acct9", Some(site), Some(9)),
            // 升级前生成的：没记账号 ⇒ 只按站点归，两个账号都看得到（见函数文档）。
            owned("t-legacy", Some(site), None),
        ];

        let seven: Vec<_> = tiers_of(&tiers, site, Some(7))
            .iter()
            .map(|t| t.provider_id.clone())
            .collect();
        assert_eq!(
            seven,
            vec!["t-acct7", "t-legacy"],
            "账号 7 只该看到自己的 + 没记归属的旧档位，**不该看到账号 9 的**"
        );

        let nine: Vec<_> = tiers_of(&tiers, site, Some(9))
            .iter()
            .map(|t| t.provider_id.clone())
            .collect();
        assert_eq!(nine, vec!["t-acct9", "t-legacy"]);

        // 还没登录的行（没有 account_id）：有主的档位都不是它的。
        let anon: Vec<_> = tiers_of(&tiers, site, None)
            .iter()
            .map(|t| t.provider_id.clone())
            .collect();
        assert_eq!(anon, vec!["t-legacy"], "未登录的行不该认领任何有主的档位");
    }

    #[test]
    fn site_matching_is_exact_not_prefix() {
        // 前缀匹配会让 https://api.store 命中 https://api.store.evil.com。
        let tiers = vec![owned("t1", Some("https://api.store"), Some(1))];
        assert_eq!(tiers_of(&tiers, "https://api.store", Some(1)).len(), 1);
        assert!(tiers_of(&tiers, "https://api.sto", Some(1)).is_empty());
        assert!(tiers_of(&tiers, "https://api.store.evil.com", Some(1)).is_empty());
    }

    #[test]
    fn chatgpt_quit_is_codex_only() {
        // 用户同意 + codex ⇒ 退。
        assert!(should_quit_chatgpt(true, &AppType::Codex));
        // 用户同意但切的是别的平台 ⇒ **不退**。ChatGPT 桌面版只读 ~/.codex，
        // 切 claude/gemini 档位去关它纯属扰民（关掉用户正开着的、与本次切换无关的对话）。
        assert!(!should_quit_chatgpt(true, &AppType::Claude));
        assert!(!should_quit_chatgpt(true, &AppType::Gemini));
        // 用户没同意 ⇒ 一律不退，哪怕是 codex。
        assert!(!should_quit_chatgpt(false, &AppType::Codex));
    }

    #[test]
    fn switch_confirmation_is_decided_before_mutating_the_target() {
        assert!(should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Claude,
            None,
            true
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Codex,
            Some(false),
            true
        ));
    }

    #[test]
    fn managed_meta_pins_api_format_for_codex_and_leaves_others_empty() {
        // codex：不写 apiFormat 会落到 ProxyChat profile —— 那是唯一会 spawn codex
        // 子进程的分支。
        assert_eq!(
            managed_meta(&AppType::Codex, Some(1), None)
                .api_format
                .as_deref(),
            Some("openai_responses")
        );

        // 其它 CLI：`api_format` **只被 codex_config.rs 消费**，给它们填值不会有人读，
        // 反而让人以为那里有语义。
        for app_type in [AppType::Claude, AppType::Gemini] {
            assert_eq!(
                managed_meta(&app_type, Some(1), None).api_format,
                None,
                "{app_type:?} 不该有 api_format —— 只有 codex 会读它"
            );
        }
    }

    #[test]
    fn default_site_is_the_placeholder_from_the_requirement() {
        assert_eq!(DEFAULT_SITE, "790053500.com");
    }

    /// ⭐ 钉住「默认站在 aff **内置表**里有码」—— 这与它上一版的规则**正好相反**。
    ///
    /// 默认站曾是维护者自己的站，那时它**有意不在** aff 表里（服务端拒绝自己邀请自己）。
    /// 换成 `790053500.com` 之后那条理由不再适用，有码才是对的 —— 但
    /// [`crate::relay::aff`] 的测试里仍留着「维护者自己的站不该有码」那条，
    /// 很容易有人按类比把默认站也从表里划掉，而那**不报任何错**，
    /// 只是每一次「留空点确定」都白丢一笔返利。
    ///
    /// ⚠️ **它守的是内置那一层，不是运行时的最终取值**（codex review 纠正）：
    /// 实际取码走 [`crate::relay::remote_config::resolve_aff_code`] 的两层回落，
    /// 远端配置命中就用远端的，且**远端给空串 = 撤销、不回落到内置**。
    /// 所以本条断言不能、也不该保证「线上一定带码」—— 那取决于维护者当天发的配置。
    #[test]
    fn the_default_site_has_a_builtin_affiliate_code() {
        assert!(
            crate::relay::aff::aff_code_for(&format!("https://{DEFAULT_SITE}")).is_some(),
            "{DEFAULT_SITE} 是默认站且不是维护者自己的站，必须在 aff 内置表里"
        );
    }

    /// 造一条 provider。`site` 进 `website_url`（归属依据），`id` 决定它是否被认作托管项。
    fn seeded(id: &str, name: &str, site: Option<&str>) -> Provider {
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            settings_config: serde_json::json!({ "env": {} }),
            website_url: site.map(str::to_string),
            category: Some("aggregator".to_string()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    /// 带账号归属的那种（provision 从此都写它，见 `managed_meta`）。
    fn seeded_owned(id: &str, name: &str, site: Option<&str>, account_id: i64) -> Provider {
        Provider {
            meta: Some(managed_meta(&AppType::Codex, Some(account_id), None)),
            ..seeded(id, name, site)
        }
    }

    /// ⭐ **A 账号 provision 不能删掉同站 B 账号的档位。**
    ///
    /// 这是本轮实测追出来的一类：归属原本只判 `website_url`（站点），而 `keep` 只装
    /// **这一次** provision（= 一个账号）生成的 id ⇒ A 刷新一次就把 B 的全部档位
    /// 当成「不再存在」删光。同一个缺陷在 `remove_site_impl`（删一个账号）下更彻底：
    /// 它传空 `keep`，等于清掉该站所有账号的档位。
    #[test]
    fn pruning_one_account_leaves_another_accounts_tiers_on_the_same_site() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 7 的两条：一条这次仍在（keep 里），一条已失效。
        let a_kept = provision::provider_id_for(site, Some(7), 1);
        let a_stale = provision::provider_id_for(site, Some(7), 2);
        // 账号 9 的一条：**这次压根没查它**（不同账号、不同分组集合）。
        let b_tier = provision::provider_id_for(site, Some(9), 1);

        for p in [
            seeded_owned(&a_kept, "A·留", Some(site), 7),
            seeded_owned(&a_stale, "A·废", Some(site), 7),
            seeded_owned(&b_tier, "B·别动", Some(site), 9),
        ] {
            db.save_provider("codex", &p).expect("seed");
        }

        let state = AppState::new(db.clone());
        let keep: std::collections::HashSet<(String, String)> =
            [("codex".to_string(), a_kept.clone())]
                .into_iter()
                .collect();

        // 以账号 7 的身份清理。
        let removed = prune_stale_tiers(&state, site, Some(7), &keep).expect("prune");
        assert_eq!(removed, 1, "只该删账号 7 那条失效的");

        let ids = db.get_provider_ids("codex").expect("list");
        assert!(ids.contains(&a_kept), "账号 7 这次生成的要留着");
        assert!(!ids.contains(&a_stale), "账号 7 失效的那条该删");
        assert!(
            ids.contains(&b_tier),
            "⭐ 账号 9 的档位**必须留着** —— 它不在这次的 keep 里只是因为压根没查它"
        );
    }

    /// 这道闸守 `prune_stale_tiers` 的三个判据。
    ///
    /// 它是**唯一会删用户数据的 relay 代码路径**，判据放宽一点就会误删用户手工配置的
    /// provider（不可挽回）；收紧一点则清不掉脏记录（就是用户撞见的「claude 下还有
    /// codex 分组，点刷新也不消失」）。所以正反两面都要钉住。
    #[test]
    fn prune_only_touches_this_sites_managed_tiers() {
        let site = "https://bestapi.store";
        let other_site = "https://other.dev";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 这次 provision 生成的（该留）。
        let kept_id = provision::provider_id_for(site, Some(1), 1);
        // 同一个站的托管项，但这次没生成（该删 —— 分组已被中转站删掉 / 旧版本写错的）。
        let stale_id = provision::provider_id_for(site, Some(1), 2);
        // **别的站**的托管项：这次压根没查它的分组，凭「这次没生成」删它是错的。
        let other_site_id = provision::provider_id_for(other_site, Some(1), 3);

        for (app, p) in [
            ("codex", seeded(&kept_id, "留下", Some(site))),
            ("codex", seeded(&stale_id, "该删", Some(site))),
            ("codex", seeded(&other_site_id, "别的站", Some(other_site))),
            // 用户手工加的：id 不是我们生成的形状 ⇒ 一律不碰，哪怕 website_url 是同一个站。
            ("codex", seeded("my-own-provider", "用户自己的", Some(site))),
            // 托管项但没有 website_url（历史数据）⇒ 归属不明，不删（宁可漏删不可错删）。
            (
                "codex",
                seeded(
                    &provision::provider_id_for(site, Some(1), 9),
                    "无归属",
                    None,
                ),
            ),
            // **另一个 app_type 下的脏记录** —— 正是用户撞见的那种（openai 分组被
            // 旧代码写进了 claude 下）。必须也被清掉，所以不能只扫参数指定的那个 app。
            ("claude", seeded(&stale_id, "串台到 claude", Some(site))),
        ] {
            db.save_provider(app, &p).expect("seed");
        }

        let state = AppState::new(db.clone());
        // 这次只在 codex 下生成了 kept_id。
        let keep: std::collections::HashSet<(String, String)> =
            [("codex".to_string(), kept_id.clone())]
                .into_iter()
                .collect();

        let removed = prune_stale_tiers(&state, site, Some(1), &keep).expect("prune");
        assert_eq!(removed, 2, "该删的是 codex 与 claude 下那两条 stale");

        let codex_ids = db.get_provider_ids("codex").expect("list codex");
        assert!(codex_ids.contains(&kept_id), "这次生成的必须留着");
        assert!(!codex_ids.contains(&stale_id), "同站的过期档位必须删掉");
        assert!(
            codex_ids.contains(&other_site_id),
            "别的站的档位不能删 —— 这次没查它的分组"
        );
        assert!(
            codex_ids.contains("my-own-provider"),
            "用户手工配的 provider 绝不能删"
        );
        assert!(
            codex_ids.contains(&provision::provider_id_for(site, Some(1), 9)),
            "没有 website_url 的托管项归属不明，不该删"
        );

        let claude_ids = db.get_provider_ids("claude").expect("list claude");
        assert!(
            !claude_ids.contains(&stale_id),
            "串到别的 app_type 下的脏记录也要清 —— 只扫一个 app 就漏了它"
        );
    }

    /// ⭐ 用户实测那个 bug 的**精确复现**：同一个 id 在一个 app 下合法、在另一个下是脏的。
    ///
    /// ## 为什么上面那条测试放过了它
    ///
    /// 那条构造的串台记录在**两个 app 下都该删**（`keep` 里压根没有它）。
    /// 而真实情形是：`pro池` 这个分组的 platform 是 openai ⇒ 它在 **codex 下合法**，
    /// 但旧版本的 bug 把它也写进了 **claude** ⇒ claude 下那条是脏的。
    ///
    /// 而 `provider_id = sha256(site_origin + group_id)`，**不含 app_type** ⇒
    /// 两条记录的 id **完全相同**（实测 `loongport-8c669ca0b007e7ea`）。
    /// 于是「keep 只放 id」时：那个 id 因为 codex 下合法而进了 keep，
    /// claude 下那条脏记录就被当成「该保留」⇒ **点多少次刷新都不消失**。
    ///
    /// 这正是用户反复报的那个现象。判据必须是 **(app_type, id) 组合**。
    #[test]
    fn a_group_valid_in_one_app_does_not_protect_its_twin_in_another_app() {
        let site = "https://790053500.com";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 同一个分组（group_id = 1）⇒ 两个 app 下**同一个 id**。
        let shared_id = provision::provider_id_for(site, Some(1), 1);
        db.save_provider("codex", &seeded(&shared_id, "pro池", Some(site)))
            .expect("seed codex");
        db.save_provider("claude", &seeded(&shared_id, "pro池", Some(site)))
            .expect("seed claude");

        let state = AppState::new(db.clone());
        // 这次 provision 只把它落到 codex（因为它的 platform 是 openai）。
        let keep: std::collections::HashSet<(String, String)> =
            [("codex".to_string(), shared_id.clone())]
                .into_iter()
                .collect();

        let removed = prune_stale_tiers(&state, site, Some(1), &keep).expect("prune");

        assert_eq!(removed, 1, "claude 下那条脏记录必须被删掉");
        assert!(
            db.get_provider_ids("codex")
                .expect("codex")
                .contains(&shared_id),
            "codex 下那条是这次生成的，必须留着"
        );
        assert!(
            !db.get_provider_ids("claude")
                .expect("claude")
                .contains(&shared_id),
            "claude 下那条必须被删 —— 它与 codex 下那条 id 相同，\
             但『在 codex 下合法』不该保护它"
        );
    }

    /// 当前项也删。
    ///
    /// `ProviderService::delete` 拒绝删当前项（防用户误删正在用的配置），但走到 prune
    /// 这一步说明**服务端已经没有这个分组了**，它的 sk 是死的 —— 留着当「当前项」只会
    /// 让 CLI 拿失效密钥去发请求。用户重新选一个可用的即可。
    #[test]
    fn prune_deletes_the_current_tier_too() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let stale_id = provision::provider_id_for(site, Some(1), 7);

        db.save_provider("codex", &seeded(&stale_id, "过期的当前项", Some(site)))
            .expect("seed");
        db.set_current_provider("codex", &stale_id)
            .expect("set current");

        let state = AppState::new(db.clone());
        let removed = prune_stale_tiers(&state, site, Some(1), &std::collections::HashSet::new())
            .expect("prune");

        assert_eq!(removed, 1, "当前项也该被删掉");
        assert!(
            db.get_provider_by_id(&stale_id, "codex")
                .expect("query")
                .is_none(),
            "过期的当前项必须真的从库里消失"
        );
    }

    /// ⚠️ **「恢复默认配置」必须按档位自己的归属找中转站，不能用全局「当前站」。**
    ///
    /// 这条钉的是 review 抓出的那个 P0：原来那行是 `creds::load()`，返回的是
    /// `ORDER BY is_current DESC LIMIT 1` —— 全局当前站。而分组页把所有中转站并列，
    /// 用户展开 B 站点它的档位时，会拿到 **A 站的 `api_base_url`** ⇒ 那个档位被写成
    /// 「B 的 sk + A 的端点」⇒ 每次调用都 401，而界面显示恢复成功。
    ///
    /// **单站用户完全碰不到**（那时当前站就是唯一的站），所以手工测试测不出来 ——
    /// 这正是它需要一条测试的原因。
    ///
    /// 会红的改法：把归属判据换回 `creds::load()` / 任何「全局当前」的东西。
    struct ResetVerifier {
        senders:
            Mutex<HashMap<TargetKey, oneshot::Sender<Result<VerificationReport, RunFailureKind>>>>,
    }

    impl ResetVerifier {
        fn new() -> Self {
            Self {
                senders: Mutex::new(HashMap::new()),
            }
        }

        fn complete(&self, target: &TargetKey, report: VerificationReport) -> bool {
            self.senders
                .lock()
                .unwrap()
                .remove(target)
                .is_some_and(|sender| sender.send(Ok(report)).is_ok())
        }
    }

    impl ActiveVerifier for ResetVerifier {
        fn prepare(
            &self,
            target: TargetKey,
            progress: ProbeProgress,
        ) -> Result<PreparedVerification, RunFailureKind> {
            let (sender, receiver) = oneshot::channel();
            self.senders.lock().unwrap().insert(target, sender);
            let future: Pin<
                Box<
                    dyn Future<Output = Result<VerificationReport, RunFailureKind>>
                        + Send
                        + 'static,
                >,
            > = Box::pin(async move { receiver.await.unwrap() });
            let future = Box::pin(async move {
                let result = future.await;
                if result.is_ok() {
                    for completed in 1..=3 {
                        progress(completed);
                    }
                }
                result
            });
            Ok(PreparedVerification {
                total_checks: 3,
                future,
            })
        }
    }

    fn verification_report(target: TargetKey, verdict: Verdict) -> VerificationReport {
        VerificationReport {
            target,
            verdict,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts: Vec::new(),
            rules_version: RULES_VERSION,
            checked_at: 1_786_214_400,
        }
    }

    fn reset_state(valid_key: bool) -> (AppState, Arc<ResetVerifier>, String, String, TargetKey) {
        let site = "https://reset.example";
        let db = Arc::new(crate::database::Database::memory().expect("init db"));
        let verifier = Arc::new(ResetVerifier::new());
        let mut state = AppState::new(db.clone());
        state.model_verification = Arc::new(ModelVerificationCoordinator::with_verifier(
            db.clone(),
            verifier.clone(),
        ));
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "Reset", "https://reset.example/v1")
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "reset@example.com",
                    login_identifier: "reset@example.com",
                },
                "token",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let other_provider_id = provision::provider_id_for(site, Some(7), 2);
        let settings_config = if valid_key {
            provision::settings_config_for(
                &AppType::Codex,
                "sk-reset",
                "Reset tier",
                "https://reset.example/v1",
                DEFAULT_MODEL,
            )
            .expect("codex config")
        } else {
            serde_json::json!({"model_provider":"custom"})
        };
        let provider = Provider {
            settings_config,
            ..seeded_owned(&provider_id, "Reset tier", Some(site), 7)
        };
        db.save_provider("codex", &provider).expect("save provider");
        db.save_provider(
            "codex",
            &Provider {
                settings_config: provision::settings_config_for(
                    &AppType::Codex,
                    "sk-other",
                    "Other tier",
                    "https://reset.example/v1",
                    DEFAULT_MODEL,
                )
                .expect("other config"),
                ..seeded_owned(&other_provider_id, "Other tier", Some(site), 7)
            },
        )
        .expect("save other provider");
        db.set_user_edited("codex", &provider_id, true)
            .expect("mark edited");

        let running = TargetKey::new(&provider_id, "codex", "gpt-running");
        for report in [
            verification_report(
                TargetKey::new(&provider_id, "codex", "gpt-a"),
                Verdict::Suspicious,
            ),
            verification_report(
                TargetKey::new(&provider_id, "codex", "gpt-b"),
                Verdict::Anomaly,
            ),
            verification_report(
                TargetKey::new(&other_provider_id, "codex", "gpt-other"),
                Verdict::Trusted,
            ),
        ] {
            crate::relay::model_verification::store::upsert_active(&db, &report)
                .expect("seed verification report");
        }

        (state, verifier, provider_id, other_provider_id, running)
    }

    #[tokio::test]
    async fn reset_tier_config_validation_failure_cancels_run_but_preserves_all_reports() {
        let (state, verifier, provider_id, other_provider_id, running) = reset_state(false);
        state
            .model_verification
            .start(running.clone())
            .await
            .expect("start run");

        let error = reset_tier_config_in_state(&state, &provider_id, AppType::Codex)
            .expect_err("missing key must reject reset");

        assert!(error.to_string().contains("密钥"));
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id.clone(), other_provider_id.clone()])
                .expect("list reports")
                .len(),
            3
        );
        let _ = verifier.complete(
            &running,
            verification_report(running.clone(), Verdict::Trusted),
        );
        tokio::task::yield_now().await;
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id, other_provider_id])
                .expect("reports after late completion")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn reset_tier_config_save_failure_cancels_run_but_preserves_all_reports() {
        let (state, verifier, provider_id, other_provider_id, running) = reset_state(true);
        state
            .model_verification
            .start(running.clone())
            .await
            .expect("start run");
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_reset BEFORE UPDATE ON providers
                 BEGIN SELECT RAISE(FAIL, 'reject reset'); END;",
            )
            .expect("install failure trigger");

        let error = reset_tier_config_in_state(&state, &provider_id, AppType::Codex)
            .expect_err("provider save must fail");

        assert!(matches!(error, AppError::Database(_)));
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id.clone(), other_provider_id.clone()])
                .expect("list reports")
                .len(),
            3
        );
        let _ = verifier.complete(
            &running,
            verification_report(running.clone(), Verdict::Trusted),
        );
        tokio::task::yield_now().await;
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id, other_provider_id])
                .expect("reports after late completion")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn reset_tier_config_success_clears_only_target_scope_and_rejects_late_completion() {
        let (state, verifier, provider_id, other_provider_id, running) = reset_state(true);
        state
            .model_verification
            .start(running.clone())
            .await
            .expect("start run");

        reset_tier_config_in_state(&state, &provider_id, AppType::Codex).expect("reset succeeds");

        let rows = state
            .model_verification
            .list_results(&[provider_id.clone(), other_provider_id.clone()])
            .expect("list reports");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target.provider_id, other_provider_id);
        assert!(!state
            .db
            .get_user_edited("codex", &provider_id)
            .expect("edited flag"));
        let _ = verifier.complete(
            &running,
            verification_report(running.clone(), Verdict::Trusted),
        );
        tokio::task::yield_now().await;
        assert!(state
            .model_verification
            .list_results(&[provider_id])
            .expect("target reports")
            .is_empty());
    }

    /// 备份是「删 auth.json」之前的唯一后路，所以它必须真的把内容拷出来。
    ///
    /// ⚠️ **测试绝不能碰真实的 `~/.codex/auth.json`** —— 那里面是用户的 OAuth
    /// refresh token，跑一次测试把开发者自己的 ChatGPT 登录搞掉是不可接受的副作用
    /// （`chatgpt_app.rs:349` 那条注释钉的是同一件事）。所以这里不调
    /// `get_codex_auth_path()`，而是自己造一个临时文件喂给 `backup_codex_auth`，
    /// 并用 `CC_SWITCH_TEST_HOME` 把备份目标也关进临时目录。
    #[test]
    #[serial_test::serial]
    fn backup_copies_auth_json_before_it_gets_deleted() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let original_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let auth_path = temp.path().join("auth.json");
        let payload = r#"{"tokens":{"refresh_token":"secret"}}"#;
        std::fs::write(&auth_path, payload).expect("write fake auth.json");

        let backup = backup_codex_auth(&auth_path)
            .expect("备份不该失败")
            .expect("有源文件时必须返回备份路径");

        let backup_path = std::path::Path::new(&backup);
        assert_eq!(
            std::fs::read_to_string(backup_path).expect("read backup"),
            payload,
            "备份内容必须与原文件逐字节一致 —— 它是用户唯一的还原来源"
        );
        assert!(
            auth_path.exists(),
            "备份是**拷贝**不是移动：这一步失败时调用方要能原地中止，源文件必须还在"
        );
        assert!(
            backup_path.starts_with(temp.path()),
            "备份必须落在 CC_SWITCH_TEST_HOME 下，绝不能写到真实的 ~/.cc-switch"
        );
        let name = backup_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        assert!(
            name.starts_with("codex-auth-") && name.ends_with(".json"),
            "文件名要能让人一眼看出这是什么、什么时候备的，实际是 {name}"
        );

        match original_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }

    /// 没有 `auth.json` 是正常状态（从没登录过 ChatGPT），**不是错误**。
    ///
    /// 判成错误的后果：整条「切回官方登录」在这类用户身上直接失败，
    /// 而他们恰恰是最该能用它的人（想清掉 LoongPort 写的路由、自己去登录）。
    #[test]
    #[serial_test::serial]
    fn missing_auth_json_is_not_an_error() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let original_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let absent = temp.path().join("auth.json");
        assert!(!absent.exists(), "前提：这个文件本来就不存在");

        assert!(
            backup_codex_auth(&absent)
                .expect("不存在不该报错")
                .is_none(),
            "没有源文件时返回 None（表示「没什么可备份」），而不是 Err"
        );
        assert!(
            !temp.path().join(".loongport").join("backups").exists(),
            "没东西要备份时不该顺手建出一个空的 backups 目录"
        );

        match original_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }

    /// ⭐ **命令层必须真的调 `refresh_live_for_current_tiers`** —— 两处都不能漏。
    ///
    /// ## 为什么这条测试读源码而不是调函数
    ///
    /// `do_provision` 仍吃 `&tauri::AppHandle`；reset 的数据库与协调器路径已经下沉到
    /// `reset_tier_config_in_state` 并由真实行为测试覆盖，但“当前项刷新 live 文件”会触碰
    /// 用户配置，单元测试不能安全执行。第二路 review 实测证明了这条接线盲区的代价：
    /// 把那两处调用注释掉，2578 条测试**全绿**——
    /// 那条集成测试（`loongport_codex_live.rs`）自己调服务层，所以它测的是服务层，
    /// 不是「命令层有没有调服务层」。
    ///
    /// 源码断言是这里唯一能把那一步钉住的手段（与仓里 `vendorSwitchGuardContract`
    /// 那条同一个理由与形态）。它守的不是实现细节，而是**这条链路还接着吗** ——
    /// 断了的症状是静默的：界面提示刷新成功，而 CLI 一直用旧密钥。
    #[test]
    fn refresh_live_for_current_tiers_is_wired_into_both_commands() {
        let src = include_str!("relay.rs");

        // 取 `do_provision` 到 `prune_stale_tiers` 调用之间那段（provision 那条路）。
        let provision = {
            let start = src
                .find("async fn do_provision")
                .expect("do_provision 还在吗");
            let end = src[start..]
                .find("let removed = prune_stale_tiers")
                .expect("provision 末尾那段清理还在吗");
            &src[start..start + end]
        };
        assert!(
            provision.contains("refresh_live_for_current_tiers(state, &refresh_live)"),
            "⭐ `do_provision` 不再刷新当前档位的 live config —— \
             sk 被撤销重建后，CLI 会一直用旧密钥，而用户点不动那个档位（UI 认为它已是当前项）"
        );

        // 取真正执行重置的 state helper 那段。
        let reset = {
            let start = src
                .find("fn reset_tier_config_in_state")
                .expect("reset_tier_config_in_state 还在吗");
            let end = src[start..]
                .find("\n/// 保存中转站行的手工顺序")
                .expect("reset 之后那个命令还在吗");
            &src[start..start + end]
        };
        assert!(
            reset.contains("refresh_live_for_current_tiers("),
            "⭐ `reset_tier_config_impl` 不再刷新 live config —— \
             那会让「恢复默认配置」这个按钮对当前项**整体无效**（改坏的配置就在 live 文件里）"
        );
    }

    /// ⭐ **默认路径下，删账号不许毁掉「别的平台」正在用的档位** —— 前端那道判据挡不住这一类。
    ///
    /// 这是 review 抓出的缺陷现场，复现路径：
    ///
    /// 1. `list_relays_impl` 吃 `app_type` ⇒ `RelayRow.tiers` 只含**当前 tab** 的档位；
    /// 2. 如果删除资格只按当前 tab 的档位判断，claude tab 可能看不到 codex 的当前项；
    /// 3. 而这个账号在 **codex** 下的档位正是 codex 的当前项 ⇒ 删下去把它清了，
    ///    `~/.codex/config.toml` 却还指着它。
    ///
    /// 所以闸必须在后端、必须扫全部 app。**会红的改法**：把
    /// `apps_using_this_accounts_tiers` 从只扫 `AppType::all()` 改成只扫某一个 app。
    #[test]
    fn removing_an_account_is_refused_while_another_app_still_uses_its_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
        })
        .expect("save site");

        // 登录这一行 —— **必须有 `account_id`**：没有它的行派生不出 provider id、
        // 名下不可能有档位，守卫对那种行有意不拦（见
        // `an_untagged_row_is_not_blocked_by_another_accounts_current_tier`）。
        let row_id = with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "me@example.com",
                    login_identifier: "me@example.com",
                },
                "tok",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        // 这个账号在 codex 下的档位，且**它就是 codex 的当前项**。
        let codex_tier = provision::provider_id_for(site, Some(7), 1);
        db.save_provider(
            "codex",
            &seeded_owned(&codex_tier, "BestApi · Pro", Some(site), 7),
        )
        .expect("seed codex tier");
        db.set_current_provider("codex", &codex_tier)
            .expect("set codex current");

        // 用户此刻停在 claude tab 上（那边这一行没有当前项）—— 前端会放行，后端必须拦。
        let err = remove_site_impl(&state, row_id, false, None)
            .expect_err("⭐ 名下有档位是别的平台的当前项时，删除必须失败");
        let msg = err.to_string();
        assert!(
            msg.contains("codex"),
            "文案必须点名是哪个平台 —— 用户要去那里切走，实际：{msg}"
        );
        assert!(
            msg.contains("BestApi · Pro"),
            "文案必须点名是哪个档位，实际：{msg}"
        );

        // 全有或全无：拦下之后**一条都不能少**，账号行也必须还在。
        assert!(
            db.get_provider_by_id(&codex_tier, "codex")
                .expect("query")
                .is_some(),
            "被拦下时那条档位必须完好 —— 半删会留下用户处置不了的孤儿记录"
        );
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("query row")
                .is_some(),
            "档位没删掉，账号行也不该删"
        );
    }

    /// 反面：没有任何平台在用它时，删除照常进行（连带清掉档位）。
    ///
    /// 这条与上一条成对 —— 只有上一条的话，把闸写成「无条件拒绝」也能过。
    #[test]
    fn removing_an_account_still_works_when_no_app_uses_its_tiers() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
        })
        .expect("save site");

        let tier = provision::provider_id_for(site, None, 1);
        db.save_provider("codex", &seeded(&tier, "BestApi · Pro", Some(site)))
            .expect("seed");
        // **不设 current** —— 别的 provider 是当前项，或压根没有当前项。

        remove_site_impl(&state, row_id, false, None).expect("没人在用它时删除该成功");

        assert!(
            db.get_provider_by_id(&tier, "codex")
                .expect("query")
                .is_none(),
            "档位该被连带清掉"
        );
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("query row")
                .is_none(),
            "账号行该被删掉"
        );
    }

    /// 第三条出路：用户在前端弹窗里看着「Codex 正在用 xxx」按了确认（`force`）⇒
    /// 连在用的档位一起删干净。
    ///
    /// 这条与第一条成对 —— 没有它的话，把闸写成「在用就无条件拒绝（连 force 也拦）」
    /// 也能过前两条。钉住的语义：
    ///
    /// - force 放行后**全有或全无**：档位、账号行都得没了（不留孤儿记录）；
    /// - 被删的是 codex 的**当前项** —— 这条测试的内存库里**没有官方 seed**
    ///   （`init_default_official_providers` 只在真应用启动时跑），切官方必然失败
    ///   ⇒ 它同时钉住降级路径：**安置失败不阻断删除**，current 悬空自愈。
    ///   （正向「切回官方」那条没法单测 —— `ProviderService::switch` 会写真实
    ///   live 配置文件，codex/claude 没有测试沙箱；映射由
    ///   `official_seed_id_maps_text_apps_and_denies_codex_image` 把守。）
    #[test]
    fn forced_removal_deletes_even_while_another_app_uses_its_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
        })
        .expect("save site");

        let row_id = with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "me@example.com",
                    login_identifier: "me@example.com",
                },
                "tok",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let codex_tier = provision::provider_id_for(site, Some(7), 1);
        db.save_provider(
            "codex",
            &seeded_owned(&codex_tier, "BestApi · Pro", Some(site), 7),
        )
        .expect("seed codex tier");
        db.set_current_provider("codex", &codex_tier)
            .expect("set codex current");

        remove_site_impl(&state, row_id, true, None).expect("force 是用户知情后的选择，该放行");

        assert!(
            db.get_provider_by_id(&codex_tier, "codex")
                .expect("query")
                .is_none(),
            "force 该连当前项档位一起删掉"
        );
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("query row")
                .is_none(),
            "账号行该被删掉"
        );
    }

    /// 闸的归属判据必须与 `prune_stale_tiers` 是**同一份** —— 否则守卫与删除各认一套：
    /// 守卫说「这条不是你的、不拦」，删除说「这条是你的、删了」⇒ 恰好绕过守卫。
    ///
    /// 这条钉的是「别人的当前项不该拦住我」这一半（宽松方向的误判）。
    #[test]
    fn the_guard_ignores_another_accounts_current_tier_on_the_same_site() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的档位是 codex 的当前项。
        let b_tier = provision::provider_id_for(site, Some(9), 1);
        db.save_provider("codex", &seeded_owned(&b_tier, "B 的档位", Some(site), 9))
            .expect("seed");
        db.set_current_provider("codex", &b_tier)
            .expect("set current");

        let state = AppState::new(db.clone());

        // 以账号 7 的身份问「我名下有在用的吗」—— 答案必须是「没有」。
        assert!(
            apps_using_this_accounts_tiers(&state, site, Some(7)).is_empty(),
            "同站另一个账号的当前项不该拦住我删自己的账号"
        );
        // 而账号 9 自己问，必须撞上。
        assert_eq!(
            apps_using_this_accounts_tiers(&state, site, Some(9)).len(),
            1,
            "账号 9 名下那条正是当前项，必须被认出来"
        );
    }

    /// ⭐ **还没登录的行（`account_id` 为 `None`）不该被别人的档位拦住**。
    ///
    /// 第二路 review 抓出的：`belongs_to_account` 对 `None` 返回 `true`（"不按账号过滤"），
    /// 那对**删除**方向是对的（同站没记归属的旧档位该跟着清），但守卫方向反过来就成了
    /// 「把别人正在用的档位算成你的」。
    ///
    /// 这种行真实可达：`clear_credentials` 会把 `account_id` 置 `NULL`（站点换了后端
    /// 协议时走这条），而唯一索引把 `NULL` 视为互不相等 ⇒ 它与已登录的行并存。
    /// 症状是用户删一个**空行**时被告知「你名下还有档位正在使用中：B 的档位（codex）」，
    /// 而唯一出路是去 codex 把 B 切走。
    ///
    /// 会红的改法：去掉 `apps_using_this_accounts_tiers` 里那个 `account_id.is_some()`。
    #[test]
    fn an_untagged_row_is_not_blocked_by_another_accounts_current_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的档位是 codex 的当前项。
        let b_tier = provision::provider_id_for(site, Some(9), 1);
        db.save_provider("codex", &seeded_owned(&b_tier, "B 的档位", Some(site), 9))
            .expect("seed");
        db.set_current_provider("codex", &b_tier)
            .expect("set current");

        let state = AppState::new(db.clone());

        assert!(
            apps_using_this_accounts_tiers(&state, site, None).is_empty(),
            "⭐ 还没登录的行认不出归属 ⇒ 不该拦。它派生不出 provider id，\
             名下本来就不可能有档位，漏拦没有代价；而误拦会让用户删不掉一个空行"
        );

        // 而删除方向的语义不变：`prune_stale_tiers` 传 `None` 时仍会清同站没记归属的档位。
        // 这条只是确认上面那个改动没顺手改掉 `belongs_to_account` 本身。
        let legacy = provision::provider_id_for(site, None, 5);
        db.save_provider("codex", &seeded(&legacy, "旧数据", Some(site)))
            .expect("seed legacy");
        let legacy_provider = db
            .get_provider_by_id(&legacy, "codex")
            .expect("query")
            .expect("在");
        assert!(
            belongs_to_account(&legacy_provider, site, None),
            "删除方向对 `None` 仍是「算是我的」—— 那是旧数据能被清掉的前提"
        );
    }

    /// ⭐ **还没登录的 relay 行，不能把同站别人账号的档位记到自己头上。**
    ///
    /// Task 3 review 抓出的：把 `relay_balance_inputs` 的内联判据收敛到
    /// `belongs_to_account` 时，`(relay.account_id, 档位账号)` 的 `(None, Some)` 那格
    /// 从「不认」翻成了「认」—— 未登录行会收走别人档位的 sk、对账会把别人的成本
    /// 算进这一行。现在余额 / 对账走严格版 [`belongs_to_relay`]（还原原内联语义
    /// `(None, Some(_)) => false`），清理 / 守卫路径仍走宽松版 [`belongs_to_account`]。
    ///
    /// 会红的改法：`relay_balance_inputs` 改回 `belongs_to_account`。
    #[test]
    fn an_unlogged_relay_row_is_not_attributed_another_accounts_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的档位，带真实 sk（否则「没收走」的断言没有判别力）。
        let b_tier = provision::provider_id_for(site, Some(9), 1);
        let mut b_provider = seeded_owned(&b_tier, "B 的档位", Some(site), 9);
        b_provider.settings_config = serde_json::json!({ "auth": { "OPENAI_API_KEY": "sk-b" } });
        db.save_provider("codex", &b_provider).expect("seed B");

        // 同站一条没记账号的旧档位（升级前生成），也没有 sk —— 只用于钉住
        // `(None, None) => true` 这格没被顺手改掉。
        let legacy = provision::provider_id_for(site, None, 5);
        db.save_provider("codex", &seeded(&legacy, "旧数据", Some(site)))
            .expect("seed legacy");

        let state = AppState::new(db.clone());
        let mut unlogged = purchase_capability_relay(creds::BackendKind::Sub2Api);
        unlogged.site_origin = site.to_string();
        unlogged.account_id = None;

        let (_, keys) = relay_balance_inputs(&state, &unlogged);
        assert!(
            keys.is_empty(),
            "⭐ 未登录的行认不出归属 ⇒ 同站别人账号的档位（哪怕有 sk）不该被收走：{keys:?}"
        );

        // 对照组：账号 9 自己的行必须能拿到那把 sk —— 证明上面不是「本来就收不到」。
        let mut owner_row = unlogged.clone();
        owner_row.account_id = Some(9);
        let (_, keys) = relay_balance_inputs(&state, &owner_row);
        assert_eq!(
            keys,
            vec!["sk-b".to_string()],
            "档位自己的账号必须收得到 sk"
        );

        // 两个判据函数在关键那格的分歧是**有意的**，钉住防止将来被「顺手统一」：
        // 删除方向（belongs_to_account）对 None 宽松（旧数据要能清），
        // 归属方向（belongs_to_relay）对 None 严格（别人的不能认领）。
        let b_in_db = db
            .get_provider_by_id(&b_tier, "codex")
            .expect("query")
            .expect("在");
        assert!(
            belongs_to_account(&b_in_db, site, None),
            "删除方向对 `None` 仍宽松 —— 别改"
        );
        assert!(
            !belongs_to_relay(&b_in_db, site, None),
            "归属方向对 `(None, Some)` 必须严格 —— 别人的档位不能记到未登录行头上"
        );
        let legacy_in_db = db
            .get_provider_by_id(&legacy, "codex")
            .expect("query")
            .expect("在");
        assert!(
            belongs_to_relay(&legacy_in_db, site, None),
            "同站没记归属的旧档位仍是「可能是我的」（(None, None) => true）"
        );
    }

    /// ⭐ **登录态失效之后，那一行仍然带着它的档位、昵称和「已过期」这个状态。**
    ///
    /// 修之前 `check_session` 走的是 `clear_credentials`，它把 `account_id` 一起抹掉，
    /// 于是三件事同时静默出错（都不报任何错）：
    ///
    /// 1. `tiers_of_site` 对「行没有 account_id、档位有」判为不属于它
    ///    ⇒ **返回空 tiers**，界面退化成「没有可用分组 + 获取密钥」；
    /// 2. `session_expired()` 要求 `account_id.is_some()` ⇒ 变成 `false`
    ///    ⇒ 界面说「还没登录」，而用户明明登录过；
    /// 3. `account_label` 被清空 ⇒ 昵称没了。
    ///
    /// 而 sk 一把都没失效。用户看到的是「密钥没了」，然后去重建一遍。
    ///
    /// 会红的改法：把 `check_session` 里的 `clear_session` 换回 `clear_credentials`。
    #[test]
    fn an_expired_session_keeps_its_tiers_label_and_usable_status() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());

        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestAPI", "https://bestapi.store")
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "tok",
                None,
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let tier_id = provision::provider_id_for(site, Some(7), 1);
        let settings_config = provision::settings_config_for(
            &AppType::Codex,
            "sk-valid",
            "Pro池",
            "https://bestapi.store/v1",
            "gpt-5.6-sol",
        )
        .expect("settings");
        db.save_provider(
            "codex",
            &Provider {
                settings_config,
                ..seeded_owned(&tier_id, "Pro池", Some(site), 7)
            },
        )
        .expect("seed tier");

        with_conn(&state, |conn| creds::clear_session(conn, row_id)).expect("clear session");

        let rows = list_relays_impl(&state, AppType::Codex).expect("list relays");
        let row = rows.iter().find(|r| r.id == row_id).expect("行还在");

        assert!(
            matches!(row.status, RelayRowStatus::SessionExpiredUsable),
            "登录过 + 没 token + 没 refresh ⇒ 必须报「登录已过期」，而不是「还没登录」"
        );
        assert_eq!(row.account_label, "我的号", "昵称不该跟着会话一起没");
        assert_eq!(
            row.tiers.len(),
            1,
            "⭐ 分组与 sk 与网页登录态无关，不该从界面消失"
        );
        assert_eq!(row.tiers[0].provider_id, tier_id);
    }

    #[test]
    fn a_relay_with_a_managed_key_can_query_balance_without_a_session() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        let provider_id = provision::provider_id_for(site, None, 1);
        let settings = provision::settings_config_for(
            &AppType::Codex,
            "sk-test",
            "Pro池",
            "https://bestapi.store/v1",
            "gpt-5.6-sol",
        )
        .expect("settings");
        db.save_provider(
            "codex",
            &Provider {
                id: provider_id,
                name: "Pro池".into(),
                settings_config: settings,
                website_url: Some(site.into()),
                category: Some("aggregator".into()),
                created_at: Some(1),
                sort_index: Some(0),
                notes: None,
                meta: None,
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .expect("provider");

        let row = list_relays_impl(&state, AppType::Codex)
            .expect("list")
            .into_iter()
            .find(|row| row.id == row_id)
            .expect("row");
        assert!(matches!(row.status, RelayRowStatus::NotLoggedIn));
        assert!(row.can_query_balance);
        assert!(!row.can_refresh);
        assert!(
            relay_refresh_targets(&state, &AppType::Codex)
                .expect("refresh targets")
                .iter()
                .any(|(id, _, can_refresh)| *id == row_id && !can_refresh),
            "顶部全量刷新也要包含只能用 SK 查余额的账号"
        );
    }

    #[test]
    fn a_refreshable_session_is_not_reported_as_not_logged_in() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);
        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "expired-token",
                Some("refresh-token"),
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");

        let row = list_relays_impl(&state, AppType::Codex)
            .expect("list")
            .into_iter()
            .find(|row| row.id == row_id)
            .expect("row");

        assert!(
            !matches!(row.status, RelayRowStatus::NotLoggedIn),
            "refresh token 可自动续期时，后端不能要求用户重新登录"
        );
        assert!(row.can_refresh);
    }

    #[test]
    fn session_expired_usable_requires_an_extractable_managed_key() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "token",
                None,
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");

        let tier_id = provision::provider_id_for(site, Some(7), 1);
        db.save_provider(
            "codex",
            &Provider {
                id: tier_id,
                name: "坏配置".into(),
                settings_config: serde_json::json!({}),
                website_url: Some(site.into()),
                category: Some("aggregator".into()),
                created_at: Some(1),
                sort_index: Some(0),
                notes: None,
                meta: Some(managed_meta(&AppType::Codex, Some(7), None)),
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .expect("provider");
        with_conn(&state, |conn| creds::clear_session(conn, row_id)).expect("clear session");

        let row = list_relays_impl(&state, AppType::Codex)
            .expect("list")
            .into_iter()
            .find(|row| row.id == row_id)
            .expect("row");

        assert!(matches!(row.status, RelayRowStatus::SessionExpired));
        assert!(!row.can_query_balance);
    }

    /// ⭐ **倍率必须活过 provision → 库 → `listRelays` 这一整条**。
    ///
    /// 它是这次改动的核心：倍率从「每次渲染现拉」改成「provision 写一次、之后只读本地」。
    /// 链路上任何一环断掉，症状都是**界面永远显示「倍率未知」**，而没有报错 ——
    /// 只有这条端到端的断言守得住。
    ///
    /// 会红的改法：`persist_provision_batch` 里不写 `set_tier_rate_multiplier`，
    /// 或 `list_tiers_impl` 把 `rate_multiplier` 改回写死 `None`。
    #[test]
    fn a_provisioned_rate_survives_into_list_relays() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());

        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "tok",
                None,
                Some(i64::MAX),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");
        let op = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("load")
            .expect("exists");

        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let batch = ManagedProvisionBatch {
            account_id: Some(7),
            site_declaration: None,
            candidates: vec![ManagedProvisionCandidate {
                provider_id: provider_id.clone(),
                app_type: AppType::Codex,
                group_id: "1".into(),
                group_name: "Pro池".into(),
                rate_multiplier: Some(0.15),
                api_key: "sk-test".into(),
                model: "gpt-5.6-sol".into(),
                models: None,
                roles: None,
                allow_image_generation: Some(false),
                api_base_url: site.into(),
            }],
            observed_keep: Default::default(),
            failures: Vec::new(),
            keys_created: 0,
        };
        persist_provision_batch(&state, &op, batch).expect("persist");

        let rows = list_relays_impl(&state, AppType::Codex).expect("list relays");
        let tier = rows
            .iter()
            .find(|r| r.id == row_id)
            .expect("行在")
            .tiers
            .first()
            .expect("档位在");
        assert_eq!(
            tier.rate_multiplier,
            Some(0.15),
            "⭐ 倍率必须从本地库读回来 —— 它不再靠任何网络请求补齐"
        );
    }

    /// 「恢复内置默认」把应用过声明的档位退回去：settings 重建为
    /// `settings_config_for` 形状（sk/端点/模型保留）、标注清空。
    /// 与 `first_import_applies_site_declaration_segment` 构成一对往返。
    #[test]
    fn reset_site_config_restores_builtin_defaults() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let site = "https://api.example.com";

        // 直接造一条「应用过声明」的托管档位（不跑 persist，那段已有专测）。
        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let mut settings = provision::settings_config_for(
            &AppType::Codex,
            "sk-test",
            "Example·Pro池",
            "https://api.example.com/v1",
            "gpt-5.6-sol",
        )
        .expect("defaults");
        let declared = crate::relay::site_config::parse_site_config(
            r#"{
                "schema_version": 1,
                "site_origin": "https://api.example.com",
                "platforms": { "openai": { "model": "gpt-5.6-codex", "model_reasoning_effort": "minimal" } }
            }"#,
        )
        .expect("declaration");
        crate::relay::site_config::apply_segment_to_app(
            &AppType::Codex,
            declared
                .segment_for(platform_map::Platform::OpenAI)
                .unwrap(),
            &mut settings,
        )
        .expect("apply");
        let provider = crate::provider::Provider {
            id: provider_id.clone(),
            name: "Example·Pro池".into(),
            settings_config: settings,
            website_url: Some(site.into()),
            category: Some("aggregator".into()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: Some(0),
            notes: None,
            meta: Some(crate::provider::ProviderMeta {
                site_declared_origin: Some(site.into()),
                ..Default::default()
            }),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        state.db.save_provider("codex", &provider).expect("save");

        // 重建要素提取 + 重建（command 体内联逻辑的等价直调，不起 tauri runtime）。
        let (api_key, base_url, model) =
            rebuild_inputs_from_settings(&AppType::Codex, &provider.settings_config)
                .expect("rebuild inputs");
        assert_eq!(api_key, "sk-test");
        assert_eq!(base_url, "https://api.example.com/v1");
        // 模型提取自声明覆盖后的值——重建以现状为基线，不回滚站长的模型选择
        assert_eq!(model, "gpt-5.6-codex");
        let defaults = provision::settings_config_for(
            &AppType::Codex,
            &api_key,
            "Example·Pro池",
            &base_url,
            &model,
        )
        .expect("rebuild");
        let toml_value: toml::Value = toml::from_str(defaults["config"].as_str().unwrap()).unwrap();
        // 声明的参数键已退掉（reasoning 回到内置 high），sk/端点保留
        assert_eq!(toml_value["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(toml_value["model"].as_str(), Some("gpt-5.6-codex"));
        assert_eq!(
            toml_value["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(defaults["auth"]["OPENAI_API_KEY"], "sk-test");
    }

    /// 站点声明（relay/site_config.rs）随首次导入自动应用：段覆盖内置默认的调用
    /// 参数、deny 键进不来、meta 落「站点推荐配置」标注。spec 的 M2 硬门槛。
    #[test]
    fn first_import_applies_site_declaration_segment() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let site = "https://api.example.com";
        let row_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                site,
                "Example",
                site,
                discovery::BackendKind::Sub2Api,
            )
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "tok",
                None,
                Some(i64::MAX),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");
        let op = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("load")
            .expect("exists");

        let declaration = crate::relay::site_config::parse_site_config(
            r#"{
                "schema_version": 1,
                "site_origin": "https://api.example.com",
                "platforms": {
                    "openai": {
                        "model": "gpt-5.6-codex",
                        "model_reasoning_effort": "minimal",
                        "model_context_window": 272000,
                        "mcp_servers": { "evil": {} }
                    }
                }
            }"#,
        )
        .expect("declaration");

        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let batch = ManagedProvisionBatch {
            account_id: Some(7),
            site_declaration: Some(declaration),
            candidates: vec![ManagedProvisionCandidate {
                provider_id: provider_id.clone(),
                app_type: AppType::Codex,
                group_id: "1".into(),
                group_name: "Pro池".into(),
                rate_multiplier: Some(0.15),
                api_key: "sk-test".into(),
                model: "gpt-5.6-sol".into(),
                models: None,
                roles: None,
                allow_image_generation: Some(false),
                api_base_url: site.into(),
            }],
            observed_keep: Default::default(),
            failures: Vec::new(),
            keys_created: 0,
        };
        persist_provision_batch(&state, &op, batch).expect("persist");

        let provider = state
            .db
            .get_provider_by_id(&provider_id, "codex")
            .expect("read")
            .expect("provider exists");
        let config_text = provider.settings_config["config"].as_str().expect("toml");
        let parsed: toml::Value = toml::from_str(config_text).expect("valid toml");
        // 站长声明优先：模型与推理档位都来自声明段
        assert_eq!(parsed["model"].as_str(), Some("gpt-5.6-codex"));
        assert_eq!(
            parsed["model_reasoning_effort"].as_str(),
            Some("minimal"),
            "声明段覆盖内置写死的 high"
        );
        assert_eq!(parsed["model_context_window"].as_integer(), Some(272000));
        // deny 键进不来；端点与 sk 保持建档值
        assert!(parsed.get("mcp_servers").is_none());
        assert_eq!(
            parsed["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(
            provider.settings_config["auth"]["OPENAI_API_KEY"],
            "sk-test"
        );
        // 来源标注（UI 的「站点推荐配置」徽标 + 回退入口数据）
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|m| m.site_declared_origin.as_deref()),
            Some("https://api.example.com")
        );
    }

    #[test]
    fn session_probe_clears_only_confirmed_auth_failures() {
        assert!(should_clear_credentials_after_probe_error(
            &AppError::Config(
                "newapi self 失败: 登录态已失效（HTTP 401），请重新登录中转站账号".into()
            )
        ));
        assert!(should_clear_credentials_after_probe_error(
            &AppError::Config("登录已过期，请重新登录".into())
        ));
        assert!(!should_clear_credentials_after_probe_error(
            &AppError::Config("newapi self 请求失败: HTTP 500".into())
        ));
        assert!(!should_clear_credentials_after_probe_error(
            &AppError::Config("newapi self 请求失败: 连不上服务器（boom）".into())
        ));
    }

    #[test]
    fn persisting_a_newapi_refresh_updates_rotated_cookie_and_account_identity() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                "https://newapi.example",
                "NewAPI",
                "https://newapi.example",
                discovery::BackendKind::NewApi,
            )
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "Old Label",
                    login_identifier: "old-login",
                },
                "stale-access",
                Some("old-refresh"),
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let current = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("load relay")
            .expect("relay exists");
        let renewed = persist_refreshed_session(
            &state,
            &current,
            &backend::RefreshedSession {
                auth_token: "new-access".into(),
                refresh_credential: Some("rotated-refresh".into()),
                token_expires_at: Some(1_900_000_000),
                account: Some(backend::RuntimeAccount {
                    id: 7,
                    label: "NewAPI Display".into(),
                    login_identifier: "newapi-login".into(),
                }),
            },
        )
        .expect("persist refresh");

        assert_eq!(renewed.auth_token, "new-access");
        assert_eq!(renewed.refresh_token.as_deref(), Some("rotated-refresh"));
        assert_eq!(renewed.account_label, "NewAPI Display");
        assert_eq!(renewed.login_identifier, "newapi-login");

        let persisted = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("reload relay")
            .expect("relay exists");
        assert_eq!(persisted.auth_token, "new-access");
        assert_eq!(persisted.refresh_token.as_deref(), Some("rotated-refresh"));
        assert_eq!(persisted.token_expires_at, Some(1_900_000_000));
        assert_eq!(persisted.account_label, "NewAPI Display");
        assert_eq!(persisted.login_identifier, "newapi-login");
    }

    #[test]
    fn identity_refresh_failure_keeps_a_refreshed_session_usable() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                "https://newapi.example",
                "NewAPI",
                "https://newapi.example",
                discovery::BackendKind::NewApi,
            )
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "Old Label",
                    login_identifier: "old-login",
                },
                "stale-access",
                Some("old-refresh"),
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let current = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("load relay")
            .expect("relay exists");
        let renewed = persist_refreshed_session_with_identity_writer(
            &state,
            &current,
            &backend::RefreshedSession {
                auth_token: "new-access".into(),
                refresh_credential: Some("rotated-refresh".into()),
                token_expires_at: Some(1_900_000_000),
                account: Some(backend::RuntimeAccount {
                    id: 7,
                    label: "NewAPI Display".into(),
                    login_identifier: "newapi-login".into(),
                }),
            },
            |_state, _relay_id, _account| Err(AppError::Database("identity write failed".into())),
        )
        .expect("token refresh should stay usable");

        assert_eq!(renewed.auth_token, "new-access");
        assert_eq!(renewed.refresh_token.as_deref(), Some("rotated-refresh"));
        assert_eq!(renewed.account_label, "Old Label");
        assert_eq!(renewed.login_identifier, "old-login");

        let persisted = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("reload relay")
            .expect("relay exists");
        assert_eq!(persisted.auth_token, "new-access");
        assert_eq!(persisted.refresh_token.as_deref(), Some("rotated-refresh"));
        assert_eq!(persisted.token_expires_at, Some(1_900_000_000));
        assert_eq!(persisted.account_label, "Old Label");
        assert_eq!(persisted.login_identifier, "old-login");
    }

    #[tokio::test]
    async fn pricing_refresh_skips_fresh_rows_and_continues_after_one_failure() {
        let mut fresh = test_newapi_relay(1);
        fresh.pricing_synced_at = Some(100);
        let failed = test_newapi_relay(2);
        let succeeded = test_newapi_relay(3);
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_refresh = Arc::clone(&attempts);

        let summary = refresh_due_relay_pricing_rows(
            vec![fresh, failed, succeeded],
            159,
            std::time::Duration::from_secs(60),
            move |relay| {
                let attempts = Arc::clone(&attempts_for_refresh);
                async move {
                    attempts.lock().unwrap().push(relay.id);
                    if relay.id == 2 {
                        Err(AppError::Message("expected failure".into()))
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await;

        assert_eq!(summary.attempted, 2);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].0, 2);
        let mut attempted_ids = attempts.lock().unwrap().clone();
        attempted_ids.sort_unstable();
        assert_eq!(attempted_ids, vec![2, 3]);
    }

    fn pricing_timestamp_state(initial: Option<i64>) -> (AppState, i64) {
        let db = Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);
        let relay_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                "https://pricing.example",
                "Pricing",
                "https://pricing.example/v1",
                discovery::BackendKind::Sub2Api,
            )
        })
        .unwrap();
        if let Some(initial) = initial {
            with_conn(&state, |conn| {
                creds::mark_pricing_synced(conn, relay_id, initial)
            })
            .unwrap();
        }
        (state, relay_id)
    }

    #[test]
    fn successful_full_refresh_marks_pricing_fresh() {
        let (state, relay_id) = pricing_timestamp_state(None);

        mark_pricing_after_success(&state, relay_id, 456, Ok(())).unwrap();

        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))
            .unwrap()
            .unwrap();
        assert_eq!(relay.pricing_synced_at, Some(456));
    }

    #[test]
    fn failed_full_refresh_keeps_the_previous_pricing_time() {
        let (state, relay_id) = pricing_timestamp_state(Some(123));

        let result: Result<(), AppError> = mark_pricing_after_success(
            &state,
            relay_id,
            456,
            Err(AppError::Message("expected failure".into())),
        );

        assert!(result.is_err());
        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))
            .unwrap()
            .unwrap();
        assert_eq!(relay.pricing_synced_at, Some(123));
    }
}
