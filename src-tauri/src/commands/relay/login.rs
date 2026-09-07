//! 站点导入（浏览器发现协议）与登录 WebView 全流程，以及登录态/凭据的生命周期。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::login;

/// 等用户走完登录流程的上限（秒）。
///
/// 5 分钟够走完注册 + 邮箱验证码 + 2FA。**超时不是错误** —— 用户可能就是走开了，
/// 那时安静收场（返回 `false`）而不是弹一条他看不懂的失败。
///
/// 提成常量而不是内联 `300`：日志里要把它打出来（「最多 N 秒」），
/// 两处各写一个字面量迟早对不上（`vendor.rs` 的 `LOGIN_TIMEOUT` 同一形状）。
const LOGIN_TIMEOUT_SECS: u64 = 300;

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
    pub(crate) fn authenticated(site: DiscoveredRelaySite, relay_id: i64) -> Self {
        Self {
            relay_id,
            site_origin: site.site_origin,
            site_name: site.site_name,
            backend_kind: site.backend_kind,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DiscoveredRelaySite {
    pub(crate) site_origin: String,
    pub(crate) site_name: String,
    pub(crate) api_base_url: String,
    pub(crate) backend_kind: discovery::BackendKind,
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserLoginContext {
    pub(crate) site: DiscoveredRelaySite,
    pub(crate) login_script: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum IncompleteImportReason {
    Closed,
    TimedOut,
}

pub(crate) fn incomplete_new_site_import_error(reason: IncompleteImportReason) -> RelayImportError {
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

pub(crate) enum RefreshWait<T, I> {
    Interrupted(I),
    Refreshed(Result<T, AppError>),
}

/// Refresh-token rotation is a non-cancellable write once the HTTP request starts: the server
/// may invalidate the old cookie before the client observes the rotated one. An interrupt stops
/// future polling, but this helper drains the bounded refresh request and preserves any success.
pub(crate) async fn await_refresh_preserving_rotation<T, I>(
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

    // 曾经在这里挂「首个站点接入成功 → 弹 Star 邀请」（2026-09-06 删除）：
    // 刚接入站点的用户还没有任何使用感，此刻弹点赞礼只会被打断。Star 礼
    // 的入口只剩顶栏 GitHub 红点，见 `commands::onboarding` 的模块文档。
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

pub(crate) fn directory_entry_source(
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

pub(crate) fn recoverable_native_discovery_error(
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
pub(crate) fn import_anchor_origin(
    requested: String,
    detected: Option<&discovery::DetectedSite>,
) -> String {
    detected
        .and_then(|site| site.final_origin.clone())
        .filter(|final_origin| *final_origin != requested)
        .unwrap_or(requested)
}

/// 生成浏览器首次打开的地址：站点 origin 与后端归一化规则一致，但保留用户给的
/// path/query/fragment（例如邀请链接 `/register?aff=...`）。
pub(crate) fn browser_entry_url(input: &str) -> Result<url::Url, AppError> {
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
pub(crate) fn browser_start_url(
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

pub(crate) fn browser_login_context(
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
pub(crate) async fn destroy_stale_login_window_with_timeout<R: tauri::Runtime>(
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
pub(crate) fn browser_api_fallback(app_handle: &tauri::AppHandle) -> api::BrowserApiFallback {
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

pub(crate) fn persist_newapi_login_session(
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
/// 见 [`refresh_relay_provision`] 的文档）。2026-08-04 连带 `is_current` 一起删掉了
/// 那条 `Option` 分支。
pub(crate) async fn usable_relay<R: tauri::Runtime>(
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
pub(crate) async fn relay_read_with_refresh_retry<R, F, Fut, T>(
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
pub(crate) async fn backfill_account_identity<R: tauri::Runtime>(
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

pub(crate) fn should_clear_credentials_after_probe_error(error: &AppError) -> bool {
    backend::is_confirmed_auth_failure(error)
}

pub(crate) fn persist_refreshed_session(
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

pub(crate) fn persist_refreshed_session_with_identity_writer(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relay::test_support::*;

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
}
