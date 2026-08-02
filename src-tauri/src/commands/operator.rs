//! LoongPort 运营商的 Tauri 命令层。
//!
//! 六个命令，对应需求的六步：
//!
//! | 命令 | 干什么 |
//! |---|---|
//! | [`operator_status`] | 首启该弹哪个弹窗、当前是什么状态 |
//! | [`operator_probe_site`] | 域名弹窗点确定 → 探测这是不是 sub2api 站 |
//! | [`operator_login`] | 开登录 WebView，等凭据回来 |
//! | [`operator_provision`] | 拉分组 → 每组备好 sk → 写成 codex provider |
//! | [`operator_switch_tier`] | 选分组 → 退 ChatGPT → 切换 → 重开 |
//! | [`operator_logout`] | 清凭据（保留站点与 device_id） |
//!
//! ## 为什么切换编排在 Rust 侧而不是前端
//!
//! 「退出 ChatGPT → 切换 → 重开」如果写在前端的按钮回调里，那么**托盘快切、deeplink 导入、
//! 项目快照**这三条路径都会绕过它（它们在 Rust 侧直接调 `ProviderService::switch`），用户
//! 从托盘切完就会发现 codex 还连着旧分组。放在这一层是让「切换分组」只有一个入口。

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::operator::{api, chatgpt_app, creds, login, provision};
use crate::provider::Provider;
use crate::services::ProviderService;
use crate::store::AppState;

/// 默认运营商域名。域名输入框的底纹词，用户直接点确定就用它。
const DEFAULT_SITE: &str = "bestapi.store";

/// codex 的默认模型。
///
/// 三个来源给了三个不同的值（sub2api 面板片段 `gpt-5.5`、cc-switch 第三方模板 `gpt-5.6-sol`、
/// 上游 `UniversalProvider` 默认 `gpt-4o`），所以这个值是**查了真实服务端定的**：
///
/// - bestapi.store 的 codex 分组（openai 平台）下，`gpt-5.6-sol` 是**全部可调度账号都支持**的
///   最新一代；`gpt-5.6` 只有一家上游有，选它会让另外几家路由不到。
/// - 与 `gpt-5.5` 同价（输入 5 / 输出 30 每百万），所以选新的没有额外成本。
/// - `gpt-4o` 三个候选里唯一没人推荐的，别回退到它。
///
/// **这是「默认值」不是「唯一值」**：用户在 provider 编辑里能改，运营商上新一代模型后也该
/// 跟着调。它只决定「刚 provision 完、用户还没动手」时用哪个。
const DEFAULT_MODEL: &str = "gpt-5.6-sol";

/// 当前状态，前端据此决定显示哪一屏。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorStatus {
    /// 域名输入框的底纹词。
    pub default_site: String,
    /// 已选定的站点（没选过则为 `None` → 前端弹域名输入框）。
    pub site_origin: Option<String>,
    pub site_name: Option<String>,
    /// 当前站登录的账号名（昵称优先，回落邮箱）。空串 = 还没登录。
    ///
    /// 同一个站可以挂多个账号，所以「登录了」不够 —— 得说清是**哪个**账号。
    /// 内部去重认的是服务端的数值 id，不是这个标签（用户改昵称不该被当成换了账号）。
    pub account_label: String,
    /// 是否已有可用凭据（否 → 前端引导去登录）。
    pub logged_in: bool,
    /// **登录过、但凭据已经不能用了** —— 前端据此提示「登录已过期，请重新登录」，
    /// 而不是像从没登录过一样只摆一个「登录」按钮。
    ///
    /// 与 `!logged_in` 不同：那个把「从没登录」与「登录过但过期」混成一件事，而对用户是
    /// 两种处境（前者要输邮箱+密码，后者只需确认一下密码与人机验证）。
    ///
    /// 判据是「有 `account_id`（登录过）但 token 不可用」。`refresh_token` 还在时下一次
    /// 请求会自动续期、用户根本不必管，所以那种情况**不算过期**。
    pub session_expired: bool,
    /// 重新登录时预填进登录框的值（空串 = 没有，让用户自己输）。
    ///
    /// 存的是登录标识本身而不是 `account_label`：后者昵称优先，设了昵称的用户拿它填
    /// 登录框是错的。见 `creds::Operator::login_identifier`。
    pub login_identifier: String,
    /// 已经备好 sk 的档位数。
    pub tier_count: usize,
    /// 切换分组前要不要先提示用户处理 ChatGPT。
    ///
    /// 不是「装了没有」—— 非 macOS 平台查不到那个事实，那边恒为 true（宁可多问一句，
    /// 也不能让装了 ChatGPT 的用户静默用错分组）。见 `chatgpt_app::needs_user_attention`。
    pub chatgpt_needs_attention: bool,
}

/// 一个已添加的站点（给站点切换器用）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteInfo {
    pub id: i64,
    pub site_origin: String,
    pub site_name: String,
    /// 登录后的账号名（昵称优先，回落邮箱），未登录为空串。同一个站挂多个账号时靠它分辨。
    pub account_label: String,
    /// 展示名：站名 +（有账号时）邮箱。
    pub label: String,
    pub logged_in: bool,
    pub is_current: bool,
}

/// 探测结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub site_origin: String,
    pub site_name: String,
    /// 归一后的 codex base_url（带 `/v1`）。
    pub api_base_url: String,
    pub registration_enabled: bool,
}

/// 一个可选的档位。
///
/// `group_id` / `rate_multiplier` 是 `Option`：列表命令从本地 DB 读，而倍率只在 provision
/// 时从服务端拿到。**用 `Option` 而不是填 0 占位** —— 0 倍率意味着"免费"，UI 会把它显示成
/// 最便宜的一档，那是错的。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierInfo {
    pub provider_id: String,
    pub group_id: Option<i64>,
    pub group_name: String,
    pub display_name: String,
    pub rate_multiplier: Option<f64>,
    pub is_current: bool,
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

/// 探一次凭据是不是真的还能用，并处置失效的情况。
///
/// 为什么需要这个：[`operator_status`] 的 `logged_in` 只看本地记的过期时间。而凭据可能在
/// 网页端被撤销、账号被禁用、或会话被踢掉 —— 那些情况下本地看起来一切正常，用户点任何操作
/// 才会撞到错误。第 2 次打开 app 到第 100 次都走这条路，不能共用第 1 次的假设。
///
/// 返回 true 表示凭据可用（可能是刚静默续期过的）。返回 false 表示已清掉本地凭据、前端该回
/// 到登录入口 —— **不是错误**，用户重新登录一次即可。
#[tauri::command]
pub async fn operator_check_session(app_handle: tauri::AppHandle) -> Result<bool, String> {
    check_session(&app_handle).await.map_err(|e| e.to_string())
}

async fn check_session(app_handle: &tauri::AppHandle) -> Result<bool, AppError> {
    let has_creds = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, creds::load)?
            .map(|op| !op.auth_token.is_empty())
            .unwrap_or(false)
    };
    if !has_creds {
        return Ok(false);
    }

    // usable_operator 会在快过期时先续期；拿 /user/profile 当探活请求（最便宜的鉴权端点）。
    let probe = async {
        let op = usable_operator(app_handle).await?;
        api::Client::new(&op.site_origin, &op.auth_token)?
            .balance()
            .await
    }
    .await;

    match probe {
        Ok(_) => Ok(true),
        Err(e) => {
            let msg = e.to_string();
            // 「登录态已失效」是 api 层对不可恢复的那一类 401 的措辞（账号被禁 / 会话被撤销 /
            // 用户不存在）。这类清掉本地凭据、让用户重新登录。
            //
            // 其它失败（网络不通、运营商关了用户面板返 403）**不清凭据** —— 那不是凭据的问题，
            // 清掉只会逼用户在网络恢复后白重登一次。
            if msg.contains("登录态已失效") || msg.contains("请重新登录") {
                let state = app_handle.state::<AppState>();
                with_conn(&state, |conn| {
                    // 只清当前站的凭据 —— 别的站可能还是好的。
                    match creds::load(conn)? {
                        Some(op) => creds::clear_credentials(conn, op.id),
                        None => Ok(()),
                    }
                })?;
                log::info!("运营商凭据已失效，已清除本地凭据：{msg}");
                return Ok(false);
            }
            log::warn!("探活失败但保留凭据（可能只是网络问题）：{msg}");
            Ok(true)
        }
    }
}

fn operator_status_impl(state: &AppState) -> Result<OperatorStatus, AppError> {
    let op = with_conn(state, creds::load)?;
    let tier_count = ProviderService::list(state, AppType::Codex)
        .map(|list| list.values().filter(|p| is_managed(p)).count())
        .unwrap_or(0);

    Ok(match op {
        None => OperatorStatus {
            default_site: DEFAULT_SITE.to_string(),
            site_origin: None,
            site_name: None,
            account_label: String::new(),
            logged_in: false,
            session_expired: false,
            login_identifier: String::new(),
            tier_count,
            chatgpt_needs_attention: chatgpt_app::needs_user_attention(),
        },
        Some(op) => {
            let logged_in = op.token_looks_valid(chrono::Utc::now().timestamp());
            OperatorStatus {
                default_site: DEFAULT_SITE.to_string(),
                logged_in,
                // 「登录过但用不了了」才算过期。还有 refresh_token 的话下次请求会自动续期、
                // 用户不必管，所以不报过期 —— 报了他会白跑一次重登。
                session_expired: !logged_in
                    && op.account_id.is_some()
                    && op.refresh_token.is_none(),
                // 预填值与 account_id 无关：即使凭据已清空（clear_credentials 会清
                // account_id），登录标识仍留着，那正是重登时要用的。
                login_identifier: op.login_identifier.clone(),
                // 有 account_id 才算真的认得这个账号 —— email 可能被运营商留空。
                account_label: if op.account_id.is_some() {
                    op.account_label.clone()
                } else {
                    String::new()
                },
                site_origin: Some(op.site_origin),
                site_name: Some(op.site_name),
                tier_count,
                chatgpt_needs_attention: chatgpt_app::needs_user_attention(),
            }
        }
    })
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
    with_conn(&state, |conn| {
        creds::save_site(conn, &site_origin, &site_name, &api_base_url)
    })?;

    Ok(ProbeResult {
        site_origin,
        site_name,
        api_base_url,
        registration_enabled: settings.registration_enabled,
    })
}

/// 开登录窗，等凭据回来。
///
/// 凭据由注入脚本经一次被拦下的自定义 scheme 跳转送回（见 [`login`]）。本命令在收到凭据、
/// 或用户关掉窗口、或超时之后返回。
#[tauri::command]
pub async fn operator_login(app_handle: tauri::AppHandle) -> Result<bool, String> {
    do_login(&app_handle).await.map_err(|e| e.to_string())
}

async fn do_login(app_handle: &tauri::AppHandle) -> Result<bool, AppError> {
    // 登录的是**当前选中的那个站**。同时记下它的行 id —— 凭据要写回这一行，而
    // `save_credentials` 可能因为发现重复账号而把它合并到别的行去。
    // 顺带取出登录标识：重登时预填进登录框，用户只需补密码与人机验证。
    let (operator_id, site_origin, login_identifier) = {
        let state = app_handle.state::<AppState>();
        let op = with_conn(&state, creds::load)?
            .ok_or_else(|| AppError::Config("请先选择运营商站点".into()))?;
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

    let url = url::Url::parse(&login::login_url(&site_origin))
        .map_err(|e| AppError::Config(format!("登录页地址不对: {e}")))?;

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
    .user_agent(login::WEBVIEW_USER_AGENT)
    .initialization_script(login::login_script(&site_origin, &login_identifier))
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
    .map_err(|e| AppError::Config(format!("打开登录窗口失败: {e}")))?;

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
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), async {
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
            let account = api::Client::new(&site_origin, &c.auth_token)?
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
        Ok(None) | Err(_) => {
            let _ = window.destroy();
            Ok(false)
        }
    }
}

/// 取一份**能用**的凭据：token 快过期时先静默续期。
///
/// 没有这一步的话，token 一过期用户就得重新走一遍 WebView 登录 —— 而 sub2api 的
/// `/auth/login` 有 20 次/分钟的限流，反复登录会把自己锁在外面。
async fn usable_operator(app_handle: &tauri::AppHandle) -> Result<creds::Operator, AppError> {
    let op = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, creds::load)?
            .ok_or_else(|| AppError::Config("请先选择运营商站点".into()))?
    };

    if op.token_looks_valid(chrono::Utc::now().timestamp()) {
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

    let mut renewed = creds::Operator {
        auth_token: fresh.auth_token,
        refresh_token: fresh.refresh_token.or(Some(refresh)),
        token_expires_at: fresh.token_expires_at,
        ..op
    };

    // 顺手刷一次账号身份：用户可能在运营商那边改了昵称或邮箱，而续期响应里没有账号信息
    // （`/auth/refresh` 只回 token），所以只有在这里额外打一次 profile 才发现得了。
    // 不刷的话站点选择器上会一直挂着旧标签 —— 而他改邮箱的动机往往就是「换个能认的」。
    //
    // **失败不影响续期**：token 已经换好、库也写了，标签陈旧只是显示问题。为它把整次
    // 续期判失败会让用户在「明明能用」的时候被挡住。
    match api::Client::new(&renewed.site_origin, &renewed.auth_token) {
        Ok(client) => match client.account().await {
            Ok(account) => {
                let label = account.display_name();
                let state = app_handle.state::<AppState>();
                if let Err(e) = with_conn(&state, |conn| {
                    creds::refresh_account_identity(conn, renewed.id, &label, &account.email)
                }) {
                    log::warn!("续期后刷新账号信息失败（不影响使用）: {e}");
                } else {
                    renewed.account_label = label;
                    renewed.login_identifier = account.email;
                }
            }
            Err(e) => log::warn!("续期后读取账号信息失败（不影响使用）: {e}"),
        },
        Err(e) => log::warn!("续期后构造客户端失败（不影响使用）: {e}"),
    }

    Ok(renewed)
}

/// 拉分组、为每组备好 sk、写成 codex provider。
#[tauri::command]
pub async fn operator_provision(app_handle: tauri::AppHandle) -> Result<ProvisionSummary, String> {
    do_provision(&app_handle).await.map_err(|e| e.to_string())
}

async fn do_provision(app_handle: &tauri::AppHandle) -> Result<ProvisionSummary, AppError> {
    let op = usable_operator(app_handle).await?;
    let client = api::Client::new(&op.site_origin, &op.auth_token)?;
    let mut result = provision::provision(&client, &op.device_id).await?;
    provision::sort_tiers(&mut result.tiers);

    // 写 provider 记录。这一段是同步的（碰 DB），所以拿完网络数据再做。
    let state = app_handle.state::<AppState>();
    let current = ProviderService::current(&state, AppType::Codex).unwrap_or_default();

    let mut tiers = Vec::new();
    for (idx, tier) in result.tiers.iter().enumerate() {
        let provider_id = provision::provider_id_for(&op.site_origin, tier.group_id);
        let display_name = provision::provider_display_name(&op.site_name, &tier.group_name);

        let provider = Provider {
            id: provider_id.clone(),
            name: display_name.clone(),
            settings_config: provision::settings_config_for(
                &tier.api_key,
                &display_name,
                &op.api_base_url,
                DEFAULT_MODEL,
            ),
            website_url: Some(op.site_origin.clone()),
            // aggregator 而不是 official：official 那条分类会触发一批只对官方订阅成立的
            // 逻辑（stale auth 清理、统一会话桶注入）。
            category: Some("aggregator".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: Some(idx),
            notes: None,
            meta: Some(managed_meta()),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };

        state
            .db
            .save_provider(AppType::Codex.as_str(), &provider)
            .map_err(|e| AppError::Database(format!("保存档位 {display_name} 失败: {e}")))?;

        tiers.push(TierInfo {
            is_current: current == provider_id,
            provider_id,
            group_id: Some(tier.group_id),
            group_name: tier.group_name.clone(),
            display_name,
            rate_multiplier: Some(tier.rate_multiplier),
        });
    }

    Ok(ProvisionSummary {
        keys_created: result.tiers.iter().filter(|t| t.key_was_created).count(),
        tiers,
        failures: result
            .failures
            .into_iter()
            .map(|(group_name, reason)| FailureInfo { group_name, reason })
            .collect(),
    })
}

/// 列出已备好的档位。
#[tauri::command]
pub fn operator_list_tiers(state: State<'_, AppState>) -> Result<Vec<TierInfo>, String> {
    list_tiers_impl(state.inner()).map_err(|e| e.to_string())
}

fn list_tiers_impl(state: &AppState) -> Result<Vec<TierInfo>, AppError> {
    let current = ProviderService::current(state, AppType::Codex).unwrap_or_default();
    let providers = ProviderService::list(state, AppType::Codex)?;

    let mut tiers: Vec<TierInfo> = providers
        .values()
        .filter(|p| is_managed(p))
        .map(|p| TierInfo {
            provider_id: p.id.clone(),
            // 倍率不在本地存 —— 它是服务端的定价，可能已经变了。要看倍率就重新 provision，
            // 那时会从服务端拿到当前值。这里返回 None 让 UI 知道"不知道"，而不是编一个 0。
            group_id: None,
            rate_multiplier: None,
            group_name: p.name.clone(),
            display_name: p.name.clone(),
            is_current: current == p.id,
        })
        .collect();

    // 按 provision 时写下的 sort_index 排（倍率低的在前）。provider_id 是哈希，
    // 按它排等于随机顺序。
    let order: std::collections::HashMap<&str, usize> = providers
        .values()
        .map(|p| (p.id.as_str(), p.sort_index.unwrap_or(usize::MAX)))
        .collect();
    tiers.sort_by_key(|t| {
        (
            order
                .get(t.provider_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
            t.provider_id.clone(),
        )
    });
    Ok(tiers)
}

/// 切换档位：退 ChatGPT → 切换 → 重开。
///
/// `quit_chatgpt` 由前端在用户确认弹窗后传 true。传 false 则只切换（用户自己管重启）。
#[tauri::command]
pub async fn operator_switch_tier(
    app_handle: tauri::AppHandle,
    provider_id: String,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, String> {
    switch_tier_impl(&app_handle, &provider_id, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}

async fn switch_tier_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, AppError> {
    let mut warnings = Vec::new();
    let mut was_running = false;

    // 1) 先退 ChatGPT，**退不掉就中止切换**。
    //
    // 为什么中止而不是「照写配置 + 提示手动重启」：ChatGPT 在有进行中的对话时会弹阻塞式
    // 确认框，用户点 Cancel 就是明确表示「先别动」。而它自己**会回写 config.toml** —— 这时
    // 硬写配置的结果是两边互相覆盖，用户既没切成、也不知道自己现在连的是哪个分组。
    if quit_chatgpt {
        match chatgpt_app::quit_and_wait() {
            // 没装 / 没在跑：不需要重开，切换照常。
            chatgpt_app::QuitOutcome::NotRunning => {}
            chatgpt_app::QuitOutcome::Quit => was_running = true,
            // 自动退出没成功（平台没实现 / 权限被拒 / 命令出错）：**照常切换**，提示用户
            // 自己重启。配置写进 config.toml 就已经生效，手动重启一样能用。
            chatgpt_app::QuitOutcome::NeedsManualRestart(why) => {
                warnings.push(format!("{why}，配置已切换，请手动重启 ChatGPT 让它生效。"));
            }
            // 唯一中止切换的情况：用户在确认框里点了取消，明确表示「先别动」。
            chatgpt_app::QuitOutcome::UserDeclined => {
                return Err(AppError::Config(
                    "ChatGPT 还在运行（它可能弹出了确认退出的对话框，或有进行中的对话）。\
                     请先手动退出它，然后重试切换。配置未改动。"
                        .into(),
                ));
            }
        }
    }

    // 2) 切换。走 cc-switch 既有链路，不另写落盘逻辑。
    //
    // **失败时必须把 ChatGPT 开回去**：我们已经把它关掉了，如果这里直接 `?` 返回，用户手上
    // 就是「ChatGPT 被关了、分组没切成、也没人告诉他现在是什么状态」。切换链路上有真实的
    // 失败点（settings_config 缺 auth 键、config.toml 语法校验失败），不是理论风险。
    //
    // 重开的是**旧 provider** —— 切换失败意味着配置没动，所以开回去就是原样。
    let switch_outcome = {
        let state = app_handle.state::<AppState>();
        ProviderService::switch(&state, AppType::Codex, provider_id)
    };

    let switched = match switch_outcome {
        Ok(s) => s,
        Err(e) => {
            if was_running {
                // 恢复失败也要说出来，但主错误是切换失败那条 —— 别让恢复的错盖住它。
                if let Err(re) = chatgpt_app::relaunch() {
                    return Err(AppError::Config(format!(
                        "切换失败：{e}。配置未改动，但重新打开 ChatGPT 也失败了：{re}，请手动打开它。"
                    )));
                }
                return Err(AppError::Config(format!(
                    "切换失败：{e}。配置未改动，已重新打开 ChatGPT。"
                )));
            }
            return Err(e);
        }
    };
    warnings.extend(switched.warnings);

    let provider_name = {
        let state = app_handle.state::<AppState>();
        ProviderService::list(&state, AppType::Codex)
            .ok()
            .and_then(|list| list.get(provider_id).map(|p| p.name.clone()))
            .unwrap_or_else(|| provider_id.to_string())
    };

    // 3) 重开 —— 只在我们确实把它关掉了的情况下。用户本来没开着，我们不该替他开。
    let mut relaunched = false;
    if was_running {
        match chatgpt_app::relaunch() {
            Ok(()) => relaunched = true,
            // 重开失败不回滚：配置已经切好了，用户手动打开 ChatGPT 就能用上新分组。
            Err(e) => warnings.push(format!("重新打开 ChatGPT 失败：{e}")),
        }
    }

    Ok(SwitchTierResult {
        provider_name,
        chatgpt_was_running: was_running,
        chatgpt_relaunched: relaunched,
        warnings,
    })
}

/// 登出当前站：清凭据，保留站点与 device_id。
#[tauri::command]
pub fn operator_logout(state: State<'_, AppState>) -> Result<(), String> {
    with_conn(state.inner(), |conn| {
        // 只登出当前站，别的站的登录态不动。
        match creds::load(conn)? {
            Some(op) => creds::clear_credentials(conn, op.id),
            None => Ok(()),
        }
    })
    .map_err(|e| e.to_string())
}

/// 列出全部已添加的站点。
#[tauri::command]
pub fn operator_list_sites(state: State<'_, AppState>) -> Result<Vec<SiteInfo>, String> {
    with_conn(state.inner(), |conn| {
        Ok(creds::list(conn)?
            .into_iter()
            .map(|op| SiteInfo {
                label: op.display_label(),
                logged_in: op.token_looks_valid(chrono::Utc::now().timestamp()),
                id: op.id,
                site_origin: op.site_origin,
                site_name: op.site_name,
                account_label: op.account_label,
                is_current: op.is_current,
            })
            .collect())
    })
    .map_err(|e| e.to_string())
}

/// 切换当前站点。
#[tauri::command]
pub fn operator_switch_site(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    with_conn(state.inner(), |conn| {
        // 先确认这行还在 —— 用户可能在别处删掉了它。静默成功会让 UI 显示切换成功而实际没切。
        if creds::get(conn, id)?.is_none() {
            return Err(AppError::Config("这个站点已经不存在了".into()));
        }
        creds::set_current(conn, id)
    })
    .map_err(|e| e.to_string())
}

/// 删掉一个站点。
///
/// **不删它已经生成的 provider 记录** —— 那些是用户可能正在用的配置，删站点不该顺带把
/// codex 的当前配置抽掉。要清理走 provider 列表。
#[tauri::command]
pub fn operator_remove_site(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    with_conn(state.inner(), |conn| creds::remove(conn, id)).map_err(|e| e.to_string())
}

/// 余额。
#[tauri::command]
pub async fn operator_balance(app_handle: tauri::AppHandle) -> Result<api::Balance, String> {
    let op = usable_operator(&app_handle)
        .await
        .map_err(|e| e.to_string())?;
    let client = api::Client::new(&op.site_origin, &op.auth_token).map_err(|e| e.to_string())?;
    client.balance().await.map_err(|e| e.to_string())
}

/// 这条 provider 是不是 LoongPort 管的。
///
/// 判据是 id 前缀 —— 与 [`provision::provider_id_for`] 生成的前缀对应。用前缀而不是往 meta
/// 里加字段：`ProviderMeta` 是上游的结构，加字段会扩大与上游 merge 的接触面。
fn is_managed(p: &Provider) -> bool {
    p.id.starts_with("loongport-")
}

/// 托管 provider 的 meta。
///
/// **`apiFormat` 必须显式写 `openai_responses`**：不写它会落到 `ProxyChat` profile，而那是
/// 唯一会去 spawn `codex debug models --bundled` 子进程的分支。sub2api 的 openai 网关原生走
/// Responses，写对了就永远走内嵌模板、不起子进程。
fn managed_meta() -> crate::provider::ProviderMeta {
    crate::provider::ProviderMeta {
        api_format: Some("openai_responses".to_string()),
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

    #[test]
    fn managed_detection_matches_generated_ids_only() {
        // 正面：provision 生成的 id 必须被认出来。
        let real = provision::provider_id_for("https://bestapi.store", 42);
        assert!(is_managed(&provider_with_id(&real)));

        // 反面：用户自己加的 provider 不能被当成托管的（否则会被 provision 覆盖）。
        for id in ["custom-1", "codex-official", "", "LoongPort-1"] {
            assert!(!is_managed(&provider_with_id(id)), "id: {id}");
        }
    }

    #[test]
    fn managed_meta_pins_api_format_to_native_responses() {
        // 不写 apiFormat 会落到 ProxyChat profile —— 那是唯一会 spawn codex 子进程的分支。
        assert_eq!(
            managed_meta().api_format.as_deref(),
            Some("openai_responses")
        );
    }

    #[test]
    fn default_site_is_the_placeholder_from_the_requirement() {
        assert_eq!(DEFAULT_SITE, "bestapi.store");
    }
}
