//! 中转站的窄 DTO 与鉴权 HTTP 客户端。**当前实现是 sub2api 形状。**
//!
//! 只覆盖用得到的端点，字段只取用得上的那几个（sub2api 的 `Group` 有 33 个字段、
//! `APIKey` 有 30+，全解出来等于把上游的字段变更面全接进来）：
//!
//! | 端点 | 用途 | 鉴权 |
//! |---|---|---|
//! | `GET /api/v1/settings/public` | 域名探测（是不是 sub2api 站） | 无 |
//! | `GET /api/v1/groups/available` | 拉可用分组 | Bearer(JWT) |
//! | `GET /api/v1/keys` | 认领已有 sk（明文返回） | Bearer(JWT) |
//! | `POST /api/v1/keys` | 建新 sk | Bearer(JWT) |
//! | `GET /api/v1/user/profile` | 余额 + 账号身份 | Bearer(JWT) |
//! | `GET /v1/sub2api/billing` | 一把 sk 的最终倍率 | **Bearer(sk)** |
//!
//! ## 接第二家中转站（如 new-api）时改这里
//!
//! 本模块是**唯一**与中转站 HTTP 协议耦合的地方 —— 其它模块只碰这里导出的类型。
//! 所以接 new-api 时：把 [`Client`] 的方法抽成 trait、按中转站各写一份实现，
//! 上层（`provision` / `commands::relay`）不用动。
//!
//! 已经为此让过路的两处（别推翻）：
//! - `creds` 的登录标识叫 `login_identifier` 而非 `account_email` ——
//!   new-api 用 username 登录，sub2api 用 email，中立命名两边都装得下
//! - [`PROBE_ADAPTER`] 拥有 sub2api 指纹；通用候选遍历与收敛在 `relay::discovery`
//!
//! ## 四条会静默出错的约定
//!
//! 1. **响应是信封** `{code, message, data}`，业务数据在 `data` 里，且 **`code` 是整数、
//!    成功是 `0`**（`message` 才是 `"success"`）。HTTP 200 不代表业务成功。
//! 2. **鉴权中间件（401/403）用的是另一套信封**，`code` 在那边是**字符串**错误码。所以
//!    401 的分类不能复用业务信封的解析，见 [`classify_401`]。
//! 3. **`/groups/available` 返回的是平数组**，不是分页信封；`/keys` 才是分页
//!    （`{items, total, page, page_size, pages}`）。两者形状不同，别复用同一个解析。
//! 4. **`base_url` 按 CLI 分形状**：sub2api 后台的 `api_base_url` 可能是空串
//!    （bestapi.store 实测），且 codex 要 `/v1` 结尾而 Claude Code 要不带 `/v1` 的站点根
//!    （它自己拼 `/v1/messages`）。一律走 [`base_url_for`]，别直接用 `api_base_url`。

use serde::{Deserialize, Serialize};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::relay::backend::{BackendKind, DetectedSite, ProbeAdapter, ProbeCandidate};
use crate::relay::platform_map::{parse_platform, Platform};

pub const PROBE_ADAPTER: ProbeAdapter = ProbeAdapter {
    candidate: ProbeCandidate {
        id: "sub2api",
        path: "/api/v1/settings/public",
        bearer_token_storage_key: Some("auth_token"),
        detector_json_paths: &[
            "code",
            "data.site_name",
            "data.version",
            "data.api_base_url",
            "data.registration_enabled",
            "data.promo_code_enabled",
            "data.invitation_code_enabled",
        ],
    },
    detect: detect_site,
};

fn detect_site(body: &str) -> Option<DetectedSite> {
    let settings = parse_sub2api_public_settings(body).ok()?;
    Some(DetectedSite {
        backend_kind: BackendKind::Sub2Api,
        site_name: settings.site_name,
        api_base_url: settings.api_base_url,
    })
}

/// sub2api 业务响应信封。
///
/// **`code` 是整数，成功是 `0`**（`response.Success` 写 `Code: 0, Message: "success"`）。
/// 别把 `message` 那个 `"success"` 当成 code —— 实测踩过：判 `code == "success"` 会让每一次
/// 调用都失败在反序列化上（`code` 解不成 String），整条链路根本跑不起来。
///
/// 服务端错误时 `code` 放 HTTP 状态码、字符串错误码在 `reason` 里。
///
/// ⚠️ **鉴权中间件（401/403）用的是另一套信封** `{code: <字符串>, message}`，与这个不兼容 ——
/// 那条路径不走本结构，见 [`classify_401`]。
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    message: String,
    /// 结构化错误码（成功时不存在）。
    #[serde(default)]
    reason: String,
    data: Option<T>,
}

impl<T> Envelope<T> {
    /// 取出 `data`，`code != 0` 或 `data` 缺失时报可见错误。
    fn into_data(self, what: &str) -> Result<T, AppError> {
        if self.code != 0 {
            let mut msg = if self.message.is_empty() {
                format!("code {}", self.code)
            } else {
                self.message
            };
            if !self.reason.is_empty() {
                msg = format!("{msg} ({})", self.reason);
            }
            return Err(AppError::Config(format!("{what}失败: {msg}")));
        }
        self.data
            .ok_or_else(|| AppError::Config(format!("{what}失败: 响应缺少 data")))
    }
}

/// 分页信封（`/keys` 用）。
///
/// `items` / `pages` 都用 `Option` 手动兜底而不是 `#[serde(default)]`：后者会要求
/// `T: Default`，而 DTO 不该为了反序列化去派生 `Default`（那会造出「字段全空的 ApiKey」
/// 这种实际不存在的值）。
#[derive(Debug, Deserialize)]
struct Paginated<T> {
    items: Option<Vec<T>>,
    pages: Option<i64>,
}

/// `GET /api/v1/settings/public` 的窄子集。用作「这是不是一个 sub2api 站」的指纹。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PublicSettings {
    /// 站点展示名。V2 用它当中转站名字。
    #[serde(default)]
    pub site_name: String,
    /// 服务端注入的版本号；协议识别还需由严格 parser 校验整组指纹字段。
    #[serde(default)]
    pub version: String,
    /// 后台配置的 API 基址。**可能是空串**，不可盲信，见 [`normalize_api_base`]。
    #[serde(default)]
    pub api_base_url: String,
    /// 是否开放注册。**当前没有消费方** —— 留着是因为它就在
    /// `/settings/public` 的响应里，删掉这个字段只是让我们看不见它。
    ///
    /// ⚠️ 这里原来写着「关闭时 `/register` 是死页（只显示黄条），所以登录窗一律加载
    /// `/login`」，**两句都不属实**（2026-08-05 review 抓出）：
    ///
    /// - [`super::login::login_url`] 从不查这个标志 —— 新站一律落 `/register`，
    ///   重登一律落 `/login`，判据是「这一行有没有 `login_identifier`」。
    /// - 关闭注册时那一页也不是死页：sub2api 的 `RegisterView.vue` 只把**表单**换成一条
    ///   提示，而页脚那个「已有账号？去登录」的 `router-link` 由 `AuthLayout` 无条件渲染
    ///   （它是个兄弟 slot）。所以那一页仍然走得通 —— 我们那条横幅也照样能显示。
    #[serde(default)]
    pub registration_enabled: bool,
    /// 是否开放在线支付。关闭时 `/purchase` 会被站点路由守卫重定向到 dashboard，
    /// 充值入口应改走兑换码页 `/redeem`。
    ///
    /// `None` = 老版本 sub2api 没有这个公开字段；为兼容旧站，调用方继续按开启处理。
    #[serde(default)]
    pub payment_enabled: Option<bool>,
}

/// sub2api 公共设置端点的严格 wire shape。
///
/// 协议识别不能只看 `version`：兼容站、验证页包装器乃至其它面板都可能带同名字段。
/// 这里要求 sub2api 当前稳定公开的整组字段及类型同时匹配，再转换成上层真正消费的窄 DTO。
#[derive(Debug, Deserialize)]
struct Sub2ApiPublicSettingsWire {
    site_name: String,
    version: String,
    api_base_url: String,
    registration_enabled: bool,
    #[serde(default)]
    payment_enabled: Option<bool>,
    promo_code_enabled: bool,
    invitation_code_enabled: bool,
}

/// 严格解析 sub2api 的 `GET /api/v1/settings/public` 响应。
///
/// 原生 HTTP 探针与浏览器辅助探针必须共用这一处判据，避免两条路径对同一站点得出不同结论。
pub fn parse_sub2api_public_settings(body: &str) -> Result<PublicSettings, AppError> {
    let env: Envelope<Sub2ApiPublicSettingsWire> = serde_json::from_str(body)
        .map_err(|e| AppError::Config(format!("响应不是 sub2api 公共设置格式: {e}")))?;
    let wire = env.into_data("探测站点")?;

    if wire.version.trim().is_empty() {
        return Err(AppError::Config(
            "响应不是 sub2api 公共设置格式: version 为空".into(),
        ));
    }

    // 这两个布尔值目前不参与业务逻辑，但要求它们存在且类型正确是协议指纹的一部分。
    let _ = (wire.promo_code_enabled, wire.invitation_code_enabled);

    Ok(PublicSettings {
        site_name: wire.site_name,
        version: wire.version,
        api_base_url: wire.api_base_url,
        registration_enabled: wire.registration_enabled,
        payment_enabled: wire.payment_enabled,
    })
}

/// 分组（`GET /api/v1/groups/available`）的窄子集。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Group {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    /// sub2api 的平台标识。取值域 6 个，V2 只要 `openai`。
    #[serde(default)]
    pub platform: String,
    /// 计费倍率，越小越便宜。V2 用它排序档位。
    #[serde(default)]
    pub rate_multiplier: f64,
    #[serde(default)]
    pub status: String,
    /// 这个分组允许生图吗（服务端 `allow_image_generation`）。
    ///
    /// ⚠️ **它不等于「这是个纯生图分组」** —— 实测 `pro池`（6 个文本模型）也是 `true`：
    /// 它的生图走 sub2api 的 codex 生图桥（给 `/v1/responses` 请求注入
    /// `image_generation` tool），主模型仍是文本模型。而纯生图分组是「`/v1/models`
    /// 里只有 `gpt-image-*`」，那是另一件事（见 [`super::provision::pick_model`]）。
    ///
    /// 两者压成一个字段就分不回来了：`福利Pro-禁luna` 是 `false`（选它生图会拿 403
    /// `permission_error`），而它同样不是纯生图分组。
    ///
    /// `#[serde(default)]` ⇒ 老版本服务端没这个字段时取 `false`，即不显示「支持生图」
    /// 标记。保守方向：漏说一个能力无害，错说一个不存在的能力会让用户白试。
    #[serde(default)]
    pub allow_image_generation: bool,
}

/// 倍率高于这个值的分组不呈现给用户。
///
/// 中转站会建「渠道监控专用分组」这类探针池，故意把 `rate_multiplier` 设成 100 之类的惩罚性
/// 数值，好让真实流量绝不落进去 —— 而它们同样是 `platform=openai` + `status=active`，
/// 光按平台过滤会把它们混进档位列表（bestapi.store 实测就有一个 `rate=100` 的
/// `渠道监控专属分组-GPT`）。用户手滑选中的代价是 100 倍计费。
///
/// 阈值取 10：正常档位的倍率在 0.1–2 这个量级（实测线上便宜档是 0.1），10 倍以上不会是
/// 给人日常用的定价。**这是启发式判断而非契约** —— 服务端没有「这是探针池」的标记字段，
/// 所以只能按定价异常来认。宁可漏掉一个真的贵档（用户会来问「我的档位怎么没了」，看得见），
/// 也不能默默把探针池摆上去（用户看不见，直到账单来）。
const MAX_SANE_RATE_MULTIPLIER: f64 = 10.0;

impl Group {
    /// 某个 cc-switch app 能用的分组：platform 映射到该 app、活跃、且定价不是探针池那种
    /// 惩罚性倍率。
    ///
    /// 服务端**不按 platform 过滤**（`api_key_service.go` 的 `GetAvailableGroups` 只判
    /// 「活跃 + 可绑」），所以这个过滤必须客户端做。
    ///
    /// ⚠️ **参数是 [`AppType`] 而不是 platform 字符串，这是有意的**：`composite`
    /// （一把 Key 跨多平台，与「一分组一 provider」不对齐）与所有未知 platform 在
    /// [`crate::relay::platform_map`] 里都取不到 `AppType`，于是它们在这个边界上
    /// **不可表示** ——
    /// 无论调用方传什么 app，composite 分组都拿不到 `true`。
    /// 若参数是 `&str`，`is_usable_for("composite")` 会返回 `true`，从前靠
    /// `platform == "openai"` 这个等值判断顺带排除 composite 的那道守卫就蒸发了。
    pub fn is_usable_for(&self, app_type: &AppType) -> bool {
        parse_platform(&self.platform)
            .and_then(Platform::app_type)
            .as_ref()
            == Some(app_type)
            && self.status == "active"
            && self.rate_multiplier <= MAX_SANE_RATE_MULTIPLIER
    }
}

/// API Key（`GET /api/v1/keys`）的窄子集。
///
/// `key` 字段是**明文完整 sk、未脱敏**（`dto/mappers.go` 没有 mask），所以「认领已有 Key」
/// 拿得到可直接用的 sk，不必重建。
#[derive(Debug, Clone, Deserialize)]
pub struct ApiKey {
    pub id: i64,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
}

impl ApiKey {
    /// 可用的 Key。非 active 的不得认领——否则「认领到废 Key → 调用失败 → 再认领同一把」
    /// 本身就是个环。
    /// 这把 Key 能不能认领来用。
    ///
    /// ## ⚠️ 空 status 判为「可用」，这是有意的
    ///
    /// `status` 带 `#[serde(default)]`，服务端不返回该字段时它是空串。如果那时判成
    /// 不可用，后果是**认领必然失败 → 每次 provision 都新建一把 → 用户账号里 sk 爆炸**
    /// （而且每次都涨，因为下次认领同样失败）。
    ///
    /// 两种误判的代价不对称：
    ///
    /// | 误判 | 后果 | 可恢复性 |
    /// |---|---|---|
    /// | 把废 Key 判成可用 | 调用报 401，用户点一次「获取密钥」重建 | 容易 |
    /// | 把好 Key 判成不可用 | **每次 provision 都建新 sk，服务端越堆越多** | 要用户去网页端手工删 |
    ///
    /// 所以判据是「**明确说了不能用**才不可用」，而不是「明确说了能用才可用」。
    /// 实测 sub2api 确实返回 `status`（`APIKeyFromService` 里 `Status: k.Status`），
    /// 这条兜底是为**别的中转站**（如 new-api）字段名或取值不同时准备的 ——
    /// 那时宁可认领到一把可能失效的 Key，也不能反复新建。
    ///
    /// 非 active 的仍然不认领：那会形成环（认领到废 Key → 调用失败 → 再认领同一把）。
    pub fn is_usable(&self) -> bool {
        // 空 = 服务端没给这个字段，乐观处理；非空则必须是 active。
        self.status.is_empty() || self.status == "active"
    }
}

/// 余额（`GET /api/v1/user/profile`）的窄子集。
///
/// ## `rename_all` 是必需的，不是风格偏好
///
/// 这个结构体**同时**做两件事：`Deserialize` 解服务端的 snake_case 响应、`Serialize`
/// 送给前端。前端的 `RelayBalance` 声明的是 camelCase（`frozenBalance`），
/// 而 serde 默认按 Rust 字段名输出 ⇒ 不写这一行就会送出 `frozen_balance`，
/// 前端读到 `undefined`。
///
/// **加它不会破坏反序列化**：`rename_all` 之后 serde 认的是 camelCase，
/// 而服务端给的是 snake_case，所以每个字段都配一条 `alias`（`serde` 的
/// `rename_all` 不影响 `alias`）。两个方向各自明确，比让一个字段名兼职两种约定安全。
///
/// ⚠️ 这条是历史债：原来没有 `rename_all`，前端那份 `frozenBalance` 声明一直是
/// 假的 —— 没炸只因为**没有任何代码读它**。本轮余额上行，`Balance` 要多带字段，
/// 再不修就会有人踩。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    // `balance` 单词形态在两种约定下同名，不必加 alias。
    #[serde(default)]
    pub balance: f64,
    #[serde(default, alias = "frozen_balance")]
    pub frozen_balance: f64,
}

/// 一把 sk 的计费倍率（`GET /v1/sub2api/billing`）的窄子集。
///
/// ## 为什么用这条而不是 `list_groups()` 拿倍率
///
/// **它是 sk 鉴权，不需要登录态**（源码 `routes/gateway.go:162-163`：注册在
/// `apiKeyAuth` 之后）。我们每个档位都握着明文 sk，所以哪怕账号登录已过期，
/// 倍率照样查得到。
///
/// 而且它给的是**服务端算好的最终值**：`effective_rate_multiplier` 已经把
/// 「分组倍率 × 用户专属倍率 × 当前时刻高峰因子」乘完了
/// （`handler/gateway_key_billing.go` 的 `resolveKeyBillingRate` + `PeakMultiplierAt`）。
/// 走 `list_groups()` 只能拿到分组的基础倍率，用户专属倍率还得另查 `/groups/rates` ——
/// 那条端点有个坑（无专属倍率时返回 `null` 而非空 map，V1 为它专门写过兜底）。
///
/// **服务端就是扣钱那方，它给的数就是账单** —— 客户端别自己乘。
///
/// ⚠️ **两处与本模块其它 DTO 不同**（照抄会踩）：
/// 1. 路径是 `/v1/sub2api/billing`，**不在 `/api/v1` 下** —— 不能用 [`Client::url`]。
/// 2. 响应是**裸 JSON，不套 [`Envelope`]**（handler 直接 `c.JSON(200, ...)`）。
#[derive(Debug, Clone, Deserialize)]
pub struct KeyBilling {
    /// 最终生效倍率：分组 × 用户专属 × 当前时刻高峰，服务端算好的。
    #[serde(default)]
    pub effective_rate_multiplier: f64,
}

/// 账号身份（同一个 `GET /api/v1/user/profile` 响应的另一半）。
///
/// ## 内部认 `id`，外面显示昵称
///
/// - **去重键是 `id`**：数值、服务端主键，改邮箱改昵称都不变。用 email 或 username 做键的话
///   用户在中转站那边改一次名，我们就会把同一个账号当成两个、给他堆重复 sk。
/// - **给人看的是 `username`（昵称）**，回落到 `email`。昵称是用户自己设的、他认得；
///   邮箱在截图或演示时还多一层隐私顾虑。
#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub id: i64,
    #[serde(default)]
    pub email: String,
    /// 昵称。中转站可能允许留空，那时回落到邮箱。
    #[serde(default)]
    pub username: String,
}

impl Account {
    /// 展示名：昵称优先，回落邮箱，都没有就用 `#<id>`（总得有个能指认的东西）。
    pub fn display_name(&self) -> String {
        if !self.username.trim().is_empty() {
            self.username.clone()
        } else if !self.email.trim().is_empty() {
            self.email.clone()
        } else {
            format!("#{}", self.id)
        }
    }
}

/// 把用户输入的域名归一成面板 origin（`https://host`，无尾斜杠、无路径）。
///
/// 用户实际会粘贴的形态（实测列举）：
///
/// | 输入 | 归一到 |
/// |---|---|
/// | `bestapi.store` | `https://bestapi.store` |
/// | `https://bestapi.store/` | 同上 |
/// | `http://bestapi.store/login?next=/` | 同上（路径与查询串都丢掉） |
/// | `https://www.790053500.com/usage` | `https://www.790053500.com`（**`www.` 保留**，见下） |
///
/// **一律升到 https**：sub2api 站点都跑 TLS，而登录页要过 WebView，明文 http 会被拦。
///
/// ## ⚠️ **不要剥 `www.`** —— 试过一次，是个 P0
///
/// 直觉上该剥：带不带 `www.` 会让同一个站变成两行（`site_origin` 进了
/// `creds.rs` 的 `UNIQUE(site_origin, account_id)`，也进了
/// [`super::provision::provider_id_for`] 的哈希）⇒ 界面上两行同名中转站、
/// 各存一套凭据、同一分组在两行下各有一条档位。那个困扰是真的。
///
/// **但剥掉的代价高一个量级，而且是静默的**（2026-08-05 review 抓出）：
///
/// 本函数的产出**就是我们要连的那个 origin** —— 它同时是探测地址、登录窗的 URL，
/// 以及注入脚本里那个 `ALLOWED_ORIGIN` 守卫的比较基准（[`super::login::login_script`]）。
/// 而**有些站把裸域 301 到 `www.`**（实测 `gnu.org` → `https://www.gnu.org/`）：
///
/// 1. 用户粘 `https://www.relay.com/usage`，剥成 `https://relay.com`；
/// 2. 探测**成功** —— `build_client` 没设 redirect policy，reqwest 默认跟随 10 次跳转；
/// 3. 登录窗打开 `https://relay.com/register`，站点 301 到 `https://www.relay.com/register`；
/// 4. 脚本里 `window.location.origin !== ALLOWED_ORIGIN` ⇒ **整段 return**；
/// 5. 用户看到一个完全正常的注册页，注册、登录 —— 而**凭据永远不回传**，
///    `do_login` 干等到 5 分钟超时才报「没拿到凭据」。
///
/// 那正是 `login_script` 里那条 early-return 的注释早就点名的白屏成因。剥 `www.`
/// 等于**主动制造**它，而症状里没有任何东西指向域名归一化。
///
/// 反过来「两行」是**可见**的困扰：用户看得到那两行、删得掉一行。可见的困扰比静默的
/// 失败便宜得多。所以这一层只做「一定安全」的归一（scheme / 路径 / 查询串），
/// 去重要做也得在 `creds::save_site` 那一层做（那里不决定连哪个地址）。
///
/// ⚠️ 本仓另外两处剥 `www.`（`aff.rs` / `stats.rs`）**不构成反例**：那两处剥出来的
/// 字符串只当查表的 key 用，从不拿去发请求。三处不是同一种归一化。
pub fn normalize_site_origin(input: &str) -> Result<String, AppError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(AppError::InvalidInput("域名不能为空".into()));
    }
    // 补 scheme 再交给 url crate——否则 `bestapi.store` 会被解析成一个 scheme 而不是 host。
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = url::Url::parse(&with_scheme)
        .map_err(|e| AppError::InvalidInput(format!("域名格式不对: {e}")))?;

    let host = url
        .host_str()
        .ok_or_else(|| AppError::InvalidInput("域名里没有主机名".into()))?;
    // 拒绝畸形 host：空标签（`x..y`）会被 url crate 当合法域交出来。
    if host.split('.').any(|label| label.is_empty()) || !host.contains('.') {
        return Err(AppError::InvalidInput(format!("主机名不合法: {host}")));
    }
    // `origin().ascii_serialization()` 而不是 `to_string()`：后者恒带尾斜杠。
    let mut origin = format!("https://{host}");
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    Ok(origin)
}

/// 站点的 **API 根**：sub2api 的两套路由都挂在它下面（OpenAI 兼容在 `{root}/v1/…`，
/// Anthropic 在 `{root}/v1/messages`）。这是「这个站点的接口地址」这一事实的唯一形态，
/// 各 CLI 的 `base_url` 一律由它派生，见 [`base_url_for`]。
///
/// 两处要兜：
/// - 后台声明的 `api_base_url` **可能是空串**（bestapi.store 实测）⇒ 回落面板 origin。
/// - 历史上 `loongport_relay.api_base_url` 存的是**已经补过 `/v1` 的 codex 形态**
///   ⇒ 这里把尾部 `/v1` 剥掉，让新旧两种存量行读出同一个根（所以不需要数据迁移）。
pub fn site_api_root(site_origin: &str, api_base_url: &str) -> String {
    let base = api_base_url.trim();
    let root = if base.is_empty() { site_origin } else { base };
    let root = root.trim_end_matches('/');
    // `strip_suffix` 而不是 `trim_end_matches`：后者会把 `/v1/v1` 一路剥光，
    // 那是在替一个畸形配置做「猜」。只剥一段，剥不掉就原样。
    root.strip_suffix("/v1").unwrap_or(root).to_string()
}

/// codex 形状的 `base_url`（必须以 `/v1` 结尾）。
///
/// sub2api 前端生成 codex 配置时**偏偏不对 `api_base_url` 做补 `/v1` 的处理**
/// （grok / gemini 分支都做了），等于把责任推给后台配置。所以这里自己兜。
pub fn codex_base_url(site_origin: &str, api_base_url: &str) -> String {
    format!("{}/v1", site_api_root(site_origin, api_base_url))
}

/// 由站点信息算出**某个 CLI** 该用的 `base_url`。
///
/// 分派判据是「**客户端自己拼不拼版本段**」，不是平台名：
///
/// | CLI | 客户端会拼 | 该给的 base |
/// |---|---|---|
/// | Claude Code | `/v1/messages`、`/v1/models` | 站点根 |
/// | Gemini CLI | `/v1beta/…` | 站点根 |
/// | codex / 生图 / grok | 不拼 | 站点根 + `/v1` |
///
/// ⚠️ **给会自己拼的客户端带上 `/v1` 不是「多一段无害路径」，而是整条链路失效**：
/// base 再带一段就打成 `{root}/v1/v1/…` ⇒ 404。claude 那条的表现是拉不到模型列表 ⇒
/// **不管用户选哪个模型**都报 `Model 'X' is not in the list of available models`
/// （2026-08-08 线上 bug 的真因）；gemini 那条同理。实测 `{root}/v1beta/models` 通、
/// `{root}/v1/v1beta/models` 返回 `404 page not found`。
///
/// 存在的意义是**让调用方无从选错**：`loongport_relay` 只有一列地址，此前三个调用点
/// 都把它直接当成品端点传，于是 claude / gemini 档位整片带上多余的 `/v1`。
pub fn base_url_for(app_type: &AppType, site_origin: &str, api_base_url: &str) -> String {
    match app_type {
        // 自己拼版本段的客户端 ⇒ 站点根原样给它。
        AppType::Claude | AppType::Gemini => site_api_root(site_origin, api_base_url),
        _ => codex_base_url(site_origin, api_base_url),
    }
}

/// 带 Bearer token 的 sub2api 客户端。
///
/// 会话绑定开启时，UA 必须与登录 WebView 一致。当前设计以引擎真实 UA 为唯一
/// 数据源：登录时从 `navigator.userAgent` 回传并持久化，HTTP 客户端跟随它。
/// 值为空时不设（不再伪造默认值）。
pub struct Client {
    http: reqwest::Client,
    site_origin: String,
    token: String,
    /// 这个客户端**以谁的身份**在说话（服务端的用户 id）。
    ///
    /// 只用于给写请求的 `Idempotency-Key` 分命名空间，见 [`idempotency_key_for`]。
    /// 鉴权本身认的是 `token`，不认这个字段 —— 它填错不会造成越权，只会让幂等键
    /// 分错组（进而在同站多账号时撞 409）。
    ///
    /// `None` = 还没登录 / 还没回填。放在 `Client` 上而不是当参数往
    /// [`super::provision`] 里传：它是「这个连接的身份」，与 `token` 同级，
    /// 让每个构造点都必须表态是谁 —— 而 `provision` 不必多一个它不关心的参数。
    account_id: Option<i64>,
}

impl Client {
    /// 建一个**知道自己是谁**的客户端。
    ///
    /// `account_id` 传 `None` 只在「还没拿到身份」时才对（拉 profile 本身）。
    /// 发写请求的路径要传 `Some`，否则同站多账号会撞幂等冲突
    /// —— 见 [`idempotency_key_for`]。
    pub fn new(
        site_origin: impl Into<String>,
        token: impl Into<String>,
        account_id: Option<i64>,
        user_agent: Option<&str>,
        cf_clearance: Option<&str>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            http: build_client_with_session(user_agent, cf_clearance)?,
            site_origin: site_origin.into(),
            token: token.into(),
            account_id,
        })
    }

    /// 这个客户端以谁的身份在说话。
    ///
    /// [`super::provision`] 要它来拼 Key 名字（Key 按账号而不按机器命名，
    /// 见那边的「Key 命名契约」）。暴露成方法而不是让调用方另传一遍参数：
    /// 那样两处可能不一致，而「用哪个账号建 Key」与「用哪个账号发请求」
    /// 必须是同一个答案。
    pub fn account_id(&self) -> Option<i64> {
        self.account_id
    }

    /// 这个 client 连的站点（形如 `https://example.com`，无路径）。
    ///
    /// 给需要打 `/v1` 下端点的调用方用（[`list_models`] / [`key_billing`]）——
    /// 那些不走 [`Self::url`]（它拼的是 `/api/v1`）。**从 client 取而不是让调用方另传**：
    /// 「用哪个站建 Key」与「用哪个站查模型」必须是同一个答案，两处各传一遍就可能不一致。
    pub fn site_origin(&self) -> &str {
        &self.site_origin
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.site_origin, path)
    }

    /// 发一个带鉴权的请求并解信封。
    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        req: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<T, AppError> {
        let resp =
            req.bearer_auth(&self.token).send().await.map_err(|e| {
                AppError::Config(format!("{what}失败: {}", describe_send_error(&e)))
            })?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| AppError::Config(format!("{what}失败: 读响应出错 {e}")))?;

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(classify_401(&body, what));
        }
        if !status.is_success() {
            return Err(AppError::Config(format!(
                "{what}失败: HTTP {} {}",
                status.as_u16(),
                first_line(&body)
            )));
        }
        let env: Envelope<T> = serde_json::from_str(&body)
            .map_err(|e| AppError::Config(format!("{what}失败: 响应解析出错 {e}")))?;
        env.into_data(what)
    }

    /// 拉站点公开设置。
    ///
    /// 这是站点能力的权威来源；充值页选择读取 `payment_enabled`，不按域名猜。
    /// 端点公开可读，故有意不附带账号 token。
    pub async fn public_settings(&self) -> Result<PublicSettings, AppError> {
        let resp = self
            .http
            .get(self.url("/settings/public"))
            .send()
            .await
            .map_err(|e| {
                AppError::Config(format!("获取站点公开设置失败: {}", describe_send_error(&e)))
            })?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| AppError::Config(format!("获取站点公开设置失败: 读响应出错 {e}")))?;
        if !status.is_success() {
            return Err(AppError::Config(format!(
                "获取站点公开设置失败: HTTP {} {}",
                status.as_u16(),
                first_line(&body)
            )));
        }
        parse_sub2api_public_settings(&body)
    }

    /// 拉可用分组。**返回平数组，不是分页信封。**
    pub async fn list_groups(&self) -> Result<Vec<Group>, AppError> {
        self.send(self.http.get(self.url("/groups/available")), "获取分组列表")
            .await
    }

    /// 拉当前用户的全部 API Key（分页迭代到取完）。
    ///
    /// `page_size` 上限 1000，超限**静默回落 20**（不报错），所以取 100 这个稳妥值。
    /// `search` 是 name 或 key 的子串匹配（大小写不敏感），**不是前缀匹配** —— 所以拿回来
    /// 之后仍要客户端做精确比对。
    pub async fn list_keys(&self, search: &str) -> Result<Vec<ApiKey>, AppError> {
        let mut all = Vec::new();
        let mut page = 1i64;
        loop {
            let req = self.http.get(self.url("/keys")).query(&[
                ("page", page.to_string()),
                ("page_size", "100".to_string()),
                ("search", search.to_string()),
            ]);
            let paged: Paginated<ApiKey> = self.send(req, "获取密钥列表").await?;
            // 服务端保证 pages >= 1；缺字段时按「只有这一页」处理。
            let last = paged.pages.unwrap_or(1);
            all.extend(paged.items.unwrap_or_default());
            if page >= last || page >= 50 {
                // 50 页 × 100 = 5000 把，正常账号远达不到；这是防御失控分页的上限。
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// 组装建 Key 的请求（**不发**）。
    ///
    /// **抽出来只为了可测**：组好就发的话，没有缝隙取回它设的头 ⇒
    /// 「`account_id` 到底有没有接进 `Idempotency-Key`」无从断言，而那正是
    /// 撞 409 的那个点。返回 `RequestBuilder` 让测试自己 build 去查头，
    /// 这样**构造逻辑只有这一份**，不会与「给测试用的另一份」分叉。
    fn create_key_request(&self, name: &str, group_id: i64) -> reqwest::RequestBuilder {
        let body = serde_json::json!({ "name": name, "group_id": group_id });
        self.http
            .post(self.url("/keys"))
            // **一律带 `Idempotency-Key`**：建 Key 走服务端的幂等协调器，当前
            // `idempotency.observe_only` 默认 true（即不强制），但中转站可以关掉它，
            // 届时不带头就是 400。现在带上的成本是零，将来不会静默变 400。
            .header(
                "Idempotency-Key",
                idempotency_key_for(self.account_id, name),
            )
            .json(&body)
    }

    /// 建一把新 Key。
    ///
    /// 幂等键用 `account_id + name` 的哈希（**账号必须参与**，见
    /// [`idempotency_key_for`]），重试时天然复用同一个 key。
    /// 请求的组装在 [`Self::create_key_request`]（那里可测）。
    pub async fn create_key(&self, name: &str, group_id: i64) -> Result<ApiKey, AppError> {
        self.send(self.create_key_request(name, group_id), "创建密钥")
            .await
    }

    /// 删除一把 Key（分组下架后清掉服务端孤儿 sk）。
    ///
    /// ⚠️ **有副作用的删除**：调用方必须确认这把 key 对应的分组真的不存在了
    /// （不是临时不可用）。见 `provision::provision` 里 stale 清理那段。
    pub async fn delete_key(&self, key_id: i64) -> Result<(), AppError> {
        let _: serde_json::Value = self
            .send(
                self.http.delete(self.url(&format!("/keys/{key_id}"))),
                "删除密钥",
            )
            .await?;
        Ok(())
    }

    /// 余额。
    pub async fn balance(&self) -> Result<Balance, AppError> {
        self.send(self.http.get(self.url("/user/profile")), "获取余额")
            .await
    }

    /// 账号身份。与 [`Self::balance`] 打的是同一个端点，只是取另外几个字段 ——
    /// 两个窄 DTO 各自只解自己要的那部分，比塞一个大结构体好维护。
    pub async fn account(&self) -> Result<Account, AppError> {
        self.send(self.http.get(self.url("/user/profile")), "获取账号信息")
            .await
    }

    /// 完整的账号档案，**不做窄化**（`serde_json::Value` 原样）。
    ///
    /// ## 为什么这一个方法反着来
    ///
    /// 本模块的惯例是窄 DTO（见模块文档），目的是**隔离上游字段变更**。
    /// 而这个值的用途恰恰是**转发**：它要被原样注入进充值页的 localStorage，
    /// 由站点前端自己拿去渲染用户名、头像、余额、各种绑定状态
    /// （`userProfileResponse` 有 36 个字段）。
    ///
    /// 用窄 DTO 会把没声明的字段**吞掉** ⇒ 站点那边变成「登录了但用户信息一片空白」，
    /// 而且 sub2api 每加一个字段都要跟着改这里 —— 那正是转发场景不该承担的成本。
    ///
    /// ⚠️ **别拿它当「反正 Value 更灵活」的先例**：需要判断字段值的地方一律用窄 DTO
    /// （那时吞掉字段是好事）。只有「整段交给别人」才用这个。
    /// 见 [`crate::relay::purchase`] 的模块文档第 3 条。
    pub async fn profile_raw(&self) -> Result<serde_json::Value, AppError> {
        self.send(self.http.get(self.url("/user/profile")), "获取账号信息")
            .await
    }
}

/// 查一把 sk 的计费倍率。
///
/// **自由函数而不是 [`Client`] 的方法**，与 [`refresh_token`] 同理：`Client` 持有的是账号
/// JWT，而这个端点认的是 **sk**（`apiKeyAuth` 中间件）。两种凭据不该混进一个结构体。
///
/// 这也是它的好处：**不依赖登录态**。账号 token 过期了、甚至清空了，只要 sk 还在，
/// 倍率照样查得到。
///
/// ## 三种「不是错误」的失败，全部返回 `Ok(None)`
///
/// | 状态 | 含义（源码 `handler/gateway_key_billing.go`） |
/// |---|---|
/// | 404 | 站点跑在 `RunModeSimple`，**不提供计费信息** |
/// | 403 | 这把 sk 没绑分组 |
/// | 401 | sk 已失效（被吊销 / 分组被删） |
///
/// 这三种都是「这个档位现在拿不到倍率」，UI 显示「倍率未知」即可 ——
/// **不能当成错误弹 toast**：倍率是附加信息，为它打断用户的主流程是错的。
/// 真正的错误（网络不通、响应解析失败）才返回 `Err`，由调用方决定要不要提示。
pub async fn key_billing(site_origin: &str, api_key: &str) -> Result<Option<KeyBilling>, AppError> {
    // ⚠️ 不走 `Client::url()` —— 那个拼的是 `/api/v1{path}`，而这个端点在 `/v1` 下。
    let url = format!("{site_origin}/v1/sub2api/billing");
    let resp = build_client()?
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| AppError::Config(format!("查询倍率失败: {}", describe_send_error(&e))))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND
        || status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::UNAUTHORIZED
    {
        return Ok(None);
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Config(format!(
            "查询倍率失败: HTTP {} {}",
            status.as_u16(),
            first_line(&body)
        )));
    }

    // 裸 JSON，**不套 Envelope** —— handler 是 `c.JSON(200, response)`。
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Config(format!("查询倍率失败: 读响应出错 {e}")))?;
    serde_json::from_str::<KeyBilling>(&body)
        .map(Some)
        .map_err(|e| AppError::Config(format!("查询倍率失败: 响应解析出错 {e}")))
}

/// 用某把 sk 拉「这个分组能调哪些模型」（`GET /v1/models`）。
///
/// ## 为什么需要它：决定该给这条档位写什么模型名
///
/// 档位的 `model` 原来无条件写 `DEFAULT_MODEL`（一个文本模型）。而中转站会建
/// **纯生图分组** —— `/v1/models` 里只有 `gpt-image-*`，一个文本模型都没挂。
/// 给那种分组写文本模型名的后果是选中即 **404**（实测 `鑫旺Neko API · image原生 2/4k生图`
/// 配 `gpt-5.6-sol`：`Upstream request failed`）。
///
/// 而写对之后 codex 对话内生图是通的（维护者在 `vokotoken.cc` 上实测：config.toml 里
/// `model = "gpt-image-2"` ⇒ 对话里直接出图）—— 上游会把 image-only 主模型的请求
/// 归一化成带 `image_generation` tool 的形状再转发
/// （sub2api `service/openai_codex_transform.go` 的 `normalizeOpenAIResponsesImageOnlyModel`）。
///
/// ## 与 [`key_billing`] 同一条路
///
/// 都是「用 sk 打 `/v1` 下的端点」：**不走 [`Client::url`]**（那个拼 `/api/v1`），
/// 401/403/404 返回 `Ok(None)` 而不是错误。
///
/// 返回 `None` = 查不到（没这个端点 / 权限不够 / 解析不了）。调用方据此**回落到
/// `DEFAULT_MODEL`** —— 那正是本函数出现之前的行为，所以查不到不会让任何事变糟。
pub async fn list_models(
    site_origin: &str,
    api_key: &str,
) -> Result<Option<Vec<String>>, AppError> {
    /// `{"object":"list","data":[{"id":"gpt-image-2",...}]}`（OpenAI 兼容形状）。
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        data: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        #[serde(default)]
        id: String,
    }

    let url = format!("{site_origin}/v1/models");
    let resp = build_client()?
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| AppError::Config(format!("获取模型列表失败: {}", describe_send_error(&e))))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND
        || status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::UNAUTHORIZED
    {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Config(format!(
            "获取模型列表失败: HTTP {}",
            status.as_u16()
        )));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Config(format!("获取模型列表失败: 读响应出错 {e}")))?;
    let parsed: Resp = serde_json::from_str(&body)
        .map_err(|e| AppError::Config(format!("获取模型列表失败: 响应解析出错 {e}")))?;

    // 空 id 丢掉（服务端不该给，但给了就别让它污染判据）。
    let ids: Vec<String> = parsed
        .data
        .into_iter()
        .map(|m| m.id)
        .filter(|id| !id.is_empty())
        .collect();
    // **空列表与「查不到」同义** —— 都表示「问不出这个分组有什么」，
    // 让调用方走同一条回落路径，不必在两处各判一次。
    if ids.is_empty() {
        return Ok(None);
    }
    Ok(Some(ids))
}

/// 用 refresh token 换一对新的 token。
///
/// 这个端点**不需要** Bearer（refresh token 就在请求体里），所以是自由函数而不是
/// [`Client`] 的方法 —— 过期的时候我们手上正好没有可用的 access token。
///
/// 失败即意味着「重登」：refresh token 也过期了、被复用检测拦了（`REFRESH_TOKEN_REUSED`）、
/// 或者会话家族已被撤销。这些都不该重试。
pub async fn refresh_token(
    site_origin: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, AppError> {
    #[derive(Deserialize)]
    struct Resp {
        access_token: Option<String>,
        refresh_token: Option<String>,
        /// 毫秒时间戳（与前端 localStorage 里那个同源）。
        expires_at: Option<i64>,
    }

    let client = build_client()?;
    let resp = client
        .post(format!("{site_origin}/api/v1/auth/refresh"))
        .json(&serde_json::json!({ "refresh_token": refresh_token }))
        .send()
        .await
        .map_err(|e| AppError::Config(format!("续期失败: {}", describe_send_error(&e))))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Config(format!("续期失败: 读响应出错 {e}")))?;

    if !status.is_success() {
        // 401/403 都是「refresh 也救不了」，一律要求重登，不重试。
        return Err(AppError::Config(format!(
            "续期失败（HTTP {}），请重新登录",
            status.as_u16()
        )));
    }

    let env: Envelope<Resp> = serde_json::from_str(&body)
        .map_err(|e| AppError::Config(format!("续期失败: 响应解析出错 {e}")))?;
    let data = env.into_data("续期")?;

    let access = data
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Config("续期失败: 服务端没给新 token".into()))?;

    Ok(RefreshedTokens {
        auth_token: access,
        // 服务端可能只轮换 access 而不给新 refresh；那时沿用旧的。
        refresh_token: data.refresh_token.filter(|t| !t.is_empty()),
        token_expires_at: data
            .expires_at
            .map(|ms| if ms > 100_000_000_000 { ms / 1000 } else { ms }),
    })
}

/// 续期结果。
#[derive(Debug, Clone)]
pub struct RefreshedTokens {
    pub auth_token: String,
    /// `None` 表示服务端没轮换 refresh token，调用方应沿用旧的。
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<i64>,
}

/// 401 的两类处置：能靠 refresh 救的，与必须让用户重新登录的。
///
/// 这个区分是**防死循环的关键**：账号态问题（被禁用 / 会话被撤销）重试多少次都是同一个
/// 结果，把它们当「token 过期」去刷新就是无限重试。
fn classify_401(body: &str, what: &str) -> AppError {
    /// 见到这些 code 说明凭据本身失效、重登也没用之前先清掉本地凭据。
    const UNRECOVERABLE: &[&str] = &[
        "USER_NOT_FOUND",
        "USER_INACTIVE",
        "USER_NOT_ACTIVE",
        "TOKEN_REVOKED",
        "SESSION_BINDING_MISMATCH",
    ];

    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
        .unwrap_or_default();

    if UNRECOVERABLE.contains(&code.as_str()) {
        AppError::Config(format!(
            "{what}失败: 登录态已失效（{code}），请重新登录中转站账号"
        ))
    } else {
        AppError::Config(format!("{what}失败: 登录已过期（{code}），请重新登录"))
    }
}

fn first_line(body: &str) -> String {
    body.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// `Idempotency-Key` 的取值：`account_id + name` 的 SHA-256 十六进制。
///
/// 服务端要求 ≤128 字节且全 ASCII，64 个 hex 字符正好在范围内。
///
/// ## ⚠️ `account_id` 必须参与，否则同站多账号必撞 409
///
/// 服务端的幂等记录**唯一键是 `(scope, idempotency_key_hash)`**
/// （`migrations/057_add_idempotency_records.sql` 的 `idx_idempotency_records_scope_key`），
/// 而 `scope` 是常量 `"user.api_keys.create"` —— **全站所有用户共用一个命名空间**。
/// 账号身份只进 *fingerprint*（`BuildIdempotencyFingerprint` 里的 `user:<id>`），
/// 不进唯一键。
///
/// 于是 Key 名字里不带账号时（名字只有 device_id / platform / group_id，
/// 而同站两个账号看到的分组是同一批 ⇒ 名字逐字相同）：
///
/// ```text
/// 账号 A 建 anthropic/8 → 落一条记录，fingerprint 含 user:A
/// 账号 B 建 anthropic/8 → 同一个 Idempotency-Key → 命中 A 那条记录
///                       → fingerprint 是 user:B ≠ user:A
///                       → 409 IDEMPOTENCY_KEY_CONFLICT
/// ```
///
/// 2026-08-03 实测到的正是这个：同一台机器上给 bestapi.store 挂了两个账号，
/// 后加的那个刷新时报「创建密钥失败: HTTP 409」，而 24h TTL 一过又自己好了
/// （记录过期被回收）—— 所以它表现为**偶发**，最难查。
///
/// ⚠️ **不能靠「回落去认领」兜**：`list_keys` 是按用户隔离的
/// （handler 传 `subject.UserID`，隔离实际由 `api_key_service.go` 的 `ListByUserID`
/// 落实）⇒ 撞 409 时本账号名下**压根没有**那把 Key（它在另一个账号名下），
/// 认领必然认领到空。实测两个账号 `search=LoongPort` 各返回 8 把 / 1 把，互不可见。
/// 客户端这侧能做的修法就是让幂等键本身带上账号身份（服务端把 scope 按账号分
/// 也能解，但那不在我们手里）。
///
/// `None` = 还没拿到 `account_id`（登录回填之前）。用 `"anon"` 参与哈希而不是跳过，
/// 免得「未知账号」与「account_id 恰好是某个值」撞到同一个键上 ——
/// 与 [`provision::provider_id_for`](super::provision::provider_id_for) 同一套理由。
fn idempotency_key_for(account_id: Option<i64>, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    match account_id {
        Some(id) => h.update(id.to_string().as_bytes()),
        None => h.update(b"anon"),
    }
    // 分隔符不可省：没有它 `(account=1, name="2/x")` 与 `(account=12, name="/x")`
    // 喂进哈希的字节流完全相同。
    h.update(b"/");
    h.update(name.as_bytes());
    format!("{:x}", h.finalize())
}

/// 把一个 `reqwest` 发送错误描述成**能定位问题**的一行。
///
/// ## 为什么不能直接 `{e}`
///
/// `reqwest::Error` 的 `Display` 只打印最外层，形如
/// `error sending request for url (https://…)` —— **超时、DNS 解析失败、连接被拒、
/// TLS 握手失败、代理不可达打印出来完全一样**，真实原因在 `std::error::Error::source()`
/// 链里（hyper → 系统错误）。
///
/// 2026-08-03 的实测代价：用户报「获取分组列表失败: error sending request for url
/// (https://bestapi.store/api/v1/groups/available)」，日志里就这一句。为判断是哪一类，
/// 只能专门写一个最小复现程序传到那台 Windows 上，逐层验证 DNS / TCP / TLS / 5 种 client
/// 变体 —— 而那本该是日志里现成的一行。
///
/// 所以这里做两件事：**给出失败类别**（`is_timeout` / `is_connect` 这些谓词，比原始
/// 措辞更适合展示给用户），以及**展开整条 source 链**（给维护者定位用）。
fn describe_send_error(e: &reqwest::Error) -> String {
    // 类别前缀：用户看得懂的话术。判定顺序按「越具体越先」——
    // 超时优先于连接：连接阶段超时时两个谓词可能同时为真，而「超时」对用户更有指导性
    // （等一下重试），「连不上」会让人以为是地址错了。
    let kind = if e.is_timeout() {
        "请求超时"
    } else if e.is_connect() {
        "连不上服务器"
    } else if e.is_request() {
        "请求发送失败"
    } else {
        "网络错误"
    };

    let mut out = format!("{kind}（{e}）");
    // 整条链都带上：真实原因常在第 2-3 层（hyper 之下的系统错误）。
    // 首层往往就是判据本身（超时是 `operation timed out`、连接被拒是系统 errno）。
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(s) = src {
        out.push_str(&format!(" cause: {s}"));
        src = s.source();
    }
    out
}

pub(crate) fn build_client_with_user_agent(
    user_agent: Option<&str>,
) -> Result<reqwest::Client, AppError> {
    build_client_with_session(user_agent, None)
}

/// 建一个带登录会话特征的客户端。
///
/// `cf_clearance` 是 Cloudflare 托管挑战的放行 cookie：本 app 登录走 WebView（能执行 JS
/// ⇒ 过得了挑战），之后 API 全走 reqwest（**永远过不了**）。把 WebView 拿到的那个 cookie
/// 带上，reqwest 才不会在开了挑战的站上撞 403 `Just a moment...`。
///
/// ⚠️ 该 cookie 绑定 IP + UA ⇒ 必须与 `user_agent` 成对使用，两者都取自同一次登录。
/// 任一为空就不设对应的头，绝不伪造。
pub(crate) fn build_client_with_session(
    user_agent: Option<&str>,
    cf_clearance: Option<&str>,
) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));
    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }
    if let Some(clearance) = cf_clearance.map(str::trim).filter(|v| !v.is_empty()) {
        let mut headers = reqwest::header::HeaderMap::new();
        let cookie = format!("cf_clearance={clearance}");
        // 值来自服务端下发的 cookie，理论上都是合法 header 值；解析不了就不设它 ——
        // 那只是回到「没有放行 cookie」的状态，不该让整个客户端建不起来。
        match reqwest::header::HeaderValue::from_str(&cookie) {
            Ok(value) => {
                headers.insert(reqwest::header::COOKIE, value);
                builder = builder.default_headers(headers);
            }
            Err(error) => log::warn!("Cloudflare 放行 cookie 不是合法的 header 值: {error}"),
        }
    }
    builder
        .build()
        .map_err(|e| AppError::Config(format!("创建 HTTP 客户端失败: {e}")))
}

pub(crate) fn build_client() -> Result<reqwest::Client, AppError> {
    build_client_with_user_agent(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 有无放行 cookie 都要能建出客户端；无值时**不发空 Cookie 头**。
    #[test]
    fn client_builds_with_and_without_cf_clearance() {
        assert!(build_client_with_session(Some("UA/1.0"), Some("tok")).is_ok());
        assert!(build_client_with_session(Some("UA/1.0"), None).is_ok());
        assert!(build_client_with_session(None, None).is_ok());
        // 空白值等同于没有：不该拼出 `cf_clearance=` 这种空头。
        assert!(build_client_with_session(None, Some("   ")).is_ok());
    }

    #[test]
    fn normalize_site_origin_accepts_bare_host_and_strips_path() {
        for input in [
            "bestapi.store",
            "https://bestapi.store",
            "https://bestapi.store/",
            "http://bestapi.store/login",
            "  bestapi.store  ",
        ] {
            assert_eq!(
                normalize_site_origin(input).unwrap(),
                "https://bestapi.store",
                "input: {input}"
            );
        }
    }

    /// ⭐ **`www.` 必须原样保留** —— 剥它试过一次，是个 P0。
    ///
    /// 本函数的产出就是我们要连的 origin，同时也是注入脚本里 `ALLOWED_ORIGIN` 的基准。
    /// 而有些站把裸域 301 到 `www.`（实测 `gnu.org`）⇒ 剥掉之后：探测成功（reqwest 默认
    /// 跟随跳转）、登录窗被 301 到 `www.` 那个 origin、脚本的 origin 守卫失配整段 return
    /// ⇒ **用户在一个看起来完全正常的注册页上登录，而凭据永远不回传**，
    /// `do_login` 干等 5 分钟超时。
    ///
    /// 那是 `login_script` 那条 early-return 早就点名的白屏成因，症状里没有任何东西
    /// 指向域名归一化。相比之下「同一站变两行」是可见的、用户删得掉的困扰。
    ///
    /// 会红的改法：为了去重在这里加 `strip_prefix("www.")`。**去重要做请在
    /// `creds::save_site` 那一层做** —— 那里不决定连哪个地址。
    #[test]
    fn the_www_prefix_is_preserved_because_it_decides_what_we_connect_to() {
        // 路径与查询串照旧剥掉，`www.` 原样留着。
        for (input, want) in [
            ("www.bestapi.store", "https://www.bestapi.store"),
            (
                "https://www.bestapi.store/usage",
                "https://www.bestapi.store",
            ),
            (
                "https://www.bestapi.store/login?next=/panel",
                "https://www.bestapi.store",
            ),
        ] {
            assert_eq!(
                normalize_site_origin(input).unwrap(),
                want,
                "input: {input} —— `www.` 被剥掉了，那会让 301 到 www. 的站凭据永不回传"
            );
        }
    }

    /// 子域原样保留 —— `api.x.com` / `panel.x.com` 是真的不同主机，
    /// 有些中转站的面板就挂在子域上。
    #[test]
    fn other_subdomains_are_preserved() {
        assert_eq!(
            normalize_site_origin("https://panel.relay.dev").unwrap(),
            "https://panel.relay.dev"
        );
    }

    #[test]
    fn normalize_site_origin_rejects_malformed_hosts() {
        // 空标签会被 url crate 当合法域交出来（实测 `x..y` → Domain("x..y")），
        // 必须显式拦掉，否则畸形输入会被当成正常站点去探测。
        for bad in [
            "",
            "   ",
            "localhost",
            "bestapi.store..",
            "x..bestapi.store",
        ] {
            assert!(
                normalize_site_origin(bad).is_err(),
                "should reject: {bad:?}"
            );
        }
    }

    #[test]
    fn normalize_site_origin_keeps_explicit_port() {
        assert_eq!(
            normalize_site_origin("https://my.relay.dev:8443/panel").unwrap(),
            "https://my.relay.dev:8443"
        );
    }

    /// 打真实站点验探测链路。**默认不跑**（`#[ignore]`）—— CI 不该依赖外网可达，
    /// 而这条要验的恰恰是「真的连得上、字段真的对得上」。
    ///
    /// 手动跑：`cargo test --lib probe_live_site -- --ignored --nocapture`
    ///
    /// 它守的是**契约漂移**：上游改字段名、中转站换后端版本时，纯函数单测全绿而这条会红。
    #[test]
    #[ignore = "需要外网；手动跑 --ignored"]
    fn probe_live_site_matches_our_narrow_dto() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建 runtime");

        let origin = normalize_site_origin("bestapi.store").expect("归一化域名");
        assert_eq!(origin, "https://bestapi.store");

        let body = rt.block_on(async {
            build_client()
                .expect("建 client")
                .get(format!("{origin}/api/v1/settings/public"))
                .send()
                .await
                .expect("请求公开设置")
                .text()
                .await
                .expect("读取公开设置")
        });
        let settings = parse_sub2api_public_settings(&body).expect("探测应成功");

        // version 是我们的站点指纹判据 —— 它没了，探测就认不出 sub2api。
        assert!(!settings.version.is_empty(), "指纹字段 version 必须有值");
        // site_name 用作中转站展示名。
        assert!(!settings.site_name.is_empty(), "site_name 应有值");

        // 这条是本测试最想钉住的事实：后台的 api_base_url **可能是空串**，
        // 补 /v1 的责任在客户端。若哪天它有值了，下面的断言仍成立（codex_base_url 两种都处理）。
        let base = codex_base_url(&origin, &settings.api_base_url);
        assert!(
            base.ends_with("/v1"),
            "codex base_url 必须以 /v1 结尾，实际 {base}（api_base_url={:?}）",
            settings.api_base_url
        );

        println!(
            "站点 {} / 版本 {} / api_base_url={:?} → codex base_url {}",
            settings.site_name, settings.version, settings.api_base_url, base
        );
    }

    #[test]
    fn codex_base_url_falls_back_to_site_origin_when_api_base_is_blank() {
        // bestapi.store 实测 api_base_url 就是空串，这条是它的正面测点。
        assert_eq!(
            codex_base_url("https://bestapi.store", ""),
            "https://bestapi.store/v1"
        );
        assert_eq!(
            codex_base_url("https://bestapi.store", "   "),
            "https://bestapi.store/v1"
        );
    }

    #[test]
    fn codex_base_url_appends_v1_once_and_only_once() {
        assert_eq!(
            codex_base_url("https://x.dev", "https://api.x.dev"),
            "https://api.x.dev/v1"
        );
        assert_eq!(
            codex_base_url("https://x.dev", "https://api.x.dev/v1"),
            "https://api.x.dev/v1"
        );
        assert_eq!(
            codex_base_url("https://x.dev", "https://api.x.dev/v1/"),
            "https://api.x.dev/v1"
        );
    }

    /// 自己拼版本段的客户端（claude / gemini），base **绝不能带 `/v1`**。
    ///
    /// 这是 2026-08-08 那个线上 bug 的回归钉：`loongport_relay.api_base_url` 存的是
    /// codex 形态（带 `/v1`），三个调用点直接把它当成品端点传 ⇒ 客户端自己再拼一段 ⇒
    /// `/v1/v1/…` 404。claude 那条的表现是模型列表拉空 ⇒ **选任何模型**都报
    /// 「不在可用列表里」。
    ///
    /// 实测（瓜子站）：`/v1/models` 200、`/v1/v1/models` 404；
    /// `/v1beta/models` 通（返回真实 gemini 路由错误）、`/v1/v1beta/models`
    /// 返回 `404 page not found`。
    ///
    /// 两种存量行（带 `/v1` 的旧行、不带的新行）必须读出同一个根 —— 所以不需要数据迁移。
    #[test]
    fn version_appending_clients_never_get_v1() {
        for app_type in [AppType::Claude, AppType::Gemini] {
            for stored in [
                "https://api.guazi.shop",
                "https://api.guazi.shop/v1",
                "https://api.guazi.shop/v1/",
                "https://api.guazi.shop/",
            ] {
                assert_eq!(
                    base_url_for(&app_type, "https://guazi.shop", stored),
                    "https://api.guazi.shop",
                    "{} + 存量形态 {stored} 必须读出同一个站点根",
                    app_type.as_str()
                );
            }
            // 后台声明为空时回落面板 origin（bestapi.store 实测那条路）。
            assert_eq!(
                base_url_for(&app_type, "https://bestapi.store", ""),
                "https://bestapi.store"
            );
        }
    }

    /// `/v1` 只剥一段：`/v1/v1` 是畸形配置，不替它猜。
    #[test]
    fn site_api_root_strips_at_most_one_v1() {
        assert_eq!(
            site_api_root("https://x.dev", "https://api.x.dev/v1/v1"),
            "https://api.x.dev/v1"
        );
        // `apiv1` 结尾不是 `/v1`，不许误伤。
        assert_eq!(
            site_api_root("https://x.dev", "https://api.x.dev/apiv1"),
            "https://api.x.dev/apiv1"
        );
    }

    /// 分派闸：同一行存量数据，自己拼版本段的拿到根、其余拿到带 `/v1` 的。
    ///
    /// 钉的是「调用方无从选错」这件事本身 —— 此前三个调用点各自传 `op.api_base_url`，
    /// 任何一处漏改都会复发。
    #[test]
    fn base_url_for_splits_version_appending_clients_from_the_v1_shaped_ones() {
        const ORIGIN: &str = "https://guazi.shop";
        const STORED: &str = "https://api.guazi.shop";

        for app_type in [AppType::Claude, AppType::Gemini] {
            assert_eq!(
                base_url_for(&app_type, ORIGIN, STORED),
                "https://api.guazi.shop",
                "{} 自己拼版本段，base 不能带 /v1",
                app_type.as_str()
            );
        }
        for app_type in [AppType::Codex, AppType::CodexImage, AppType::GrokBuild] {
            assert_eq!(
                base_url_for(&app_type, ORIGIN, STORED),
                "https://api.guazi.shop/v1",
                "{} 的 base 必须自带 /v1",
                app_type.as_str()
            );
        }
    }

    fn group(platform: &str, status: &str, rate: f64) -> Group {
        Group {
            id: 1,
            name: "t".into(),
            platform: platform.into(),
            rate_multiplier: rate,
            status: status.into(),
            // 这些测试只关心 platform / status / 倍率那三道过滤，生图开关与它们无关。
            allow_image_generation: false,
        }
    }

    #[test]
    fn group_usable_only_for_active_openai() {
        assert!(group("openai", "active", 1.0).is_usable_for(&AppType::Codex));
        // composite 一把 Key 跨多平台，与「一分组一 provider」不对齐，必须排除。
        assert!(!group("composite", "active", 1.0).is_usable_for(&AppType::Codex));
        assert!(!group("anthropic", "active", 1.0).is_usable_for(&AppType::Codex));
        assert!(!group("openai", "disabled", 1.0).is_usable_for(&AppType::Codex));
    }

    /// composite 的排除**不依赖调用方传对 app**：从前它是靠 `platform == "openai"` 顺带被
    /// 排除的，参数化之后这条守卫必须由 `platform_map` 接住 —— 对**每一个** app_type 问
    /// 一遍，composite 都得是 false。未知 platform 同理（上游加了新平台不该被误绑）。
    #[test]
    fn composite_and_unknown_platforms_are_unusable_for_every_app() {
        for app_type in AppType::all() {
            assert!(
                !group("composite", "active", 1.0).is_usable_for(&app_type),
                "composite 不该对 {} 可用",
                app_type.as_str()
            );
            assert!(
                !group("bedrock", "active", 1.0).is_usable_for(&app_type),
                "未知 platform 不该对 {} 可用",
                app_type.as_str()
            );
        }
    }

    /// 参数化没有把平台判定弄丢：其它平台各自只对自己那个 app 可用。
    #[test]
    fn each_mapped_platform_matches_only_its_own_app() {
        assert!(group("anthropic", "active", 1.0).is_usable_for(&AppType::Claude));
        assert!(group("gemini", "active", 1.0).is_usable_for(&AppType::Gemini));
        assert!(group("grok", "active", 1.0).is_usable_for(&AppType::GrokBuild));
        // 交叉不成立：anthropic 的分组不能被 codex 页拿去用。
        assert!(!group("anthropic", "active", 1.0).is_usable_for(&AppType::Codex));
        assert!(!group("openai", "active", 1.0).is_usable_for(&AppType::Claude));
    }

    #[test]
    fn monitor_probe_pools_are_filtered_out_by_punitive_rate() {
        // 中转站的「渠道监控专用分组」是 openai + active，只有倍率异常能认出来。
        // bestapi.store 实测有一个 rate=100 的 `渠道监控专属分组-GPT` —— 不滤掉的话它会
        // 出现在档位列表里，用户手滑选中就是 100 倍计费。
        assert!(!group("openai", "active", 100.0).is_usable_for(&AppType::Codex));

        // 正常档位不受影响：线上便宜档实测 0.1，常规档 1.0-2.0。
        for rate in [0.1, 1.0, 2.0, 10.0] {
            assert!(
                group("openai", "active", rate).is_usable_for(&AppType::Codex),
                "倍率 {rate} 是正常定价，不该被滤掉"
            );
        }
    }

    #[test]
    fn envelope_rejects_non_success_code() {
        let env: Envelope<Group> = serde_json::from_str(
            r#"{"code":429,"message":"too many","reason":"RATE_LIMITED","data":null}"#,
        )
        .unwrap();
        let err = env.into_data("测试").unwrap_err().to_string();
        assert!(err.contains("too many"), "{err}");
        assert!(err.contains("RATE_LIMITED"), "{err}");
    }

    #[test]
    fn envelope_rejects_success_without_data() {
        // 服务端说成功却没给 data，属契约破裂，必须报错而不是当成空结果。
        let env: Envelope<Vec<Group>> =
            serde_json::from_str(r#"{"code":0,"message":"success","data":null}"#).unwrap();
        assert!(env.into_data("测试").is_err());
    }

    #[tokio::test]
    async fn public_settings_reads_payment_capability_without_sending_credentials() {
        async fn settings(headers: axum::http::HeaderMap) -> axum::Json<serde_json::Value> {
            assert!(
                headers.get(axum::http::header::AUTHORIZATION).is_none(),
                "公开设置端点不需要账号 token"
            );
            axum::Json(serde_json::json!({
                "code": 0,
                "message": "success",
                "data": {
                    "site_name": "WawAPI",
                    "version": "1.0.0",
                    "api_base_url": "",
                    "registration_enabled": true,
                    "payment_enabled": false,
                    "promo_code_enabled": false,
                    "invitation_code_enabled": true
                }
            }))
        }

        let app =
            axum::Router::new().route("/api/v1/settings/public", axum::routing::get(settings));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind settings server");
        let origin = format!("http://{}", listener.local_addr().expect("server addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve settings response");
        });

        let client =
            Client::new(origin, "account-secret", Some(7), None, None).expect("build client");
        let settings = client
            .public_settings()
            .await
            .expect("read public settings");

        assert_eq!(settings.site_name, "WawAPI");
        assert_eq!(settings.payment_enabled, Some(false));
    }

    #[test]
    fn sub2api_public_settings_parser_accepts_a_strict_protocol_match() {
        let body = r#"{"code":0,"message":"success","data":{
            "site_name":"贾维斯","version":"0.1.169","api_base_url":"",
            "registration_enabled":true,"payment_enabled":false,
            "promo_code_enabled":true,"invitation_code_enabled":true}}"#;

        let settings = parse_sub2api_public_settings(body).expect("完整 sub2api 契约应通过");
        assert_eq!(settings.site_name, "贾维斯");
        assert_eq!(settings.version, "0.1.169");
        assert_eq!(settings.payment_enabled, Some(false));
    }

    #[test]
    fn sub2api_public_settings_parser_rejects_browser_verification_html() {
        let err = parse_sub2api_public_settings(
            "<!doctype html><title>Just a moment...</title><p>Verify you are human</p>",
        )
        .expect_err("验证页不是 sub2api 协议响应");
        assert!(err.to_string().contains("sub2api"));
    }

    #[test]
    fn sub2api_public_settings_parser_rejects_new_api_status_shape() {
        let body = r#"{"success":true,"message":"","data":{
            "version":"0.9.0","system_name":"New API","register_enabled":true}}"#;
        assert!(parse_sub2api_public_settings(body).is_err());
    }

    #[test]
    fn sub2api_public_settings_parser_rejects_version_only_lookalike() {
        let body = r#"{"code":0,"message":"success","data":{
            "site_name":"lookalike","version":"1.0.0","api_base_url":"",
            "registration_enabled":true}}"#;
        assert!(parse_sub2api_public_settings(body).is_err());
    }

    #[test]
    fn sub2api_public_settings_parser_rejects_wrong_envelope_contract() {
        for body in [
            r#"{"code":1,"message":"no","data":{}}"#,
            r#"{"code":"success","message":"","data":{}}"#,
            r#"{"code":0,"message":"success","data":null}"#,
        ] {
            assert!(parse_sub2api_public_settings(body).is_err(), "body={body}");
        }
    }

    #[test]
    fn envelope_parses_the_real_wire_format_byte_for_byte() {
        // 这条钉住一个**已经踩过**的坑：`code` 是整数、成功是 0，而 `message` 才是 "success"。
        // 之前把 code 声明成 String 判 == "success"，结果每一次调用都失败在反序列化上
        // （错误现场是「响应不是 sub2api 格式」，看不出真正原因），而单测因为编码了同一个
        // 错误假设而全绿 —— 是打真站的那条 ignored 测试才抓出来的。
        //
        // 下面这段是 `curl https://bestapi.store/api/v1/settings/public` 的真实形状（截取字段）。
        let real = r#"{"code":0,"message":"success","data":{
            "site_name":"百适 BestApi","version":"0.1.169",
            "api_base_url":"","registration_enabled":true,
            "promo_code_enabled":true,"invitation_code_enabled":true}}"#;
        let s = parse_sub2api_public_settings(real).expect("必须能解真实响应");
        assert_eq!(s.version, "0.1.169");
        assert_eq!(s.api_base_url, "", "实测这个字段就是空串");
        assert_eq!(
            s.payment_enabled, None,
            "老版本响应缺字段时必须保持未知，而不是误判为关闭支付"
        );

        // 反面：把 message 的 "success" 误当 code 会解不出来 —— 这正是原来的写法。
        assert!(
            serde_json::from_str::<Envelope<PublicSettings>>(
                r#"{"code":"success","message":"","data":{}}"#
            )
            .is_err(),
            "code 是整数，字符串形态不该被接受（否则等于容忍那个旧 bug）"
        );
    }

    #[test]
    fn balance_reads_snake_case_from_the_server_and_writes_camel_case_to_the_frontend() {
        // `Balance` 同时做两件事，两个方向的命名约定**不一样**：
        //   服务端 → snake_case（`frozen_balance`，见 sub2api `dto.User`）
        //   前端   → camelCase（`frozenBalance`，见 `src/lib/api/relay.ts`）
        //
        // 这条钉住的是一个**已经存在过的静默错误**：原来没有 `rename_all`，
        // Serialize 输出 `frozen_balance`，而前端类型声明的是 `frozenBalance` ⇒
        // 前端读到 undefined。当时没炸只因为没有任何代码读它，
        // 本轮余额上到中转站行、字段会被真的读，所以必须两个方向都锁住。
        let from_server: Balance =
            serde_json::from_str(r#"{"balance":12.34,"frozen_balance":5.6}"#)
                .expect("要能解服务端");
        assert_eq!(from_server.balance, 12.34);
        assert_eq!(from_server.frozen_balance, 5.6);

        let to_frontend = serde_json::to_value(&from_server).expect("要能序列化");
        assert_eq!(
            to_frontend.get("frozenBalance").and_then(|v| v.as_f64()),
            Some(5.6),
            "送给前端的必须是 camelCase：{to_frontend}"
        );
        assert!(
            to_frontend.get("frozen_balance").is_none(),
            "不该同时输出 snake_case（前端只声明了 camelCase）：{to_frontend}"
        );
    }

    #[test]
    fn classify_401_separates_account_state_from_expiry() {
        // 账号态问题必须提示重新登录，且文案要与「过期」不同——混成一句会让用户
        // 反复点重试，撞 /auth/login 的 20 次/分钟限流。
        let revoked = classify_401(r#"{"code":"TOKEN_REVOKED"}"#, "测试").to_string();
        assert!(revoked.contains("已失效"), "{revoked}");

        let expired = classify_401(r#"{"code":"TOKEN_EXPIRED"}"#, "测试").to_string();
        assert!(expired.contains("已过期"), "{expired}");
        assert!(!expired.contains("已失效"), "{expired}");
    }

    #[test]
    fn refresh_normalizes_millisecond_expiry_and_keeps_rotation_optional() {
        // 这里测的是解析形状而不是网络调用：服务端两种响应形态（轮换 refresh / 不轮换）
        // 都必须能解，且毫秒过期时间要归一到秒。
        #[derive(Deserialize)]
        struct Resp {
            access_token: Option<String>,
            refresh_token: Option<String>,
            expires_at: Option<i64>,
        }

        let rotated: Envelope<Resp> = serde_json::from_str(
            r#"{"code":0,"data":{"access_token":"a2","refresh_token":"r2","expires_at":1800000000000}}"#,
        )
        .unwrap();
        let d = rotated.into_data("测试").unwrap();
        assert_eq!(d.refresh_token.as_deref(), Some("r2"));
        let secs = d
            .expires_at
            .map(|ms| if ms > 100_000_000_000 { ms / 1000 } else { ms });
        assert_eq!(secs, Some(1_800_000_000));

        // 只轮换 access 的形态：refresh 缺失不是错误，调用方应沿用旧的。
        let not_rotated: Envelope<Resp> =
            serde_json::from_str(r#"{"code":0,"data":{"access_token":"a2"}}"#).unwrap();
        let d = not_rotated.into_data("测试").unwrap();
        assert_eq!(d.access_token.as_deref(), Some("a2"));
        assert!(d.refresh_token.is_none());
    }

    #[test]
    fn idempotency_key_is_ascii_hex_within_128_bytes() {
        let k = idempotency_key_for(Some(13), "LoongPort/dev-1/42");
        assert_eq!(k.len(), 64);
        assert!(k.is_ascii());
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        // 同名必须得到同一个 key，否则重试就不是幂等重试了。
        assert_eq!(k, idempotency_key_for(Some(13), "LoongPort/dev-1/42"));
        assert_ne!(k, idempotency_key_for(Some(13), "LoongPort/dev-1/43"));
    }

    /// **同站两个账号必须拿到不同的幂等键** —— 2026-08-03 那个 409 的直接成因。
    ///
    /// 服务端幂等记录的唯一键是 `(scope, key_hash)`，`scope` 全站共用；账号身份只进
    /// fingerprint。所以键不带账号时，后一个账号会命中前一个的记录、fingerprint
    /// 不符 ⇒ 409 `IDEMPOTENCY_KEY_CONFLICT`。
    ///
    /// 同一台机器上同一个站挂两个账号时，两边看到的分组是同一批 ⇒ Key 名字**逐字相同**，
    /// 所以这里刻意用同一个 name。
    #[test]
    fn two_accounts_on_one_site_never_share_an_idempotency_key() {
        let name = "LoongPort/64b0b373/anthropic/8";
        let a = idempotency_key_for(Some(13), name);
        let b = idempotency_key_for(Some(60), name);
        assert_ne!(
            a, b,
            "同名 Key 在两个账号下必须得到不同的幂等键，否则撞 409"
        );
        // 未登录也不能与任何真实账号撞上。
        assert_ne!(idempotency_key_for(None, name), a);
        assert_ne!(idempotency_key_for(None, name), b);
        // 拿小 id 试：真实用户 id 从 1 开始，`Some(1)` 是最可能与 `"anon"` 撞上的值。
        assert_ne!(
            idempotency_key_for(None, name),
            idempotency_key_for(Some(1), name),
            "未登录的命名空间不能与 account_id=1 撞上"
        );
    }

    /// 从**真实组装出来的请求**上把 `Idempotency-Key` 读回来。
    ///
    /// 上面那两条测试直接调 `idempotency_key_for`，守的是哈希函数；而 bug 长在
    /// **调用点** —— 把请求组装处的 `self.account_id` 换成 `None`（等于修法作废、
    /// 409 复发），它们一条都不会红。所以这条必须走 [`Client`]。
    fn idempotency_header_on_request(account_id: Option<i64>, name: &str) -> String {
        let req = Client::new("https://example.com", "token", account_id, None, None)
            .expect("构造 client 不该失败")
            .create_key_request(name, 8)
            .build()
            .expect("组装请求不该失败");
        req.headers()
            .get("Idempotency-Key")
            .expect("建 Key 的请求必须带 Idempotency-Key 头")
            .to_str()
            .expect("头值必须是 ASCII")
            .to_string()
    }

    #[test]
    fn the_create_key_request_carries_a_per_account_idempotency_key() {
        let name = "LoongPort/64b0b373/anthropic/8";
        let a = idempotency_header_on_request(Some(13), name);

        assert_ne!(
            a,
            idempotency_header_on_request(Some(60), name),
            "两个账号对同一个 name 必须发出不同的 Idempotency-Key —— \
             相等说明 account_id 没被接进请求，409 会复发"
        );
        // 同一个身份必须稳定复现，否则重试就不是幂等重试了。
        assert_eq!(a, idempotency_header_on_request(Some(13), name));
        // 未登录的也不能与已登录的撞上。
        assert_ne!(idempotency_header_on_request(None, name), a);
        // name 必须真的参与：否则同一个账号的不同分组会共用一个幂等键 ——
        // 那会让第二个分组撞 409（payload 不同、键相同）。
        assert_ne!(
            a,
            idempotency_header_on_request(Some(13), "LoongPort/64b0b373/anthropic/5"),
            "不同分组必须有不同的 Idempotency-Key"
        );
    }

    /// 分隔符不可省：`(account, name)` 拼接不能有歧义。
    #[test]
    fn the_account_and_name_cannot_bleed_into_each_other() {
        // 没有分隔符时 "1" + "2/x" 与 "12" + "/x" 的字节流相同。
        assert_ne!(
            idempotency_key_for(Some(1), "2/x"),
            idempotency_key_for(Some(12), "/x")
        );
    }

    #[test]
    fn api_key_usable_only_when_active() {
        let mk = |status: &str| ApiKey {
            id: 1,
            key: "sk-x".into(),
            name: "n".into(),
            status: status.into(),
        };
        assert!(mk("active").is_usable());
        assert!(!mk("disabled").is_usable());
    }

    #[tokio::test]
    async fn list_models_does_not_expose_authenticated_response_bodies() {
        const API_KEY: &str = "sk-model-list-secret";
        const RESPONSE_MARKER: &str = "upstream-echoed-private-payload";

        async fn leaking_error(
            headers: axum::http::HeaderMap,
        ) -> (axum::http::StatusCode, &'static str) {
            assert_eq!(
                headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer sk-model-list-secret")
            );
            (
                axum::http::StatusCode::BAD_GATEWAY,
                "upstream-echoed-private-payload sk-model-list-secret",
            )
        }

        let app = axum::Router::new().route("/v1/models", axum::routing::get(leaking_error));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind model-list server");
        let origin = format!("http://{}", listener.local_addr().expect("server addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve model-list response");
        });

        let error = list_models(&origin, API_KEY)
            .await
            .expect_err("502 model list must fail")
            .to_string();

        assert!(
            error.contains("HTTP 502"),
            "status remains diagnostic: {error}"
        );
        assert!(!error.contains(API_KEY), "error leaked API key: {error}");
        assert!(
            !error.contains(RESPONSE_MARKER),
            "error leaked authenticated response body: {error}"
        );
    }

    /// **传输错误必须带上 `source()` 链**。
    ///
    /// 2026-08-03 实测代价：用户报「获取分组列表失败: error sending request for url
    /// (https://bestapi.store/api/v1/groups/available)」，而那串正是 `{e}` 对
    /// `reqwest::Error` 的全部输出 —— 超时 / DNS / 连接被拒 / TLS 失败**打印出来一模一样**，
    /// 真实原因在 `source()` 链里被丢掉了。结果：为了知道是哪一种，只能专门编一个最小
    /// 复现程序传到那台机器上跑（DNS、TCP、TLS、5 个 client 变体逐层验证），
    /// 而这本该是日志里的一行。
    ///
    /// 这条钉住「链被展开了」，不钉具体措辞（那是 reqwest 的措辞，会随版本变）。
    ///
    /// ## 两处刻意的写法
    ///
    /// **1. 用本机 listener 制造失败，不打任何外部地址。** 起初用的是保留地址
    /// `192.0.2.1`（RFC 5737）+ 1ms 超时，但那不由本进程说了算：CI 的网络命名空间可能
    /// 立即回 network-unreachable（那是 connect 而非 timeout）、透明代理也可能把它接走
    /// 变成一个 HTTP 响应 ⇒ 拿到 `Ok` 而不是错误。现在连一个**接受连接但永不回应**的
    /// 本机 listener，超时由我们自己的 timeout 决定，与外网和 CI 网络配置无关。
    ///
    /// **2. 断言比对首层 source 的原文**，而不是「长度变长了」或「包含某个词」——
    /// 那两种都能被类别前缀单独满足，把 source 遍历整段删掉测试照样过（codex review
    /// 抓到的正是这一点）。
    #[tokio::test]
    async fn transport_errors_carry_their_source_chain() {
        // 接受连接后什么都不做（连 listener 都不 drop）⇒ 客户端等响应等到超时。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            // 握住连接不放：一 drop 就变成「连接被对端关闭」，那是另一类错误。
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        // `.no_proxy()` 不可省，理由见下一条测试。
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .no_proxy()
            .build()
            .expect("build client");
        let err = client
            .get(format!("http://{addr}/whatever"))
            .send()
            .await
            .expect_err("对端永不回应，必须超时");

        let bare = format!("{err}");
        let described = describe_send_error(&err);

        // 前提一：`{e}` 真的什么都没说 —— 这正是 bug 的形状。
        assert!(
            !bare.contains("cause"),
            "`{{e}}` 不该带 cause，否则这条测试的前提不成立: {bare}"
        );
        // 前提二：这个错误确实有 source 可展开（没有的话下面的断言就是空转）。
        let first = std::error::Error::source(&err).expect("传输错误必须有 source 可展开");

        // 本体：首层 source 的原文必须出现在描述里。删掉 source 遍历这条就会红。
        assert!(
            described.contains(&format!("cause: {first}")),
            "描述必须带上首层 source 的原文\n  source: {first}\n  desc: {described}"
        );
    }

    /// 分类前缀不能张冠李戴：连接失败不该被说成超时。
    #[tokio::test]
    async fn describe_send_error_labels_connect_failures_as_connect() {
        // 先绑一个端口再立即释放 ⇒ 拿到一个**确定没人监听**的地址。
        // 不用写死的 `127.0.0.1:1`：没有哪条规矩保证它在所有 CI 上都空着。
        let addr = {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind probe");
            probe.local_addr().expect("local addr")
            // probe 在这里 drop，端口回到无人监听状态。
        };

        // `.no_proxy()` 不可省：维护者机器上开着 Clash，系统代理会把这个请求接走并回
        // **503**（`proxy-connection: close`）⇒ 拿到的是 `Ok(response)` 而不是传输错误，
        // 测试会以「这个端口竟然有人监听」的形式失败。这里要的是「连接失败」这个事件本身，
        // 不该受运行环境有没有代理影响。
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("build client");
        let err = client
            .get(format!("http://{addr}/whatever"))
            .send()
            .await
            .expect_err("刚释放的端口不该有人监听");

        // 前提：这确实是一个连接类失败、且不是超时。否则下面在验别的东西。
        assert!(err.is_connect(), "前提不成立，这不是连接类错误: {err:?}");
        assert!(!err.is_timeout(), "前提不成立，这是超时错误: {err:?}");

        // 本体：分类前缀必须如实说「连不上」，不能标成超时或含糊的兜底措辞。
        let described = describe_send_error(&err);
        assert!(
            described.starts_with("连不上服务器"),
            "连接类失败的分类前缀必须是「连不上服务器」: {described}"
        );
    }
}
