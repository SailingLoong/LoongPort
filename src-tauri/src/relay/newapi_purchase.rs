//! NewAPI 的充值 WebView：cookie 形态登录态 + refresh 轮换跟踪。
//!
//! 与 sub2api 充值窗（[`super::purchase`]）的形态**完全不同**，这是协议决定的：
//!
//! | | sub2api | NewAPI |
//! |---|---|---|
//! | 登录态形态 | localStorage（`auth_token` / `auth_user`，注入脚本写入） | HttpOnly cookie（`new_api_refresh`，WebView cookie store 种入） |
//! | 「已登录」信号 | router 守卫读 localStorage | 站点后端认 cookie 会话 |
//! | 续期轮换 | refresh token 我们**不注入**（一次性，注入即被抢用） | refresh cookie **必须注入**，且站点会轮换它 |
//!
//! 所以这里**禁止 `initialization_script`**（NewAPI 的登录态不是 localStorage 形态，
//! 脚本写不进 HttpOnly cookie），登录态唯一正确的载体是 cookie store 本身。
//!
//! ## 安全红线（全部有测试或源码闸钉着）
//!
//! 1. refresh cookie 值绝不进 initialization_script（根本不用脚本）、localStorage、
//!    URL、事件 payload、日志、错误文案。
//! 2. **先 `set_cookie` 成功，后 `navigate`** —— 绝不让真实充值页在无登录态的窗口里
//!    加载（源码顺序闸：`the_seed_cookie_call_precedes_the_navigation_call_in_source`）。
//! 3. 不把 purchase URL 持久化或暴露给前端 —— 它只在 `open` 的栈上存在。
//!
//! ## 生命周期
//!
//! `open` 建 blank 窗口 → 种 cookie → 导航 → 启动后台 monitor 轮询窗口 cookie store；
//! **首次**观察到 refresh cookie 轮换并写库成功后 `open` 才返回 `Ok`（那是「种子
//! cookie 被站点接受」的信号）。monitor 持有 [`PurchaseSessionLease`] 直到窗口销毁
//! 或致命错误，期间持续把后续轮换写回库（只写 `relay.id` 那一列）。
//!
//! ## 接线顺序：`usable_relay` 先于 `try_acquire`（编排者预裁决）
//!
//! 命令层（`commands::relay::dispatch_purchase`）在取 lease **之前**先走
//! `usable_relay`（它可能在 token 过期时续期、轮换一次 refresh cookie）：
//!
//! - 开窗前的续期不会撞自己的闸（闸只拦「lease 已被持有」的续期，此刻还没取）；
//! - 取 lease 前续期留下的毫秒级 TOCTOU（续期完成 → 取得 lease 之间）由服务端
//!   refresh 单次轮换语义兜底：轮换后的 cookie 会先写进库、再种进即将创建的窗口
//!   （窗口此刻还不存在，不存在「窗口里那颗被作废」的问题），与现网双击行为一致。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};
use tokio::sync::{oneshot, watch};

use crate::error::AppError;
use crate::events::PURCHASE_CLOSED;
use crate::relay::creds;
use crate::relay::newapi;
use crate::relay::purchase_session::PurchaseSessionLease;
use crate::store::AppState;

/// 充值窗口等待**首次** cookie 轮换的上限（生产 20s）。
///
/// NewAPI 的钱包页加载后会自己调一次续期端点、拿到轮换后的 refresh cookie —— 那是
/// 「种子 cookie 被站点接受」的信号。等不到它说明种子 cookie 已失效（或站点改了
/// 行为），继续挂着一个未登录的充值窗只会让用户看到登录页。20s 覆盖慢速站点的
/// 首屏加载加一次续期请求。
#[cfg(not(test))]
pub const NEWAPI_PURCHASE_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// 测试专用的同名变体：MockRuntime 的 `cookies_for_url` 恒返回空 ⇒ 命令级测试必然
/// 走超时分支，20s 会让每个用例白等；300ms 足够确定性地观察到该分支。两个 const
/// 故意同名、由 cfg 切换 —— 值的分歧只影响测试二进制，生产语义见上面那份。
#[cfg(test)]
pub const NEWAPI_PURCHASE_STARTUP_TIMEOUT: Duration = Duration::from_millis(300);

/// monitor 的轮询间隔（生产 500ms）。
///
/// 轮询读的是**本进程内存里的** cookie store（WebView 查询），不是网络请求，
/// 500ms 粒度对「站点首屏自己续期一次」的观察足够细，也不会让 CPU 有感。
/// `monitor_rotation` 接受注入的间隔，测试用 1ms 级 —— 不睡真实 500ms。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 在窗口 cookie store 里找**与 `last_seen` 不同**的那颗 refresh cookie。
///
/// 语义是「轮换检测」而不是「提取」：值与上次看到的一致 ⇒ 没轮换 ⇒ `None`。
/// 认 cookie 的判据（名字、HttpOnly、非空值）与 [`newapi::extract_refresh_cookie`]
/// 是同一条 —— 我们种下去的就是那颗 HttpOnly cookie，JS 可读的同名赝品不该被当真。
pub fn rotated_refresh_cookie(
    cookies: &[tauri::webview::Cookie<'_>],
    last_seen: &str,
) -> Option<String> {
    newapi::extract_refresh_cookie(cookies).filter(|value| value != last_seen)
}

/// monitor 的 cookie 读端。生产实现包 `WebviewWindow::cookies_for_url`（同步、
/// 只读本进程的 cookie store）；注入成 `Arc<dyn Fn>` 是为了让轮换跟踪的逻辑
/// （何时持久化、何时停机）能不依赖 Tauri 直接测试。
pub(crate) type ReadCookies =
    Arc<dyn Fn() -> Result<Vec<tauri::webview::Cookie<'static>>, AppError> + Send + Sync>;

/// monitor 的持久化端。生产实现包 `creds::update_refresh_credential`（只写
/// `relay.id` 那一列，见它的文档：绝不允许顺带作废还活着的 access token）。
pub(crate) type PersistCookie = Arc<dyn Fn(String) -> Result<(), AppError> + Send + Sync>;

/// 跟踪窗口 cookie store 里的 refresh cookie 轮换，把每次轮换写回库。
///
/// 契约（每条都有测试钉着）：
///
/// - 首次轮换持久化成功 ⇒ 经 `ready` 上报 `Ok`（那是 `open` 返回 `Ok` 的前提）；
/// - 首次持久化失败 ⇒ 经 `ready` 上报「重新登录」错误并返回 `Err`；
/// - 首次轮换前超时 ⇒ 同上（[`NEWAPI_PURCHASE_STARTUP_TIMEOUT`] 由调用方传入）；
/// - `ready` 之后的持久化失败 ⇒ 调用方早已返回，只能记日志（不含凭据值）并返回 `Err`；
/// - 停机信号（窗口销毁）或 `shutdown` 发送端被丢弃 ⇒ 返回 `Ok` 正常收场。
///
/// `lease` 由本函数持有到返回 —— 返回即 drop 即释放，`usable_relay` 的续期闸
/// 随之解除。
// 参数多过 clippy 的 7 个上限是有意的：每个都是 monitor 契约的一个独立维度
// （读端/写端/租约/初值/就绪信号/停机信号/节奏），合并进 config struct 只会让
// 测试用例多两行构造、少一行可读性（与 proxy 模块的既有 allow 同一取舍）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn monitor_rotation(
    read_cookies: ReadCookies,
    persist_cookie: PersistCookie,
    lease: PurchaseSessionLease,
    mut last_seen: String,
    ready: oneshot::Sender<Result<(), AppError>>,
    mut shutdown: watch::Receiver<bool>,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<(), AppError> {
    // lease 在这里只被持有（RAII），从不读 —— 语句本身就是为了把它的生命周期
    // 钉在本函数上。
    let _lease_guard = lease;
    let deadline = Instant::now() + timeout;
    let mut ready = Some(ready);
    // 读失败只在**第一次** warn（review F4）：500ms 轮询下持续失败会每秒刷两条
    // warn，把真正要人看的日志淹掉。此后降为 debug —— 一个 bool 就够，不做按
    // 错误内容去重那种更复杂的机制。
    let mut read_failure_reported = false;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        match read_cookies() {
            Ok(cookies) => {
                if let Some(rotated) = rotated_refresh_cookie(&cookies, &last_seen) {
                    match persist_cookie(rotated.clone()) {
                        Ok(()) => {
                            if let Some(tx) = ready.take() {
                                let _ = tx.send(Ok(()));
                            }
                            last_seen = rotated;
                        }
                        Err(error) => {
                            if let Some(tx) = ready.take() {
                                // 首次轮换就写不进库：种子 cookie 已被站点作废、轮换值
                                // 却没落库 —— 本仓与窗口从此各执一份互相作废的凭据，
                                // 唯一干净的出路是重新登录。写库错误文案（库层）不含
                                // 凭据值，可以带上。
                                let _ = tx.send(Err(AppError::Config(format!(
                                    "NewAPI 充值会话的新登录态保存失败（{error}），\
                                     请重新登录该中转站账号"
                                ))));
                            } else {
                                // ready 之后的写库失败：命令早已返回，错误只能进日志，
                                // 且不得含凭据值。
                                log::warn!(
                                    "NewAPI 充值窗口的后续 cookie 轮换写库失败，\
                                     停止跟踪该窗口: {error}"
                                );
                            }
                            return Err(error);
                        }
                    }
                }
            }
            Err(error) => {
                // 单次读取失败不致命：窗口还活着就继续轮询（窗口销毁有独立的停机信号）。
                if read_failure_reported {
                    log::debug!("读取充值窗口 cookie 失败（已报过一次），继续轮询: {error}");
                } else {
                    log::warn!("读取充值窗口 cookie 失败，继续轮询: {error}");
                    read_failure_reported = true;
                }
            }
        }

        // 等待下一轮；未 ready 时这一等同时也是「启动超时」的计时。
        let sleep_for = match ready.as_ref() {
            Some(_) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let _ = ready.take().map(|tx| tx.send(Err(startup_timeout_error())));
                    return Err(startup_timeout_error());
                }
                poll_interval.min(remaining)
            }
            None => poll_interval,
        };
        let stopped = tokio::select! {
            changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
            _ = tokio::time::sleep(sleep_for) => false,
        };
        if stopped {
            return Ok(());
        }
    }
}

fn startup_timeout_error() -> AppError {
    AppError::Config(
        "NewAPI 充值窗口未能建立登录会话（没有观察到登录态轮换），\
         请重新登录该中转站账号"
            .into(),
    )
}

/// 从 relay 行取出非空的 refresh credential；空缺即「要求重新登录」的错误。
fn required_refresh_credential(relay: &creds::Relay) -> Result<&str, AppError> {
    relay
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Config(
                "NewAPI 登录态缺失（refresh cookie 为空），请重新登录该中转站账号".into(),
            )
        })
}

/// 建窗阶段：blank 窗口 → 种 refresh cookie → 导航到钱包 URL。
///
/// ⚠️ **顺序是安全红线**：必须 `set_cookie` 成功后才 `navigate`（源码顺序闸
/// `the_seed_cookie_call_precedes_the_navigation_call_in_source` 钉着）。先建
/// `about:blank` 窗口是为了让 cookie 有一个可种的 cookie store，也是为了让真实
/// 充值 URL 永远不在无登录态的窗口里加载。
///
/// 窗口 title / 尺寸 / incognito 与 sub2api 站点窗保持一致：两者由同一分派层
/// （`commands::relay::dispatch_site_window`）开窗，同一行第二击的聚焦检查按
/// label 分不出协议，同前缀空间的窗口也不该长得两样。
/// **有意不调用 initialization_script**（模块文档）。
///
/// `shutdown` 是窗口销毁的停机信号端，接进 `Destroyed` 事件后交给 monitor。
pub(crate) fn build_window<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay: &creds::Relay,
    window: super::purchase::SiteWindow,
    target_url: &url::Url,
    refresh_credential: &str,
    shutdown: watch::Sender<bool>,
) -> Result<tauri::WebviewWindow<R>, AppError> {
    // 关窗事件要带上是哪一行 —— 前端据此只刷那一行的余额（与 sub2api 站点窗同一事件）。
    let handle_for_close = app_handle.clone();
    let closed_relay_id = relay.id;

    let built = tauri::WebviewWindowBuilder::new(
        app_handle,
        // label 由调用方传入（充值 / 用量各自的 per-relay 窗）：「同一行第二击聚焦
        // 现有窗口」的检查在命令分派层做，同一页面种类的两种协议必须落到同一个
        // label 上它才成立；window-state 的过滤也依赖 `purchase::is_site_window_label`。
        window.label,
        tauri::WebviewUrl::External(
            url::Url::parse("about:blank").expect("about:blank 必是合法 URL"),
        ),
    )
    .title(window.title)
    // 尺寸与安全说明与 sub2api 站点窗一致（USDT 充值页的「转错网络资产不可找回」
    // 警告需要一屏内可读；防溢出用框架原生 clamp，避免 Retina 尺寸翻倍）。
    .inner_size(1000.0, 800.0)
    .resizable(true)
    .prevent_overflow_with_margin(tauri::LogicalSize::new(40.0, 40.0))
    .center()
    // ⚠️ **必须 incognito**，理由同 sub2api 站点窗（purchase.rs 模块文档第 1 条）：
    // 持久 profile 是全 app 共享的，不隔离会读到别的账号残留的登录态 —— 钱充错账号。
    .incognito(true)
    // 放行 window.open 弹窗：NewAPI 站点的支付 / OAuth 弹窗默认会被 wry 静默
    // 吞掉，理由与 `relay::browser_import` 那段逐条相同（子窗口共享会话与
    // opener 语义，无注入脚本与 IPC，不新增攻击面）。
    .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Allow)
    .build()
    .map_err(|e| AppError::Config(format!("打开站点窗口失败: {e}")))?;

    // 认 `Destroyed`（窗口真的没了）而不是 `CloseRequested`（可被拦下、可能取消）：
    // 关窗即刷余额（emit）+ 停 monitor（shutdown）。
    built.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = handle_for_close.emit(PURCHASE_CLOSED, closed_relay_id);
            let _ = shutdown.send(true);
        }
    });

    let cookie = newapi::purchase_refresh_cookie(&relay.site_origin, refresh_credential)?;
    if let Err(error) = built.set_cookie(cookie) {
        let _ = built.destroy();
        return Err(AppError::Config(format!("种入充值登录态失败: {error}")));
    }
    if let Err(error) = built.navigate(target_url.clone()) {
        let _ = built.destroy();
        return Err(AppError::Config(format!("站点窗口导航失败: {error}")));
    }
    Ok(built)
}

/// 写库端的生产实现：只轮换 `relay_id` 的 refresh credential 那一列。
fn persist_refresh_credential<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
    value: &str,
) -> Result<(), AppError> {
    let state = app_handle.state::<AppState>();
    let conn = state
        .db
        .conn
        .lock()
        .map_err(|e| AppError::Database(format!("获取数据库连接失败: {e}")))?;
    creds::update_refresh_credential(&conn, relay_id, value)
}

/// 打开 NewAPI 的站点页面窗（充值 / 查看用量）：种 cookie → 导航 → 等首次轮换落库
/// → 交给后台 monitor。
///
/// `open` 只在 WebView 完成**首次** cookie 轮换并持久化成功后才返回 `Ok`；
/// 超时或首次持久化失败会销毁窗口并返回「重新登录」类错误。`lease` 移交给后台
/// monitor 任务持有，直到窗口销毁或 monitor 致命错误（届时窗口一并销毁）。
///
/// 窗口身份（`window`：label + 标题）与目标 URL 都由调用方从签名配置解析后传入；
/// 本函数不持久化它们、不回传给前端。
pub async fn open<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay: creds::Relay,
    window: super::purchase::SiteWindow,
    target_url: url::Url,
    lease: PurchaseSessionLease,
) -> Result<(), AppError> {
    let refresh_credential = required_refresh_credential(&relay)?;
    let initial_credential = refresh_credential.to_string();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // `shutdown_keeper` 让通道在**启动阶段**保持打开：
    //
    // - MockRuntime 的 `on_window_event` 在注册时就丢弃回调（连同闭包里的发送端），
    //   通道一关，monitor 的 `changed()` 立刻按「窗口已销毁」收场 —— 那是 mock 的
    //   实现细节，不该让命令级测试全部走「窗口被关闭」分支。open 返回时 keeper
    //   随栈帧 drop。
    // - 生产里不受影响：`Destroyed` 事件先于回调丢弃到达（显式 `send(true)`），
    //   keeper drop 后真正的停机信号仍由窗口回调发出；回调被丢弃本身也是兜底信号。
    let _shutdown_keeper = shutdown_tx.clone();
    let window = build_window(
        app_handle,
        &relay,
        window,
        &target_url,
        refresh_credential,
        shutdown_tx,
    )?;

    let refresh_url = newapi::refresh_url(&relay.site_origin)?;
    let window_for_read = window.clone();
    let read_cookies: ReadCookies = Arc::new(move || {
        window_for_read
            .cookies_for_url(refresh_url.clone())
            .map_err(|error| AppError::Config(format!("读取充值窗口 cookie 失败: {error}")))
    });

    let app_for_persist = app_handle.clone();
    let persist_relay_id = relay.id;
    let persist_cookie: PersistCookie = Arc::new(move |value| {
        persist_refresh_credential(&app_for_persist, persist_relay_id, &value)
    });

    let (ready_tx, ready_rx) = oneshot::channel();
    let monitor_window = window.clone();
    let monitor_relay_id = relay.id;
    tauri::async_runtime::spawn(async move {
        let outcome = monitor_rotation(
            read_cookies,
            persist_cookie,
            lease,
            initial_credential,
            ready_tx,
            shutdown_rx,
            POLL_INTERVAL,
            NEWAPI_PURCHASE_STARTUP_TIMEOUT,
        )
        .await;
        if let Err(error) = outcome {
            // monitor 的致命错误有两条出口：首次轮换超时 / 首次持久化失败时，open 已
            // 通过 ready 拿到错误并销毁窗口（这里的 destroy 是幂等兜底）；ready 之后的
            // 持久化失败则调用方早已返回 —— 错误只能进日志，且不得含凭据值。
            log::warn!(
                "NewAPI 充值窗口（relay {monitor_relay_id}）的 cookie 轮换跟踪已停止: {error}"
            );
            let _ = monitor_window.destroy();
        }
    });

    match ready_rx.await {
        Ok(Ok(())) => Ok(()),
        // 错误结果（超时 / 首次持久化失败）的**销毁窗口动作由 monitor 的 wrapper 任务
        // 负责**（见上面 spawn 里的 Err 分支）：那里 destroy 不会与这里并发 —— mock
        // 运行时的窗口表是 RefCell，两次并发 destroy 会直接 panic；生产里双份 destroy
        // 虽无害，但让销毁只有一个 owner 更清晰。代价只是错误先到、窗口晚几毫秒消失。
        Ok(Err(error)) => Err(error),
        Err(_dropped_without_a_result) => {
            // ready 没带结果就被丢弃：要么 monitor 在会话建立前收到停机（用户秒关窗口），
            // 要么任务异常终止 —— 两种情况 wrapper 都已按 Ok 收场（不会再 destroy），
            // 窗口的清理只能由这里做。
            let _ = window.destroy();
            Err(AppError::Config(
                "充值窗口在登录会话建立前已关闭，请重试".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::purchase_session::PurchaseSessionCoordinator;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // ======================================================================
    // Step 1: rotated_refresh_cookie（纯函数）
    // ======================================================================

    fn refresh_cookie_frame(value: &str) -> Vec<tauri::webview::Cookie<'static>> {
        let mut cookie =
            tauri::webview::Cookie::new(newapi::REFRESH_COOKIE_NAME, value.to_string());
        cookie.set_http_only(true);
        vec![cookie]
    }

    #[test]
    fn rotated_refresh_cookie_is_none_without_a_new_cookie() {
        assert_eq!(rotated_refresh_cookie(&[], "last-seen"), None);
    }

    #[test]
    fn rotated_refresh_cookie_ignores_the_value_we_already_have() {
        // 语义是「轮换检测」：值与上次看到的一致 ⇒ 没轮换 ⇒ None。
        // 这条也是 mutation 闸 —— 把过滤改成「只要提取到就 Some」它当场红。
        assert_eq!(
            rotated_refresh_cookie(&refresh_cookie_frame("same"), "same"),
            None
        );
    }

    #[test]
    fn rotated_refresh_cookie_returns_a_genuinely_new_value() {
        assert_eq!(
            rotated_refresh_cookie(&refresh_cookie_frame("fresh"), "stale").as_deref(),
            Some("fresh")
        );
    }

    #[test]
    fn rotated_refresh_cookie_requires_the_same_judgment_as_extraction() {
        // JS 可读的同名 cookie 不是我们种的那颗 —— 认 cookie 的判据（HttpOnly）必须与
        // `newapi::extract_refresh_cookie` 同一条，否则页面脚本能伪造「轮换」。
        let readable =
            tauri::webview::Cookie::new(newapi::REFRESH_COOKIE_NAME, "fresh".to_string());
        assert_eq!(readable.http_only(), None);
        assert_eq!(rotated_refresh_cookie(&[readable], "stale"), None);
    }

    // ======================================================================
    // Step 2: monitor（注入依赖）
    // ======================================================================

    /// 可编排的 cookie 读端：每读一次弹出一帧，弹完回落到 `default` 帧。
    /// 轮询时机不受测试控制，但「每帧至多触发一次持久化」的不变量与读多少次无关。
    struct CookieFrames {
        default: Vec<tauri::webview::Cookie<'static>>,
        frames: Mutex<VecDeque<Vec<tauri::webview::Cookie<'static>>>>,
        reads: AtomicUsize,
    }

    impl CookieFrames {
        fn new(
            default: Vec<tauri::webview::Cookie<'static>>,
            frames: Vec<Vec<tauri::webview::Cookie<'static>>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                default,
                frames: Mutex::new(frames.into_iter().collect()),
                reads: AtomicUsize::new(0),
            })
        }

        fn reader(self: &Arc<Self>) -> ReadCookies {
            let source = Arc::clone(self);
            Arc::new(move || {
                source.reads.fetch_add(1, Ordering::SeqCst);
                let mut frames = source.frames.lock().expect("cookie frames 锁");
                Ok(frames.pop_front().unwrap_or_else(|| source.default.clone()))
            })
        }

        fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
    }

    fn recording_persist(persisted: &Arc<Mutex<Vec<String>>>) -> PersistCookie {
        let persisted = Arc::clone(persisted);
        Arc::new(move |value| {
            persisted.lock().expect("persist 记录锁").push(value);
            Ok(())
        })
    }

    fn failing_persist() -> PersistCookie {
        Arc::new(|_value| Err(AppError::Config("测试注入的写库失败".into())))
    }

    fn fresh_lease(relay_id: i64) -> (Arc<PurchaseSessionCoordinator>, PurchaseSessionLease) {
        let coordinator = Arc::new(PurchaseSessionCoordinator::default());
        let lease = coordinator.try_acquire(relay_id).expect("acquire lease");
        (coordinator, lease)
    }

    #[tokio::test]
    async fn monitor_persists_the_first_rotation_once_and_signals_ready() {
        let source = CookieFrames::new(
            refresh_cookie_frame("same"),
            vec![
                refresh_cookie_frame("same"),
                refresh_cookie_frame("same"),
                refresh_cookie_frame("rotated-a"),
                refresh_cookie_frame("rotated-a"),
                refresh_cookie_frame("rotated-a"),
            ],
        );
        let persisted = Arc::new(Mutex::new(Vec::<String>::new()));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (coordinator, lease) = fresh_lease(7);

        let task = tokio::spawn(monitor_rotation(
            source.reader(),
            recording_persist(&persisted),
            lease,
            "same".into(),
            ready_tx,
            shutdown_rx,
            Duration::from_millis(1),
            Duration::from_secs(5),
        ));

        ready_rx
            .await
            .expect("首次轮换持久化后 monitor 必须发 ready")
            .expect("首次持久化成功，ready 携带 Ok");

        shutdown_tx.send(true).expect("发送停机信号");
        task.await
            .expect("monitor 任务不该 panic")
            .expect("正常停机返回 Ok");

        assert_eq!(
            *persisted.lock().unwrap(),
            vec!["rotated-a".to_string()],
            "首次轮换恰好持久化一次：last_seen 更新后，同值帧不再触发写入"
        );
        assert!(!coordinator.is_active(7), "停机后 lease 必须已释放");
    }

    #[tokio::test]
    async fn monitor_persists_a_second_rotation_to_a_new_value() {
        let source = CookieFrames::new(
            // 帧耗尽后维持「最终值」：last_seen 已是 rotated-b，再读到它不算轮换。
            refresh_cookie_frame("rotated-b"),
            vec![
                refresh_cookie_frame("same"),
                refresh_cookie_frame("rotated-a"),
                refresh_cookie_frame("rotated-a"),
                refresh_cookie_frame("rotated-b"),
                refresh_cookie_frame("rotated-b"),
            ],
        );
        let persisted = Arc::new(Mutex::new(Vec::<String>::new()));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (coordinator, lease) = fresh_lease(7);

        let task = tokio::spawn(monitor_rotation(
            source.reader(),
            recording_persist(&persisted),
            lease,
            "same".into(),
            ready_tx,
            shutdown_rx,
            Duration::from_millis(1),
            Duration::from_secs(5),
        ));

        ready_rx
            .await
            .expect("首次轮换必须发 ready")
            .expect("首次持久化成功");

        // 让 monitor 读完 rotated-b 帧（1ms 轮询下 50ms 足够，且与精确读次无关）。
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown_tx.send(true).expect("发送停机信号");
        task.await
            .expect("monitor 任务不该 panic")
            .expect("正常停机返回 Ok");

        assert_eq!(
            *persisted.lock().unwrap(),
            vec!["rotated-a".to_string(), "rotated-b".to_string()],
            "第二次轮换到新值必须再持久化一次"
        );
        assert!(!coordinator.is_active(7));
    }

    #[tokio::test]
    async fn monitor_first_persist_failure_is_a_relogin_error_and_releases_the_lease() {
        let source = CookieFrames::new(
            refresh_cookie_frame("rotated-a"),
            vec![refresh_cookie_frame("rotated-a")],
        );
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let (coordinator, lease) = fresh_lease(7);

        let outcome = monitor_rotation(
            source.reader(),
            failing_persist(),
            lease,
            "same".into(),
            ready_tx,
            shutdown_rx,
            Duration::from_millis(1),
            Duration::from_secs(5),
        )
        .await;

        let error = outcome.expect_err("首次持久化失败必须返回错误");
        assert!(
            !error.to_string().contains("rotated-a"),
            "monitor 的返回错误不得包含 cookie 值：{error}"
        );
        assert!(
            !coordinator.is_active(7),
            "monitor 返回时 lease 必须已随 drop 释放"
        );

        let reported = ready_rx
            .await
            .expect("ready 通道必须带上失败结果")
            .expect_err("首次持久化失败必须经 ready 上报为错误")
            .to_string();
        assert!(
            reported.contains("重新登录"),
            "上报给用户的错误要指明出路：{reported}"
        );
        assert!(
            !reported.contains("rotated-a"),
            "错误文案不得包含 cookie 值（credential 安全红线）：{reported}"
        );
    }

    #[tokio::test]
    async fn monitor_stops_polling_and_releases_the_lease_after_shutdown() {
        let source = CookieFrames::new(refresh_cookie_frame("same"), vec![]);
        let persisted = Arc::new(Mutex::new(Vec::<String>::new()));
        let (ready_tx, _ready_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (coordinator, lease) = fresh_lease(7);

        let task = tokio::spawn(monitor_rotation(
            source.reader(),
            recording_persist(&persisted),
            lease,
            "same".into(),
            ready_tx,
            shutdown_rx,
            Duration::from_millis(1),
            // 超时给足：本用例只关心停机信号，不想让超时分支抢先收场。
            Duration::from_secs(60),
        ));

        tokio::time::sleep(Duration::from_millis(30)).await;
        shutdown_tx.send(true).expect("发送停机信号");
        task.await
            .expect("monitor 任务不该 panic")
            .expect("停机是正常收场");

        assert!(!coordinator.is_active(7), "停机后 lease 必须已释放");
        let reads_at_stop = source.reads();
        assert!(reads_at_stop > 0, "前提：停机前确实在轮询");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            source.reads(),
            reads_at_stop,
            "停机信号之后不得再轮询（不得只停不发，也不得继续读）"
        );
        assert!(persisted.lock().unwrap().is_empty(), "值没轮换就绝不持久化");
    }

    #[tokio::test]
    async fn monitor_timeout_before_any_rotation_is_a_relogin_error() {
        let source = CookieFrames::new(refresh_cookie_frame("same"), vec![]);
        let persisted = Arc::new(Mutex::new(Vec::<String>::new()));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let (coordinator, lease) = fresh_lease(7);

        let outcome = monitor_rotation(
            source.reader(),
            recording_persist(&persisted),
            lease,
            "same".into(),
            ready_tx,
            shutdown_rx,
            Duration::from_millis(1),
            Duration::from_millis(30),
        )
        .await;

        let error = outcome.expect_err("首次轮换前超时必须返回错误").to_string();
        assert!(
            error.contains("重新登录"),
            "超时错误要有「重新登录」语义：{error}"
        );
        assert!(
            persisted.lock().unwrap().is_empty(),
            "没观察到轮换就绝不持久化"
        );
        assert!(!coordinator.is_active(7), "超时收场也要释放 lease");

        let reported = ready_rx
            .await
            .expect("超时必须经 ready 上报")
            .expect_err("超时是错误")
            .to_string();
        assert!(reported.contains("重新登录"), "{reported}");
    }

    // ======================================================================
    // Step 4/7: 建窗阶段（生产序列的 URL 断言放这里 —— MockRuntime 的
    // cookies_for_url 恒空，命令级 happy-path 走超时路径，URL 只能在建窗后观察）
    // ======================================================================

    fn newapi_relay(id: i64, site_origin: &str) -> creds::Relay {
        creds::Relay {
            id,
            site_origin: site_origin.into(),
            site_name: "NewAPI".into(),
            backend_kind: creds::BackendKind::NewApi,
            api_base_url: String::new(),
            account_id: Some(id),
            account_label: "account".into(),
            login_identifier: "account".into(),
            auth_token: "access-token".into(),
            refresh_token: Some("seed-refresh-cookie".into()),
            token_expires_at: None,
            user_agent: None,
            cf_clearance: None,
            pricing_synced_at: None,
            sort_index: 0,
        }
    }

    #[test]
    fn build_window_seeds_the_cookie_then_navigates_to_the_configured_url() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock app");
        let relay = newapi_relay(3, "https://newapi.example");
        let purchase_url = url::Url::parse("https://newapi.example/console/topup?tab=wallet")
            .expect("合法钱包 URL");

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let window = build_window(
            app.handle(),
            &relay,
            crate::relay::purchase::purchase_window(relay.id, &relay.site_origin),
            &purchase_url,
            "seed-refresh-cookie",
            shutdown_tx,
        )
        .expect("建窗阶段成功");

        // MockRuntime 的 navigate 会更新存储的 url —— 导航后的地址必须逐字符是配置值。
        assert_eq!(
            window.url().expect("mock 窗口能读回 url").as_str(),
            purchase_url.as_str(),
            "窗口最终地址必须恰好是签名配置的钱包 URL（about:blank 只是种 cookie 的脚手架）"
        );
        assert!(
            app.get_webview_window(&crate::relay::purchase::window_label(3)).is_some(),
            "窗口 label 必须复用 purchase 的同一前缀空间（window-state 过滤与「第二击聚焦」都靠它）"
        );
    }

    #[test]
    fn the_seed_cookie_call_precedes_the_navigation_call_in_source() {
        // 安全红线的源码闸：必须先种 cookie、成功后再导航。mock 的 set_cookie 是 no-op，
        // 运行期观察不到顺序 —— 这里钉住源码顺序。顺序反了意味着真实充值页会先在
        // 无登录态的窗口里加载一次（站点视它为未登录，可能整页跳登录）。
        //
        // 注意：本文件的生产代码在测试之前，find() 命中的是生产调用点而不是这里的字面量。
        let source = include_str!("newapi_purchase.rs");
        let seed = source.find(".set_cookie(").expect("建窗阶段必须种 cookie");
        let navigate = source.find(".navigate(").expect("建窗阶段必须导航");
        assert!(
            seed < navigate,
            "必须 set_cookie 成功后才 navigate —— 顺序是安全红线"
        );
    }

    #[test]
    fn the_window_builder_never_uses_an_initialization_script() {
        // NewAPI 的登录态是 HttpOnly cookie，脚本根本写不进去；出现脚本注入即说明
        // 有人把 sub2api 的形态错搬过来（那会把凭据往 localStorage 里带）。
        // 字面量拆两段拼接，避免本测试自匹配。
        let source = include_str!("newapi_purchase.rs");
        let forbidden = [".initialization", "_script("].concat();
        assert!(
            !source.contains(&forbidden),
            "NewAPI 充值窗禁止 initialization_script：登录态只能以 HttpOnly cookie 形态存在"
        );
    }
}
