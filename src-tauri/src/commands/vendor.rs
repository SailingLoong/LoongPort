//! 官网直连账号（vendor）的 Tauri 命令层。与 [`super::relay`] **平级并列**。
//!
//! 六个命令：
//!
//! | 命令 | 干什么 |
//! |---|---|
//! | [`vendor_list_accounts`] | 列出已添加的官网账号（**只读本地、不发网络**）|
//! | [`vendor_open_login`] | 开登录 WebView，等凭据回来，存账号行 |
//! | [`vendor_provision`] | 备好密钥 → 展开成六个平台的 provider 记录 |
//! | [`vendor_balance`] | 查一行的余额 |
//! | [`vendor_remove`] | 删一行（连带清它的 provider 记录）|
//! | [`vendor_reorder`] | 保存行序 |
//!
//! ## 为什么另起一个命令模块而不是塞进 `commands/relay.rs`
//!
//! 那个文件已经 2700 行，而两边**没有一个命令能共用**：中转站要探测域名、选分组、
//! 退 ChatGPT 再切；官网这边域名是编译期常量、无分组、无倍率。硬合只会让每个命令
//! 里多一个 `if is_vendor` 分支。分层理由与 [`crate::vendor`] 那条相同。
//!
//! ## 失败语义（两条最容易写错，写错了都是持久的脏状态）
//!
//! - **账号身份拿不到 ⇒ 不存 token**。存了会造出「有 token 却没 account_id」的行：
//!   它既进不了去重索引 `(vendor_id, account_id)`，也认不回官网上已建的 key。
//!   判据在 [`crate::vendor::deepseek::parse_creds_navigation`] 里就拦掉了。
//! - **`40002`（登录过期）⇒ 只 `clear_token`，不动 `api_key`**。后者是厂商侧的
//!   独立凭据，网页登录态过期**不影响它** —— 清掉等于无端废掉一把好 key。

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::events::VENDOR_LOGIN_ERROR;
use crate::provider::Provider;
use crate::services::ProviderService;
use crate::store::AppState;
use crate::vendor::{creds, deepseek, provision, Vendor, VendorError};

/// 等用户走完登录流程的上限。5 分钟够走完注册 + 短信验证码 + 微信扫码。
///
/// 超时**不是错误**：用户可能就是走开了，返回 `false` 让前端安静收场。
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// 一个已添加的官网账号（给前端列表用）。
///
/// ⚠️ **不含 `auth_token` 与 `api_key`** —— 凭据不出 Rust 侧。前端要的是
/// 「这一行能不能用、显示什么」，给它明文 sk 只会让那把 key 出现在
/// devtools 的网络面板与前端状态里。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorAccountRow {
    pub id: i64,
    pub vendor_id: String,
    /// 厂商展示名（`DeepSeek`）。认不出的 `vendor_id` 回落到它本身。
    pub vendor_name: String,
    /// 给人看的账号名（手机号，空则回落 account_id）。
    pub account_label: String,
    /// 已有可用登录态。
    pub logged_in: bool,
    /// **登录过、但登录态已经不能用了** —— 前端据此提示「登录已过期，请重新登录」，
    /// 而不是像从没登录过一样只摆一个「登录」按钮。
    ///
    /// 与 `!logged_in` 不同：那个把「从没登录」与「过期」混成一件事，而对用户是两种
    /// 处境。判据是「有 `account_id`（登录过）但 token 空了」。
    pub session_expired: bool,
    /// **本地已经有这个账号的 sk 明文**。
    ///
    /// ⚠️ 判据就是 `api_key` 非空，**不代表六个平台的 provider 记录都写成功了**。
    /// 别把它读成「配置已就绪」：`provision_impl` 第 5 步先落 sk、第 6 步才展开六条，
    /// 中途某条 `save_provider` 失败会让这一行处于「有 sk、记录不全」的状态。
    ///
    /// 那种情况**不虚报**：第 6 步失败会让整条命令返回 `Err`，用户看到的是错误 toast，
    /// 而这一行确实已经有 sk 了 —— 补救手段正是行内那个「刷新」（重新展开一次，
    /// 已有的记录只换 sk、缺的补上）。所以这里如实反映「sk 有没有」就够了，
    /// 不必为它去读六次 DB（这条命令的契约是只读本地、不发网络、首屏别卡）。
    ///
    /// ⚠️ 与 `logged_in` **独立**：登录态过期时这里仍可以是 `true`，那种情况下
    /// 用户的 CLI 照样能用（sk 没失效），不该催他去重新登录。
    pub key_ready: bool,
    /// 这一行名下那六条 provider 记录的 id（六个平台**共用一个**）。
    ///
    /// ## 为什么必须由后端给
    ///
    /// 它是 `sha256(vendor_id + "/" + account_id)` 的前 16 位 hex
    /// （[`crate::vendor::provision::provider_id_for`]）——
    /// **前端算不出来**：没有 `account_id`（有意不给，那是厂商侧的内部 id），
    /// 也没有 sha256。
    ///
    /// 缺了它的后果（Task 6 实现时撞到的）：官网行的**「当前在用」高亮判不了**
    /// —— 前端只能靠「本次会话 provision 过」临时记住那个 id，app 一重启就没了。
    ///
    /// 空串 = 还没登录过（没有 `account_id` 就派生不出 id）。
    pub provider_id: String,
    /// **当前 tab 那个 app** 下，这一行是不是正在用的那个。
    ///
    /// 判据是「`providers` 表里 `app_id` 那一栏的当前项 == 本行的 `provider_id`」——
    /// 由后端在 `vendor_list_accounts` 里按 `app_id` 现算（同 `user_edited` 的时机），
    /// **前端不自己维护**。与中转站档位的 `tier.is_current` 共用同一个事实源
    /// （上游 `ProviderService::current`），所以一个 app 下所有组天然互斥。
    ///
    /// `false` 也可能是还没登录（`provider_id` 为空）—— 未登录的行不可能在用。
    pub is_current: bool,
    /// **当前 tab 那个平台**的配置是不是被用户改过。
    ///
    /// ⚠️ **按平台算，不是整行一个值** —— 一行背后六条 provider 记录各自能被独立
    /// 编辑。这里给的是 `vendor_list_accounts` 收到的那个 `app_id` 对应的那一条。
    ///
    /// `None` = 判不了（没 provision 过 / 这个平台不适用 / 判据本身判不了），
    /// **UI 在 `None` 时不显示标记** —— 同 relay 的 `TierInfo.user_edited`：
    /// 不知道就别断言。
    ///
    /// 判据见 [`user_edited_for`]，它不存标记、靠与默认配置整份比对
    /// （所以用户把配置改回默认，标记会自动消失）。
    pub user_edited: Option<bool>,
}

impl From<creds::VendorRow> for VendorAccountRow {
    fn from(row: creds::VendorRow) -> Self {
        let vendor_name = Vendor::from_id(&row.vendor_id)
            .map(|v| v.display_name().to_string())
            .unwrap_or_else(|| row.vendor_id.clone());
        // 未登录（无 account_id）时派生不出 id —— 给空串，前端据此不显示高亮。
        let provider_id = row
            .account_id
            .as_deref()
            .map(|acct| crate::vendor::provision::provider_id_for(&row.vendor_id, acct))
            .unwrap_or_default();
        VendorAccountRow {
            logged_in: !row.auth_token.is_empty(),
            session_expired: row.account_id.is_some() && row.auth_token.is_empty(),
            key_ready: !row.api_key.is_empty(),
            account_label: row.account_label,
            provider_id,
            vendor_id: row.vendor_id,
            vendor_name,
            id: row.id,
            // 这个 `From` 是纯转换、拿不到 DB。命令层用 `user_edited_for` / `is_current_for`
            // 填它们（要读 provider 记录才算得出来）。
            user_edited: None,
            is_current: false,
        }
    }
}

/// `vendor_provision` 的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorProvisionSummary {
    /// 这个账号的 provider id（六个平台**共用一个**，见 [`provision`] 模块文档）。
    pub provider_id: String,
    /// 实际写成功的平台（kebab-case 的 `app_type`）。
    pub platforms: Vec<String>,
    /// 这一轮有没有真的去官网建了一把新 key。
    ///
    /// `false` = 本地已有明文，零请求就完事（**正常路径**）。前端据此决定要不要
    /// 提示「已在官网新建密钥」—— 每次刷新都提示会让用户以为在重复建 key。
    pub key_created: bool,
}

/// 列出已添加的官网账号。
///
/// **契约：只读本地、不发网络**（与 `relay_list_relays` 一致）—— 首屏不能卡在
/// 网络上。余额走 [`vendor_balance`]，由前端渲染完再异步填。
///
/// ## `app_id` 是干什么的
///
/// **只用来算 `user_edited`** —— 一行官网账号背后是六条 provider 记录（六个平台），
/// 各自能被用户独立编辑，所以「改过没有」这件事**必须按平台问**。
///
/// ⚠️ 别把它理解成「按 app 过滤行」：一把 sk 展开到全部平台，一行在哪些 tab 出现
/// 是纯展示判断，仍然由前端 `VENDOR_APPS` 决定（那条注释还成立）。
#[tauri::command]
pub async fn vendor_list_accounts(
    state: State<'_, AppState>,
    app_id: String,
) -> Result<Vec<VendorAccountRow>, String> {
    // 认不出的 app_id 不该让整条列表失败 —— 首屏契约是「只读本地、不卡」。
    // 那种情况下 `user_edited` 全给 `None`、`is_current` 全给 `false`（判不了就别断言）。
    let app_type: Option<AppType> = app_id.parse().ok();
    with_conn(state.inner(), creds::list)
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    let mut out = VendorAccountRow::from(row);
                    if let Some(app) = app_type.as_ref() {
                        out.user_edited = user_edited_for(state.inner(), &out, app);
                        out.is_current = is_current_for(state.inner(), &out, app);
                    }
                    out
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// 这一行在 `app_type` 这个平台上的配置**是不是被用户改过**。
///
/// `None` = 判不了：还没 provision（没有 provider 记录）、这个平台不适用
/// （`config_for` 返回 `None`，如 gemini），或 `is_user_edited` 自己判不了。
/// UI 在 `None` 时不显示标记 —— 与 relay 的 `TierInfo.user_edited` 同一条原则：
/// **不知道就别断言**。
///
/// ## ⚠️ `roles` 必须与生成配置时用的完全一致
///
/// 两边都取 [`crate::vendor::provision::claude_roles_for`]。不一致 ⇒ 基准里缺
/// fable / subagent 两个键 ⇒ 整份比对失配 ⇒ **每个 Claude 档位都误报「已手工维护」**。
/// 那个函数的文档写了完整理由。
fn user_edited_for(state: &AppState, row: &VendorAccountRow, app_type: &AppType) -> Option<bool> {
    if row.provider_id.is_empty() {
        return None; // 还没登录过，派生不出 provider id。
    }
    // 读存库标记 ——「已手工维护」的唯一来源（编辑页置位、恢复默认复位）。
    // 读失败返回 None（不知道就别断言，同原语义）。
    state
        .db
        .get_user_edited(app_type.as_str(), &row.provider_id)
        .ok()
}

/// 这一行在 `app_type` 这个平台下**是不是正在用的那个**。
///
/// 判据与中转站档位的 `is_current` **同源**：`providers` 表里该 `app_type` 的当前项
/// （上游 `ProviderService::current`）== 本行的 `provider_id`。所以「DeepSeek 官方组」
/// 与「中转站档位 / 手工 provider」共享同一份互斥，一个 app 下永远只有一个在用。
///
/// ⚠️ **`provider_id` 为空时必须返回 `false`**：未登录的行派生不出 id（给空串），
/// 而 `ProviderService::current` 在无当前项时也返回空串 —— 不守卫会让「从未登录」
/// 的行被误判成当前项（空 == 空）。
fn is_current_for(state: &AppState, row: &VendorAccountRow, app_type: &AppType) -> bool {
    if row.provider_id.is_empty() {
        return false;
    }
    ProviderService::current(state, app_type.clone())
        .map(|current| current == row.provider_id)
        .unwrap_or(false)
}

/// 开登录窗，等凭据回来，存成一行账号。
///
/// 返回 `true` = 拿到凭据并已入库；`false` = 用户关窗或超时（**都不是错误**）。
///
/// ## 为什么不像 relay 那样先建行再登录
///
/// 那边要先让用户输域名（一行 = 一个站），所以行先于登录存在。这边域名是编译期常量，
/// 「一行」的唯一身份是 `account_id` —— 而它只有登录后才知道。所以行是登录成功后
/// 由 `save_account` 建的（同 `(vendor_id, account_id)` 已存在则更新，天然幂等）。
#[tauri::command]
pub async fn vendor_open_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    vendor_id: String,
) -> Result<bool, String> {
    let vendor = Vendor::from_id(&vendor_id).ok_or_else(|| format!("不认识的厂商：{vendor_id}"))?;
    // 预填值：这个厂商下**最近一次**登录过的那个标识。
    //
    // 取「任一行」而不是让前端指定：用户点的是「添加账号」，此刻还不知道他要登哪个。
    // 填错了他自己会改（脚本那边 `if (el.value)` 也不覆盖用户已输的内容），
    // 而绝大多数人只有一个账号 —— 少输一次手机号是净收益。
    let login_hint = with_conn(state.inner(), creds::list)
        .map_err(|e| e.to_string())?
        .into_iter()
        .rev()
        .find(|r| r.vendor_id == vendor_id && !r.login_identifier.is_empty())
        .map(|r| r.login_identifier)
        .unwrap_or_default();

    do_login(&app, vendor, &login_hint)
        .await
        .map_err(|e| e.to_string())
}

async fn do_login(
    app_handle: &tauri::AppHandle,
    vendor: Vendor,
    login_hint: &str,
) -> Result<bool, AppError> {
    let url = url::Url::parse(deepseek::LOGIN_URL)
        .map_err(|e| AppError::Config(format!("登录页地址不对: {e}")))?;

    // 已经有一个登录窗时：**销毁它再开新的**，而不是聚焦了就早退。
    // 残留窗口可能是隐藏状态，而 `set_focus` 对不可见窗口是 no-op ⇒ 用户点了登录
    // 什么都没发生，且 label 被占，再点多少次都一样（照 `relay::do_login` 那段）。
    if let Some(stale) = app_handle.get_webview_window(deepseek::LOGIN_WINDOW_LABEL) {
        log::info!("发现残留的官网登录窗口，销毁后重开");
        let _ = stale.destroy();
    }

    // 凭据经这个 channel 从导航回调回到本函数。容量 1：只需要第一份。
    let (tx, mut rx) = tokio::sync::mpsc::channel::<deepseek::VendorCreds>(1);
    // 用户自己关窗的信号。没有它就只能干等满超时。
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::channel::<()>(1);

    let handle_for_nav = app_handle.clone();
    let window = tauri::WebviewWindowBuilder::new(
        app_handle,
        deepseek::LOGIN_WINDOW_LABEL,
        tauri::WebviewUrl::External(url),
    )
    .title(format!("登录 {}", vendor.display_name()))
    .inner_size(480.0, 720.0)
    .resizable(true)
    // ⚠️ **每次登录都必须是全新的登录态**。理由与 `relay::do_login` 那处逐条相同
    // （详见那段长注释）：不加的话 WebView 用的是全 app 共享的**持久** profile，
    // 于是「删掉账号 → 重新添加」会被官网的 SPA 认成已登录直接跳走 ⇒
    // **同一个厂商永远只能挂第一个登录过的账号**，而多账号正是本表唯一索引
    // `(vendor_id, account_id)` 特意支持的能力。
    .incognito(true)
    .user_agent(crate::relay::login::WEBVIEW_USER_AGENT)
    .initialization_script(deepseek::login_script(login_hint))
    .on_navigation(move |url| {
        match deepseek::parse_creds_navigation(url) {
            // 普通导航，放行。
            None => true,
            Some(Ok(creds)) => {
                // try_send：这个回调不能 await，而我们只要第一份凭据。
                let _ = tx.try_send(creds);
                false
            }
            Some(Err(e)) => {
                log::warn!("官网凭据回传解析失败: {e}");
                let _ = handle_for_nav.emit(VENDOR_LOGIN_ERROR, e.to_string());
                false
            }
        }
    })
    .build()
    .map_err(|e| AppError::Config(format!("打开登录窗口失败: {e}")))?;

    // 只认 `Destroyed`（窗口真的没了）而不是 `CloseRequested`（可被拦下的关闭请求）——
    // 后者在某些平台上会先于实际销毁触发，甚至可能被取消。
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = closed_tx.try_send(());
        }
    });

    let outcome = tokio::time::timeout(LOGIN_TIMEOUT, async {
        tokio::select! {
            creds = rx.recv() => creds,
            _ = closed_rx.recv() => None,
        }
    })
    .await;

    let Ok(Some(creds)) = outcome else {
        // 用户关掉了窗口，或超时。都不是错误。
        return Ok(false);
    };

    // `account_id` 已经由 `parse_creds_navigation` 保证非空 —— 「账号身份拿不到就不存
    // token」那条语义在解析那一步就落实了，走到这里的凭据一定是完整的。
    let token = creds.auth_token.clone();
    let account = crate::vendor::VendorAccount::from(creds);

    let state = app_handle.state::<AppState>();
    with_conn(&state, |conn| {
        creds::save_account(conn, vendor, &token, &account)
    })?;

    // **不关窗**：用户拿到凭据的那一刻页面往往刚跳到控制台 —— 上面有余额、充值入口，
    // 都是他接着要用的东西。把窗口关掉等于替他决定「你看完了」。
    // 改成把标题写清楚 + 页面上浮一条提示，窗口留给用户自己关。
    let _ = window.set_title(&format!("已连接 {} — 可关闭此窗口", vendor.display_name()));
    let _ = window.eval(crate::relay::login::CONNECTED_BANNER_JS);

    Ok(true)
}

/// 备好这个账号的密钥，并展开成六个平台的 provider 记录。
///
/// **六步，顺序不能变**（见 [`provision`] 模块文档的「删了才建」）：
///
/// 1. 取行 + device-id
/// 2. 本地已有明文 ⇒ 直接跳到第 6 步（**零请求，这是正常路径**）
/// 3. 拉列表 → 筛出本机上次留下的 → 逐个删（⚠️ 删失败不阻断）
/// 4. 建新的
/// 5. 校验是明文 → 落库
/// 6. 展开六个平台
/// ⚠️ **没有 `app: AppHandle` 参数**：这条路上唯一要用它的地方本来是刷新 live config，
/// 而 `ProviderService::switch` 收的是 `&AppState`（不是 AppHandle）⇒ 那个参数会是死的。
#[tauri::command]
pub async fn vendor_provision(
    state: State<'_, AppState>,
    row_id: i64,
) -> Result<VendorProvisionSummary, String> {
    provision_impl(state.inner(), row_id)
        .await
        .map_err(|e| e.to_string())
}

async fn provision_impl(state: &AppState, row_id: i64) -> Result<VendorProvisionSummary, AppError> {
    // ── 1. 取行 + 账号 id ───────────────────────────────────────────
    let row = with_conn(state, |conn| {
        creds::get(conn, row_id)?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {row_id} 的官网账号")))
    })?;

    let vendor = Vendor::from_id(&row.vendor_id)
        .ok_or_else(|| AppError::Config(format!("不认识的厂商：{}", row.vendor_id)))?;

    // ⚠️ **`account_id` 在这里就要取出并要求非空**（不是等到第 6 步）——
    // Key 名字（第 4 步，`key_name_for(account_id)`）与要删的旧 Key（第 3 步）
    // 都按它算。为 `None` 时报错而不回落的完整推理见第 6 步上方那段注释。
    let account_id = row.account_id.clone().ok_or_else(|| {
        AppError::Config("这个账号还没有完成登录（缺账号标识），请先重新登录再获取密钥".to_string())
    })?;

    // ── 2. 本地已有明文 ⇒ 零请求（正常路径）────────────────────────
    let mut key_created = false;
    let api_key = if !row.api_key.is_empty() {
        row.api_key.clone()
    } else {
        if row.auth_token.is_empty() {
            return Err(VendorError::AuthExpired.into());
        }

        // ── 3. 删本机上次留下的（删了才建）──────────────────────────
        //
        // ⚠️ 两处失败都只 warn 不 return：**删是清理、建是目的**。
        // 最坏情况是官网多一把废 key（下次 provision 的这一步正好删掉它），
        // 而阻断的话用户拿不到任何可用密钥。
        match deepseek::list_keys(&row.auth_token).await {
            Ok(all) => {
                for stale in provision::keys_to_delete(&all, &account_id) {
                    if let Err(e) = deepseek::delete_key(&row.auth_token, &stale).await {
                        log::warn!("清理旧密钥 {} 失败（不阻断建新的）: {e:?}", stale.name);
                    }
                }
            }
            Err(e) => {
                on_vendor_error(state, row_id, &e);
                log::warn!("拉取密钥列表失败，跳过清理: {e:?}");
                // 登录态失效就没必要继续了 —— 建 key 也会拿同一个 40002。
                if matches!(e, VendorError::AuthExpired) {
                    return Err(e.into());
                }
            }
        }

        // ── 4 + 5. 建新的 → 校验明文（在 `create_key` 里）→ 落库 ────
        let plaintext =
            match deepseek::create_key(&row.auth_token, &crate::vendor::key_name_for(&account_id))
                .await
            {
                Ok(k) => k,
                Err(e) => {
                    on_vendor_error(state, row_id, &e);
                    return Err(e.into());
                }
            };
        key_created = true;

        with_conn(state, |conn| creds::set_api_key(conn, row_id, &plaintext)).map_err(|e| {
            // 最坏情况：官网多了一把、本地没记住。必须告诉用户重试会自愈
            // （下一轮的第 3 步正好把它删掉），否则他会以为要去官网手工清理。
            AppError::Database(format!(
                "{e}（已在官网建了一把密钥但本地保存失败，重试会清理它）"
            ))
        })?;
        plaintext
    };

    // ── 6. 展开六个平台 ─────────────────────────────────────────────
    //
    // ⚠️ **`account_id` 为 `None` 时必须报错，不能回落**（final review I-4 抓出）。
    //
    // 初版用 `unwrap_or_else(|| "anon")`，论证是「这一步一定是 `Some`（登录才建得出行），
    // 回落只是不想为不可达的分支报错」。但 [`remove_impl`] 对同一个「不可达」状态
    // 给的是**另一种**处理 —— `if let (Some(_), Some(account_id))` 直接**跳过整个
    // provider 清理**。两处口径不一致的后果：
    //
    // 1. provision 用 `"anon"` 派生出一个 id，建了六条 provider 记录；
    // 2. 用户删这个账号 ⇒ remove 认不出它们（`None` 那条路径压根不去删）；
    // 3. 它们命中 `MANAGED_ID_PREFIX` ⇒ 用户从 provider 列表里也删不掉
    //    （`delete_provider` 的 `reject_if_managed` 拦下），而前端还把托管项过滤掉了
    //    ⇒ **六条记录永久留在库里，完全不可见、不可删。**
    //
    // 与 spec §3.1 那条硬语义同一精神（「账号身份拿不到 ⇒ 不存 token，
    // 否则造出死局」）：这里也报错。两处口径统一成「`None` ⇒ Err」之后，
    // 那个死局在结构上就不存在了。
    // （`account_id` 已在第 1 步取出并要求非空 —— Key 名字也按它算。）
    let provider_id = provision::provider_id_for(vendor.vendor_id(), &account_id);

    let mut platforms = Vec::new();
    for (idx, (app_type, defaults)) in provision::provider_rows_for(vendor, &api_key)
        .into_iter()
        .enumerate()
    {
        // ⚠️ **已存在的记录只换 sk，不覆盖用户的编辑**：`save_provider` 是全量覆盖
        // `settings_config` 的，照写默认配置会把用户改过的模型名 / 自定义端点全冲掉 ——
        // 而他点「获取密钥」通常只是想刷新一下（照 `relay::provision_impl` 那段）。
        let existing = state
            .db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .ok()
            .flatten();

        let settings_config = match existing {
            Some(old) => {
                let mut kept = old.settings_config;
                // patch 失败（形状被改坏 / 该放 sk 的 section 没了）⇒ 回落默认配置。
                // 否则用户会留着一把旧 sk 却以为刷新成功了。
                if crate::relay::provision::patch_api_key(&mut kept, &app_type, &api_key) {
                    kept
                } else {
                    log::warn!(
                        "{} 的官网配置里找不到放密钥的位置，已重置为默认配置",
                        app_type.as_str()
                    );
                    defaults
                }
            }
            None => defaults,
        };

        let provider = Provider {
            id: provider_id.clone(),
            name: vendor.display_name().to_string(),
            settings_config,
            website_url: Some(deepseek::SITE_ORIGIN.to_string()),
            // ⚠️ `cn_official` —— **不是** `aggregator`（那是中转站的），
            // 更**绝不能**是 `official`：那条分类会触发一批只对官方订阅成立的逻辑
            // （stale auth 清理、统一会话桶注入）。
            category: Some("cn_official".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: Some(idx),
            notes: None,
            meta: Some(vendor_meta(&app_type, Some(account_id.clone()))),
            icon: Some("deepseek".to_string()),
            icon_color: Some("#1E88E5".to_string()),
            in_failover_queue: false,
        };

        state
            .db
            .save_provider(app_type.as_str(), &provider)
            .map_err(|e| AppError::Database(format!("保存 {} 配置失败: {e}", app_type.as_str())))?;

        platforms.push(app_type.as_str().to_string());
    }

    // 写完六条之后刷一次 live config：不刷的话「当前就是这条 provider」的用户拿到的
    // 仍是旧 sk（CLI 读的是落地文件，不是数据库）。失败只 warn —— 记录已经存对了，
    // 用户手工切一次就能生效，不该因为快照写不下去而报「备密钥失败」。
    //
    // 走 `sync_current_provider_for_app` 而不是 `switch`：这条路不是**切换**当前项
    // （它本来就是当前项），只是让落地配置追上 DB。那个 API 内部已处理代理接管
    // （接管时更新备份而不是覆盖 live 文件），而 `switch` 会多跑一遍切换语义
    // （接管态下走 `hot_switch_provider_inner`）—— 那不是这里要的。
    // `commands::relay` 的两条同型路径也用它，三处一份写法。
    for app_type in provision::DEEPSEEK_APPS {
        let is_current = ProviderService::current(state, app_type.clone())
            .ok()
            .as_deref()
            == Some(provider_id.as_str());
        if !is_current {
            continue;
        }
        if let Err(e) = ProviderService::sync_current_provider_for_app(state, app_type.clone()) {
            log::warn!(
                "刷新 {} 的当前配置失败（记录已保存，切换一次即生效）: {e}",
                app_type.as_str()
            );
        }
    }

    Ok(VendorProvisionSummary {
        provider_id,
        platforms,
        key_created,
    })
}

/// 把**一个平台**的配置恢复成 LoongPort 生成的默认值。**密钥保留不变。**
///
/// ## 为什么不复用 `relay_reset_tier_config`
///
/// 那条路有三段硬依赖中转站模型，vendor 一样都没有（`commands/relay.rs:1456` 起）：
///
/// | 它要什么 | 为什么 vendor 没有 |
/// |---|---|
/// | `existing.website_url` 定站点归属 | vendor 的 base_url 由 `deepseek::config_for` 直接给，不需要反查 |
/// | `creds::list` 找中转站账号 | 那是 relay 的凭据表，vendor 的在 `vendor::creds` |
/// | `meta.loongportAccountId` 认账号 | vendor 一个 provider_id 就唯一确定账号（它是 `sha256(vendor+account)`） |
///
/// 硬塞会把「中转站归属」这套概念带进 vendor 层。所以照它的**形状**写一份短的
/// （含 `is_managed` 那道正向判据），而不是共用它的**实现**。
///
/// ## 只动传入的这一个平台
///
/// 一行背后六条记录，用户点的是「当前 tab 这个平台的恢复」（`user_edited` 也是按平台
/// 算的）。一次恢复六条会把他在别的 tab 里的编辑一起冲掉，而界面上没有任何地方
/// 告诉过他这一点。
#[tauri::command]
pub async fn vendor_reset_tier_config(
    state: State<'_, AppState>,
    provider_id: String,
    app_id: String,
) -> Result<(), String> {
    vendor_reset_tier_config_impl(state.inner(), &provider_id, &app_id).map_err(|e| e.to_string())
}

fn vendor_reset_tier_config_impl(
    state: &AppState,
    provider_id: &str,
    app_id: &str,
) -> Result<(), AppError> {
    // 用正向判据 `is_managed`（照 relay 那条的注释：别拿 `reject_if_managed`
    // 的 Err 反着判 —— 那个函数语义是「撞到托管项就拦下」，这里要的恰好相反）。
    if !crate::relay::is_managed(provider_id) {
        return Err(AppError::Config(
            "只有 LoongPort 托管的档位才能恢复默认配置".into(),
        ));
    }

    let app_type: AppType = app_id.parse()?;
    let (base_url, model) = deepseek::config_for(&app_type).ok_or_else(|| {
        AppError::Config(format!(
            "{app_id} 这个平台没有 DeepSeek 配置，恢复不了默认值"
        ))
    })?;

    let existing = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())
        .map_err(|e| AppError::Database(format!("读取档位失败: {e}")))?
        .ok_or_else(|| AppError::Config("这个档位不存在".into()))?;

    // sk 从现有配置里取（照 relay 那条）。取不到就让用户走「获取密钥」重建 ——
    // 生成一份没有 sk 的「默认配置」比保持现状更糟（那是一条必定 401 的记录）。
    let api_key = crate::relay::provision::extract_api_key(&existing.settings_config, &app_type)
        .ok_or_else(|| {
            AppError::Config("这个档位的配置里读不出密钥了，请用「获取密钥」重新生成它。".into())
        })?;

    // ⚠️ **`roles` 必须与生成时一致**，否则「恢复默认」写出的配置与
    // `user_edited` 的基准不同 ⇒ 恢复完立刻又显示「已手工维护」。
    let defaults = crate::relay::provision::settings_config_with_roles(
        &app_type,
        &api_key,
        &existing.name,
        &base_url,
        &model,
        provision::claude_roles_for(&app_type),
    )
    .ok_or_else(|| AppError::Config(format!("{app_id} 这个平台生成不出默认配置")))?;

    let restored = Provider {
        settings_config: defaults,
        ..existing
    };
    state
        .db
        .save_provider(app_type.as_str(), &restored)
        .map_err(|e| AppError::Database(format!("保存配置失败: {e}")))?;

    // 「恢复默认配置」= 回到 LoongPort 的默认 ⇒ 清掉「已手工维护」标记。
    state
        .db
        .set_user_edited(app_type.as_str(), provider_id, false)
        .map_err(|e| AppError::Database(format!("清除已手工维护标记失败: {e}")))?;

    // 恢复的若正是当前在用的那条，落地文件要跟着走 —— 不刷的话 CLI 读到的仍是
    // 用户改坏的那份，而「恢复默认」恰恰是他在档位坏了时点的按钮。
    // 失败只 warn：DB 已经存对了，手工切一次就能生效（同 provision 结尾那段）。
    // `current` 给的是 id 字符串本身（不是 Provider），照本文件 provision 结尾那处写。
    let is_current = ProviderService::current(state, app_type.clone())
        .ok()
        .as_deref()
        == Some(provider_id);
    if is_current {
        if let Err(e) = ProviderService::sync_current_provider_for_app(state, app_type.clone()) {
            log::warn!("恢复默认配置后刷新 {} 落地配置失败: {e}", app_type.as_str());
        }
    }

    Ok(())
}

/// 查一行的余额。`None` = 拿不到（没有钱包 / 金额解不动）—— **不是显示 0**。
///
/// 返回**已格式化的字符串**（`"¥547.08"`）：币种符号已经在里面，前端不做币种分派、
/// 也不做算术（见 [`crate::vendor::VendorBalance`] 那条「只有一个字段」）。
#[tauri::command]
pub async fn vendor_balance(
    state: State<'_, AppState>,
    row_id: i64,
) -> Result<Option<String>, String> {
    let row = with_conn(state.inner(), |conn| creds::get(conn, row_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("找不到 id 为 {row_id} 的官网账号"))?;

    if row.auth_token.is_empty() {
        return Err(AppError::from(VendorError::AuthExpired).to_string());
    }

    match deepseek::balance(&row.auth_token).await {
        Ok(b) => Ok(b.map(|v| v.0)),
        Err(e) => {
            on_vendor_error(state.inner(), row_id, &e);
            Err(AppError::from(e).to_string())
        }
    }
}

/// 删一行，连带清掉它名下六个平台的 provider 记录。
///
/// ⚠️ **清 provider 要在删行之前** —— 删完就拿不到 `account_id` 了，而 provider id
/// 正是从它派生的。
///
/// ## ⚠️ 名下有 provider 正在被某个平台用着 ⇒ 一条都不删，报错
///
/// 与 `relay::remove_site_impl` 同一条不变量（那边写了完整推理）：删掉一份**还能用**的
/// 当前配置，会让那个 CLI 的落地文件指向一条已经不存在的记录，而用户不会收到任何提示。
///
/// 这条路比 relay 那条更容易撞上：**六个平台共用同一个 `provider_id`**，所以这一行
/// 只要在任何一个平台上被选中，删账号就会毁掉那个平台的当前配置 —— 而官网行的删除按钮
/// 原来连前端那道提示都没有（`VendorRow` 有意不拦，理由是「确认框里写清了会清掉配置」，
/// 但那句话说的是「清掉这个账号的配置」，用户读不出「你 codex 现在就在用它」）。
///
/// 清理失败（`delete_provider` 报错）仍**不阻止删行**：那和「不许删正在用的」是两件事 ——
/// 前者是清理动作出了意外，此时卡住用户没有意义；后者是已知会造成破坏，必须拦。
#[tauri::command]
pub async fn vendor_remove(state: State<'_, AppState>, row_id: i64) -> Result<(), String> {
    remove_impl(state.inner(), row_id).map_err(|e| e.to_string())
}

fn remove_impl(state: &AppState, row_id: i64) -> Result<(), AppError> {
    let row = with_conn(state, |conn| creds::get(conn, row_id))?
        .ok_or_else(|| AppError::Config("这个账号已经不存在了".into()))?;

    if let (Some(vendor), Some(account_id)) = (Vendor::from_id(&row.vendor_id), row.account_id) {
        let provider_id = provision::provider_id_for(vendor.vendor_id(), &account_id);

        // 闸：先扫一遍六个平台，撞上当前项就整条路中止（全有或全无 —— 半删会留下
        // 用户再也处置不了的孤儿记录，见 `relay::remove_site_impl` 那段）。
        let in_use: Vec<&str> = provision::DEEPSEEK_APPS
            .iter()
            .filter(|app_type| {
                ProviderService::current(state, (*app_type).clone())
                    .ok()
                    .as_deref()
                    == Some(provider_id.as_str())
            })
            .map(|app_type| app_type.as_str())
            .collect();
        if !in_use.is_empty() {
            return Err(AppError::Config(format!(
                "这个账号的配置正在被以下平台使用：{}。请先在对应平台切换到别的供应商，再删除这个账号。",
                in_use.join("、")
            )));
        }

        for app_type in provision::DEEPSEEK_APPS {
            // ⚠️ **必须带 app_type**：`provider_id` 不含它（六个平台共用一个 id），
            // 所以要逐个平台删。
            if let Err(e) = state.db.delete_provider(app_type.as_str(), &provider_id) {
                log::warn!(
                    "删除账号时清理 {} 的配置失败（账号仍会删掉）: {e}",
                    app_type.as_str()
                );
            }
        }
    }

    with_conn(state, |conn| creds::remove(conn, row_id))
}

/// 保存官网账号行的手工顺序。`ids` 是拖动后的完整顺序，下标即新的 `sort_index`。
///
/// ⚠️ **只排 vendor 自己那几行** —— 中转站行与官网行不可跨类拖动（spec §6.2 已裁决）：
/// 两类行的 `sort_index` 各自存在自己的表里，本来就没有一个共同的序。
#[tauri::command]
pub async fn vendor_reorder(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    with_conn(state.inner(), |conn| creds::reorder(conn, &ids)).map_err(|e| e.to_string())
}

/// vendor provider 的 `meta`。
///
/// `account_id` 是**归属依据**，不是装饰：一个厂商可以挂多个账号，而 `website_url`
/// 是编译期常量（对所有 DeepSeek 账号都一样）⇒ 少了它，删账号 / 重建配置两处都会
/// 误伤同厂商另一个账号的记录。
fn vendor_meta(app_type: &AppType, account_id: Option<String>) -> crate::provider::ProviderMeta {
    crate::provider::ProviderMeta {
        // `api_format` **只被 `codex_config.rs` 消费**（`CodexCatalogToolProfile::from_api_format`），
        // 对 claude / opencode 无意义 —— 给它们填值不会有人读，反而让人以为那里有语义。
        api_format: match app_type {
            AppType::Codex => Some("openai_responses".to_string()),
            _ => None,
        },
        loongport_vendor_account: account_id,
        ..Default::default()
    }
}

/// 按错误类型落实持久状态。
///
/// ⚠️ **`AuthExpired` 只清 `auth_token`，绝不动 `api_key`**：后者是厂商侧的独立凭据，
/// 网页登录态过期**不影响它**，用户的 CLI 照样能用。清掉等于无端废掉一把好 key
/// （`creds::clear_token` 的实现也把这条钉在那里了）。
///
/// 其余错误类型**什么都不改**：`KeyLimitReached` 是官网侧的状态、`Transient` 可能只是
/// 网断了 —— 为它们清凭据是把一次可重试的失败变成一次真实的凭据丢失。
fn on_vendor_error(state: &AppState, row_id: i64, e: &VendorError) {
    if !matches!(e, VendorError::AuthExpired) {
        return;
    }
    if let Err(err) = with_conn(state, |conn| creds::clear_token(conn, row_id)) {
        log::warn!("清除失效登录态失败: {err}");
    }
}

/// 取数据库连接。
///
/// 自己写这三行而不是把 `commands::relay::with_conn` 改成 `pub`：那会让一个私有
/// 辅助函数变成两个模块之间的接口，而它就是三行。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(auth_token: &str, api_key: &str, account_id: Option<&str>) -> creds::VendorRow {
        creds::VendorRow {
            id: 1,
            vendor_id: "deepseek".into(),
            account_id: account_id.map(str::to_string),
            account_label: "13800000000".into(),
            login_identifier: "13800000000".into(),
            auth_token: auth_token.into(),
            api_key: api_key.into(),
            sort_index: 0,
        }
    }

    // ─────────────── DTO：三个状态位互相独立 ───────────────

    #[test]
    fn a_logged_in_row_is_not_reported_as_expired() {
        let dto = VendorAccountRow::from(row("tok", "", Some("uuid-a")));
        assert!(dto.logged_in);
        assert!(!dto.session_expired);
        assert!(!dto.key_ready);
        assert_eq!(dto.vendor_name, "DeepSeek");
    }

    /// 「从没登录」与「登录过但过期」对用户是两种处境，不能混成 `!logged_in`。
    #[test]
    fn expiry_requires_having_logged_in_before() {
        let expired = VendorAccountRow::from(row("", "", Some("uuid-a")));
        assert!(!expired.logged_in);
        assert!(expired.session_expired, "有 account_id + 空 token = 过期");

        let never = VendorAccountRow::from(row("", "", None));
        assert!(
            !never.session_expired,
            "从没登录过不是「过期」—— 前端那两种处境要给不同的按钮"
        );
    }

    /// 登录态过期时 sk 仍然好用 —— 这两个状态位必须独立，否则会催用户去做多余的事。
    #[test]
    fn a_usable_key_survives_an_expired_session() {
        let dto = VendorAccountRow::from(row("", "sk-plaintext", Some("uuid-a")));
        assert!(dto.session_expired);
        assert!(
            dto.key_ready,
            "sk 是独立凭据，网页登录态过期不影响它 —— UI 不该显示成「没配好」"
        );
    }

    /// 凭据不出 Rust 侧：DTO 序列化后不能带上 token 或明文 sk。
    #[test]
    fn the_dto_never_carries_credentials() {
        let dto = VendorAccountRow::from(row("tok-secret", "sk-secret", Some("uuid-a")));
        let json = serde_json::to_string(&dto).expect("序列化");
        assert!(!json.contains("tok-secret"), "登录态不出 Rust 侧：{json}");
        assert!(!json.contains("sk-secret"), "明文 sk 不出 Rust 侧：{json}");
    }

    #[test]
    fn an_unknown_vendor_id_falls_back_to_itself_as_the_name() {
        let mut r = row("tok", "", Some("uuid-a"));
        r.vendor_id = "kimi".into();
        let dto = VendorAccountRow::from(r);
        assert_eq!(dto.vendor_name, "kimi", "认不出也要有个名字显示，不能空着");
    }

    // ─────────────── 当前在用：与中转站档位同源互斥 ───────────────

    /// DeepSeek 行的「在用」必须与 `providers.is_current` 同源 —— 只有那样它才与
    /// 中转站档位、手工 provider 一起互斥，一个 app 下只亮一个。
    #[test]
    fn is_current_tracks_the_providers_current_of_the_app() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());

        let in_use = VendorAccountRow::from(row("tok", "sk", Some("uuid-a")));
        let idle = VendorAccountRow::from(row("tok", "sk", Some("uuid-b")));

        db.save_provider(
            "claude",
            &crate::provider::Provider {
                id: in_use.provider_id.clone(),
                name: "DeepSeek".into(),
                settings_config: serde_json::json!({}),
                website_url: Some("https://platform.deepseek.com".into()),
                category: Some("cn_official".into()),
                created_at: None,
                sort_index: Some(0),
                notes: None,
                meta: None,
                icon: Some("deepseek".into()),
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .expect("save provider");
        db.set_current_provider("claude", &in_use.provider_id)
            .expect("set current");

        assert!(is_current_for(&state, &in_use, &AppType::Claude));
        assert!(!is_current_for(&state, &idle, &AppType::Claude));
    }

    /// 未登录的行（provider_id 为空）绝不能被判成当前项 ——
    /// `ProviderService::current` 在无当前项时返回空串，空 == 空 会误判。
    #[test]
    fn an_unlogged_row_is_never_current() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let never = VendorAccountRow::from(row("", "", None));
        assert!(never.provider_id.is_empty());
        assert!(!is_current_for(&state, &never, &AppType::Claude));
    }

    // ─────────────── meta ───────────────

    #[test]
    fn only_codex_gets_an_api_format() {
        let codex = vendor_meta(&AppType::Codex, Some("uuid-a".into()));
        assert_eq!(codex.api_format.as_deref(), Some("openai_responses"));
        for app in [AppType::Claude, AppType::OpenCode, AppType::Hermes] {
            assert!(
                vendor_meta(&app, None).api_format.is_none(),
                "{app:?} 不消费 api_format，填值只会让人以为那里有语义"
            );
        }
    }

    /// UUID 装不进 `loongport_account_id`（那是 `i64`）—— 这条钉住新字段的类型。
    #[test]
    fn the_vendor_account_field_holds_a_uuid() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        let meta = vendor_meta(&AppType::Claude, Some(uuid.to_string()));
        assert_eq!(meta.loongport_vendor_account.as_deref(), Some(uuid));
        assert!(
            meta.loongport_account_id.is_none(),
            "那是中转站的 i64 字段，不该被 vendor 占用"
        );

        let json = serde_json::to_string(&meta).expect("序列化");
        assert!(
            json.contains("loongportVendorAccount"),
            "前端读的是 camelCase 那个名字：{json}"
        );
    }

    #[test]
    fn meta_omits_the_vendor_field_when_there_is_no_account() {
        let json = serde_json::to_string(&vendor_meta(&AppType::Claude, None)).expect("序列化");
        assert!(
            !json.contains("loongportVendorAccount"),
            "None 时整个字段不该出现（skip_serializing_if）：{json}"
        );
    }

    // ─────────────── 错误映射 ───────────────

    #[test]
    fn auth_expired_maps_to_a_user_visible_message() {
        let e: AppError = VendorError::AuthExpired.into();
        assert!(format!("{e}").contains("登录"), "要给用户看得懂的话");
    }

    #[test]
    fn key_limit_message_points_to_the_official_site() {
        let e: AppError = VendorError::KeyLimitReached.into();
        let msg = format!("{e}");
        assert!(msg.contains("100"), "要报出上限数字");
        assert!(msg.contains("官网"), "要指路，不能只说不允许");
    }

    // ─────────────── 状态机：只有 40002 清 token ───────────────

    fn in_memory_state() -> (AppState, i64) {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("内存库"));
        let state = AppState::new(db);
        let id = with_conn(&state, |conn| {
            let id = creds::save_account(
                conn,
                Vendor::DeepSeek,
                "tok",
                &crate::vendor::VendorAccount {
                    account_id: "uuid-a".into(),
                    label: "13800000000".into(),
                    login_identifier: "13800000000".into(),
                },
            )?;
            creds::set_api_key(conn, id, "sk-plaintext")?;
            Ok(id)
        })
        .expect("准备数据");
        (state, id)
    }

    #[test]
    fn auth_expired_clears_the_token_but_keeps_the_key() {
        let (state, id) = in_memory_state();
        on_vendor_error(&state, id, &VendorError::AuthExpired);

        let row = with_conn(&state, |conn| creds::get(conn, id))
            .expect("读")
            .expect("有");
        assert!(row.auth_token.is_empty(), "40002 要清掉失效的登录态");
        assert_eq!(
            row.api_key, "sk-plaintext",
            "⚠️ api_key 是独立凭据，网页登录态过期不影响它 —— 清掉等于无端废掉一把好 key"
        );
    }

    #[test]
    fn other_errors_touch_nothing() {
        let (state, id) = in_memory_state();
        for e in [
            VendorError::KeyLimitReached,
            VendorError::RedactedValueReturned,
            VendorError::Transient("网断了".into()),
        ] {
            on_vendor_error(&state, id, &e);
            let row = with_conn(&state, |conn| creds::get(conn, id))
                .expect("读")
                .expect("有");
            assert_eq!(
                row.auth_token, "tok",
                "{e:?} 是可重试的失败，为它清凭据是把它变成一次真实的凭据丢失"
            );
            assert_eq!(row.api_key, "sk-plaintext");
        }
    }

    // ─────────────── 与 relay 的清理路径隔离 ───────────────

    /// vendor 的 provider 命中 `MANAGED_ID_PREFIX`（守卫要继承），但**不能**被
    /// relay 的 `prune_stale_tiers` 当成某个站的档位删掉。
    ///
    /// 当前的隔离靠 `website_url` 不相等（`platform.deepseek.com` 永不等于任何 sub2api
    /// origin）—— 那是**巧合不是设计**，所以要有闸钉住：改了 `SITE_ORIGIN` 就得重新
    /// 评估 prune 路径。
    /// 行 DTO 必须带上那六条 provider 记录的 id —— 前端算不出来
    /// （`sha256(vendor_id + "/" + account_id)`，而 DTO 有意不给 account_id）。
    ///
    /// 缺了它官网行的「当前在用」高亮判不了（app 重启后前端手上没有那个 id）。
    #[test]
    fn row_dto_carries_the_derived_provider_id() {
        let row = creds::VendorRow {
            id: 1,
            vendor_id: "deepseek".to_string(),
            account_id: Some("uuid-a".to_string()),
            account_label: "手机号".to_string(),
            login_identifier: "138".to_string(),
            auth_token: "tok".to_string(),
            api_key: "sk-x".to_string(),
            sort_index: 0,
        };
        let dto = VendorAccountRow::from(row);
        assert_eq!(
            dto.provider_id,
            crate::vendor::provision::provider_id_for("deepseek", "uuid-a"),
            "必须与 provision 算出的 id 一致 —— 不一致则前端比不出当前态"
        );
        assert!(
            crate::relay::is_managed(&dto.provider_id),
            "顺带钉住它仍命中托管前缀"
        );
    }

    /// 未登录的行没有 account_id ⇒ 派生不出 id ⇒ 给空串（不是伪造一个）。
    #[test]
    fn a_row_without_an_account_has_no_provider_id() {
        let row = creds::VendorRow {
            id: 1,
            vendor_id: "deepseek".to_string(),
            account_id: None,
            account_label: String::new(),
            login_identifier: String::new(),
            auth_token: String::new(),
            api_key: String::new(),
            sort_index: 0,
        };
        assert_eq!(VendorAccountRow::from(row).provider_id, "");
    }

    #[test]
    fn vendor_provider_website_url_never_matches_a_sub2api_origin() {
        let id = provision::provider_id_for("deepseek", "uuid-a");
        assert!(crate::relay::managed::is_managed(&id), "守卫要认它");
        assert_eq!(
            deepseek::SITE_ORIGIN,
            "https://platform.deepseek.com",
            "隔离依赖这个值与中转站 origin 不同 —— 改它要重新评估 prune 路径"
        );
    }

    /// 事件名是跨语言契约（前端 `useTauriEvent` 写同一个字符串）。
    #[test]
    fn the_login_error_event_name_is_stable() {
        assert_eq!(VENDOR_LOGIN_ERROR, "vendor-login-error");
        assert_ne!(
            VENDOR_LOGIN_ERROR, "relay-login-error",
            "两个登录窗各发自己的事件，撞名会让中转站的弹窗显示官网的错"
        );
    }

    // ──────────── final review I-1 要求补的四条闸（2026-08-04）────────────
    //
    // 这四条守的都是「已实现且人工核过」的行为。补它们不是因为怀疑实现，
    // 而是因为**回归时这些行为的失效都是静默的** —— 按全局规则的 defer 准入闸，
    // 现在做得了就现在做（「花成本 / 非阻塞」都不是 defer 的理由）。

    fn mem_state() -> AppState {
        AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().expect("init db"),
        ))
    }

    fn seed_vendor_row(state: &AppState, account_id: Option<&str>) -> i64 {
        with_conn(state, |conn| {
            match account_id {
                Some(acct) => creds::save_account(
                    conn,
                    Vendor::DeepSeek,
                    "tok",
                    &crate::vendor::VendorAccount {
                        account_id: acct.to_string(),
                        label: "手机号".into(),
                        login_identifier: "138".into(),
                    },
                ),
                // 无 account_id 的行：直接插，`save_account` 造不出这种行
                // （它必带 account_id）—— 而我们要测的正是这种「理论上不可达」的状态。
                None => {
                    conn.execute(
                        "INSERT INTO loongport_vendor (vendor_id, account_id, auth_token, api_key)
                         VALUES ('deepseek', NULL, 'tok', 'sk-x')",
                        [],
                    )
                    .map_err(|e| AppError::Database(e.to_string()))?;
                    Ok(conn.last_insert_rowid())
                }
            }
        })
        .expect("插行")
    }

    /// ⭐ **删一个官网账号，必须把它名下**六个平台**的 provider 记录全清掉。**
    ///
    /// `provider_id` 不含 `app_type`（六个平台共用一个 id），所以清理必须逐平台删 ——
    /// 少了那个循环，症状是「账号删了、六条配置还在」，而它们命中
    /// `MANAGED_ID_PREFIX` ⇒ 用户从 provider 列表里也删不掉（守卫拦下）、
    /// 前端还把托管项过滤掉了 ⇒ **不可见、不可删的孤儿记录**。
    #[test]
    fn deleting_a_vendor_account_removes_all_six_rows() {
        let state = mem_state();
        let row_id = seed_vendor_row(&state, Some("uuid-a"));
        let provider_id = provision::provider_id_for("deepseek", "uuid-a");

        // 六个平台各种一条（形状与 provision 落库的那条一致即可）。
        for app_type in provision::DEEPSEEK_APPS {
            let p = Provider {
                id: provider_id.clone(),
                name: "DeepSeek".into(),
                settings_config: serde_json::json!({"env": {}}),
                website_url: Some(deepseek::SITE_ORIGIN.to_string()),
                category: Some("cn_official".into()),
                created_at: Some(0),
                sort_index: Some(0),
                notes: None,
                meta: None,
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            };
            state
                .db
                .save_provider(app_type.as_str(), &p)
                .expect("种 provider");
        }
        // 前提断言：六条真的在（否则这条闸没有判别力）。
        for app_type in provision::DEEPSEEK_APPS {
            assert!(
                state
                    .db
                    .get_provider_by_id(&provider_id, app_type.as_str())
                    .expect("查")
                    .is_some(),
                "{} 的记录应当先存在",
                app_type.as_str()
            );
        }

        remove_impl(&state, row_id).expect("删账号");

        for app_type in provision::DEEPSEEK_APPS {
            assert!(
                state
                    .db
                    .get_provider_by_id(&provider_id, app_type.as_str())
                    .expect("查")
                    .is_none(),
                "{} 的记录没被清掉 —— 会变成不可见不可删的孤儿",
                app_type.as_str()
            );
        }
    }

    /// ⭐ **`account_id` 为 `None` 的行不许 provision**（review I-4）。
    ///
    /// 初版回落成 `"anon"` 建六条记录，而 `remove_impl` 对 `None` 直接跳过清理
    /// ⇒ 两处口径不一致就会留下永久孤儿。统一成「报错」之后那个死局不存在。
    #[test]
    fn a_row_without_an_account_id_cannot_be_provisioned() {
        let state = mem_state();
        let row_id = seed_vendor_row(&state, None);

        let err = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt")
            .block_on(provision_impl(&state, row_id))
            .expect_err("必须报错，不能回落成 anon");

        let msg = err.to_string();
        assert!(
            msg.contains("登录"),
            "错误信息要指路（重新登录），实际: {msg}"
        );
        // 且**没有**建出任何 provider 记录。
        let anon_id = provision::provider_id_for("deepseek", "anon");
        for app_type in provision::DEEPSEEK_APPS {
            assert!(
                state
                    .db
                    .get_provider_by_id(&anon_id, app_type.as_str())
                    .expect("查")
                    .is_none(),
                "不该为 anon 建 {} 的记录",
                app_type.as_str()
            );
        }
    }

    /// ⭐ **删官网账号不许毁掉正在用的配置** —— 与 `relay::remove_site_impl` 同一条不变量。
    ///
    /// 这条路比 relay 那条更容易撞上：**六个平台共用同一个 `provider_id`**，所以这一行
    /// 只要在任何一个平台上被选中，删账号就会清掉那个平台的当前配置。而官网行原来连前端
    /// 那道提示都没有（`VendorRow` 有意不拦）⇒ 后端是唯一的闸。
    ///
    /// **会红的改法**：去掉 `remove_impl` 里那段 `in_use` 检查。
    #[test]
    fn removing_a_vendor_account_is_refused_while_a_platform_still_uses_it() {
        let state = mem_state();
        let row_id = seed_vendor_row(&state, Some("uuid-a"));
        let provider_id = provision::provider_id_for("deepseek", "uuid-a");

        // 六个平台各一条记录（`provision_impl` 就是这么写的），并把 codex 那个设成当前项。
        for app_type in provision::DEEPSEEK_APPS {
            let mut p = crate::provider::Provider::with_id(
                provider_id.clone(),
                "DeepSeek".to_string(),
                serde_json::json!({ "env": {} }),
                None,
            );
            p.website_url = Some("https://platform.deepseek.com".to_string());
            state
                .db
                .save_provider(app_type.as_str(), &p)
                .expect("seed provider");
        }
        state
            .db
            .set_current_provider("codex", &provider_id)
            .expect("set current");

        let err = remove_impl(&state, row_id).expect_err("⭐ 有平台在用它时，删除必须失败");
        let msg = err.to_string();
        assert!(
            msg.contains("codex"),
            "文案要点名哪个平台在用（用户得去那里切走），实际：{msg}"
        );

        // 全有或全无：六条记录一条都不能少，账号行也必须还在。
        for app_type in provision::DEEPSEEK_APPS {
            assert!(
                state
                    .db
                    .get_provider_by_id(&provider_id, app_type.as_str())
                    .expect("查")
                    .is_some(),
                "被拦下时 {} 的记录必须完好 —— 半删会留下用户处置不了的孤儿",
                app_type.as_str()
            );
        }
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("查行")
                .is_some(),
            "配置没删掉，账号行也不该删"
        );
    }

    /// 反面：没有平台在用它时，删除照常（连带清掉六条记录）。
    ///
    /// 与上一条成对 —— 只有上一条的话，把闸写成「无条件拒绝」也能过。
    #[test]
    fn removing_a_vendor_account_still_works_when_no_platform_uses_it() {
        let state = mem_state();
        let row_id = seed_vendor_row(&state, Some("uuid-a"));
        let provider_id = provision::provider_id_for("deepseek", "uuid-a");

        for app_type in provision::DEEPSEEK_APPS {
            let p = crate::provider::Provider::with_id(
                provider_id.clone(),
                "DeepSeek".to_string(),
                serde_json::json!({ "env": {} }),
                None,
            );
            state
                .db
                .save_provider(app_type.as_str(), &p)
                .expect("seed provider");
        }
        // **不设 current**。

        remove_impl(&state, row_id).expect("没人在用它时删除该成功");

        for app_type in provision::DEEPSEEK_APPS {
            assert!(
                state
                    .db
                    .get_provider_by_id(&provider_id, app_type.as_str())
                    .expect("查")
                    .is_none(),
                "{} 的记录该被连带清掉",
                app_type.as_str()
            );
        }
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("查行")
                .is_none(),
            "账号行该被删掉"
        );
    }
}
