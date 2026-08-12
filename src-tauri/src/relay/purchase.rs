//! 充值 WebView：把登录态注入进中转站的充值页，让用户不必再登一次。
//!
//! ## 与 [`super::login`] 方向相反
//!
//! 登录窗**读** localStorage（等站点把凭据写进去，再捞出来回传原生侧）；
//! 充值窗**写** localStorage（我们已经有凭据了，注入进去让站点认出「已登录」）。
//! 两者共用的是同一组键名常量与同一个 User-Agent，别各写一份。
//!
//! ## 为什么要注入，而不是直接开系统浏览器
//!
//! 系统浏览器里用户是未登录状态（我们的凭据在本地 DB 里，不在浏览器里），
//! 打开充值页只会看到登录框 —— 他得再输一遍账号密码。而登录态一注入，
//! 点开就是可以直接付款的页面。
//!
//! ## ⚠️ 三条会静默出错的决定（每条都有实测或源码依据）
//!
//! ### 1. 必须 `.incognito(true)`，且它**不影响**注入
//!
//! 这两件事容易被混为一谈，实测确认是两个独立维度：
//!
//! - `initialization_script` 在 macOS 下是 `WKUserScript(AtDocumentStart)`，
//!   与页面脚本**同一个 JS 世界、同一个 origin** ⇒ 写进去的 localStorage 页面读得到。
//! - `incognito` 只决定这份 localStorage **落不落盘**（`nonPersistentDataStore`），
//!   不决定本进程内读不读得到。
//!
//! 不加它的后果**不是**「多存了点数据」，而是一个真实的资损路径（已实测复现）：
//! 持久 profile 是全 app 共享的，A 账号登录过就留下了 A 的 `refresh_token`；
//! 在 B 那一行点充值时我们只注入 B 的 `auth_token`，于是页面里是
//! **B 的 access token + A 残留的 refresh token**。站点的 401 拦截器拿
//! `localStorage.getItem('refresh_token')` 去续期，会用 A 的 refresh token
//! 换出 A 的 access token 覆盖写进 `auth_token` ⇒ **用户在 B 行点充值，钱充进 A 账号。**
//!
//! 而且 `incognito` 给的是**每个窗口各自独立**的内存 store（wry 每次 build 各调一次
//! `nonPersistentDataStore`），所以充值窗既读不到持久 profile、也读不到登录窗那份 ——
//! 注入是它唯一的登录态来源，正是我们要的。
//!
//! ### 2. **只注入两个键**，有意不写 `refresh_token` 与 `token_expires_at`
//!
//! sub2api 的 refresh token 是**一次性轮换**的：`RefreshTokenPair` 在换发新对之前
//! 先 `DeleteRefreshToken(tokenHash)` 把旧的**立刻作废**
//! （`auth_service.go:1631`，注释原话「Token轮转：立即使旧Token失效」）。
//! 把它注入进去之后：站点会起一个续期定时器（`scheduleTokenRefreshAt`）用掉它 ⇒
//! **本仓 DB 里的那份当场失效** ⇒ 下次本仓自己续期时服务端认不出它、返回
//! `REFRESH_TOKEN_INVALID`（`:1565`）⇒ **用户被迫重新登录整个中转站**。
//!
//! ⚠️ 早先这里写的错误码是 `REFRESH_TOKEN_REUSED` —— **那个码只被定义、从没被返回过**
//! （全仓唯一一处是 `auth_service.go:38` 的定义）。实际是 `REFRESH_TOKEN_INVALID`。
//! 结论不变（旧 token 确实当场失效），只是别再引那个不存在的码。
//! 另外「撤销整个会话家族」（`DeleteTokenFamily`）**不由复用触发** ——
//! 那是用户被删/被禁、TokenVersion 变化、会话绑定不匹配这几条路走的。
//!
//! 而站点判「已登录」只看两个：`isAuthenticated = !!token.value && !!user.value`
//! ⇒ `auth_token` + [`AUTH_USER_KEY`] 就够。充值是短会话（几分钟），
//! access token 到期由站点自己的 401 拦截器处理，不需要我们替它准备续期能力。
//!
//! **少注入两个键既避开了抢 token，又天然避开了那个毫秒/秒的单位陷阱**
//! （本仓 DB 存秒、站点存毫秒）—— 两个问题一起消失。
//!
//! #### ⚠️ 这个决定的代价，以及为什么仍然选它
//!
//! 代价是真实的：站点每 60 秒调一次 `/auth/me`（`auth.ts:141-152` 的
//! `startAutoRefresh`），access token 一过期那次就 401，而**没有 refresh token 的
//! 401 分支会清掉登录态并硬跳 `/login`**（`api/client.ts:292-302`，是
//! `window.location.href` 整页导航而不是 router push）⇒ 付款页在无人操作的情况下
//! 被打断。注入四个键则会被站点的续期机制自动续上，不会有这一幕。
//!
//! 仍然选「只注入两个」，因为两边的代价不对称：
//!
//! | 选择 | 坏后果 | 触发条件 |
//! |---|---|---|
//! | 注入四个键 | 本仓的登录态被搞坏，**用户被迫重新登录整个中转站** | **必然发生**（站点一定会用掉那个一次性 refresh token） |
//! | 只注入两个 | 付款页可能中途掉登录态 | 只在「access token 剩余寿命撑不完这次付款」时 |
//!
//! 而后者已经被 `commands::relay::ensure_token_outlasts_a_payment` 压到很小：
//! 开窗前**无条件**换一把新 token，让它以完整 TTL 起步（用的是**我们自己的**那把
//! refresh token、续完写回库，不存在被站点抢走的问题）。sub2api 的 access token
//! 默认 24 小时（`config.go`: `jwt.expire_hour = 24`），所以正常部署下碰不到。
//!
//! **残留风险**：站点把 `access_token_expire_minutes` 配得很短（比如 5 分钟）时，
//! 续完也只有 5 分钟 ⇒ 慢付款（USDT 转账等链上确认）仍可能被打断。
//! 那时用户的出路是在浏览器里付款。**不为这种配置去注入 refresh token** ——
//! 那会把一个「少数部署下的偶发中断」换成「所有人必然被迫重登」。
//!
//! ### 3. `auth_user` 的值直接用 `/user/profile` 的响应，不必另打 `/auth/me`
//!
//! 站点自己的 `refreshUser()` 存进 `auth_user` 的是 `/auth/me` 的响应去掉 `run_mode`。
//! 而逐字段核对过：**`/user/profile` 的 `data` 与那个结果完全相同**（服务端是同一个
//! `userProfileResponseFromService` builder 构造的）⇒ 复用本仓已有的
//! [`super::api::Client`] 打一次 `/user/profile` 就行，既不必新增端点，
//! 也不必做「删 `run_mode`」这一步。

use crate::error::AppError;

use super::login::{AUTH_TOKEN_KEY, AUTH_USER_KEY};

/// 充值窗口 label 的前缀。**每个中转站一个自己的窗口**，label 是 `<前缀><relay_id>`。
///
/// 前缀必须与另外两个 label 都不相同：
///
/// 1. 与 `tauri.conf.json` 的 `app.windows` 里的 label 重名会在 setup 时 panic。
/// 2. 与 `MAIN_WINDOW_LABEL` 重名会被主窗那个「最小化到托盘」的 `CloseRequested`
///    拦截器吃掉，关不掉还占着 label。
/// 3. 与 `LOGIN_WINDOW_LABEL` 重名会让「开充值窗」把用户正在填的登录窗销毁掉。
///
/// ## 为什么按行分窗，而不是全局一个（review 抓出的资损面）
///
/// 全局单窗时，「开新窗前销毁残留窗」这条（照 `do_login` 抄的）会在充值场景下变成
/// **销毁一个可能正在付款的窗口**：用户在 A 行下了单、页面已经显示二维码或跳到了
/// 支付网关，此时他回主窗点 B 行的余额 ⇒ A 那个窗连同它的 incognito 存储一起消失。
/// 服务端的订单与网关扣款并不会因此取消，但用户失去了轮询与确认页面，
/// 很可能重新下一单 ⇒ 两笔待支付、甚至重复付款。
///
/// 登录窗可以全局单例，是因为「上一个登录窗已经没人在等它的凭据了」；
/// 而充值窗背后是**已经发生的钱**，两者的代价完全不对称。
///
/// 按 id 分窗之后：不同账号各有自己的窗口互不干扰；**同一行**再点则聚焦已有的那个
/// （而不是销毁重开 —— 那同样会打断这一行自己的付款流程）。
pub const PURCHASE_WINDOW_LABEL_PREFIX: &str = "loongport-purchase-";

/// 某个中转站的充值窗 label。
pub fn window_label(relay_id: i64) -> String {
    format!("{PURCHASE_WINDOW_LABEL_PREFIX}{relay_id}")
}

/// 一次性注入 marker 的 `sessionStorage` 键名。
///
/// 带 `loongport_` 前缀避免与站点自己的键撞（站点用的是 `auth_expired` 这类裸名字）。
const INJECTED_MARKER_KEY: &str = "loongport_purchase_injected";

/// 充值页 URL。
///
/// sub2api 在公开设置里声明 `payment_enabled`。在线支付关闭时，站点的路由守卫会
/// 把 `/purchase` 重定向到 dashboard；此时唯一可用的充值入口是兑换码页 `/redeem`。
/// 开启时仍走 `/purchase`，里面是充值还是订阅由用户自己选。
pub fn purchase_url(site_origin: &str, payment_enabled: Option<bool>) -> String {
    let path = if payment_enabled == Some(false) {
        "redeem"
    } else {
        "purchase"
    };
    format!("{site_origin}/{path}")
}

/// 生成注入脚本：把登录态写进 localStorage，让站点的 router 守卫认出「已登录」。
///
/// `auth_user` 是 `/user/profile` 的 `data` 原样（见模块文档第 3 条）。
///
/// ## 为什么必须 `initialization_script` 而不是 `eval`
///
/// 站点是 Vue SPA，它的 router 守卫在**应用启动那一刻**就读 localStorage 判登录态。
/// `eval` 在页面加载完之后才跑 ⇒ 那时守卫已经把用户重定向到 `/login` 了，
/// 再写进去也来不及。`initialization_script` 是 `AtDocumentStart`、早于页面所有脚本。
///
/// 顺带一个好处：它走 `WKUserScript`，**不受页面 CSP 管**。
///
/// ## ⚠️ 注入必须是**一次性**的（review 抓出的死循环）
///
/// `initialization_script` 是 **per-document** 的：每一次顶层导航都会再跑一遍。
/// 而站点的 401 拦截器在 token 失效时会**删掉那四个键并 `window.location.href = '/login'`**
/// （`upstream/sub2api/frontend/src/api/client.ts:292-301`）—— 那是一次新的文档加载
/// ⇒ 本脚本又执行，把刚被删掉的**同一枚失效 token** 写回去 ⇒ router 守卫再判「已登录」
/// 并跳离登录页 ⇒ 初始化时又 401 ⇒ 无限重载，用户只能关窗，而且看不到付款结果。
///
/// 所以用 `sessionStorage` 放一个一次性 marker：**同一个标签页内只注入一次**。
/// 为什么用 `sessionStorage` 而不是模块级变量：脚本每个 document 都是新的执行环境，
/// 变量根本活不过导航；而 `sessionStorage` 在同一个标签页的同源导航间存续，
/// 且随窗口关闭消失（我们本来就是 incognito、内存 store）。
///
/// 副作用是「用户在充值窗里主动登出、再想重新登录」时我们不再帮他注入 —— 那是**正确的**：
/// 那时他就该看到登录页。
pub fn inject_script(site_origin: &str, auth_token: &str, auth_user: &serde_json::Value) -> String {
    // origin 来自用户输入，含引号会破坏脚本语法 —— 这是注入面。
    let origin_literal = serde_json::to_string(site_origin).unwrap_or_else(|_| "\"\"".to_string());

    // 要写进 localStorage 的键值对**先构造成一份显式清单**，再由它生成 JS。
    // 这样「写了哪几个键」就是一个可以直接断言的值，而不是从脚本文本里猜出来的
    // （review 指出：数 `setItem(` 出现几次是脆的 —— 属性赋值形式
    // `localStorage['refresh_token'] = x` 同样能写键，却一次 `setItem(` 都不出现）。
    let entries = injected_entries(auth_token, auth_user);
    let writes = entries
        .iter()
        .map(|(key, value_literal)| {
            format!("    window.localStorage.setItem('{key}', {value_literal});")
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"(function () {{
  'use strict';

  // 只在顶层 frame 跑：同源 iframe 会让脚本多执行一份（写两遍虽无害，但没必要）。
  if (window.top !== window.self) return;

  // origin 守卫：跳到第三方（支付网关的授权页之类）时不执行 ——
  // 我们的凭据只该出现在中转站自己的页面上。
  var ALLOWED_ORIGIN = {origin_literal};
  if (window.location.origin !== ALLOWED_ORIGIN) return;

  try {{
    // 一次性守卫：本脚本每次顶层导航都会重跑，而站点的 401 拦截器失效时会
    // 删掉认证键并跳 /login（那是一次新导航）。不设这个 marker 就会把刚被删掉的
    // 失效 token 写回去 ⇒ 登录页与 dashboard 之间无限重载。
    if (window.sessionStorage.getItem('{INJECTED_MARKER_KEY}')) return;
    window.sessionStorage.setItem('{INJECTED_MARKER_KEY}', '1');

    // 写哪几个键由 `injected_entries` 决定（就那两个）——
    // 为什么不写另外两个见 purchase.rs 的模块文档第 2 条。
{writes}
  }} catch (e) {{
    // 存储不可用（隐私模式的极端配置）。什么都做不了 —— 用户会看到登录页，
    // 那比抛一个他看不懂的错误好。
  }}
}})();
"#
    )
}

/// 要注入的 localStorage 键值对清单：`(键名, 已编码成 JS 字面量的值)`。
///
/// ## 为什么单独抽出来
///
/// 「究竟写了哪几个键」是本模块唯一真正危险的决定（多写一个 `refresh_token`
/// 就会让用户被迫重登，见模块文档第 2 条）。把它做成一个**返回值**而不是散在
/// 模板字符串里，闸就能直接断言键集合，而不是去数脚本文本里 `setItem(` 出现几次
/// —— 后者拦不住属性赋值形式（`localStorage['x'] = v` 一样能写键）。
fn injected_entries(
    auth_token: &str,
    auth_user: &serde_json::Value,
) -> Vec<(&'static str, String)> {
    // ⚠️ **`auth_user` 要两次序列化**：它在 localStorage 里的值本身就是一段 JSON
    // **字符串**（站点存的是 `JSON.stringify(userData)`，读时 `JSON.parse`）。
    // 第一层把对象变成那段 JSON 文本，第二层把文本安全编码成 JS 字符串字面量。
    // 少一层的话脚本里会出现裸对象 ⇒ `setItem` 存成 `[object Object]` ⇒ 站点 parse 抛异常。
    let user_json = serde_json::to_string(auth_user).unwrap_or_else(|_| "{}".to_string());

    vec![
        (
            AUTH_TOKEN_KEY,
            serde_json::to_string(auth_token).unwrap_or_else(|_| "\"\"".to_string()),
        ),
        (
            AUTH_USER_KEY,
            serde_json::to_string(&user_json).unwrap_or_else(|_| "\"{}\"".to_string()),
        ),
    ]
}

/// 从 `/user/profile` 的响应里取出可以当 `auth_user` 用的那一层。
///
/// ## 为什么参数是 [`serde_json::Value`] 而不是一个窄 DTO
///
/// 本模块其它地方都用窄 DTO（只解要用的字段），这里**必须**反着来：
/// 这个值是要**原样交给站点前端**的，站点会拿它渲染用户名、头像、余额、
/// 各种绑定状态（`userProfileResponse` 有 22 + 14 个字段）。用窄 DTO 会把
/// 没声明的字段**吞掉**，站点那边就变成「登录了但用户信息一片空白」——
/// 而且每次 sub2api 加字段都要跟着改，那正是窄 DTO 不该管的事。
///
/// 这条与模块开头「只取用得上的字段」的惯例不冲突：**取用得上的字段是为了
/// 隔离上游变更，而这里的用途恰恰是转发**。
pub fn auth_user_from_profile(profile: serde_json::Value) -> Result<serde_json::Value, AppError> {
    // 服务端的业务信封是 `{code, message, data}`，`auth_user` 该是 `data` 那一层
    // （站点自己存的就是响应体的 `data`）。
    //
    // 不用 `Envelope<T>` 走一遍：那个类型是私有的、且带 code 校验 —— 而调用方
    // （`Client::send`）已经校验并剥掉信封了，这里拿到的本来就是 data。
    // 留这个函数是为了把「哪一层」这个决定写在一处，而不是散在调用点。
    // 判据是「**有没有 `id`**」，不是「是不是 null」。
    //
    // 只判 null 会让 `{}` / `[]` / 一个 JSON 字符串都通过（review 抓出）——
    // 那些值注入进去是**合法的非空 JSON**，站点的 `checkAuth` 闸
    // （`if (savedToken && savedUser)`）照样过，于是用户看到一个「登录了但用户信息
    // 一片空白」的充值页；而真正的原因（profile 读取降级）被吞掉了。
    //
    // 用 `id` 当判据：它是 `dto.User` 的第一个字段、账号的主键，
    // 任何一个真实的 profile 响应都必然带它。
    if profile.get("id").is_none() {
        return Err(AppError::Config(
            "读取账号信息失败：响应里没有账号 id，无法带登录态打开充值页".into(),
        ));
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_user() -> serde_json::Value {
        serde_json::json!({
            "id": 42,
            "email": "me@x.com",
            "username": "阿伦",
            "role": "user",
            "balance": 12.34,
            "frozen_balance": 0.0,
            "wechat_bound": true
        })
    }

    #[test]
    fn purchase_url_uses_purchase_when_online_payment_is_enabled() {
        assert_eq!(
            purchase_url("https://bestapi.store", Some(true)),
            "https://bestapi.store/purchase"
        );
    }

    #[test]
    fn purchase_url_uses_redeem_when_online_payment_is_disabled() {
        assert_eq!(
            purchase_url("https://wawapii.com", Some(false)),
            "https://wawapii.com/redeem"
        );
    }

    #[test]
    fn purchase_url_keeps_legacy_sites_on_purchase_when_the_flag_is_absent() {
        assert_eq!(
            purchase_url("https://legacy.example", None),
            "https://legacy.example/purchase"
        );
    }

    #[test]
    fn window_labels_are_per_relay_and_collide_with_no_other_window() {
        // **按行分窗**是资损面的修法：全局单窗时「开新窗前销毁残留窗」会销毁一个
        // 可能正在付款的窗口（服务端订单不会因此取消，用户却失去确认页面）。
        assert_ne!(window_label(1), window_label(2), "不同中转站必须是不同窗口");

        for id in [1_i64, 42, -1] {
            let label = window_label(id);
            // 与主窗重名 → 被那个「最小化到托盘」的拦截器吃掉，关不掉还占 label。
            assert_ne!(label, crate::MAIN_WINDOW_LABEL);
            // 与登录窗重名 → 开充值窗会把用户正在填的登录窗销毁掉。
            assert_ne!(label, super::super::login::LOGIN_WINDOW_LABEL);
            // 前缀是 `lib.rs` 那道 window-state 过滤的判据（充值窗不进状态跟踪，
            // 否则会继承 Retina 上尺寸翻倍那个 bug）。label 不带前缀就漏出去了。
            assert!(
                label.starts_with(PURCHASE_WINDOW_LABEL_PREFIX),
                "label 必须带前缀，window-state 的过滤靠它: {label}"
            );
        }
    }

    #[test]
    fn injects_exactly_two_keys_and_never_the_refresh_token() {
        // ⭐ 这条是本模块最重要的闸。
        //
        // 注入 `refresh_token` 会让站点的续期定时器把那个**一次性**token 用掉，
        // 本仓 DB 里的那份随即失效 ⇒ 下次续期被服务端判成重放
        // （REFRESH_TOKEN_REUSED）⇒ 用户被迫重新登录。
        //
        // 「顺手把四个键都注入」看起来更完整，所以这条必须是断言而不是注释。
        //
        // 断言的是 `injected_entries` 的**键集合**，不是脚本文本里 `setItem(` 出现几次
        // （review 指出后者是脆的：属性赋值形式 `localStorage['refresh_token'] = x`
        // 一样能写键，却一次 `setItem(` 都不出现 ⇒ 那种退化能骗过计数式断言）。
        let keys: Vec<&str> = injected_entries("tok-abc", &sample_user())
            .into_iter()
            .map(|(k, _)| k)
            .collect();

        assert_eq!(
            keys,
            vec![
                super::super::login::AUTH_TOKEN_KEY,
                super::super::login::AUTH_USER_KEY
            ],
            "注入的键集合必须严格是这两个：多一个 refresh_token 就会让用户被迫重登"
        );

        // 再确认那两个禁止项一个字都没进最终脚本（含属性赋值等任何形式）。
        let s = inject_script("https://bestapi.store", "tok-abc", &sample_user());
        for forbidden in [
            super::super::login::REFRESH_TOKEN_KEY,
            super::super::login::TOKEN_EXPIRES_AT_KEY,
        ] {
            assert!(
                !s.contains(forbidden),
                "绝不能把 {forbidden} 写进注入脚本 —— refresh token 是一次性的\
                 （写了会让本仓续期被判重放），而 expires 正是站点续期定时器的触发条件：{s}"
            );
        }
    }

    #[test]
    fn injection_is_once_per_tab_so_a_dead_token_is_never_written_back() {
        // ⭐ review 抓出的死循环：`initialization_script` 是 per-document 的，
        // 而站点的 401 拦截器在 token 失效时会删掉认证键并
        // `window.location.href = '/login'`（`api/client.ts:292-301`）——
        // 那是一次新的文档加载 ⇒ 本脚本又跑 ⇒ 把刚被删掉的**同一枚失效 token** 写回去
        // ⇒ router 又判「已登录」跳离登录页 ⇒ 无限重载，用户只能关窗。
        let s = inject_script("https://bestapi.store", "tok", &sample_user());

        // marker 必须**先读后写**，且早于任何 localStorage 写入 —— 顺序反了就不起作用。
        let guard = s
            .find(&format!("sessionStorage.getItem('{INJECTED_MARKER_KEY}')"))
            .expect("必须有一次性守卫");
        let set_marker = s
            .find(&format!("sessionStorage.setItem('{INJECTED_MARKER_KEY}'"))
            .expect("必须写下 marker，否则守卫永远不成立");
        let first_write = s
            .find("localStorage.setItem(")
            .expect("总得写 localStorage");

        assert!(guard < set_marker, "要先读 marker 再写它");
        assert!(
            set_marker < first_write,
            "marker 必须在写认证键之前落下，否则中途异常会让守卫失效"
        );
        // 守卫必须是 early return，不然读了也白读。
        assert!(
            s.contains(&format!(
                "if (window.sessionStorage.getItem('{INJECTED_MARKER_KEY}')) return;"
            )),
            "守卫要 early return：{s}"
        );
    }

    #[test]
    fn auth_user_is_a_json_string_not_a_bare_object() {
        // 站点存的是 `JSON.stringify(userData)`，读的时候 `JSON.parse` 它。
        // 少一层序列化会让脚本里出现裸对象字面量 ⇒ setItem 存成 "[object Object]"
        // ⇒ 站点 parse 时抛异常 ⇒ 落回登录页。
        let s = inject_script("https://bestapi.store", "tok", &sample_user());

        // 值必须是**带引号的字符串**，且内部的引号是转义过的。
        assert!(
            s.contains(r#"setItem('auth_user', "{\"balance\":12.34"#)
                || s.contains(r#"setItem('auth_user', "{\""#),
            "auth_user 的值必须是 JSON 字符串字面量（内层引号转义）：{s}"
        );
        assert!(
            !s.contains("setItem('auth_user', {"),
            "不能是裸对象 —— 那会存成 [object Object]：{s}"
        );
    }

    #[test]
    fn profile_fields_are_forwarded_verbatim_not_narrowed() {
        // 窄 DTO 会吞掉没声明的字段，站点那边就是「登录了但信息一片空白」。
        // 这条钉住「原样转发」：随便挑几个我们自己压根不关心的字段，它们也必须在。
        let s = inject_script("https://bestapi.store", "tok", &sample_user());
        for field in ["wechat_bound", "role", "username", "阿伦"] {
            assert!(s.contains(field), "字段 {field} 被吞掉了：{s}");
        }
    }

    #[test]
    fn script_guards_on_origin_so_credentials_never_reach_a_third_party() {
        // 充值流程会跳到支付网关（stripe / airwallex 之类）。那些页面上不该出现我们的 token。
        let s = inject_script("https://bestapi.store", "tok", &sample_user());
        assert!(s.contains(r#"ALLOWED_ORIGIN = "https://bestapi.store""#));
        assert!(s.contains("window.location.origin !== ALLOWED_ORIGIN"));
    }

    #[test]
    fn every_interpolated_value_is_json_encoded_so_quotes_cannot_break_out() {
        // 三个值都来自外部（origin 是用户输入、token 与 profile 来自服务端）。
        //
        // ⚠️ **断言的是「字面量能原样解回来」，不是「脚本里没出现某个探针字符串」**
        // （review 指出后者是结构性无效的：`serde_json` 会把引号转义成 `\"`，
        // 所以未转义形态**必然**不出现 —— 哪怕换成一个只转义反斜杠的坏编码器，
        // 那种断言照样通过）。round-trip 才真的在验「编码是对的」。
        let nasty_token = "tok\" + alert(2) + \"";
        let entries = injected_entries(nasty_token, &sample_user());

        let (_, token_literal) = &entries[0];
        let decoded: String = serde_json::from_str(token_literal)
            .expect("注入的 token 必须是一个合法的 JSON 字符串字面量");
        assert_eq!(
            decoded, nasty_token,
            "字面量解回来必须与原值逐字节相同，否则编码有问题"
        );

        // origin 那一处同理（它不经 `injected_entries`，单独验一次）。
        let nasty_origin = "https://evil\" + alert(1) + \"";
        let s = inject_script(nasty_origin, "t", &sample_user());
        let line = s
            .lines()
            .find(|l| l.contains("ALLOWED_ORIGIN ="))
            .expect("必须有 origin 守卫");
        let literal = line
            .split_once("= ")
            .map(|(_, rest)| rest.trim_end_matches(';'))
            .expect("取出 origin 字面量");
        let decoded_origin: String =
            serde_json::from_str(literal).expect("origin 也必须是合法 JSON 字符串字面量");
        assert_eq!(decoded_origin, nasty_origin);
    }

    #[test]
    fn keys_come_from_the_shared_constants_not_local_literals() {
        // 与登录窗共用同一组常量 —— 那是「提成常量」的全部意义。
        // 若有人在这里改回字面量，login.rs 改键名时这里不会跟着改。
        assert_eq!(AUTH_TOKEN_KEY, "auth_token");
        assert_eq!(AUTH_USER_KEY, "auth_user");
        let s = inject_script("https://x.dev", "t", &sample_user());
        assert!(s.contains(&format!("setItem('{AUTH_TOKEN_KEY}'")));
        assert!(s.contains(&format!("setItem('{AUTH_USER_KEY}'")));
    }

    #[test]
    fn a_degenerate_profile_is_a_visible_error_not_a_silent_blank_page() {
        // 拿不到真正的账号信息时，注入一个「合法但没内容」的 auth_user 比报错更糟：
        // 它能过站点的 `checkAuth` 闸（那只判非空），于是用户看到一个**登录了但
        // 用户信息一片空白**的充值页，而真正的原因被吞掉了。
        //
        // ⚠️ 四种形状都要拦 —— 初版只判 `null`，`{}` / `[]` / 字符串全都放过去了
        // （review 抓出）。判据是「有没有 id」。
        for degenerate in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!("not-an-object"),
            // 有别的字段但**没有 id** —— 上游改名或代理改写响应时的形状。
            serde_json::json!({ "email": "me@x.com" }),
        ] {
            let err = auth_user_from_profile(degenerate.clone())
                .expect_err(&format!("{degenerate} 该被拦下"))
                .to_string();
            assert!(err.contains("账号"), "错误要说清是账号信息的问题：{err}");
        }
    }

    #[test]
    fn a_real_profile_passes_through_untouched() {
        let profile = sample_user();
        let out = auth_user_from_profile(profile.clone()).unwrap();
        assert_eq!(out, profile, "不该改动任何字段");
    }
}
