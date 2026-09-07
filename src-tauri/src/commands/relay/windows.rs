//! 站点侧窗口（充值/查看用量）的开启与分派。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;

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
pub(crate) async fn dispatch_site_window<R: tauri::Runtime>(
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
pub(crate) async fn open_sub2api_site_window<R: tauri::Runtime>(
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
    let client = sub2api::Client::new(
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
    // 只 emit 事件、不在这里查余额：查余额要发 HTTP，而这个回调不能 await ——
    // 站点级缓存（看板用）的失效+补刷走 spawn，不受此限。
    let close_handle = app_handle.clone();
    let close_site = op.site_origin.clone();
    let close_api_base = op.api_base_url.clone();
    built.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = handle_for_close.emit(PURCHASE_CLOSED, closed_relay_id);
            let handle = close_handle.clone();
            let (site, api_base) = (close_site.clone(), close_api_base.clone());
            // db 在事件内取：开窗路径不碰全局 state（headless 窗口测试没有 manage）
            tauri::async_runtime::spawn(async move {
                let db = handle.state::<AppState>().db.clone();
                crate::services::site_balance_refresh::refresh_after_purchase(
                    &db,
                    Some(handle),
                    &site,
                    &api_base,
                );
            });
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

    match sub2api::refresh_token(&op.site_origin, &refresh).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relay::test_support::*;

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
        // sub2api.rs / 既有测试形状，这里只对照黑名单）。
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
}
