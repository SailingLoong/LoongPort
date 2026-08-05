//! LoongPort 运营商的 Tauri 命令层。
//!
//! 五个命令，对应需求的五步：
//!
//! | 命令 | 干什么 |
//! |---|---|
//! | [`operator_status`] | 首启该弹哪个弹窗、当前是什么状态 |
//! | [`operator_probe_site`] | 域名弹窗点确定 → 探测这是不是 sub2api 站 |
//! | [`operator_login`] | 开登录 WebView，等凭据回来 |
//! | [`operator_provision`] | 拉分组 → 每组备好 sk → 写成 codex provider |
//! | [`operator_switch_tier`] | 选分组 → 退 ChatGPT → 切换 → 重开 |
//!
//! ## 为什么切换编排在 Rust 侧而不是前端
//!
//! 「退出 ChatGPT → 切换 → 重开」如果写在前端的按钮回调里，那么**托盘快切、deeplink 导入、
//! 项目快照**这三条路径都会绕过它（它们在 Rust 侧直接调 `ProviderService::switch`），用户
//! 从托盘切完就会发现 codex 还连着旧分组。放在这一层是让「切换分组」只有一个入口。
//!
//! ⚠️ 编排在这一层**不等于**别处进不来 —— 那要靠 [`crate::operator::managed`] 的守卫。
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

use serde::Serialize;
use std::str::FromStr;
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::operator::{api, chatgpt_app, creds, login, provision, purchase};
use crate::provider::Provider;
use crate::services::{McpService, ProviderService};
use crate::store::AppState;

/// 默认运营商域名。域名输入框的底纹词，用户直接点确定就用它。
///
/// ⚠️ **它不再是维护者自己的站**（2026-08-04 从 `bestapi.store` 改过来 —— 那个站
/// 没有精力持续运维，默认值不该指向一个自己都不盯着的站）。那个巧合曾让三处文档把
/// 「默认站」与「维护者自己的站」写成一件事（本文档、[`crate::operator::aff`] 的
/// `aff_code_for` 与它那条「维护者自己的站有意缺席」的测试）—— 别再绑回去。
///
/// ⇒ 换这个值时**要重新确认它在 [`crate::operator::aff`] / [`crate::operator::promo`]
/// 两张内置表里各该有什么**（两张表各自按 host 查，彼此独立，但都与「谁是默认站」
/// 有关：默认站是最常被走到的那条路）。当前这个站在 aff 内置表里、不在 promo 内置表里，
/// 前者**本模块 `tests` 里有一条闸钉着**（跟着这个常量一起改，它会当场告诉你）。
//
// ⚠️ 有意**不写那条测试的名字**：rustdoc 的 intra-doc link 链不进 `#[cfg(test)]`，
// 写成反引号裸名字就没有任何东西能验它 —— 2026-08-04 同一次改名里连漏两处指针
// （两路 review 各抓一次）。指「本模块 tests 里」而不指名字，改名就不会让它悬空。
const DEFAULT_SITE: &str = "790053500.com";

// `DEFAULT_MODEL` 住在 `provision` 里 —— 它与 `HISTORICAL_DEFAULT_MODELS`、`pick_model`
// 互为前提（前者的文档要求改值时同步后者；`pick_model` 要在「问不出模型列表」时回落到它），
// 三者分居两个文件迟早分叉。这里只 `use`。
use provision::DEFAULT_MODEL;

/// 等用户走完登录流程的上限（秒）。
///
/// 5 分钟够走完注册 + 邮箱验证码 + 2FA。**超时不是错误** —— 用户可能就是走开了，
/// 那时安静收场（返回 `false`）而不是弹一条他看不懂的失败。
///
/// 提成常量而不是内联 `300`：日志里要把它打出来（「最多 N 秒」），
/// 两处各写一个字面量迟早对不上（`vendor.rs` 的 `LOGIN_TIMEOUT` 同一形状）。
const LOGIN_TIMEOUT_SECS: u64 = 300;

/// 「加站弹窗要什么」+「切档位前要不要提醒处理 ChatGPT」。
///
/// ## 为什么只剩两个字段（2026-08-04 收缩）
///
/// 原来它有 9 个字段，服务的是已删的 LoongPort 独立页那个**单站视图**
/// （顶部显示「当前站是 X、登录的是 Y、已过期了没、有几个档位」）。运营商行现在
/// 每行各显示自己的状态、数据走 [`operator_list_operators`]，那个「当前站」的概念
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
pub struct OperatorStatus {
    /// 域名输入框的底纹词。
    pub default_site: String,
    /// 切换分组前要不要先提示用户处理 ChatGPT。
    ///
    /// 不是「装了没有」—— 非 macOS 平台查不到那个事实，那边恒为 true（宁可多问一句，
    /// 也不能让装了 ChatGPT 的用户静默用错分组）。见 `chatgpt_app::needs_user_attention`。
    pub chatgpt_needs_attention: bool,
}

/// 一个已添加的站点。
///
/// 两个消费者：加站弹窗（判「点这个推荐站会不会撞上已有账号」，读
/// `site_origin` + `account_label`）与「一个站都没有吗」那个自动引导判据（只数条数）。
///
/// 2026-08-04 一并收缩：原来还有 `id` / `site_name` / `label` / `logged_in` /
/// `is_current` 五个字段，服务的是已删独立页顶部那个**站点切换器**（要显示名、
/// 要标出当前选中的是哪个、要能按 id 切换）。那个控件删了之后没有消费者。
///
/// ⚠️ **含未登录的占位行** —— 「加了站但还没登录」也算配过站，不该再弹引导。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteInfo {
    pub site_origin: String,
    /// 登录后的账号名（昵称优先，回落邮箱），未登录为空串。同一个站挂多个账号时靠它分辨。
    pub account_label: String,
}

/// 探测结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// 探测成功后这个站在本地的行 id（已存在则是原来那行，`save_site` 会收口）。
    ///
    /// **前端必须拿它接着调 [`operator_login`]** —— 那条命令的 `operator_id` 是
    /// 必填的，没有「回落到当前站」这种东西（那个概念已随 `is_current` 一起删）。
    pub operator_id: i64,
    pub site_origin: String,
    pub site_name: String,
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
    pub rate_multiplier: Option<f64>,
    pub is_current: bool,
    /// 用户在 cc-switch 编辑页改过这个档位的配置吗。
    ///
    /// 判据是「当前 `settings_config` ≠ 我们会生成的默认配置（sk 除外）」——
    /// 见 [`provision::is_user_edited`] 那段「为什么不存标记」。
    ///
    /// `None` = **判不了**（读不出 sk / 这个 CLI 没有默认形状）。UI 在 `None` 时
    /// 什么标记都不显示：`false` 是在断言「刷新不会覆盖你的改动」，
    /// 而事实是「不知道」—— 让用户误信比不说更糟。
    ///
    /// ⚠️ 只有 [`list_operators_impl`] 填得出它（判据要 `api_base_url`，那在
    /// `creds` 里按站点存）。[`operator_list_tiers`] 那条路恒为 `None` ——
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
    /// 只有 provision 那条路填得出，[`list_operators_impl`] 恒为 `None`。
    /// UI 在 `None` 时不显示标记 —— 与 `user_edited` 同一条原则：不知道就别断言。
    pub allow_image_generation: Option<bool>,
}

/// 「运营商 × 分组」页的一行运营商，连带它在当前 app 下的档位。
///
/// spec §三 定的是 `OperatorRow { ..., tiers: Vec<TierRow> }`，这里**复用已有的
/// [`TierInfo`] 而不新建 `TierRow`** —— 两者字段本就一致（含那个关键的
/// `rate_multiplier: Option<f64>`），再建一个只会让同一个概念有两种形状，
/// 前端也得写两套类型（CLAUDE.md §一：能复用就复用）。
///
/// **它是只读本地的**，与 [`OperatorStatus`] 的首屏契约一致（不发网络请求）——
/// 所以 `tiers` 里的 `rate_multiplier` 恒为 `None`，倍率要等用户主动刷新（provision）
/// 才有值。这不是缺陷：填 0 占位会让 UI 显示成「最便宜的一档」，那是错的。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorRow {
    pub id: i64,
    pub site_origin: String,
    pub site_name: String,
    /// 登录后的账号名（昵称优先，回落邮箱），未登录为空串。
    /// 同一个站可以挂多个账号，所以「登录了」不够 —— 得说清是**哪个**账号。
    pub account_label: String,
    pub logged_in: bool,
    /// 登录过但凭据已不可用（≠ `!logged_in`，那会把「从没登录」与「登录过期」混为一谈）。
    /// 语义与 [`OperatorStatus::session_expired`] 完全一致，判据也复用同一个函数。
    pub session_expired: bool,
    /// 这个运营商在**当前 app_id** 下已备好的档位。
    pub tiers: Vec<TierInfo>,
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

/// 读当前状态。
///
/// **只读本地**，不发网络请求 —— 这是首屏渲染要等的东西，不该卡在网络上。
/// 「凭据是不是真的还活着」由 [`operator_check_session`] 单独探，前端拿到本地状态先渲染，
/// 再让探活的结果去修正它。
#[tauri::command]
pub fn operator_status(state: State<'_, AppState>) -> Result<OperatorStatus, String> {
    operator_status_impl(state.inner()).map_err(|e| e.to_string())
}

/// 探一遍**每一行**已登录的凭据是不是真的还能用，并清掉确认失效的那些。
///
/// 为什么需要这个：行 DTO 的 `logged_in` 只看本地记的过期时间。而凭据可能在网页端被
/// 撤销、账号被禁用、会话被踢掉 —— 那些情况下本地看起来一切正常，用户点任何操作才会
/// 撞到错误。第 2 次打开 app 到第 100 次都走这条路，不能共用第 1 次的假设。
///
/// ## 为什么是逐行而不是「探当前站」（2026-08-04 改）
///
/// 原来它探的是 `creds::load()` 那一行（全局 `is_current = 1`），返回一个 bool。
/// 那个形状只对「同时只有一个站」的旧界面成立 —— 运营商区是**多行并列**的，
/// 探一行的活等于让另外 N-1 行继续显示错的状态，而用户看不出区别。
///
/// 现在返回**这次被清掉凭据的行 id**（空 = 全都还好）。前端据此提示并刷新。
///
/// 未登录的行直接跳过：`usable_operator` 对它们必然 Err，白打一次请求还得过滤噪音。
#[tauri::command]
pub async fn operator_check_session(app_handle: tauri::AppHandle) -> Result<Vec<i64>, String> {
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
    // **串行而不是 join_all**：这些请求打的往往是同一个运营商（同一个 IP 段、
    // 同一份 rate limit），而这是启动时的后台探活、没人在等它返回。
    // 并发省下的几百毫秒换来的是撞限流的风险，不值得。
    for id in targets {
        // usable_operator 会在快过期时先续期、并顺手补齐缺失的账号身份（见它的文档）；
        // 拿 /user/profile 当探活请求（最便宜的鉴权端点）。
        let probe = async {
            let op = usable_operator(app_handle, id).await?;
            api::Client::new(&op.site_origin, &op.auth_token, op.account_id)?
                .balance()
                .await
        }
        .await;

        if let Err(e) = probe {
            let msg = e.to_string();
            // 「登录态已失效」是 api 层对不可恢复的那一类 401 的措辞（账号被禁 /
            // 会话被撤销 / 用户不存在）。这类清掉本地凭据、让用户重新登录。
            //
            // 其它失败（网络不通、运营商关了用户面板返 403）**不清凭据** ——
            // 那不是凭据的问题，清掉只会逼用户在网络恢复后白重登一次。
            if msg.contains("登录态已失效") || msg.contains("请重新登录") {
                let state = app_handle.state::<AppState>();
                with_conn(&state, |conn| creds::clear_credentials(conn, id))?;
                log::info!("运营商 {id} 凭据已失效，已清除本地凭据：{msg}");
                expired.push(id);
            } else {
                log::warn!("运营商 {id} 探活失败但保留凭据（可能只是网络问题）：{msg}");
            }
        }
    }
    Ok(expired)
}

fn operator_status_impl(_state: &AppState) -> Result<OperatorStatus, AppError> {
    // 两个字段都不看库：底纹词是常量，ChatGPT 那个探的是本机装了什么。
    // 收缩之前这里还 `creds::load` + 遍历整个 provider 表数托管档位，那两笔
    // 开销随对应字段一起去掉了（见 `OperatorStatus` 的文档）。
    Ok(OperatorStatus {
        default_site: DEFAULT_SITE.to_string(),
        chatgpt_needs_attention: chatgpt_app::needs_user_attention(),
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
/// ⚠️ **有意不把它并进 [`OperatorStatus`]**：那条命令是**首屏渲染要等的东西**
/// （它的文档为此删掉过一个有遍历开销的字段），而这个事实只有统计告知那一屏要用。
/// 单独一条命令让它不参与首屏的关键路径。
///
/// ⇒ **端点配好那天这里自动放行**，不需要有人记得回来撤掉什么开关 ——
/// 判据就是端点本身，不是一个另行维护的标记。
#[tauri::command]
pub fn operator_stats_endpoint_configured() -> bool {
    crate::operator::stats::is_configured()
}

/// 推荐运营商（首启屏那几个按钮）。
///
/// ## 为什么读缓存而不是现拉
///
/// 与 [`operator_login`] 里取 aff 码同一个理由：拉取由启动时那个后台任务做
/// （`lib.rs`，延迟 5 秒），这里只同步读一份磁盘文件（含重新验签）——
/// **不让用户对着一个转圈的弹窗等一次网络往返**。
///
/// ⇒ **首启第一次打开时这里通常是空的**（那 5 秒还没到，或者根本没网）。
/// 那不是错误：UI 拿到空数组就只显示手动输入框，与这个功能上线前的样子一致。
/// 下次启动就有了（缓存已落盘）。
///
/// 返回空数组的三种情形都正常：没网 / 还没拉到 / 维护者临时撤空了列表。
#[tauri::command]
pub fn operator_list_sponsors() -> Vec<crate::operator::remote_config::Sponsor> {
    // 不返 `Result` —— 拿不到推荐不是错误，是「今天没有推荐」。
    // 返 Err 会让前端不得不写一个 catch 去把错误咽掉，那是把非错误伪装成错误。
    crate::operator::remote_config::load_cached()
        .map(|cfg| cfg.sponsors)
        .unwrap_or_default()
}

/// 探测一个域名，成功即存为当前站点。
///
/// 空输入用默认域名 —— 需求要的就是「不输入直接点确定也能走」。
#[tauri::command]
pub async fn operator_probe_site(
    app_handle: tauri::AppHandle,
    site: String,
) -> Result<ProbeResult, String> {
    let input = if site.trim().is_empty() {
        DEFAULT_SITE.to_string()
    } else {
        site
    };
    probe_and_save(&app_handle, &input)
        .await
        .map_err(|e| e.to_string())
}

async fn probe_and_save(
    app_handle: &tauri::AppHandle,
    input: &str,
) -> Result<ProbeResult, AppError> {
    let site_origin = api::normalize_site_origin(input)?;
    let settings = api::probe_site(&site_origin).await?;
    let api_base_url = api::codex_base_url(&site_origin, &settings.api_base_url);

    let site_name = if settings.site_name.trim().is_empty() {
        // 运营商可能没配站名。回落到主机名而不是留空 —— 空名字会让 UI 里那家没有标识。
        site_origin
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    } else {
        settings.site_name.clone()
    };

    let state = app_handle.state::<AppState>();
    let operator_id = with_conn(&state, |conn| {
        creds::save_site(conn, &site_origin, &site_name, &api_base_url)
    })?;

    Ok(ProbeResult {
        operator_id,
        site_origin,
        site_name,
    })
}

/// 开登录窗，等凭据回来。
///
/// 凭据由注入脚本经一次被拦下的自定义 scheme 跳转送回（见 [`login`]）。本命令在收到凭据、
/// 或用户关掉窗口、或超时之后返回。
///
/// `operator_id` 指定登录**哪一行**，**必填**。
///
/// 没有「回落到当前站」这条路：那要靠全局 `is_current` 定位，而界面是多行并列的
/// ⇒ 用户点第 3 行的「重新登录」可能给第 1 行登了录。加站那条路拿
/// [`ProbeResult::operator_id`] 接着调（`save_site` 已经把 id 给出来了）。
#[tauri::command]
pub async fn operator_login(
    app_handle: tauri::AppHandle,
    operator_id: i64,
) -> Result<bool, String> {
    do_login(&app_handle, operator_id)
        .await
        .map_err(|e| e.to_string())
}

async fn do_login(app_handle: &tauri::AppHandle, target_id: i64) -> Result<bool, AppError> {
    // 记下行 id —— 凭据要写回这一行，而 `save_credentials` 可能因为发现重复账号
    // 而把它合并到别的行去。
    // 顺带取出登录标识：重登时预填进登录框，用户只需补密码与人机验证。
    let (operator_id, site_origin, login_identifier) = {
        let state = app_handle.state::<AppState>();
        let op = with_conn(&state, |conn| creds::get(conn, target_id))?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {target_id} 的运营商")))?;
        (op.id, op.site_origin, op.login_identifier)
    };

    // 已经有一个登录窗时：**销毁它再开新的**，而不是聚焦了就早退。
    //
    // 「聚焦已有的」听起来更礼貌，但它会卡死：残留窗口可能是隐藏状态（被别处 hide 过、
    // 或某次 close 请求被拦下），而 `set_focus` 对不可见窗口是 no-op —— 用户点了登录什么
    // 都没发生，且因为 label 被占，再点多少次都一样，只能重启 app。
    //
    // 直接销毁重开则总能给用户一个可见的窗口。代价是「他正在填的表单没了」，但能走到这里
    // 说明上一轮的 `do_login` 已经返回（否则那边还持有窗口），也就是那个窗口已经没人在等它
    // 的凭据了 —— 留着它反而是个陷阱。
    if let Some(stale) = app_handle.get_webview_window(login::LOGIN_WINDOW_LABEL) {
        log::info!("发现残留的登录窗口，销毁后重开");
        // destroy 而不是 close：close 是可被拦截的请求，见下方 destroy 那处的说明。
        let _ = stale.destroy();
    }

    // 邀请码走三层回落：**远端（上次拉到并缓存的）> 编译期内置**。
    // 在这里解析而不是在 `login_script` 里查表 —— 那样远端那层永远进不来。
    //
    // 读缓存而不是现拉：拉取由启动时那个后台任务做（见 `lib.rs`），
    // 这里只同步读一份磁盘文件（含重新验签），不让用户等一次网络往返。
    // 缓存不存在 / 验签不过 ⇒ `load_cached` 返回 None ⇒ 自动落到内置那层。
    let cached_config = crate::operator::remote_config::load_cached();
    let login_aff_code =
        crate::operator::remote_config::resolve_aff_code(cached_config.as_ref(), &site_origin);
    // 注册优惠码（用户得赠额）走**同一套**三层回落，同一份缓存、同一次解析时机。
    let login_promo_code =
        crate::operator::remote_config::resolve_promo_code(cached_config.as_ref(), &site_origin);

    // 落哪个页面由「这一行登录过没有」决定：新加的站落 `/register`，重登落 `/login`。
    let url = url::Url::parse(&login::login_url(&site_origin, &login_identifier))
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
    let (tx, mut rx) = tokio::sync::mpsc::channel::<login::Credentials>(1);
    // 用户自己关掉窗口的信号。没有它就只能干等 5 分钟超时。
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::channel::<()>(1);

    let handle_for_nav = app_handle.clone();
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
    // 1. 用户删掉某个运营商 —— `creds::remove` 是真 DELETE，本地记录确实没了
    // 2. 重新添加同一个站，开登录窗
    // 3. **那个站的 localStorage 里旧 token 还在**（我们从没清过）⇒ sub2api 的 SPA
    //    认出「已登录」直接跳 dashboard，压根不显示登录表单
    // 4. `login_script` 的轮询兜底（本来是为「用户已登录状态打开页面」设计的）
    //    把那把旧 token 捞出来回传 ⇒ 看起来像「删除是假删除」
    //
    // 真正的后果比「看起来没删掉」严重两层：
    // - **同一个站永远只能挂第一个登录过的账号** —— 想加第二个账号根本加不进来，
    //   而「同站多账号」是这个功能的核心能力（`Operator` 的去重认的是服务端 account_id，
    //   正是为了支持它）
    // - **隐私问题**：用户以为删掉了运营商，那个站的登录 cookie 还留在本机
    //
    // `incognito(true)` 在 macOS 上映射成 `WKWebsiteDataStore::nonPersistentDataStore`
    // （wry 0.55 `wkwebview/mod.rs`），Windows/Linux 上 wry 也各有实现 ——
    // 一份纯内存存储，窗口关掉就没了，也读不到 app 那份持久 profile。
    //
    // 为什么不用 `clear_all_browsing_data()`：它清的是**全部站点**的数据（
    // wry 那边是 `removeDataOfTypes_modifiedSince` 传 1970 年），会把用户在别的
    // 运营商站、以及 app 内其它 WebView 的登录态一起冲掉；而且它是异步的，
    // 没有完成回调可等 ⇒ 存在「还没清完页面就加载了」的竞态。
    .incognito(true)
    .user_agent(login::WEBVIEW_USER_AGENT)
    // 邀请码走三层回落（远端 > 本地缓存 > 编译期内置）。**在这里解析而不是在
    // `login_script` 里查表** —— 那样远端那层永远进不来。
    .initialization_script(login::login_script(
        &site_origin,
        &login_identifier,
        login_aff_code.as_deref(),
        login_promo_code.as_deref(),
    ))
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
        match login::parse_creds_navigation(url) {
            // 普通导航，放行。
            None => true,
            Some(Ok(creds)) => {
                // 用 try_send：这个回调不能 await，而我们只要第一份凭据，
                // 满了就说明已经收到过了。
                let _ = tx.try_send(creds);
                false
            }
            Some(Err(e)) => {
                log::warn!("凭据回传解析失败: {e}");
                let _ = handle_for_nav.emit("operator-login-error", e.to_string());
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

    // 等凭据或用户关窗。5 分钟够走完注册 + 邮箱验证 + 2FA；超时不是错误，用户可能就是走开了。
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS), async {
        tokio::select! {
            creds = rx.recv() => creds,
            _ = closed_rx.recv() => None,
        }
    })
    .await;

    match outcome {
        Ok(Some(c)) => {
            // 先拉一次 profile 拿账号身份 —— 去重键是「域名 + 账号」，而账号只有登录后才知道。
            //
            // 拉不到就不存：没有 account_id 的话去重判断无从做起，用户重新添加同一个站会
            // 堆出重复行、进而给他的账号里堆重复 sk。让他重试一次比留下脏数据好。
            // `account_id` 传 `None`：**这一次请求的目的就是去拿它**，此刻还不知道。
            // 只读端点，不发写请求 ⇒ 用不上幂等键。
            let account = api::Client::new(&site_origin, &c.auth_token, None)?
                .account()
                .await
                .map_err(|e| {
                    AppError::Config(format!("登录成功但读取账号信息失败：{e}。请重试登录。"))
                })?;

            let state = app_handle.state::<AppState>();
            with_conn(&state, |conn| {
                creds::save_credentials(
                    conn,
                    operator_id,
                    creds::AccountIdentity {
                        id: account.id,
                        label: &account.display_name(),
                        // 登录标识单独存：display_name() 昵称优先，设了昵称的用户拿它
                        // 预填登录框就填错了（sub2api 那个框要邮箱格式）。
                        login_identifier: &account.email,
                    },
                    &c.auth_token,
                    c.refresh_token.as_deref(),
                    c.token_expires_at,
                )
            })?;

            // **不关窗**，把标题改成「已连接」并在页面上浮一条提示。
            //
            // 为什么不关：用户拿到凭据的那一刻，页面往往刚跳到 dashboard（sub2api 登录成功后
            // `router.push(redirectTo)`，注册成功后 `push('/dashboard')`）—— 那上面有余额、
            // 充值入口、渠道状态，都是他接着要用的东西。我们把窗口关掉等于替他决定「你看完了」。
            //
            // 更糟的一种：用户之前登录过，`/login` 的路由守卫会把他直接重定向到 dashboard，
            // 而注入脚本的轮询会在几百毫秒内拿到已有 token —— 窗口开了就关，用户一眼都没看到。
            //
            // 所以改成：凭据已到手、命令正常返回（前端接着去备密钥），窗口留给用户自己关。
            let _ = window.set_title(&format!("已连接 {site_origin} — 可关闭此窗口"));
            let _ = window.eval(login::CONNECTED_BANNER_JS);

            log::info!("登录成功：{site_origin}（账号 id={}）", account.id);
            Ok(true)
        }
        // 用户关掉了窗口，或超时。都不是错误。
        //
        // 用 `destroy()` 而不是 `close()`：后者派的是可被拦截的关闭**请求**，会经过
        // `lib.rs` 里那个全局 `CloseRequested` 回调 —— 一旦将来有人放宽那道 label 守卫，
        // `close()` 就会被 `prevent_close` 吃掉，留下一个隐藏但仍占着 label 的僵尸窗口，
        // 而它会让下一次 `operator_login` 命中上面「已开着就聚焦」的早退，登录卡死。
        // `destroy()` 直接销毁、不发事件、拦不住。
        //
        // 超时那条也走这里：用户走开了，留一个卡在登录页的窗口没有意义。
        //
        // ⚠️ **两条分支的日志必须分开**（用户实测白屏后加）：「用户自己关的」与「等满超时」
        // 在界面上都表现为「窗口没了、什么也没发生」，但对我们是两件完全不同的事 ——
        // 前者是正常收场，后者说明**凭据回传这条链路断了**（页面没渲染 / 脚本没注入 /
        // 用户卡在人机验证）。合成一条日志就等于放弃了区分它们的唯一手段。
        Ok(None) => {
            log::info!("用户关闭了登录窗口（未完成登录）：{site_origin}");
            let _ = window.destroy();
            Ok(false)
        }
        Err(_) => {
            log::warn!(
                "登录等待超时（{LOGIN_TIMEOUT_SECS} 秒内没收到凭据）：{site_origin} —— \
                 若用户当时看到的是白屏，对照上面 `登录窗页面加载` 那几行判断是哪一类"
            );
            let _ = window.destroy();
            Ok(false)
        }
    }
}

/// 取一份**能用**的凭据：token 快过期时先静默续期。
///
/// 没有这一步的话，token 一过期用户就得重新走一遍 WebView 登录 —— 而 sub2api 的
/// `/auth/login` 有 20 次/分钟的限流，反复登录会把自己锁在外面。
///
/// ## `operator_id` 是必填的：**没有「回落到当前站」这条路**
///
/// 界面是多行并列的，「当前站」这个概念在这里不成立 —— 靠它定位会让
/// 「给 A 获取密钥」静默作用到 B 上（那是 review 抓出过的真实并发正确性问题，
/// 见 [`operator_provision`] 的文档）。2026-08-04 连带 `is_current` 一起删掉了
/// 那条 `Option` 分支。
async fn usable_operator(
    app_handle: &tauri::AppHandle,
    operator_id: i64,
) -> Result<creds::Operator, AppError> {
    let op = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, |conn| creds::get(conn, operator_id))?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {operator_id} 的运营商")))?
    };

    if op.token_looks_valid(chrono::Utc::now().timestamp()) {
        // ⭐ **token 够用，但账号身份可能缺** —— 补一次再返回。
        //
        // 「有 `auth_token` 却没 `account_id`」是个实测到的死局：
        // [`creds::Operator::token_looks_valid`] 对 `token_expires_at = NULL` 返回
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

    // 过期了。有 refresh token 就试着续，没有就只能重登。
    let Some(refresh) = op.refresh_token.clone() else {
        return Err(AppError::Config("登录已过期，请重新登录".into()));
    };

    let fresh = api::refresh_token(&op.site_origin, &refresh).await?;
    let state = app_handle.state::<AppState>();
    // 走 update_tokens 而不是 save_credentials：续期是「同一个账号换一把新 token」，
    // 账号没变 ⇒ 没有重复可言，不该走那条会查重并可能合并行的路径。
    with_conn(&state, |conn| {
        creds::update_tokens(
            conn,
            op.id,
            &fresh.auth_token,
            // 服务端没轮换 refresh 时沿用旧的 —— 覆写成 None 会让下次过期时无法续期。
            fresh.refresh_token.as_deref().or(Some(refresh.as_str())),
            fresh.token_expires_at,
        )
    })?;

    let renewed = creds::Operator {
        auth_token: fresh.auth_token,
        refresh_token: fresh.refresh_token.or(Some(refresh)),
        token_expires_at: fresh.token_expires_at,
        ..op
    };

    // 顺手刷一次账号身份：用户可能在运营商那边改了昵称或邮箱，而续期响应里没有账号信息
    // （`/auth/refresh` 只回 token），所以只有在这里额外打一次 profile 才发现得了。
    // 不刷的话站点选择器上会一直挂着旧标签 —— 而他改邮箱的动机往往就是「换个能认的」。
    Ok(backfill_account_identity(app_handle, renewed).await)
}

/// 打一次 profile，把账号身份写回库并更新手上这份 `op`。
///
/// 两个调用点、两种动机，但做的事完全一样，所以共用一个函数（各写一遍迟早分叉）：
///
/// 1. **token 够用但 `account_id` 为空** —— 补齐那个死局态（见 [`usable_operator`]
///    早退分支的注释）。
/// 2. **续期成功之后** —— 用户可能改了昵称/邮箱，而 `/auth/refresh` 不回账号信息。
///
/// ## 任何一步失败都只记日志
///
/// 调用方此刻的凭据**已经可用**（要么本来有效、要么刚续期成功）。账号标签陈旧或
/// `account_id` 还是空，都只影响显示与去重，不影响这一次请求 —— 为它把整个操作
/// 判失败会让用户在「明明能用」的时候被挡住。
async fn backfill_account_identity(
    app_handle: &tauri::AppHandle,
    mut op: creds::Operator,
) -> creds::Operator {
    // `account_id` 传 `op.account_id`（此刻通常正是 `None` —— 本函数存在的理由就是它缺了）。
    // 只打只读的 profile 端点，用不上幂等键。
    let client = match api::Client::new(&op.site_origin, &op.auth_token, op.account_id) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("刷新账号信息时构造客户端失败（不影响使用）: {e}");
            return op;
        }
    };
    let account = match client.account().await {
        Ok(a) => a,
        Err(e) => {
            log::warn!("读取账号信息失败（不影响使用）: {e}");
            return op;
        }
    };

    let label = account.display_name();
    let state = app_handle.state::<AppState>();
    if let Err(e) = with_conn(&state, |conn| {
        creds::refresh_account_identity(
            conn,
            op.id,
            creds::AccountIdentity {
                id: account.id,
                label: &label,
                login_identifier: &account.email,
            },
        )
    }) {
        log::warn!("刷新账号信息失败（不影响使用）: {e}");
        return op;
    }

    // 写库成功才更新手上这份 —— 否则返回的结构与库里不一致，
    // 调用方据此判断 `account_id` 已补上，而下次读库又是空的。
    if op.account_id.is_none() {
        op.account_id = Some(account.id);
    }
    op.account_label = label;
    op.login_identifier = account.email;
    op
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
/// ## `operator_id`：显式指定作用于哪个运营商（2026-08-03 加）
///
/// 原来它只吃 `AppHandle`，靠 `creds::load()` 读「`is_current = 1` 的那一行」。
/// 于是多行并列的页面必须先 `set_current(id)` 才能让它作用到对的账号上
/// （前端那个 `focusOperator`）—— 而 `is_current` 是**全局单例状态**：
///
/// 两个运营商同时 provision 时，B 的 `set_current(B)` 会改掉 A 那次操作的目标，
/// A 后续的 balance / refresh 全串到 B 上。前端当时是用「任一操作进行中就禁用所有行」
/// 兜住的 —— **那是拿全局禁用换正确性，修的是症状**：运营商之间本来毫无依赖，
/// 用户点 A 的按钮却发现 B、C 的按钮全灰了。
///
/// 现在把目标变成参数，全局状态不再参与定位 ⇒ 各行真正独立、可并发。
/// 这也正是「运营商（登录态）一个模块、分组（sk）一个模块」该有的样子：
/// 分组操作显式说明「给哪个运营商」，而不是去读一个由 UI 顺手改掉的全局变量。
///
/// `None` 保留给单站流程（LoongPort 页首启引导，全程只有一个站）。
#[tauri::command]
pub async fn operator_provision(
    app_handle: tauri::AppHandle,
    operator_id: i64,
) -> Result<ProvisionSummary, String> {
    do_provision(&app_handle, operator_id)
        .await
        // ⚠️ **失败必须落日志**（维护者实测抓出）。
        //
        // 这条路径原来一个字都不记，而前端「刷新」那处又把 `Promise.allSettled` 的
        // `reason` 丢掉、只显示「<站名> 刷新失败」⇒ 两处一叠，**用户和维护者都拿不到
        // 真实错误** —— 定位一次要手工从 DB 里取 token、逐个端点 curl 一遍。
        //
        // 带上 `operator_id`：多行并列时「哪一行失败了」本身就是信息，
        // 而错误文案里未必有站名。
        .inspect_err(|e| {
            log::error!("provision 失败（operator_id={operator_id}）：{e}");
        })
        .map_err(|e| e.to_string())
}

async fn do_provision(
    app_handle: &tauri::AppHandle,
    operator_id: i64,
) -> Result<ProvisionSummary, AppError> {
    // 判据是「`settings_config_for` 认不认这个 CLI」，**不是硬编码 codex** ——
    // 那个函数加一个分支，这里就自动放开，不必两处同步改。
    let op = usable_operator(app_handle, operator_id).await?;
    // ⭐ **`account_id` 必须带上**：这是唯一会发写请求（建 Key）的路径，而幂等键
    // 不带账号时同站多账号必撞 409（见 `api::idempotency_key_for`）。
    //
    // `usable_operator` 会**尽力**补齐它，但补不上也照样返回（`backfill_account_identity`
    // 拉 profile 失败只 warn）⇒ 这里仍可能是 `None`，那时建 Key 落在 `"anon"`
    // 命名空间。不为此把 provision 判失败：拉 profile 瞬时失败换来「拿不到密钥」
    // 是拿可用性换一个更窄的正确性，不值得。
    let client = api::Client::new(&op.site_origin, &op.auth_token, op.account_id)?;
    // **不传 app_type** —— 每个分组落到哪个 CLI 由它自己的 platform 决定
    // （见 `provision::provision` 的文档）。一次登录探全部平台。
    let mut result = provision::provision(&client).await?;
    provision::sort_tiers(&mut result.tiers);

    // 写 provider 记录。这一段是同步的（碰 DB），所以拿完网络数据再做。
    let state = app_handle.state::<AppState>();

    let mut tiers = Vec::new();
    // 这次 provision 认可的「(app_type, provider_id)」组合。见下方 insert 处的说明。
    let mut keep: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    // 这次改写到的档位里，哪些**正是所属 app 的当前项**。见循环后那段刷 live 的说明。
    let mut refresh_live: Vec<AppType> = Vec::new();
    for (idx, targeted) in result.tiers.iter().enumerate() {
        let tier = &targeted.tier;
        // ⚠️ **用这条分组自己的 app_type**，不是调用方给的 —— 那正是
        // 「claude 页出现 chatgpt 分组」那个 bug 的根因。
        let app_type = &targeted.app_type;

        let provider_id = provision::provider_id_for(&op.site_origin, op.account_id, tier.group_id);
        let display_name = provision::provider_display_name(&op.site_name, &tier.group_name);

        // 认不出配置形状的 CLI 直接跳过并如实报出来 —— 不能写一条形状不对的记录
        // （那是「看着像成功、调用必失败」）。
        //
        // ⚠️ **模型名用 `tier.model` 而不是 `DEFAULT_MODEL`**：纯生图分组
        // （`/v1/models` 里只有 `gpt-image-*`）写文本模型名就是必定 404。
        // 那个值由 `provision::pick_model` 按该分组的真实模型列表定，见它的文档。
        let Some(defaults) = provision::settings_config_for(
            app_type,
            &tier.api_key,
            &display_name,
            &op.api_base_url,
            &tier.model,
        ) else {
            result.failures.push((
                tier.group_name.clone(),
                format!("还不能为 {} 生成配置", app_type.as_str()),
            ));
            continue;
        };

        // ⚠️ **已存在的档位只换 sk，不覆盖用户的编辑**。
        //
        // `save_provider` 是全量覆盖 `settings_config` 的，所以照写默认配置会把用户在
        // cc-switch 编辑页改过的模型名 / reasoning effort / 自定义端点**全冲掉** ——
        // 而他点「获取密钥」通常只是想刷新档位列表。
        //
        // 要回到默认值走 `operator_reset_tier_config`（显式动作），不是这条路的副作用。
        let existing = state
            .db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .ok()
            .flatten();

        let settings_config = match existing {
            Some(old) => {
                let mut kept = old.settings_config;
                // patch 失败（形状被改坏 / 该放 sk 的 section 没了）⇒ 回落到默认配置。
                // 否则用户会留着一把旧 sk 却以为刷新成功了。
                if provision::patch_api_key(&mut kept, app_type, &tier.api_key) {
                    // ⚠️ **顺手修正过时的模型名**（只在用户没改过配置时）。
                    //
                    // 上面那条「已存在的档位只换 sk」的规则本意是不冲掉用户的编辑，
                    // 但它连**我们自己写错的值**也一起保护了 ⇒ `pick_model` 上线前
                    // 被写成文本模型的纯生图档位，用户点多少次刷新都不会变好
                    // （实测：选中即 404，且生图工具的入口判据一直不成立）。
                    //
                    // 判据在 `repair_stale_model` 里（`is_user_edited == Some(false)`
                    // 才动），所以用户改过的档位仍然不受影响。
                    if provision::repair_stale_model(
                        &mut kept,
                        app_type,
                        &display_name,
                        &op.api_base_url,
                        &tier.model,
                    ) {
                        log::info!(
                            "{display_name} 的模型名已修正为 {}（原值是过时的默认值，用户未改过配置）",
                            tier.model
                        );
                    }
                    kept
                } else {
                    log::warn!("{display_name} 的配置里找不到放密钥的位置，已重置为默认配置");
                    defaults
                }
            }
            None => defaults,
        };

        let current = ProviderService::current(&state, app_type.clone()).unwrap_or_default();

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
            meta: Some(managed_meta(app_type, op.account_id)),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };

        state
            .db
            .save_provider(app_type.as_str(), &provider)
            .map_err(|e| AppError::Database(format!("保存档位 {display_name} 失败: {e}")))?;

        // ⚠️ **`keep` 必须带 app_type**，不能只放 provider_id。
        //
        // `provider_id` 是 `sha256(site_origin + group_id)` —— **不含 app_type**，
        // 所以同一个分组在 claude 与 codex 下是**同一个 id**。
        // 只按 id 判的话：`pro池` 在 claude 下是脏记录、在 codex 下是合法记录，
        // 那个 id 落进 keep ⇒ claude 下那条被当成「该保留」⇒ **永远删不掉**。
        // 那正是用户实测「点刷新 claude 页下仍挂着 codex 的分组」的真根因。
        keep.insert((app_type.as_str().to_string(), provider_id.clone()));

        // 这条路上判据算得**准**：`settings_config` 就在手边，`op.api_base_url` 也有，
        // 不需要像 `list_operators_impl` 那样绕一圈。
        //
        // ⚠️ 基准的模型名同样用 `tier.model` —— 拿 `DEFAULT_MODEL` 去比对生图档位，
        // 会让**每个生图档位都显示「已手动维护」**而用户一个字没改过
        // （与 `HISTORICAL_DEFAULT_MODELS` 文档里描述的误报同一形状）。
        let user_edited = provision::is_user_edited(
            &provider.settings_config,
            app_type,
            &display_name,
            &op.api_base_url,
            &tier.model,
        );

        let is_current = current == provider_id;
        if is_current {
            refresh_live.push(app_type.clone());
        }

        tiers.push(TierInfo {
            is_current,
            provider_id,
            // **这条分组自己的 app_type**，不是调用方给的 —— 这一整段循环的前提就是
            // 「一次 provision 探全部平台」，写错会让前端把别的平台的档位算成自己的。
            app_id: app_type.as_str().to_string(),
            group_name: tier.group_name.clone(),
            display_name,
            rate_multiplier: Some(tier.rate_multiplier),
            user_edited,
            // 这条路上两个字段都有真值：模型名刚由 `pick_model` 算出来，
            // 生图开关刚从 `/groups/available` 拉到。
            allow_image_generation: Some(tier.allow_image_generation),
        });
    }

    // 被改写的档位里若有**当前项**，必须把新配置落到 live 文件上。
    //
    // ## 为什么不能只 `save_provider`（用户实测的症状）
    //
    // CLI 读的是落地文件（`~/.codex/config.toml` 等），不是我们的 DB。服务端那把 sk
    // 被撤销后 `ensure_key_for` 会重建一把（`key_was_created = true`），DB 里换了新的，
    // 而 live 里还是旧的 ⇒ **界面提示刷新成功、库里也确实是新密钥，Codex / Claude 却
    // 仍拿旧密钥去请求**。更糟的是用户没有自救手段：UI 认为这个档位已经是当前项
    // （`isCurrent` 为 true ⇒ 前端 `if (tier.isCurrent) return;` 直接跳过），
    // 再点它一次也不会触发切换。
    //
    // 走 `sync_current_provider_for_app` 而不是 `switch`：我们不是在**切换**当前项
    // （它本来就是当前项），只是让它的落地配置追上 DB。那个 API 内部已处理代理接管
    // （接管时写备份而不是覆盖 live 文件），自己比 id + 调 `switch` 会绕过那层判断。
    //
    // 失败只 warn：记录已经存对了，用户手工切一次就能生效 —— 不该因为落地文件写不下去
    // 就把整次「获取密钥」报成失败（那会让他以为连密钥都没拿到）。
    refresh_live_for_current_tiers(&state, &refresh_live);

    let removed = prune_stale_tiers(&state, &op.site_origin, op.account_id, &keep)?;
    if removed > 0 {
        log::info!("清理了 {removed} 个不再存在的档位（{}）", op.site_origin);
    }

    // 生图工具跟着「生图栏里有没有档位」对齐一次。见 `sync_imagegen_mcp` 的文档。
    //
    // ⚠️ **必须在 `prune_stale_tiers` 之后** —— 判据是「生图栏里还有档位吗」，
    // 而清理正是让最后一条生图档位消失的那一步。反过来的话，运营商下架全部生图分组后
    // 那个工具会留到下一次 provision 才撤掉，期间它每次调用都报「档位已经不在了」。
    //
    // 失败只 warn：档位已经存对了，不该因为一个 MCP 记录写不下去就把「获取密钥」
    // 整个报成失败（用户会以为连密钥都没拿到）。
    if let Err(e) = sync_imagegen_mcp(&state) {
        log::warn!("同步生图工具记录失败（生图可能暂时用不了）: {e}");
    }

    Ok(ProvisionSummary {
        keys_created: result
            .tiers
            .iter()
            .filter(|t| t.tier.key_was_created)
            .count(),
        tiers,
        failures: result
            .failures
            .into_iter()
            .map(|(group_name, reason)| FailureInfo { group_name, reason })
            .collect(),
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
fn belongs_to_account(provider: &Provider, site_origin: &str, account_id: Option<i64>) -> bool {
    if !is_managed(provider) {
        return false;
    }
    if provider.website_url.as_deref() != Some(site_origin) {
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
/// （`check_session` 发现登录态被撤销时就走这条），而唯一索引把 `NULL` 视为互不相等
/// ⇒ 它与那个已登录的行并存。此时用户删这个空行会看到
/// 「这个账号名下还有档位正在使用中：B 的档位（codex）」—— 点名一个**它并不拥有**的档位，
/// 而这一行压根没有任何档位。他唯一的出路是去 codex 把 B 切走，才能删掉一个空行。
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
/// 2. **运营商在网页端删掉一个分组，本地那条会一直留着**。用户点它 ⇒ 用一把
///    已失效的 sk 发请求 ⇒ 报一个看不懂的 401。
///
/// ## 判据必须精确，宁可漏删不可错删
///
/// 删除条件是**三个都成立**：
///
/// - `is_managed(id)` —— 是我们生成的（**前缀 + 恰好 16 位小写 hex**），用户手工加的 provider
///   一律不碰。这是最重要的一道：错删用户自己配的 provider 是不可挽回的。
/// - `website_url == 这次的 site_origin` —— **只清这个运营商的**。别的站的档位这次
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

/// 「运营商 × 分组」页的数据源：一次返回渲染整页所需的全部内容。
///
/// **只读本地，不发网络请求**（spec §三）—— 与 [`operator_status`] 的首屏契约一致。
/// 代价是 `rate_multiplier` 恒为 `None`，要等用户主动 provision 才有值；
/// 那是有意的取舍，首屏不该卡在网络上。
///
/// `app` 决定读哪个 app_type 下的 provider（前端本来就知道当前是哪个 tab）。
#[tauri::command]
pub fn operator_list_operators(
    state: State<'_, AppState>,
    app: String,
) -> Result<Vec<OperatorRow>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    list_operators_impl(state.inner(), app_type).map_err(|e| e.to_string())
}

fn list_operators_impl(state: &AppState, app_type: AppType) -> Result<Vec<OperatorRow>, AppError> {
    let operators = with_conn(state, creds::list)?;
    // 一次读全量再在内存里按站分组，而不是对每个站各查一次 —— 站点通常 1-5 个，
    // 而 ProviderService::list 每次都要解一遍 settings_config 的 JSON。
    // `app_type` 下面在闭环里要按站点各用一次（判「用户改过配置没有」），
    // 而它没派生 Copy（上游结构，别为此改它）⇒ 先 clone 一份给 `list_tiers_impl`。
    let tiers = list_tiers_impl(state, app_type.clone())?;
    let now = chrono::Utc::now().timestamp();

    Ok(operators
        .into_iter()
        .map(|op| {
            let mine = tiers_of_site(
                &tiers,
                &op.site_origin,
                op.account_id,
                &op.api_base_url,
                &app_type,
            );
            OperatorRow {
                id: op.id,
                site_origin: op.site_origin.clone(),
                site_name: op.site_name.clone(),
                // 有 account_id 才算真的认得这个账号 —— email 可能被运营商留空。
                account_label: if op.account_id.is_some() {
                    op.account_label.clone()
                } else {
                    String::new()
                },
                logged_in: op.token_looks_valid(now),
                session_expired: op.session_expired(now),
                tiers: mine,
            }
        })
        .collect())
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
pub async fn operator_reset_tier_config(
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
    // 只对托管档位有效 —— 用户自建的 provider 没有「默认配置」这个概念。
    // 用正向判据 `is_managed`，不要拿 `reject_if_managed` 的 Err 反着判 ——
    // 那个函数的语义是「撞到托管项就拦下」（给通用命令用），这里要的恰好相反
    // （只对托管项生效），借它的错误来表达「是托管的」会让代码反着读。
    if !crate::operator::is_managed(provider_id) {
        return Err(AppError::Config(
            "只有 LoongPort 托管的档位才能恢复默认配置".into(),
        ));
    }

    let state = app_handle.state::<AppState>();
    let existing = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())
        .map_err(|e| AppError::Database(format!("读取档位失败: {e}")))?
        .ok_or_else(|| AppError::Config("这个档位不存在".into()))?;

    // ⚠️ **运营商必须按这个档位自己的归属取，绝不能用 `creds::load()`**（review 抓出的 P0）。
    //
    // `creds::load` 返回的是**全局「当前站」**（`ORDER BY is_current DESC LIMIT 1`），而
    // 分组页把所有运营商并列显示 —— 用户展开 B 站那一行、点它某个档位的「恢复默认配置」时，
    // 拿到的会是 A 站的 `api_base_url`，于是那个档位被写成「B 的 sk + A 的端点」⇒
    // **每次调用都 401**，而界面显示恢复成功。「恢复默认」恰恰是用户在档位坏了时点的按钮，
    // 那等于让它把自己要修的问题弄得更糟。
    //
    // `website_url` 是档位归属的唯一可靠依据（provision 时写入，见 `:799`；
    // `prune_stale_tiers` 也是靠它认主人）—— `provider_id` 是
    // `sha256(site_origin + group_id)`，单向不可逆，反推不出属于哪个站。
    //
    // 这是 `b2400000`「运营商之间彻底解耦」那一轮的漏网之鱼：这条命令写在那之前
    // （`ea2a32b7`），保留了「靠全局当前站定位」的旧写法，而本轮给它接上 UI 入口
    // 才让这个潜在缺陷变得可达。
    let site_origin = existing
        .website_url
        .as_deref()
        .ok_or_else(|| {
            AppError::Config(
                "这个档位没有记录它属于哪个运营商，请用「获取密钥」重新生成它。".into(),
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
    let candidates: Vec<_> = with_conn(state.inner(), creds::list)?
        .into_iter()
        .filter(|candidate| candidate.site_origin == site_origin)
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
                "这个档位属于 {site_origin}，但那个运营商已经不在列表里了。\
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
    let model = provision::extract_model(&existing.settings_config)
        .filter(|m| provision::is_image_model(m))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let settings_config = provision::settings_config_for(
        &app_type,
        &api_key,
        &existing.name,
        &op.api_base_url,
        &model,
    )
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
    let restored = Provider {
        settings_config,
        meta: Some(managed_meta(&app_type, op.account_id)),
        ..existing
    };

    state
        .db
        .save_provider(app_type.as_str(), &restored)
        .map_err(|e| AppError::Database(format!("恢复默认配置失败: {e}")))?;

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
    let is_current = ProviderService::current(&state, app_type.clone())
        .map(|current| current == restored.id)
        .unwrap_or(false);
    if is_current {
        refresh_live_for_current_tiers(&state, std::slice::from_ref(&app_type));
    }

    Ok(())
}

/// 保存运营商行的手工顺序。
///
/// `operator_ids` 是拖动后的完整顺序，下标即新的 `sort_index`。
///
/// ## 为什么行序要用户说了算
///
/// 原来 `creds::list` 排的是 `ORDER BY is_current DESC, id ASC` —— 「当前站」永远第一。
/// 而 `is_current` 会因为用户点某一行的登录/获取密钥而改变 ⇒ **行序跟着跳**。
/// 用户明确指出过：选一个档位不该重排运营商的顺序。
///
/// 现在改成按 `sort_index` 排，而这个命令是唯一会写它的地方 —— 只有用户拖动才改顺序。
#[tauri::command]
pub fn operator_reorder(state: State<'_, AppState>, operator_ids: Vec<i64>) -> Result<(), String> {
    with_conn(state.inner(), |conn| creds::reorder(conn, &operator_ids)).map_err(|e| e.to_string())
}

/// 一个档位的倍率查询结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierRate {
    pub provider_id: String,
    /// `None` = 查不到（站点不提供计费信息 / sk 没绑分组 / sk 已失效）。
    /// **不是错误** —— UI 继续显示「倍率未知」即可。
    pub rate_multiplier: Option<f64>,
}

/// 查一个 app 下所有托管档位的当前倍率。**首屏渲染后由前端异步调用**，不阻塞首屏。
///
/// ## 为什么用 sk 而不是登录态
///
/// 每个档位的 sk 就在它自己的 `settings_config.auth.OPENAI_API_KEY` 里（provision 时写的），
/// 而 `/v1/sub2api/billing` 是 sk 鉴权 —— 所以**账号登录过期了也能查到倍率**。
/// 走 `list_groups()` 就必须有有效登录态，而且拿到的还只是分组基础倍率
/// （不含用户专属倍率与高峰因子），见 [`api::key_billing`] 的文档。
///
/// ## 为什么单独一个命令而不是塞进 `operator_list_operators`
///
/// 那个命令的契约是「只读本地、不发网络」（与 `operator_status` 一致，首屏不能卡在网络上）。
/// 倍率必须发网络才拿得到 ⇒ 拆成第二个命令，前端首屏渲染完再调它填空。
///
/// 并发发请求（每个档位一个）而不是串行：档位通常 1-5 个，串行会把等待时间叠起来。
#[tauri::command]
pub async fn operator_list_tier_rates(
    app_handle: tauri::AppHandle,
    app: String,
    site_origin: Option<String>,
) -> Result<Vec<TierRate>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    list_tier_rates_impl(&app_handle, app_type, site_origin.as_deref())
        .await
        .map_err(|e| e.to_string())
}

async fn list_tier_rates_impl(
    app_handle: &tauri::AppHandle,
    app_type: AppType,
    // `Some(origin)` = 只查这一个运营商的档位。
    //
    // ⚠️ **这个过滤是必要的，不是优化**：每个档位一次 HTTP 请求，而用户给账号 A
    // 获取密钥时不该把账号 B / C 的倍率也全重查一遍（用户明确指出过这件事）。
    // 档位多、运营商多时那是几十次无谓请求，还可能撞运营商的限流。
    only_site: Option<&str>,
) -> Result<Vec<TierRate>, AppError> {
    // 先在同步块里把 (provider_id, site_origin, sk) 抠出来 —— AppState 的锁不能跨 await。
    let targets: Vec<(String, String, String)> = {
        let state = app_handle.state::<AppState>();
        ProviderService::list(&state, app_type.clone())?
            .values()
            .filter(|p| is_managed(p))
            .filter_map(|p| {
                let origin = p.website_url.clone()?;
                // 指定了运营商就只查它那些。
                if only_site.is_some_and(|want| want != origin) {
                    return None;
                }
                // ⚠️ **必须走 `extract_api_key` 而不是硬编码 `auth.OPENAI_API_KEY`** ——
                // 那是 codex 的位置，claude 在 `env.ANTHROPIC_AUTH_TOKEN`、
                // gemini 在 `env.GEMINI_API_KEY`。硬编码会让那两个平台永远查不到倍率
                // （而且是静默的：filter_map 直接跳过，用户只看到「倍率未知」）。
                //
                // 取不到就跳过这条（历史数据或形状被用户改过），不报错 ——
                // 倍率是附加信息，为它中断整个查询是错的。
                let sk = provision::extract_api_key(&p.settings_config, &app_type)?;
                Some((p.id.clone(), origin, sk))
            })
            .collect()
    };

    let futures = targets
        .into_iter()
        .map(|(provider_id, origin, sk)| async move {
            // 单个档位查失败不影响其它档位 —— 部分有值优于全部未知。
            let rate = match api::key_billing(&origin, &sk).await {
                Ok(Some(b)) => Some(b.effective_rate_multiplier),
                Ok(None) => None,
                Err(e) => {
                    log::debug!("查询 {provider_id} 的倍率失败（不影响使用）: {e}");
                    None
                }
            };
            TierRate {
                provider_id,
                rate_multiplier: rate,
            }
        });

    // 并发发请求（每个档位一个），**有意不设上限**：档位数 = 用户在该运营商的分组数，
    // 实测 1-5 个，而 sub2api 面板限流是 240 次/分钟 —— 差两个量级。
    // 为假设的「几十个分组」加 `buffer_unordered` 是过度设计（尺子2）。
    // 真撞限流时的表现也是良性的：那几个档位显示「倍率未知」，不影响切换。
    Ok(futures::future::join_all(futures).await)
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
    /// provision 时写下的运营商账号 id（`meta.loongportAccountId`）。
    /// `None` = 升级前生成的档位（那时还没记账号）。
    account_id: Option<i64>,
    /// 这条档位当前的 `settings_config`，供 [`tiers_of_site`] 判「用户改过没有」。
    ///
    /// 判据需要 `api_base_url`（属于站点、存在 `creds` 里），而这里只有 provider
    /// 记录 —— 所以判据不能在 [`list_tiers_impl`] 里算，得把原料带到按站分组那一步。
    settings_config: serde_json::Value,
    /// 判据的另一半原料：档位**当前**的显示名。
    ///
    /// ⚠️ **必须用当前名字，不能用默认名重算** —— 名字会进 codex 的
    /// `config.toml`（`[model_providers.custom] name`），用默认名当基准的话，
    /// 用户改过名的档位会**永远**显示「已手动维护」，哪怕其余配置一字未动。
    display_name: String,
}

/// 从档位列表里挑出属于某一行运营商（站点 × 账号）的那些，保持原有顺序。
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
/// `api_base_url` / `app_type` 只用来判「用户改过配置没有」（[`provision::is_user_edited`]）——
/// 归属判据不需要它们。`api_base_url` 属于站点（存在 `creds` 里），所以只有到了
/// 按站分组这一步才拿得到，见 [`OwnedTier::settings_config`] 的文档。
fn tiers_of_site(
    tiers: &[OwnedTier],
    site_origin: &str,
    account_id: Option<i64>,
    api_base_url: &str,
    app_type: &AppType,
) -> Vec<TierInfo> {
    tiers
        .iter()
        .filter(|owned| owned.site_origin.as_deref() == Some(site_origin))
        .filter(|owned| match (account_id, owned.account_id) {
            // 两边都知道账号 ⇒ 必须相等。
            (Some(want), Some(owner)) => want == owner,
            // 档位没记账号（旧数据）⇒ 只按站点归，见上面的文档。
            (_, None) => true,
            // 这一行还没登录（没有 account_id），而档位有主 ⇒ 不是它的。
            (None, Some(_)) => false,
        })
        .map(|owned| TierInfo {
            // ⚠️ **`app_id` 靠 `..owned.tier.clone()` 隐式继承**（来自
            // `list_tiers_impl`，那条路按 app 查所以值天然正确）。改成显式构造、
            // 或在中间插一层跨 app 的合并时**必须重新想清楚它** —— 那时它会静默
            // 变错（前端据它筛「属于当前那一屏的档位」），而没有测试会红。
            //
            // 基准用档位**当前**的名字（`owned.display_name`），不是默认名 ——
            // 见那个字段的文档：用默认名会让改过名的档位永远显示「已手动维护」。
            user_edited: provision::is_user_edited(
                &owned.settings_config,
                app_type,
                &owned.display_name,
                api_base_url,
                DEFAULT_MODEL,
            ),
            ..owned.tier.clone()
        })
        .collect()
}

// 曾经这里有个 `operator_list_tiers`（扁平列出全部档位，不按运营商分组）。
// 它在 `operator_list_operators` 上线后就没有调用方了 —— 界面按运营商分行显示，
// 拿一份不带归属的扁平列表没法渲染。2026-08-04 删掉命令壳，
// `list_tiers_impl` 留着（`list_operators_impl` 在用它）。

/// 内部版本额外带出每条档位的 `website_url`（= 所属站点的 origin），供
/// [`list_operators_impl`] 按站分组。命令层把它丢掉 —— 那是实现细节，不进对外契约。
fn list_tiers_impl(state: &AppState, app_type: AppType) -> Result<Vec<OwnedTier>, AppError> {
    // AppType 没派生 Copy（上游结构，别为此改它），所以 clone 一份给第二个调用点。
    let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();
    // 这条路按 app 查，所以结果天然同质 —— 每条档位的 `app_id` 就是被查的那个。
    // 先取出来：`app_type` 下一行就被 move 进 `list` 了。
    let app_id = app_type.as_str().to_string();
    let providers = ProviderService::list(state, app_type)?;

    let mut tiers: Vec<OwnedTier> = providers
        .values()
        .filter(|p| is_managed(p))
        .map(|p| OwnedTier {
            tier: TierInfo {
                provider_id: p.id.clone(),
                app_id: app_id.clone(),
                // 倍率不在本地存 —— 它是服务端的定价，可能已经变了。要看倍率就重新
                // provision，那时会从服务端拿到当前值。这里返回 None 让 UI 知道
                // "不知道"，而不是编一个 0。
                rate_multiplier: None,
                group_name: p.name.clone(),
                display_name: p.name.clone(),
                is_current: current == p.id,
                // 判据要 `api_base_url`（按站点存），这里拿不到 ⇒ 留 None，
                // 由 `tiers_of_site` 在按站分组时填。见该字段的文档。
                user_edited: None,
                // **这个在本地就能算**（判据是配置里的 `model`），所以首屏就有真值 ——
                // 不像倍率那样留 None 等异步填。见该字段的文档：入口忽隐忽现是有害的。
                // 纯服务端信息，本地推不出来 ⇒ None（UI 不显示标记）。
                allow_image_generation: None,
            },
            site_origin: p.website_url.clone(),
            account_id: p.meta.as_ref().and_then(|m| m.loongport_account_id),
            settings_config: p.settings_config.clone(),
            display_name: p.name.clone(),
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
pub async fn operator_switch_tier(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_impl(&app_handle, &provider_id, app_type, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
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
    crate::commands::provider::emit_provider_switched(app_handle, &app_type_for_event, provider_id);

    Ok(SwitchTierResult {
        provider_name,
        chatgpt_was_running: chatgpt.was_running,
        chatgpt_relaunched: chatgpt.relaunched,
        warnings,
    })
}

/// 列出全部已添加的站点。
#[tauri::command]
pub fn operator_list_sites(state: State<'_, AppState>) -> Result<Vec<SiteInfo>, String> {
    with_conn(state.inner(), |conn| {
        Ok(creds::list(conn)?
            .into_iter()
            .map(|op| SiteInfo {
                site_origin: op.site_origin,
                account_label: op.account_label,
            })
            .collect())
    })
    .map_err(|e| e.to_string())
}

/// 删掉一个站点，**连带它已生成的托管档位**。
///
/// ## 为什么连带删（2026-08-03 改，原来是只删站点）
///
/// 原来的行为是「不删 provider 记录，要清理走 provider 列表」。那条在只有一个入口
/// （站点切换器上的小叉）时说得通，但运营商行现在有了自己的删除按钮，而用户对那个
/// 按钮的预期是「这一行连它下面那几个档位一起没了」—— 留下一堆没有主人的托管档位
/// （登录态已经删了 ⇒ 它们必定 401），比删干净糟。
///
/// **只删这个站的托管档位**，判据是 `website_url == site_origin`（`prune_stale_tiers`
/// 里写清了为什么 `provider_id` 反推不出归属）。用户自建的 provider 一律不碰。
///
/// ## ⚠️ 「不许删掉任何平台正在用的档位」是**后端不变量**，不能靠前端按钮态
///
/// 前端确实有一道（有档位在用的行，删除按钮渲染成不可点，见 `RowDelete`），但那道判据是
/// `operator.tiers.some(t => t.isCurrent)` —— 而 `tiers` **只含当前 tab 那个 app 的档位**
/// （`list_operators_impl` 吃 `app_type`）。于是：
///
/// 1. 用户在 **Claude** tab 上看某一行 —— 它在 claude 下没有当前项 ⇒ 按钮可点；
/// 2. 而同一个账号在 **Codex** 下的档位正是 codex 的当前项；
/// 3. 删下去 ⇒ codex 的当前 provider 记录没了，而 `~/.codex/config.toml` 还指着它。
///
/// 「删 Claude 页的账号，把 Codex 正在用的配置删掉了」对用户是完全不可预期的。所以这里
/// **必须自己查一遍全部 app**，撞上就报错让他先切走 —— 前端那道保留（它给的是即时反馈，
/// 按钮先变灰比点下去弹错误好），但**它是提示，这里是闸**。
///
/// ## 与 `prune_stale_tiers` 「当前项也删」的区别（两者都对，因为前提不同）
///
/// `prune_stale_tiers` 在 provision 路径上会删当前项，理由是走到那一步说明**服务端已经
/// 没有那个分组了** ⇒ 它的 sk 是死的，留着当当前项只会让 CLI 拿废密钥去 401（见它的文档）。
///
/// 删账号这条路的前提相反：那些档位**还是好的**，用户只是想清掉这个账号。此时删掉当前项
/// 是在毁一份能用的配置，与那条裁决不矛盾 —— 判据是「这条档位还活着吗」，不是「它是不是
/// 当前项」。
///
/// 顺序：**先删档位再删站点**。反过来的话，站点行没了而 `site_origin` 是档位归属的唯一
/// 依据 —— 删站点之后就再也认不出哪些档位属于它，那些记录会永久留在 provider 列表里。
#[tauri::command]
pub fn operator_remove_site(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    remove_site_impl(state.inner(), id).map_err(|e| e.to_string())
}

fn remove_site_impl(state: &AppState, id: i64) -> Result<(), AppError> {
    // 先取归属信息 —— 删掉那行之后就没法知道该清哪些档位了。
    //
    // ⚠️ **`account_id` 与 `site_origin` 一样必须取**：删的是**一个账号**（一行），
    // 不是「这个站的全部」。同站另一个账号的档位不该被连带清掉 ——
    // 那正是 `prune_stale_tiers` 加账号维度要挡的事（见它的文档）。
    let op = with_conn(state, |conn| creds::get(conn, id))?
        .ok_or_else(|| AppError::Config("这个站点已经不存在了".into()))?;
    let site_origin = op.site_origin;
    let account_id = op.account_id;

    // ⚠️ 闸：这个账号名下有档位正被某个 app 用着 ⇒ **一条都不删，直接报错**。
    //
    // 全有或全无（在 `prune_stale_tiers` 之前拦，而不是让它逐条跳过正在用的那些）：
    // 半删的结果是「账号行没了，名下还剩一条孤儿档位」—— 而托管档位在 provider 列表里
    // 被前端过滤、被通用删除命令拒绝 ⇒ 那条记录用户再也处置不了。
    //
    // 文案点名**哪个平台、哪个档位**：只说「有档位在使用中」的话，用户得自己去六个 tab
    // 里翻是哪一个。他要做的处置（去那个平台切走）完全取决于这个信息。
    let in_use = apps_using_this_accounts_tiers(state, &site_origin, account_id);
    if !in_use.is_empty() {
        let detail = in_use
            .iter()
            .map(|(app_type, name)| format!("{}（{}）", name, app_type.as_str()))
            .collect::<Vec<_>>()
            .join("、");
        return Err(AppError::Config(format!(
            "这个账号名下还有档位正在使用中：{detail}。请先在对应平台切换到别的供应商，再删除这个账号。"
        )));
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

    with_conn(state, |conn| creds::remove(conn, id))
}

/// 余额。`operator_id` 指定查**哪一行**的。
///
/// `None` 回落到「当前站」。**前端已没有该省略它的调用方**（运营商区的每一行都显式传），
/// 与 [`operator_login`] / [`operator_provision`] 同一套形状、同一条纪律：
/// **显式指定时查不到就报错，绝不回落到当前站**（那会把 B 的余额显示在 A 那一行上，
/// 用户会照着错的数字决定要不要充值 —— 比报错糟得多）。
///
/// 这个参数原本有意不加，理由是「运营商行上并不显示余额」。现在每一行都显示自己的余额，
/// 消费者有了，所以按当初写下的预案补上：`usable_operator` 早就支持 `Option<i64>`。
///
/// ## 一行一次请求是安全的
///
/// `/user/profile` **没挂 `Heavy()`**，只吃 `panelRateLimiter.Global()`
/// （sub2api 默认 `UserRPM = 240/分钟`，按 user_id 计数）—— 而且不同运营商行往往是
/// **不同用户**，各记各的额度。N 行各打一次远远碰不到限流。
#[tauri::command]
pub async fn operator_balance(
    app_handle: tauri::AppHandle,
    operator_id: i64,
) -> Result<api::Balance, String> {
    let op = usable_operator(&app_handle, operator_id)
        .await
        .map_err(|e| e.to_string())?;
    let client = api::Client::new(&op.site_origin, &op.auth_token, op.account_id)
        .map_err(|e| e.to_string())?;
    client.balance().await.map_err(|e| e.to_string())
}

/// 带登录态打开某个运营商的充值页。
///
/// `operator_id` 指定给**哪一行**充值。与 [`operator_login`] / [`operator_balance`] 同形
/// 同纪律：显式指定查不到就报错、绝不回落到当前站 —— 那会让用户在 B 行点充值、
/// 钱充进 A 账号。
///
/// 返回 `Ok(())` 只表示**窗口开出来了**，不表示用户付了钱。
/// 我们**有意不做支付成功感知**（维护者裁决）：关窗时刷一次余额就够，
/// 充完钱余额自然会涨。
#[tauri::command]
pub async fn operator_purchase(
    app_handle: tauri::AppHandle,
    operator_id: i64,
) -> Result<(), String> {
    open_purchase_window(&app_handle, operator_id)
        .await
        .map_err(|e| e.to_string())
}

async fn open_purchase_window(
    app_handle: &tauri::AppHandle,
    operator_id: i64,
) -> Result<(), AppError> {
    let op = usable_operator(app_handle, operator_id).await?;

    // ⚠️ **充值是长会话，`usable_operator` 的余量对它不够**（review 抓出）。
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
    let client = api::Client::new(&op.site_origin, &op.auth_token, op.account_id)?;
    let auth_user = purchase::auth_user_from_profile(client.profile_raw().await?)?;

    // 这一行已经有充值窗时**聚焦它，不销毁重开** —— 与 `do_login` 的处置**有意相反**。
    //
    // 登录窗那边销毁重开是对的：能走到那儿说明上一轮 `do_login` 已经返回，
    // 那个窗口已经没人在等它的凭据了，留着反而是陷阱。
    //
    // 充值窗背后是**已经发生的钱**：用户可能正盯着一个二维码、或已经跳到了支付网关。
    // 销毁它不会取消服务端的订单或网关扣款，只会让用户失去轮询与确认页面，
    // 进而很可能重新下一单 ⇒ 两笔待支付、甚至重复付款。
    //
    // 窗口是**按 operator_id 分的**，所以「点另一行的充值」压根不会碰到这一个
    // （那是另一个 label）。这里处理的只是「同一行连点两次」。
    let label = purchase::window_label(op.id);
    if let Some(existing) = app_handle.get_webview_window(&label) {
        log::info!("这一行的充值窗已经开着，聚焦它而不是重开");
        // 可能被用户最小化或藏到别的 Space 了，先 show 再 focus ——
        // `set_focus` 对不可见窗口是 no-op。
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = url::Url::parse(&purchase::purchase_url(&op.site_origin))
        .map_err(|e| AppError::Config(format!("充值页地址不对: {e}")))?;

    // 关窗事件要带上是哪一行 —— 前端据此只刷那一行的余额。
    let handle_for_close = app_handle.clone();
    let closed_operator_id = op.id;

    let window =
        tauri::WebviewWindowBuilder::new(app_handle, &label, tauri::WebviewUrl::External(url))
            .title(format!("充值 {}", op.site_origin))
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
            // 与 HTTP 客户端共用同一个 UA：sub2api 的会话绑定（可选，默认关）会把
            // `SHA256(clientIP + "\n" + UA)[:16]` 编进 token，UA 不一致会 401 并
            // **撤销整个会话家族**（连网页登录态一起踢）。
            .user_agent(login::WEBVIEW_USER_AGENT)
            .initialization_script(purchase::inject_script(
                &op.site_origin,
                &op.auth_token,
                &auth_user,
            ))
            .build()
            .map_err(|e| AppError::Config(format!("打开充值窗口失败: {e}")))?;

    // 关窗刷余额。认 `Destroyed`（窗口真的没了）而不是 `CloseRequested`
    // （可被拦下的关闭请求，某些平台上会先于实际销毁触发、甚至可能被取消）。
    //
    // 只 emit 事件、不在这里查余额：查余额要发 HTTP，而这个回调不能 await。
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = handle_for_close.emit(PURCHASE_CLOSED_EVENT, closed_operator_id);
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
/// **失败不算错误** —— 原样返回传进来的凭据（`usable_operator` 已经保证它现在可用），
/// 让用户至少能完成一笔快的；把「可能不够」当成「一定不行」去拦住他更糟。
async fn ensure_token_outlasts_a_payment(
    app_handle: &tauri::AppHandle,
    op: creds::Operator,
) -> creds::Operator {
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
            creds::Operator {
                auth_token: fresh.auth_token,
                refresh_token: fresh.refresh_token.or(Some(refresh)),
                token_expires_at: fresh.token_expires_at,
                ..op
            }
        }
        Err(e) => {
            // 续期失败不拦：现有 token 还没过期（`usable_operator` 已经保证了），
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
/// **没有任何反应**，请求照样带着运营商的 sk 打出去。这不是 bug，但对用户是困惑 ——
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
/// 各不相同且都很糟：只删 `auth.json` ⇒ 仍走运营商（token 还在 `config.toml` 里）；
/// 只切 provider ⇒ 走 ChatGPT auth 模式但没登录态 ⇒ codex 报 credentials incomplete。
///
/// **2 不可省**：`ProviderService::switch` 自己那套清理（`clear_stale_codex_live_auth_after_official_switch`）
/// **有意不删带 OAuth 的 auth.json**（见 `codex_config::codex_auth_has_credential_login_material`）——
/// 用户的 ChatGPT 登录正是它拒绝碰的那一类，所以第 4 步必须自己动手，而动手之前必须留后路。
#[tauri::command]
pub async fn operator_restore_official_login(
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
        // 且都很糟：只删 `auth.json` ⇒ 仍走运营商（token 还在 `config.toml` 里）；
        // 只切 provider ⇒ 走 ChatGPT auth 模式但没登录态 ⇒ 报 credentials incomplete。
        //
        // 删失败**不回滚切换**：那时 codex 已经是官方 provider（没有 bearer token 了），
        // 回滚等于把用户送回运营商路由 —— 而他刚刚明确要求离开那里。如实报出来让他
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
    // `codex-official`），所以它跟 `operator_switch_tier` / `switch_provider` 一样
    // 是一条**切换路径**，必须发。
    //
    // 漏了它的症状（2026-08-04 review 抓出）：用户在设置页点「切回官方登录」成功后
    // 回到供应商页，运营商区里原来那个托管档位**仍高亮「当前使用中」**、
    // 运营商行的删除按钮仍是灰的、title 还写着「要先切走」—— 而他已经切走了。
    // 后端是对的，坏的只有「不重开窗口就看不到」这一段（静默的界面陈旧）。
    //
    // 补在后端而不是让 `RestoreOfficialLoginButton` 自己刷：那个按钮在设置页，
    // 与运营商区互不相识；发事件是上游已有的机制，一处发射喂到全部监听者
    // （`provider.rs::emit_provider_switched` 的文档写了完整论证）。
    crate::commands::provider::emit_provider_switched(
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

/// 充值窗关闭的事件名。**payload 是 `operator_id`（i64）** —— 前端据此只刷那一行的余额。
///
/// 提成常量是因为它是跨语言契约：前端 `useTauriEvent(PURCHASE_CLOSED_EVENT, ...)` 那边
/// 写的是同一个字符串，两处各写一遍字面量的话，改名时会变成「关窗后余额永远不刷」——
/// 而那是个静默失效，没有任何东西会报错。
pub const PURCHASE_CLOSED_EVENT: &str = "operator-purchase-closed";

/// 这条 provider 是不是 LoongPort 管的。
///
/// 判据本身在 [`crate::operator::managed`]（唯一来源，托盘与命令层守卫也用它）；这里只是
/// 把「按 `&Provider` 判」这个便利形状留在本地，别在这儿重写前缀。
fn is_managed(p: &Provider) -> bool {
    crate::operator::is_managed(&p.id)
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
fn managed_meta(app_type: &AppType, account_id: Option<i64>) -> crate::provider::ProviderMeta {
    crate::provider::ProviderMeta {
        // `api_format` **只被 `codex_config.rs` 消费**（`CodexCatalogToolProfile::from_api_format`），
        // 对 claude / gemini 无意义 —— 给它们填值不会有人读，反而让人以为那里有语义。
        api_format: match app_type {
            AppType::Codex => Some("openai_responses".to_string()),
            _ => None,
        },
        loongport_account_id: account_id,
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

/// 生图 MCP 在 `mcp_servers` 表里的 id。
///
/// ## 为什么是**一条固定记录**而不是「一个档位一条」
///
/// 「用哪个档位生图」= 生图栏（`codex-image`）的当前项，MCP 进程**每次生图时现读**
/// （见 `imagegen_mcp::current_image_tier_id`）。所以这条 MCP 记录的内容与档位无关，
/// 只回答「这个宿主要不要有生图工具」—— 换档位不必改它。
///
/// 那正是「切生图档位不用重启 codex」的来源：codex 只在启动时读它的 `config.toml`，
/// 若档位 id 写在这条记录里，用户每换一次都得新开终端。
///
/// ⚠️ 这个 id 会成为 `[mcp_servers.<id>]` 的表名，**跨出了进程边界** ——
/// 改它等于让已装用户的旧配置成为孤儿（库里删不到、CLI 里还留着一条起不来的 server）。
const IMAGEGEN_MCP_ID: &str = "loongport-imagegen";

/// 启动 MCP server 模式的命令行开关。**与 `main.rs` 里那个判断必须一致**。
///
/// 两处各写一遍字面量迟早分叉（改了一处另一处没跟上 ⇒ 写出去的配置启动不了 server，
/// 而症状是宿主那边"启动超时"，看不出是拼写问题）。所以这里是唯一定义，
/// `main.rs` 用 `cc_switch_lib::IMAGEGEN_MCP_FLAG` 引它。
pub const IMAGEGEN_MCP_FLAG: &str = "--mcp-image-gen";

/// 生图工具的安装 / 撤销，跟着「生图栏里有没有档位」自动走。
///
/// ## 为什么不再有「启用生图」这个显式动作
///
/// 上一版有一对命令（`operator_set_image_tier` / `operator_current_image_tier`）在
/// `settings` 表里维护「用哪个档位生图」。分栏之后那套是纯粹的重复：
/// 「当前是哪一档」由 `providers.is_current` 表达，而它**每个 app_type 一栏** ——
/// 生图栏天然就有自己的一份，用户点「切换」走的就是与聊天档位同一条路
/// （`operator_switch_tier`）。
///
/// 所以现在只剩一个问题：**这个宿主要不要有生图工具**。答案由生图栏里有没有档位决定，
/// 在 provision 收尾时对齐一次（见 [`sync_imagegen_mcp`]）。用户不必学第二套操作。
/// 确保生图 MCP 记录在库里（进而被同步进各 CLI 的配置）。
///
/// ## 为什么写 `mcp_servers` 表而不是直接改 CLI 的配置文件
///
/// 那张表是 MCP 的 SSOT，各 CLI 的 `config.toml` / `mcp.json` 只是它的投影
/// （`codex_config.rs` 的 `strip_codex_mcp_servers_from_settings` 那段注释钉过这条）。
/// 自己去写配置文件会被下一次同步覆盖，而且绕过了「切档位时重投影」那套逻辑。
///
/// **幂等**：装的那一支走 `upsert_server`（覆盖同 id），撤的那一支走 `delete_server`
/// （不存在时返回 `Ok(false)`）。所以每次 provision 收尾都能无条件调它。
///
/// ## 「有没有生图档位」是唯一判据
///
/// 用户那个站可能压根没有生图分组（实测 bestapi.store 就没有）—— 那时生图栏是空的，
/// 这里把 MCP 记录撤掉 ⇒ **CLI 的配置里一个字都不多**，完全无感。
/// 有生图分组的用户则自动获得那个工具，不必学一个额外的「启用生图」动作。
///
/// ⚠️ **不看「有没有选当前项」** —— 那会让「装工具」依赖一个用户可能还没做的选择，
/// 而工具在没选档位时本来就会给出一句可操作的提示（`NO_IMAGE_TIER_HINT`）。
/// 反过来（有档位却没工具）才是真的坏：用户说「画一只猫」，模型答「我没有这个工具」。
fn sync_imagegen_mcp(state: &AppState) -> Result<(), AppError> {
    let has_image_tiers = ProviderService::list(state, AppType::CodexImage)
        .map(|list| list.values().any(is_managed))
        .unwrap_or(false);

    if !has_image_tiers {
        // 撤掉。**不因为它本来就不在而算失败** —— `delete_server` 返回 `Ok(false)`。
        let removed = McpService::delete_server(state, IMAGEGEN_MCP_ID)?;
        if removed {
            log::info!("生图栏里没有档位了，撤掉生图 MCP 记录");
        }
        return Ok(());
    }

    install_imagegen_mcp(state)
}

/// 把生图 MCP 记录写进库（幂等）。
fn install_imagegen_mcp(state: &AppState) -> Result<(), AppError> {
    // 当前可执行文件的绝对路径。macOS 上这是 `.app/Contents/MacOS/<bin>`，
    // 正是 CLI 该去启动的东西。
    let exe = std::env::current_exe()
        .map_err(|e| AppError::Message(format!("获取可执行文件路径失败: {e}")))?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| AppError::Message("可执行文件路径不是有效的 UTF-8".into()))?;

    // ⚠️ **args 里不带档位 id** —— 见 `IMAGEGEN_MCP_ID` 的文档：带了就等于每次换档位都
    // 改 CLI 配置文件，而 codex 只在启动时读它。
    let spec = serde_json::json!({
        "type": "stdio",
        "command": exe_str,
        "args": [IMAGEGEN_MCP_FLAG],
    });

    // 装到 codex + claude + gemini：这三个都支持 stdio MCP，而「要生图」与用户在哪个
    // CLI 里干活无关。不装 opencode / hermes —— 那两个的配置形状要另外验，没验过的不写。
    let apps = crate::app_config::McpApps {
        codex: true,
        claude: true,
        gemini: true,
        ..Default::default()
    };

    McpService::upsert_server(
        state,
        crate::app_config::McpServer {
            id: IMAGEGEN_MCP_ID.to_string(),
            name: "LoongPort 生图".to_string(),
            server: spec,
            apps,
            // ⚠️ **描述里不提某个档位名** —— 这条记录与档位无关（换档位不改它），
            // 写了档位名就得在每次切换时刷新它，而那正是「不必重启 CLI」要避免的事。
            description: Some(
                "用 LoongPort「生图」标签页里当前那个档位生图（gpt-image 系列）。\
                 由 LoongPort 自动维护，密钥不写进 CLI 配置 —— 换档位也不必重启 CLI。"
                    .to_string(),
            ),
            homepage: None,
            docs: None,
            tags: vec!["loongport".into(), "image".into()],
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// ⭐ `operator_list_sponsors` 发给前端的**键名**必须是 camelCase。
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
        let sponsor = crate::operator::remote_config::Sponsor {
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
            rate_multiplier: Some(1.0),
            is_current: false,
            user_edited: None,
            allow_image_generation: None,
        };

        let json = serde_json::to_value(&tier).expect("要能序列化");
        let obj = json.as_object().expect("是个对象");

        assert_eq!(
            obj.get("appId").and_then(|v| v.as_str()),
            Some("claude"),
            "前端要靠 appId 判断这条档位是不是属于它当前那一屏，实际：{:?}",
            obj.keys().collect::<Vec<_>>()
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

    fn tier(id: &str) -> TierInfo {
        TierInfo {
            provider_id: id.into(),
            // 归属测试只关心「哪条属于哪个站/账号」，与落在哪个 CLI 无关。
            app_id: AppType::Codex.as_str().to_string(),
            group_name: id.into(),
            display_name: id.into(),
            rate_multiplier: None,
            is_current: false,
            // 归属测试不关心它 —— `tiers_of_site` 会自己算出来覆盖掉这个值。
            user_edited: None,
            // 同上：归属判定与生图无关。
            allow_image_generation: None,
        }
    }

    /// 这些归属测试用的站点 base_url 与 app_type。
    ///
    /// 它们只喂给「用户改过配置没有」那个判据，与归属判定无关 —— 但得给个值。
    const TEST_BASE_URL: &str = "https://bestapi.store/v1";
    fn test_app() -> AppType {
        AppType::Codex
    }

    /// 构造一条带归属的档位。`account` 为 `None` 表示升级前生成的旧档位。
    ///
    /// `settings_config` 给一份**默认配置**（不是空 JSON）：空的会让判据返回
    /// `None`（读不出 sk），那样这些测试就悄悄测不到「判据被填上了」这件事。
    fn owned(id: &str, site: Option<&str>, account: Option<i64>) -> OwnedTier {
        OwnedTier {
            tier: tier(id),
            site_origin: site.map(str::to_string),
            account_id: account,
            settings_config: provision::settings_config_for(
                &test_app(),
                "sk-test",
                id,
                TEST_BASE_URL,
                DEFAULT_MODEL,
            )
            .expect("codex 必须有默认形状"),
            display_name: id.into(),
        }
    }

    /// `tiers_of_site` 的两个新参数在归属测试里恒定，包一层省得每处重复。
    fn tiers_of(tiers: &[OwnedTier], site: &str, account: Option<i64>) -> Vec<TierInfo> {
        tiers_of_site(tiers, site, account, TEST_BASE_URL, &test_app())
    }

    /// ⭐ **`tiers_of_site` 必须真的算出 `user_edited`，不能原样透传 `None`。**
    ///
    /// 这条守的是一处**静默退化**：`list_tiers_impl` 构造 `OwnedTier` 时把
    /// `tier.user_edited` 填成 `None`（那一步拿不到 `api_base_url`），指望
    /// `tiers_of_site` 在按站分组时覆盖掉。哪天那行 `user_edited: …` 被误删或
    /// 改成 `..owned.tier.clone()` 一把带过，字段就恒为 `None` ⇒
    /// **界面上所有标记集体消失**，而编译器、类型检查、其余测试全都不会红。
    #[test]
    fn grouping_computes_the_user_edited_flag_instead_of_passing_none_through() {
        let site = "https://bestapi.store";

        // 一条默认配置的档位 + 一条被改过的。
        let mut edited = owned("t-edited", Some(site), Some(1));
        edited.settings_config["config"] = serde_json::json!("model = \"用户改的\"");
        let tiers = vec![owned("t-default", Some(site), Some(1)), edited];

        let got = tiers_of(&tiers, site, Some(1));
        let flags: Vec<_> = got.iter().map(|t| t.user_edited).collect();

        assert_eq!(
            flags,
            vec![Some(false), Some(true)],
            "user_edited 没被算出来（全 None = 那行赋值丢了；全同值 = 判据没用上每条自己的配置）"
        );
    }

    /// 换过 sk 的档位不该被判成「用户改过」—— 与 `provision` 那条同一个性质，
    /// 但走的是**完整的分组路径**（含 `display_name` 当基准这一环）。
    ///
    /// ⚠️ 顺带钉住「基准用当前名字」：改过名的档位如果拿默认名重算基准，
    /// 会**永远**显示「已手动维护」。
    #[test]
    fn a_renamed_tier_with_a_rotated_key_is_still_not_user_edited() {
        let site = "https://bestapi.store";
        let mut renamed = owned("t-1", Some(site), Some(1));
        // 用户在编辑页改了显示名 —— 那个名字会进 config.toml，所以配置内容确实变了，
        // 但它不是「改坏配置」，基准必须跟着用新名字。
        renamed.display_name = "我自己起的名字".into();
        renamed.settings_config = provision::settings_config_for(
            &test_app(),
            "sk-rotated",
            "我自己起的名字",
            TEST_BASE_URL,
            DEFAULT_MODEL,
        )
        .expect("codex 必须有默认形状");

        assert_eq!(
            tiers_of(&[renamed], site, Some(1))[0].user_edited,
            Some(false),
            "改过名 + 换过 sk 被误判成「用户改坏了配置」"
        );
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
    fn managed_meta_pins_api_format_for_codex_and_leaves_others_empty() {
        // codex：不写 apiFormat 会落到 ProxyChat profile —— 那是唯一会 spawn codex
        // 子进程的分支。
        assert_eq!(
            managed_meta(&AppType::Codex, Some(1)).api_format.as_deref(),
            Some("openai_responses")
        );

        // 其它 CLI：`api_format` **只被 codex_config.rs 消费**，给它们填值不会有人读，
        // 反而让人以为那里有语义。
        for app_type in [AppType::Claude, AppType::Gemini] {
            assert_eq!(
                managed_meta(&app_type, Some(1)).api_format,
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
    /// [`crate::operator::aff`] 的测试里仍留着「维护者自己的站不该有码」那条，
    /// 很容易有人按类比把默认站也从表里划掉，而那**不报任何错**，
    /// 只是每一次「留空点确定」都白丢一笔返利。
    ///
    /// ⚠️ **它守的是内置那一层，不是运行时的最终取值**（codex review 纠正）：
    /// 实际取码走 [`crate::operator::remote_config::resolve_aff_code`] 的两层回落，
    /// 远端配置命中就用远端的，且**远端给空串 = 撤销、不回落到内置**。
    /// 所以本条断言不能、也不该保证「线上一定带码」—— 那取决于维护者当天发的配置。
    #[test]
    fn the_default_site_has_a_builtin_affiliate_code() {
        assert!(
            crate::operator::aff::aff_code_for(&format!("https://{DEFAULT_SITE}")).is_some(),
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
            meta: Some(managed_meta(&AppType::Codex, Some(account_id))),
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
    /// 它是**唯一会删用户数据的 operator 代码路径**，判据放宽一点就会误删用户手工配置的
    /// provider（不可挽回）；收紧一点则清不掉脏记录（就是用户撞见的「claude 下还有
    /// codex 分组，点刷新也不消失」）。所以正反两面都要钉住。
    #[test]
    fn prune_only_touches_this_sites_managed_tiers() {
        let site = "https://bestapi.store";
        let other_site = "https://other.dev";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 这次 provision 生成的（该留）。
        let kept_id = provision::provider_id_for(site, Some(1), 1);
        // 同一个站的托管项，但这次没生成（该删 —— 分组已被运营商删掉 / 旧版本写错的）。
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

    /// ⚠️ **「恢复默认配置」必须按档位自己的归属找运营商，不能用全局「当前站」。**
    ///
    /// 这条钉的是 review 抓出的那个 P0：原来那行是 `creds::load()`，返回的是
    /// `ORDER BY is_current DESC LIMIT 1` —— 全局当前站。而分组页把所有运营商并列，
    /// 用户展开 B 站点它的档位时，会拿到 **A 站的 `api_base_url`** ⇒ 那个档位被写成
    /// 「B 的 sk + A 的端点」⇒ 每次调用都 401，而界面显示恢复成功。
    ///
    /// **单站用户完全碰不到**（那时当前站就是唯一的站），所以手工测试测不出来 ——
    /// 这正是它需要一条测试的原因。
    ///
    /// 会红的改法：把归属判据换回 `creds::load()` / 任何「全局当前」的东西。
    #[test]
    fn reset_resolves_the_owning_site_not_the_current_one() {
        // 判据本身：从 provider 的 `website_url` 认主人，而不是问「现在哪个站是当前」。
        fn owner_of(existing: &Provider) -> Option<&str> {
            existing.website_url.as_deref()
        }

        let site_a = "https://a.example";
        let site_b = "https://b.example";

        let tier_of_b = seeded(
            &provision::provider_id_for(site_b, Some(1), 7),
            "B 的档位",
            Some(site_b),
        );

        assert_eq!(
            owner_of(&tier_of_b),
            Some(site_b),
            "B 的档位必须认 B 作主人 —— 哪怕此刻的「当前站」是 A"
        );
        assert_ne!(
            owner_of(&tier_of_b),
            Some(site_a),
            "绝不能解析到别的站：那会把 B 的 sk 配上 A 的端点，每次调用都 401"
        );

        // 没有 website_url 的档位（旧版本写的脏记录）要报错而不是猜一个站 ——
        // 猜错的代价与上面那条一样，而且用户根本不知道发生了什么。
        let orphan = seeded("loongport-orphan", "没有归属", None);
        assert!(
            owner_of(&orphan).is_none(),
            "认不出主人时必须为 None（调用方据此报错引导重新获取密钥），不能回落到当前站"
        );
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
    /// `do_provision` 与 `reset_tier_config_impl` 都吃 `&tauri::AppHandle`，单测与集成
    /// 测试里都造不出来 ⇒ 命令层这一步**没有任何测试能直接执行到**。第二路 review 实测
    /// 证明了这个盲区的代价：把那两处调用注释掉，2578 条测试**全绿**——
    /// 那条集成测试（`loongport_codex_live.rs`）自己调服务层，所以它测的是服务层，
    /// 不是「命令层有没有调服务层」。
    ///
    /// 源码断言是这里唯一能把那一步钉住的手段（与仓里 `vendorSwitchGuardContract`
    /// 那条同一个理由与形态）。它守的不是实现细节，而是**这条链路还接着吗** ——
    /// 断了的症状是静默的：界面提示刷新成功，而 CLI 一直用旧密钥。
    #[test]
    fn refresh_live_for_current_tiers_is_wired_into_both_commands() {
        let src = include_str!("operator.rs");

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
            provision.contains("refresh_live_for_current_tiers(&state, &refresh_live)"),
            "⭐ `do_provision` 不再刷新当前档位的 live config —— \
             sk 被撤销重建后，CLI 会一直用旧密钥，而用户点不动那个档位（UI 认为它已是当前项）"
        );

        // 取 `reset_tier_config_impl` 那段。
        let reset = {
            let start = src
                .find("async fn reset_tier_config_impl")
                .expect("reset_tier_config_impl 还在吗");
            let end = src[start..]
                .find("\n/// 保存运营商行的手工顺序")
                .expect("reset 之后那个命令还在吗");
            &src[start..start + end]
        };
        assert!(
            reset.contains("refresh_live_for_current_tiers("),
            "⭐ `reset_tier_config_impl` 不再刷新 live config —— \
             那会让「恢复默认配置」这个按钮对当前项**整体无效**（改坏的配置就在 live 文件里）"
        );
    }

    /// ⭐ **删账号不许毁掉「别的平台」正在用的档位** —— 前端那道判据挡不住这一类。
    ///
    /// 这是 review 抓出的缺陷现场，复现路径：
    ///
    /// 1. `list_operators_impl` 吃 `app_type` ⇒ `OperatorRow.tiers` 只含**当前 tab** 的档位；
    /// 2. 于是前端 `hasCurrentTier`（`OperatorRow.tsx`）在 claude tab 上看这一行时是
    ///    `false` ⇒ 删除按钮可点；
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
        let err = remove_site_impl(&state, row_id)
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

        remove_site_impl(&state, row_id).expect("没人在用它时删除该成功");

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
    /// 这种行真实可达：`clear_credentials` 会把 `account_id` 置 `NULL`（`check_session`
    /// 发现登录态被撤销时走这条），而唯一索引把 `NULL` 视为互不相等 ⇒ 它与已登录的行并存。
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
}
