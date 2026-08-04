//! DeepSeek 开放平台（`platform.deepseek.com`）的契约实现。
//!
//! 全部事实来自 2026-08-03 真机实测（含一次授权的 create+delete 写操作），
//! bundle `main.50ec61b52a.js` / `commit-id: a274378`。完整调研见
//! `../LoongPort-design/spec-官网直连账号与API-Key补充.md` §二。
//!
//! ## 四条会静默出错的约定
//!
//! 1. **双层信封** `{code, msg, data:{biz_code, biz_msg, biz_data}}` —— 两层都要判。
//!    外层 `code == 0` 只表示传输成功，业务成败在 `biz_code`。
//! 2. **未鉴权返回 HTTP 200** + `code: 40002`（`"Missing Token"`）——
//!    靠状态码判鉴权失败会漏。
//! 3. **明文 sk 与脱敏值长度都是 35**，同名字段 `sensitive_id`，
//!    唯一区别是有无 `*`（明文 0 个、脱敏 26 个）。见 [`validate_plaintext_key`]。
//! 4. **base_url 分四种形态**（裸根 / `/anthropic` / `/v1`），见 [`config_for`]。
//!
//! ## 鉴权只要 Bearer
//!
//! 实测裸 curl UA、无 cookie、不带那五个 `x-client-*` 头照样 200 ⇒ 用普通
//! HTTP 客户端直连即可，WebView 只用于登录那一步。
//!
//! ⚠️ **不要照抄 `operator::login::WEBVIEW_USER_AGENT` 的「必须写死」那条理由** ——
//! 那是因为 **sub2api 有会话绑定**（token 里带 `SHA256(clientIP + UA)`，可选特性、
//! 默认关）。DeepSeek 没有这个约束。

use serde::{de::DeserializeOwned, Deserialize};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::vendor::{VendorAccount, VendorBalance, VendorError, VendorKey};

/// 凭据回传用的自定义 scheme。
///
/// ⚠️ **与 operator 那条（`loongport-creds`）不同名**：两个登录窗可能同时开着，
/// 同名会让一边的回传被另一边的 `on_navigation` 认走 —— 而它们的 payload 形状不同
/// （这边多 `account_id` / `login_identifier`），认错就是解析失败。
const CREDS_SCHEME: &str = "loongport-vendor-creds";

/// 登录窗的 label。⚠️ **不得与 operator 的 `loongport-login` 重名** ——
/// label 是 Tauri 的窗口唯一键，撞名会让 `build()` 直接失败。
pub const LOGIN_WINDOW_LABEL: &str = "loongport-vendor-login";

/// 站点 origin。**编译期常量，不是用户输入**（官网不需要探测）。
pub const SITE_ORIGIN: &str = "https://platform.deepseek.com";

/// 登录页。
pub const LOGIN_URL: &str = "https://platform.deepseek.com/sign_in";

/// 官网 key 管理页。超上限时引导用户去这里。
///
/// 消费者是 Task 6 的 UI（`KeyLimitReached` 时那个「去官网删除」按钮）——
/// 命令层这一侧没有它的用途，所以单独 allow 而不是给整层开 `dead_code`
/// （那会让本层其余部分失去这道守卫）。
#[allow(dead_code)]
pub const API_KEYS_URL: &str = "https://platform.deepseek.com/api_keys";

/// 登录态所在的 localStorage 键。
///
/// ⚠️ **值包了一层 JSON**：`{"value":"<token>","__version":"0"}`，不是裸字符串
/// （与 sub2api 的 `auth_token` 不同）⇒ 注入脚本要 `JSON.parse(raw).value`。
pub const USER_TOKEN_KEY: &str = "userToken";

/// 未鉴权时的业务码。⚠️ HTTP 仍是 200。
const CODE_MISSING_TOKEN: i64 = 40002;

/// `biz_code == 1` = key 数量到上限（100 把）。
const BIZ_EXCEED_MAXIMUM_KEY_NUM: i64 = 1;

/// 外层信封。
#[derive(Debug, Deserialize)]
struct Outer<T> {
    code: i64,
    #[serde(default)]
    msg: String,
    data: Option<Inner<T>>,
}

/// 内层信封。
#[derive(Debug, Deserialize)]
struct Inner<T> {
    #[serde(default)]
    biz_code: i64,
    #[serde(default)]
    biz_msg: String,
    biz_data: Option<T>,
}

/// 解双层信封。
pub fn parse_envelope<T: DeserializeOwned>(body: &str, what: &str) -> Result<T, VendorError> {
    let outer: Outer<T> = serde_json::from_str(body)
        .map_err(|e| VendorError::Transient(format!("{what}失败: 响应不是 DeepSeek 格式: {e}")))?;

    if outer.code == CODE_MISSING_TOKEN {
        return Err(VendorError::AuthExpired);
    }
    if outer.code != 0 {
        let msg = if outer.msg.is_empty() {
            format!("code {}", outer.code)
        } else {
            outer.msg
        };
        return Err(VendorError::Transient(format!("{what}失败: {msg}")));
    }

    let inner = outer
        .data
        .ok_or_else(|| VendorError::Transient(format!("{what}失败: 响应缺少 data")))?;

    if inner.biz_code == BIZ_EXCEED_MAXIMUM_KEY_NUM {
        return Err(VendorError::KeyLimitReached);
    }
    if inner.biz_code != 0 {
        let msg = if inner.biz_msg.is_empty() {
            format!("biz_code {}", inner.biz_code)
        } else {
            inner.biz_msg
        };
        return Err(VendorError::Transient(format!("{what}失败: {msg}")));
    }

    inner
        .biz_data
        .ok_or_else(|| VendorError::Transient(format!("{what}失败: 响应缺少 biz_data")))
}

/// 校验这是**明文** sk 而不是脱敏值。
///
/// ⚠️ **两者长度都是 35**（同名字段 `sensitive_id`），唯一区别是有无 `*`
/// （明文 0 个、脱敏 26 个）。不校验就会把脱敏值写进 CLI 配置，
/// 症状是「切过去之后 401」而根因极难查。
pub fn validate_plaintext_key(s: &str) -> Result<String, VendorError> {
    if s.is_empty() || s.contains('*') {
        return Err(VendorError::RedactedValueReturned);
    }
    Ok(s.to_string())
}

/// `AppType` → `(base_url, model)`。**唯一一处该出现这些字面量的地方。**
///
/// ⚠️ **四种形态，不是一个值**（逐个核对上游 preset）：
///
/// | 平台 | base_url | preset |
/// |---|---|---|
/// | Codex | 裸根 | `codexProviderPresets.ts:958` |
/// | Claude / ClaudeDesktop | `/anthropic` ← **子路径挂载的兼容层** | `claudeProviderPresets.ts:830` |
/// | Hermes | 裸根 | `hermesProviderPresets.ts:1081` |
/// | OpenClaw / OpenCode | `/v1` | `openclawProviderPresets.ts:1626` |
///
/// `Gemini` / `GrokBuild` 返回 `None` —— 上游没有 DeepSeek preset
/// （Gemini CLI 认 Google 自家协议、GrokBuild 认 xAI 的），不是我们能补的。
///
/// ⚠️ **codex 那条钉 flash**：上游注释记着 pro 的 **Codex 集成**未开通，
/// 切过去会上游报错。**这条限制只作用于 codex** —— claude / opencode 的
/// preset 默认主模型就是 pro。
pub fn config_for(app: &AppType) -> Option<(&'static str, &'static str)> {
    match app {
        AppType::Codex => Some(("https://api.deepseek.com", "deepseek-v4-flash")),
        AppType::Claude | AppType::ClaudeDesktop => {
            Some(("https://api.deepseek.com/anthropic", "deepseek-v4-pro"))
        }
        AppType::Hermes => Some(("https://api.deepseek.com", "deepseek-v4-pro")),
        AppType::OpenClaw | AppType::OpenCode => {
            Some(("https://api.deepseek.com/v1", "deepseek-v4-pro"))
        }
        AppType::Gemini | AppType::GrokBuild => None,
    }
}

/// `GET /api/v0/users/get_api_keys` 的 `biz_data`。
#[derive(Debug, Deserialize)]
pub struct ApiKeysData {
    #[serde(default)]
    pub api_keys: Vec<RawApiKey>,
}

#[derive(Debug, Deserialize)]
pub struct RawApiKey {
    #[serde(default)]
    pub name: String,
    /// ⚠️ 列表里这个字段是**脱敏值**；create 返回的同名字段才是明文。
    #[serde(default)]
    pub sensitive_id: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub tracking_id: String,
}

impl From<RawApiKey> for VendorKey {
    fn from(r: RawApiKey) -> Self {
        VendorKey {
            name: r.name,
            redacted_key: r.sensitive_id,
            created_at: r.created_at,
            tracking_id: r.tracking_id,
        }
    }
}

/// `POST /api/v0/users/edit_api_keys`（`action: create`）的 `biz_data`。
#[derive(Debug, Deserialize)]
pub struct EditKeyData {
    pub api_key: Option<RawApiKey>,
}

/// `GET /api/v0/users/get_user_summary` 的 `biz_data`。
///
/// ⚠️ **金额是字符串、18 位小数** —— 不能让 serde 解成 `f64`（静默丢精度）。
#[derive(Debug, Deserialize)]
pub struct UserSummaryData {
    #[serde(default)]
    pub normal_wallets: Vec<RawWallet>,
    #[serde(default)]
    pub total_costs: Vec<RawCost>,
}

#[derive(Debug, Deserialize)]
pub struct RawWallet {
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub balance: String,
}

#[derive(Debug, Deserialize)]
pub struct RawCost {
    #[serde(default)]
    pub currency: String,
}

/// 把余额格式化成给人看的字符串。
///
/// 规则（spec §2.8 定死，别自己发明）：
/// - 保留 **2 位小数**，四舍五入；
/// - `CNY → ¥`、`USD → $`，其它币种用 `<code> ` 前缀；
/// - **选与 `total_costs` 同币种的那个钱包**，没有则取第一个 ——
///   ⚠️ **不要写死 `[0]`**，海外账号 CNY/USD 并存（bundle 里有两套告警开关）；
/// - `bonus_wallets`（赠送）**不显示**，与 sub2api 那边 `frozen_balance` 不显示同口径。
pub fn format_balance(data: &UserSummaryData) -> Option<VendorBalance> {
    let preferred = data.total_costs.first().map(|c| c.currency.as_str());
    let wallet = preferred
        .and_then(|cur| data.normal_wallets.iter().find(|w| w.currency == cur))
        .or_else(|| data.normal_wallets.first())?;

    let symbol = match wallet.currency.as_str() {
        "CNY" => "¥".to_string(),
        "USD" => "$".to_string(),
        other => format!("{other} "),
    };
    Some(VendorBalance(format!(
        "{symbol}{}",
        round_decimal_string(&wallet.balance)?
    )))
}

/// 十进制字符串保留两位小数（四舍五入），**全程不经 `f64`**。
///
/// ## 为什么不能 `parse::<f64>()` 再 `{:.2}`
///
/// 服务端给的是 18 位小数的字符串（实测 `"547.0842385200000000"`）。
/// IEEE754 表示不了大多数十进制小数，于是「进位边界」上会静默偏一分钱：
///
/// | 输入 | 经 f64 | 正确 |
/// |---|---|---|
/// | `"1.005"` | `1.00` | `1.01` |
/// | `"2.675"` | `2.67` | `2.68` |
/// | `"0.015"` | `0.01` | `0.02` |
///
/// spec §2.8 整段论证的就是「照初稿写必然在某个边界悄悄 parseFloat，
/// 把自己要保的精度丢掉」—— 初版把 `parseFloat` 从前端搬到了 Rust 侧，
/// 病没治，只是换了地方。final review 抓出。
///
/// 返回 `None` = 不是合法的十进制数字串（那种情况上层不显示余额，
/// 与「拿不到余额」同一处理，不编造一个 0）。
fn round_decimal_string(raw: &str) -> Option<String> {
    let s = raw.trim();
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((i, f)) => (i, f),
        None => (digits, ""),
    };
    // 只接受纯数字；空整数部分（如 ".5"）按 "0" 处理。
    if int_part
        .chars()
        .chain(frac_part.chars())
        .any(|c| !c.is_ascii_digit())
        || (int_part.is_empty() && frac_part.is_empty())
    {
        return None;
    }
    let int_part = if int_part.is_empty() { "0" } else { int_part };

    // 取两位小数 + 看第三位决定是否进位。
    let mut frac: Vec<u8> = frac_part.chars().take(2).map(|c| c as u8 - b'0').collect();
    while frac.len() < 2 {
        frac.push(0);
    }
    let carry = frac_part.chars().nth(2).map(|c| c >= '5').unwrap_or(false);

    let mut int_digits: Vec<u8> = int_part.bytes().map(|b| b - b'0').collect();
    if carry {
        let mut i = frac.len();
        let mut c = true;
        while c && i > 0 {
            i -= 1;
            if frac[i] == 9 {
                frac[i] = 0;
            } else {
                frac[i] += 1;
                c = false;
            }
        }
        if c {
            // 小数部分全进位完了，进到整数部分
            let mut j = int_digits.len();
            while c && j > 0 {
                j -= 1;
                if int_digits[j] == 9 {
                    int_digits[j] = 0;
                } else {
                    int_digits[j] += 1;
                    c = false;
                }
            }
            if c {
                int_digits.insert(0, 1);
            }
        }
    }

    let int_str: String = int_digits.iter().map(|d| (d + b'0') as char).collect();
    let frac_str: String = frac.iter().map(|d| (d + b'0') as char).collect();
    let int_str = int_str.trim_start_matches('0');
    let int_str = if int_str.is_empty() { "0" } else { int_str };
    let sign = if neg && !(int_str == "0" && frac_str == "00") {
        "-"
    } else {
        ""
    };
    Some(format!("{sign}{int_str}.{frac_str}"))
}

// ─────────────────────────── HTTP ───────────────────────────

/// 鉴权只要 Bearer（见模块文档），所以是个普通 HTTP 客户端。
///
/// UA 复用 `operator::login::WEBVIEW_USER_AGENT`（现在是按平台的 `cfg` 常量）。
/// ⚠️ **别照抄 sub2api 那条「必须与 WebView 一字不差否则撤销整个会话家族」的理由** ——
/// 那是它的会话绑定特性，DeepSeek 没有。这里复用只是为了不再多一份 UA 字面量。
fn build_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(crate::operator::login::WEBVIEW_USER_AGENT)
        .build()
        .map_err(|e| AppError::Config(format!("创建 HTTP 客户端失败: {e}")))
}

/// 一次 GET，返回响应体文本。
async fn get_text(token: &str, path: &str, what: &str) -> Result<String, VendorError> {
    let client = build_client().map_err(|e| VendorError::Transient(e.to_string()))?;
    client
        .get(format!("{SITE_ORIGIN}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| VendorError::Transient(format!("{what}失败: {e}")))?
        .text()
        .await
        .map_err(|e| VendorError::Transient(format!("{what}失败: 读取响应出错: {e}")))
}

/// 一次 POST（JSON body），返回响应体文本。
async fn post_text(
    token: &str,
    path: &str,
    body: &serde_json::Value,
    what: &str,
) -> Result<String, VendorError> {
    let client = build_client().map_err(|e| VendorError::Transient(e.to_string()))?;
    client
        .post(format!("{SITE_ORIGIN}{path}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(|e| VendorError::Transient(format!("{what}失败: {e}")))?
        .text()
        .await
        .map_err(|e| VendorError::Transient(format!("{what}失败: 读取响应出错: {e}")))
}

/// 拉这个账号下的全部 key。
///
/// ⚠️ 返回的每一把都**只有脱敏值**（`VendorKey` 有意不含明文字段）。
pub async fn list_keys(token: &str) -> Result<Vec<VendorKey>, VendorError> {
    let body = get_text(token, "/api/v0/users/get_api_keys", "拉取密钥列表").await?;
    let data: ApiKeysData = parse_envelope(&body, "拉取密钥列表")?;
    Ok(data.api_keys.into_iter().map(VendorKey::from).collect())
}

/// 建一把新 key，返回**明文**。
///
/// ⚠️ 明文只在这一刻给一次（列表接口永远拿不回来）⇒ 调用方必须落库。
/// 返回前已过 [`validate_plaintext_key`]：官网若给回脱敏值就报
/// [`VendorError::RedactedValueReturned`]，而不是把一把不能用的值传下去。
pub async fn create_key(token: &str, name: &str) -> Result<String, VendorError> {
    // 字段名与全部四个 key 都不能省 —— 服务端按 action 分派，缺字段直接 400。
    let payload = serde_json::json!({
        "action": "create",
        "name": name,
        "redacted_key": null,
        "created_at": null,
        "tracking_id": null,
    });
    let body = post_text(token, "/api/v0/users/edit_api_keys", &payload, "创建密钥").await?;
    let data: EditKeyData = parse_envelope(&body, "创建密钥")?;
    let raw = data
        .api_key
        .ok_or_else(|| VendorError::Transient("创建密钥失败: 响应里没有密钥".to_string()))?;
    // ⚠️ 与列表同名的 `sensitive_id`，但**这一处才是明文** —— 仍然要校验，
    // 服务端哪天改成回脱敏值的话，不校验就会把它写进 CLI 配置（症状是切过去 401）。
    validate_plaintext_key(&raw.sensitive_id)
}

/// 删一把 key。
///
/// ⚠️ **靠三元组定位**（`redacted_key` + `created_at` + `tracking_id`），不是靠名字 ——
/// 同名的 key 可以有多把。`created_at` 单位是**秒**（[`VendorKey::created_at`] 已是秒）。
pub async fn delete_key(token: &str, key: &VendorKey) -> Result<(), VendorError> {
    let payload = serde_json::json!({
        "action": "delete",
        "name": null,
        "redacted_key": key.redacted_key,
        "created_at": key.created_at,
        "tracking_id": key.tracking_id,
    });
    let body = post_text(token, "/api/v0/users/edit_api_keys", &payload, "删除密钥").await?;
    // 响应体里的 key 对象删除后是 null，所以解成 `EditKeyData` 而不取里面的东西 ——
    // 这里要的只是「两层信封都判过」这个副作用。
    let _: EditKeyData = parse_envelope(&body, "删除密钥")?;
    Ok(())
}

/// 查余额。`None` = 拿不到（没有钱包 / 金额解不动）—— **不是显示 0**。
pub async fn balance(token: &str) -> Result<Option<VendorBalance>, VendorError> {
    let body = get_text(token, "/api/v0/users/get_user_summary", "查询余额").await?;
    let data: UserSummaryData = parse_envelope(&body, "查询余额")?;
    Ok(format_balance(&data))
}

// ─────────────────────── 登录窗（注入脚本 + 凭据回传）───────────────────────

/// 登录窗回传的凭据。
///
/// ⚠️ **不复用 `operator::login::Credentials`** —— 那个装不下 `account_id` 与
/// `login_identifier`，而给它加字段会把 sub2api 那半边的接触面一起扩大
/// （那三个字段在它那边是「登录后再拉一次 profile」拿的，形状不同）。
///
/// 三个字段都是**必需**的：`account_id` 缺了就没法去重（`(vendor_id, account_id)`
/// 是表的唯一索引），而「有 token 却没 account_id」是个死局 —— 见
/// [`crate::commands::vendor::vendor_open_login`] 那段失败语义。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VendorCreds {
    pub auth_token: String,
    pub account_id: String,
    /// 重登时预填的值（手机号，回落邮箱）。**可以是空串** —— 微信扫码登录的账号
    /// 可能两样都没有，那时只是少了预填这个便利，不影响登录本身。
    #[serde(default)]
    pub login_identifier: String,
}

/// 判断一次导航是不是凭据回传，是则解出凭据。
///
/// 返回 `None` = 普通导航（调用方放行）；`Some` = 回传（调用方拦下）。
///
/// ⚠️ **不复用 `operator::login::parse_creds_navigation`**：scheme 不同、payload 形状
/// 不同（见 [`VendorCreds`]）。两边共用一个 scheme 才是真正的坑 —— 两个登录窗同时开着
/// 时会互相认走对方的回传。
pub fn parse_creds_navigation(url: &url::Url) -> Option<Result<VendorCreds, AppError>> {
    if url.scheme() != CREDS_SCHEME {
        return None;
    }
    Some(decode_creds(url))
}

fn decode_creds(url: &url::Url) -> Result<VendorCreds, AppError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    let encoded = url
        .query_pairs()
        .find(|(k, _)| k == "d")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| AppError::Config("凭据回传缺少数据".into()))?;

    // 脚本那边已经去掉了 `=` 填充，所以是 NO_PAD；这里再 trim 一次 `=`，
    // 免得将来改脚本时留下一个「只在某些长度的 payload 上失败」的坑。
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .map_err(|e| AppError::Config(format!("凭据回传的数据解不开: {e}")))?;
    let json = String::from_utf8(bytes)
        .map_err(|e| AppError::Config(format!("凭据回传的数据不是 UTF-8: {e}")))?;

    let creds: VendorCreds = serde_json::from_str(&json)
        .map_err(|e| AppError::Config(format!("凭据回传的格式不对: {e}")))?;

    if creds.auth_token.is_empty() {
        return Err(AppError::Config("登录页没有给出登录态".into()));
    }
    // 账号身份缺失时**在这里就拒**，不让它往下走去建行 ——
    // 「有 token 却没 account_id」的行既去不了重、也认不回已建的 key。
    if creds.account_id.is_empty() {
        return Err(AppError::Config("登录页没有给出账号标识".into()));
    }

    Ok(creds)
}

impl From<VendorCreds> for VendorAccount {
    fn from(c: VendorCreds) -> Self {
        VendorAccount {
            // 手机号本身就是给人看的名字（DeepSeek 没有昵称概念）。
            // 空的时候回落到 account_id，不留一个空标签让 UI 显示成一行空白。
            label: if c.login_identifier.is_empty() {
                c.account_id.clone()
            } else {
                c.login_identifier.clone()
            },
            account_id: c.account_id,
            login_identifier: c.login_identifier,
        }
    }
}

/// 登录页注入脚本。
///
/// ## 与 sub2api 那份的三处不同（每处都会让登录**静默**失败）
///
/// 1. **键名是 `userToken` 而非 `auth_token`**；
/// 2. **值包了一层 JSON** `{"value":"<token>","__version":"0"}` ⇒ 要 `JSON.parse(raw).value`；
/// 3. **`account_id` 不在 localStorage** —— user 对象只活在内存里的 zustand store
///    （实测把 localStorage / sessionStorage 全量扫过都没有）。所以**劫持 `fetch`**，
///    从登录响应的 `data.biz_data.user` 里取 `id` 与 `mobile`。
///
/// ⚠️ **劫持条件按「响应体里有 `biz_data.user.id`」判，不要按 URL 白名单判** ——
/// 三种登录方式端点各不同（密码 / 短信 / 微信 OAuth），白名单必漏掉一条，
/// 症状是「扫码显示登录成功但账号识别不出来」。
///
/// `login_hint` 是重登时预填进登录框的值（空串 = 不预填）。
/// ⚠️ **预填必须派 `input` 事件** —— DeepSeek 前端是 React（受控组件），
/// 只设 `el.value` 的话 DOM 上看得见字但 state 还是空的，提交上去是空手机号。
pub fn login_script(login_hint: &str) -> String {
    // JSON 编码而不是直接插进单引号里：这个值来自数据库（上次登录存的），
    // 含引号就会破坏脚本语法。
    let hint = serde_json::to_string(login_hint).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"(function () {{
  'use strict';

  // 只在顶层 frame 跑：同源 iframe 会让脚本多执行一份、重复回传。
  if (window.top !== window.self) return;

  var SENT = false;
  var TOKEN_KEY = '{USER_TOKEN_KEY}';

  function b64url(s) {{
    var bytes = new TextEncoder().encode(s);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  // localStorage 里那个值**包了一层 JSON**，不是裸 token。
  function readToken() {{
    try {{
      var raw = window.localStorage.getItem(TOKEN_KEY);
      if (!raw) return null;
      var parsed = JSON.parse(raw);
      return (parsed && parsed.value) || null;
    }} catch (e) {{
      return null;
    }}
  }}

  // 账号 id。**与登录方式无关地**从 localStorage 取，见下方 poll() 的说明。
  function readAccountId() {{
    try {{
      var keys = Object.keys(window.localStorage);
      for (var i = 0; i < keys.length; i++) {{
        var k = keys[i];
        // 埋点缓存：`{{web_id, user_unique_id, ...}}`。登录成功时由埋点 SDK 写入。
        if (k.indexOf('__tea_cache_tokens_') === 0) {{
          var v = JSON.parse(window.localStorage.getItem(k));
          if (v && v.user_unique_id) return String(v.user_unique_id);
        }}
        // 备用：usage 页的偏好键名后缀就是同一个 id（两处交叉印证过）。
        if (k.indexOf('deepseek.platform.usage.boardPreference.v1.') === 0) {{
          var parts = k.split('.');
          var last = parts[parts.length - 1];
          if (last && last.length > 8) return last;
        }}
      }}
    }} catch (e) {{}}
    return null;
  }}

  // 兜底身份：**token 的哈希**。
  //
  // 前两个来源都依赖埋点 SDK 写过 localStorage，而 `incognito(true)` 下
  // 那个 SDK 可能被拦（它自己就在读一个 feature flag 决定要不要加载）。
  // 拿不到真实 account_id 时用这个 —— 它稳定（同一个账号的 token 换了也没关系，
  // 因为换 token 只发生在重新登录，那时会走 `save_account` 的更新路径）、
  // 且不依赖任何外部来源，保证功能不会因为埋点被拦而整体失效。
  //
  // ⚠️ 代价：这种 id 与官网真实 user id 不同 ⇒ 同一个账号若一次走埋点路径、
  // 一次走兜底路径，会被当成两行。可接受（前两层几乎总能命中，
  // 且用户看到两行时删掉一个即可），远好于「登录了但什么都没发生」。
  function fallbackId(token) {{
    var h = 5381;
    for (var i = 0; i < token.length; i++) {{
      h = ((h << 5) + h + token.charCodeAt(i)) | 0;
    }}
    return 'tok-' + (h >>> 0).toString(16);
  }}

  // 登录标识（手机号 / 邮箱）。**拿不到不阻断** —— 它只用于下次重登预填。
  function readLoginIdentifier() {{
    try {{
      var el = document.querySelector('input[type=tel], input[autocomplete=tel]');
      if (el && el.value) return String(el.value);
    }} catch (e) {{}}
    return '';
  }}

  function send(token, accountId, identifier) {{
    if (SENT || !token || !accountId) return;
    SENT = true;
    var payload = JSON.stringify({{
      auth_token: String(token),
      account_id: String(accountId),
      login_identifier: String(identifier || '')
    }});
    // 顶层导航发回给 Rust。`on_navigation` 返回 false 会拦下这次跳转，
    // 页面原样留着（用户接着能在窗口里看余额 / 充值）。
    window.location.href = '{CREDS_SCHEME}://ok?d=' + b64url(payload);
  }}

  // 轮询 localStorage（不劫持任何请求）—— 判据与登录方式无关。
  // 完整理由见 Rust 侧 `login_script` 的文档注释。
  var POLL_MS = 400;
  var POLL_LIMIT = 1500; // 400ms × 1500 = 10 分钟
  // 拿到 token 后再多给埋点这么多轮去写 id（400ms × 15 = 6 秒），
  // 超了就用 fallbackId。
  var GRACE_POLLS = 15;
  var polls = 0;

  function poll() {{
    if (SENT) return;
    if (++polls > POLL_LIMIT) return;
    var token = readToken();
    if (token) {{
      var acct = readAccountId();
      if (acct) {{
        send(token, acct, readLoginIdentifier());
        return;
      }}
      // 有 token 但埋点还没写 id：**先多等一会儿**（埋点通常几百毫秒内就写），
      // 等够 GRACE 次数还没有就用兜底 —— 宁可用哈希 id 也不能卡在这里。
      if (polls > GRACE_POLLS) {{
        send(token, fallbackId(token), readLoginIdentifier());
        return;
      }}
    }}
    window.setTimeout(poll, POLL_MS);
  }}
  // 先立刻试一次（覆盖「已登录状态下打开窗口」），再进轮询。
  poll();

  // 重登时预填手机号，用户只需补验证码。空串 = 首次登录，不填。
  var HINT = {hint};
  var prefilled = false;

  function tryPrefill() {{
    if (prefilled || !HINT) return;
    // 只碰登录标识那个框，别碰密码 / 验证码框。
    var selectors = ['input[type=tel]', 'input[autocomplete=tel]', 'input[type=email]'];
    for (var i = 0; i < selectors.length; i++) {{
      var el = document.querySelector(selectors[i]);
      if (!el) continue;
      // 用户已经自己输了东西就别覆盖 —— 他可能正要换个账号登。
      if (el.value) {{ prefilled = true; return; }}
      el.value = HINT;
      // **必须派事件**：React 的受控组件只读 state，直接设 value 的话
      // DOM 上有字而 state 是空的，提交上去就是空手机号。
      el.dispatchEvent(new Event('input', {{ bubbles: true }}));
      el.dispatchEvent(new Event('change', {{ bubbles: true }}));
      prefilled = true;
      return;
    }}
  }}

  // 轮询兜底两件事：
  // 1. 用户可能是**已登录状态**直接打开页面（不会有登录响应可劫持）——
  //    但那时也没有 user 对象，所以得靠页面自己发的某个带 user 的请求；
  // 2. SPA 的登录表单是异步渲染的，脚本跑的时候那个框往往还不存在。
  var polls = 0;
  var timer = setInterval(function () {{
    polls++;
    tryPrefill();
    if (SENT || polls > 600) clearInterval(timer);
  }}, 500);
  tryPrefill();
}})();
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS_OK: &str = include_str!("fixtures/deepseek/get_api_keys.json");
    const CREATE_OK: &str = include_str!("fixtures/deepseek/edit_api_keys_create.json");
    const SUMMARY: &str = include_str!("fixtures/deepseek/get_user_summary.json");
    const MISSING_TOKEN: &str = include_str!("fixtures/deepseek/missing_token.json");

    #[test]
    fn parses_the_two_layer_envelope() {
        let data: ApiKeysData = parse_envelope(KEYS_OK, "拉列表").expect("解析");
        assert_eq!(data.api_keys.len(), 2);
        assert_eq!(data.api_keys[0].name, "用户手建的");
        assert_eq!(data.api_keys[1].name, "LoongPort专用/dev-1");
        assert_eq!(
            data.api_keys[1].tracking_id,
            "00000000-0000-0000-0000-000000000002"
        );
    }

    /// 列表接口给的是**脱敏值** —— `VendorKey` 有意不含明文字段，这条钉住映射方向。
    #[test]
    fn list_entries_map_into_vendor_keys_carrying_only_the_redacted_value() {
        let data: ApiKeysData = parse_envelope(KEYS_OK, "拉列表").expect("解析");
        let key: VendorKey = data.api_keys.into_iter().next().expect("有 key").into();
        assert!(
            key.redacted_key.contains('*'),
            "列表里的 sensitive_id 一定是脱敏值"
        );
        assert_eq!(key.created_at, 1782291938);
    }

    #[test]
    fn missing_token_is_auth_expired_even_on_http_200() {
        let got = parse_envelope::<ApiKeysData>(MISSING_TOKEN, "拉列表").unwrap_err();
        assert_eq!(got, VendorError::AuthExpired, "40002 必须归类成登录过期");
    }

    /// fixture 里那把「明文」必须真的是明文形状 —— 否则下面所有闸都失去判别力。
    #[test]
    fn the_create_fixture_carries_a_plaintext_shaped_key() {
        let data: EditKeyData = parse_envelope(CREATE_OK, "建密钥").expect("解析");
        let sk = data.api_key.expect("有 key").sensitive_id;
        assert!(!sk.contains('*'), "fixture 的 create 返回必须零星号");
        assert_eq!(sk.len(), 35, "与脱敏值同长，长度不能当判据");
        assert!(
            validate_plaintext_key(&sk).is_ok(),
            "否则 create_key_rejects_redacted_value 那条闸永远绿、测不出东西"
        );
    }

    #[test]
    fn inner_biz_code_one_is_key_limit_reached() {
        // ⚠️ 外层 code=0（传输成功），失败在内层 —— 只判外层会漏。
        let body = r#"{"code":0,"msg":"","data":{"biz_code":1,"biz_msg":"","biz_data":null}}"#;
        let got = parse_envelope::<EditKeyData>(body, "建密钥").unwrap_err();
        assert_eq!(got, VendorError::KeyLimitReached);
    }

    #[test]
    fn plaintext_and_redacted_have_the_same_length_so_only_stars_decide() {
        let plaintext = "sk-1d85aaaaaaaaaaaaaaaaaaaaaaaaaa0e";
        let redacted = "sk-25c**************************122";
        // 这两条断言是本闸的判别力来源：长度相同 ⇒ 不能靠长度区分，只有 `*` 是判据。
        assert_eq!(plaintext.len(), 35, "实测明文是 sk- + 32 位，共 35 字符");
        assert_eq!(
            plaintext.len(),
            redacted.len(),
            "同长 ⇒ 长度不是判据，只有 * 是"
        );
        assert_eq!(plaintext.matches('*').count(), 0);
        assert!(validate_plaintext_key(plaintext).is_ok());
        assert_eq!(
            validate_plaintext_key(redacted).unwrap_err(),
            VendorError::RedactedValueReturned,
            "含 * 必须拒绝 —— 否则脱敏值会被写进 CLI 配置，症状是切过去 401"
        );
        assert_eq!(
            validate_plaintext_key("").unwrap_err(),
            VendorError::RedactedValueReturned
        );
    }

    #[test]
    fn claude_gets_the_anthropic_suffix_and_codex_stays_bare() {
        assert_eq!(
            config_for(&AppType::Claude).expect("claude").0,
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(
            config_for(&AppType::ClaudeDesktop).expect("desktop").0,
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(
            config_for(&AppType::Codex).expect("codex").0,
            "https://api.deepseek.com"
        );
        assert_eq!(
            config_for(&AppType::OpenClaw).expect("openclaw").0,
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            config_for(&AppType::OpenCode).expect("opencode").0,
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            config_for(&AppType::Hermes).expect("hermes").0,
            "https://api.deepseek.com"
        );
    }

    #[test]
    fn gemini_and_grokbuild_have_no_deepseek_config() {
        assert!(config_for(&AppType::Gemini).is_none());
        assert!(config_for(&AppType::GrokBuild).is_none());
    }

    #[test]
    fn codex_is_the_only_platform_pinned_to_flash() {
        assert_eq!(
            config_for(&AppType::Codex).expect("codex").1,
            "deepseek-v4-flash"
        );
        for app in [AppType::Claude, AppType::ClaudeDesktop, AppType::OpenCode] {
            assert_eq!(
                config_for(&app).expect("pro 平台").1,
                "deepseek-v4-pro",
                "{app:?} 该用 pro —— 「不给 pro」那条只对 codex 成立"
            );
        }
    }

    #[test]
    fn balance_picks_the_wallet_matching_the_cost_currency() {
        let data: UserSummaryData = parse_envelope(SUMMARY, "余额").expect("解析");
        // ⚠️ fixture 里 [0] 故意是 USD —— 写死 [0] 会得到 "$1.00"，这条能抓住。
        assert_eq!(
            data.normal_wallets[0].currency, "USD",
            "fixture 顺序是判别力"
        );
        assert_eq!(format_balance(&data).expect("有余额").0, "¥547.08");
    }

    // ─────────────────── 注入脚本 ───────────────────

    #[test]
    fn login_script_reads_the_wrapped_token_key() {
        let s = login_script("13800000000");
        assert!(
            s.contains(USER_TOKEN_KEY),
            "键名是 userToken，不是 auth_token"
        );
        assert!(
            s.contains("JSON.parse"),
            "值包了一层 JSON，必须 parse 出 .value"
        );
        assert!(
            s.contains("__tea_cache_tokens_"),
            "account_id 从埋点缓存取 —— 那是与登录方式无关的来源"
        );
        assert!(
            s.contains("setTimeout") && s.contains("poll"),
            "必须是轮询：微信扫码的响应体里拿不到 user，劫持 fetch 判不出来"
        );
        assert!(
            s.contains("new Event('input'"),
            "预填必须派 input 事件，否则 React 的 state 还是空的、提交上去是空手机号"
        );
        assert!(s.contains("13800000000"), "预填值要注进脚本");
    }

    /// ⭐ **判据不能依赖任何单一登录方式**（2026-08-04 维护者实测撞到）。
    ///
    /// 初版劫持 `window.fetch`、判「响应体里有 `biz_data.user.id`」。
    /// **微信扫码那条路永远不成立** —— 它是两步：`/oauth/get_token` 只回 token
    /// （没有 user），user 由另一个调用拿。两步都不满足「token 与 user 齐备」。
    /// 症状：扫码登录成功，但表里一行没有、日志零记录。
    ///
    /// 现在轮询 localStorage：token 与 account_id 在那里**与登录方式无关**。
    #[test]
    fn login_script_does_not_depend_on_any_single_login_flow() {
        let s = login_script("");
        // 不按 URL 白名单判（三种登录方式端点不同，白名单必漏一条）
        assert!(!s.contains("/login_by_mobile_sms"));
        assert!(!s.contains("/oauth/get_token"));
        // 也不靠劫持 fetch 读响应体形状
        assert!(
            !s.contains("window.fetch = function"),
            "劫持 fetch 判响应体形状会漏掉微信扫码 —— 用轮询"
        );
        assert!(!s.contains("j.data.biz_data"), "不该再依赖响应体的形状");
        // 两个与登录方式无关的来源
        assert!(s.contains(USER_TOKEN_KEY), "token 从 localStorage 取");
        assert!(
            s.contains("__tea_cache_tokens_") && s.contains("boardPreference"),
            "account_id 两个来源都要有（互为备用，实测同一个 UUID）"
        );
    }

    /// ⭐ **有 token 就一定回传得出去，不许卡在「等 account_id」上。**
    ///
    /// 前两个来源（埋点缓存 / usage 偏好键）都依赖埋点 SDK 写过 localStorage，
    /// 而 `incognito(true)` 下那个 SDK 可能被拦。没有兜底的话症状与这次翻车一样：
    /// **用户登录成功了，但什么都没发生**。
    #[test]
    fn login_script_always_has_a_way_to_derive_an_account_id() {
        let s = login_script("");
        assert!(
            s.contains("fallbackId"),
            "必须有不依赖埋点的兜底身份，否则埋点被拦就整体失效"
        );
        assert!(
            s.contains("GRACE_POLLS"),
            "兜底前要给埋点一段宽限期（真实 id 优先于哈希 id）"
        );
        // 兜底必须只依赖 token 本身
        assert!(
            s.contains("fallbackId(token)"),
            "兜底的输入只能是 token —— 它是唯一保证存在的东西"
        );
    }

    /// scheme 撞名会让两个登录窗互相认走对方的回传，而两边 payload 形状不同。
    #[test]
    fn the_creds_scheme_differs_from_the_operator_one() {
        assert_eq!(CREDS_SCHEME, "loongport-vendor-creds");
        assert!(
            login_script("").contains(CREDS_SCHEME),
            "脚本里的 scheme 必须与 parse_creds_navigation 认的那个同源"
        );
        assert_ne!(
            LOGIN_WINDOW_LABEL, "loongport-login",
            "窗口 label 撞名会让 build() 直接失败"
        );
    }

    /// 预填值是 `JSON.stringify` 出来的**双引号字面量**，所以判别力在双引号与反斜杠上
    /// （单引号在双引号串里本来就无害）。这个值来自数据库，不能让它闭合掉那个字面量。
    #[test]
    fn a_hint_with_quotes_cannot_break_the_script() {
        let s = login_script("138\"; alert(1); var x=\"");
        assert!(
            !s.contains("\"; alert(1); var x=\""),
            "预填值要 JSON 编码，不能原样插进脚本"
        );
        assert!(
            s.contains(r#"138\"; alert(1); var x=\""#),
            "双引号要转义成 \\\" 留在字面量里，而不是被丢掉"
        );

        // 反斜杠同理：裸的 `\"` 会让转义错位。
        let back = login_script(r#"138\"#);
        assert!(back.contains(r#""138\\""#), "反斜杠要转义成 \\\\：{back}");
    }

    // ─────────────────── 凭据回传 ───────────────────

    fn creds_url(payload: &str) -> url::Url {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let d = URL_SAFE_NO_PAD.encode(payload);
        url::Url::parse(&format!("{CREDS_SCHEME}://ok?d={d}")).expect("URL")
    }

    #[test]
    fn parses_a_well_formed_creds_navigation() {
        let url = creds_url(
            r#"{"auth_token":"tok-1","account_id":"11111111-2222-3333-4444-555555555555",
                "login_identifier":"13800000000"}"#,
        );
        let creds = parse_creds_navigation(&url)
            .expect("要认成回传")
            .expect("要解得开");
        assert_eq!(creds.auth_token, "tok-1");
        assert_eq!(creds.account_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(creds.login_identifier, "13800000000");
    }

    #[test]
    fn ordinary_navigation_is_passed_through() {
        let url = url::Url::parse("https://platform.deepseek.com/api_keys").expect("URL");
        assert!(
            parse_creds_navigation(&url).is_none(),
            "普通导航必须放行，否则用户在窗口里什么都点不动"
        );
        // operator 那条 scheme 也不能被我们认走。
        let other = url::Url::parse("loongport-creds://ok?d=abc").expect("URL");
        assert!(
            parse_creds_navigation(&other).is_none(),
            "两个登录窗可能同时开着，不能认走对方的回传"
        );
    }

    #[test]
    fn a_broken_payload_is_an_error_not_a_pass_through() {
        let bad_b64 =
            url::Url::parse(&format!("{CREDS_SCHEME}://ok?d=!!!not-base64!!!")).expect("URL");
        assert!(
            parse_creds_navigation(&bad_b64)
                .expect("要认成回传")
                .is_err(),
            "坏 base64 要报错 —— 放行等于把凭据带着跳到一个不存在的地址"
        );

        let no_query = url::Url::parse(&format!("{CREDS_SCHEME}://ok")).expect("URL");
        assert!(parse_creds_navigation(&no_query).expect("认").is_err());

        let not_json = creds_url("not json at all");
        assert!(parse_creds_navigation(&not_json).expect("认").is_err());
    }

    /// 「有 token 却没 account_id」是个死局：既去不了重，也认不回已建的 key。
    #[test]
    fn creds_without_an_account_id_are_rejected() {
        let url = creds_url(r#"{"auth_token":"tok","account_id":"","login_identifier":"138"}"#);
        assert!(
            parse_creds_navigation(&url).expect("认").is_err(),
            "缺账号标识必须在这里就拒，不能让它往下走去建行"
        );

        let no_token = creds_url(r#"{"auth_token":"","account_id":"uuid-a"}"#);
        assert!(parse_creds_navigation(&no_token).expect("认").is_err());
    }

    /// 微信扫码的账号可能既没手机号也没邮箱 —— 那只是少了预填，不该拒登录。
    #[test]
    fn a_missing_login_identifier_is_tolerated() {
        let url = creds_url(r#"{"auth_token":"tok","account_id":"uuid-a"}"#);
        let creds = parse_creds_navigation(&url).expect("认").expect("解");
        assert_eq!(creds.login_identifier, "");

        let acct: VendorAccount = creds.into();
        assert_eq!(
            acct.label, "uuid-a",
            "标签空着会让 UI 显示成一行空白，要回落到 account_id"
        );
    }

    #[test]
    fn creds_become_an_account_carrying_the_phone_as_label() {
        let creds = VendorCreds {
            auth_token: "tok".into(),
            account_id: "uuid-a".into(),
            login_identifier: "13800000000".into(),
        };
        let acct: VendorAccount = creds.into();
        assert_eq!(acct.account_id, "uuid-a");
        assert_eq!(acct.label, "13800000000");
        assert_eq!(acct.login_identifier, "13800000000");
    }

    /// ⚠️ 这三个值经 `f64` 会各偏一分钱（`1.005→1.00` / `2.675→2.67` / `0.015→0.01`）。
    /// 它们是这条闸的判别力来源 —— 换回 `parse::<f64>()` 必红。
    #[test]
    fn rounding_never_goes_through_f64() {
        for (input, want) in [
            ("1.005", "1.01"),
            ("2.675", "2.68"),
            ("0.015", "0.02"),
            ("547.0842385200000000", "547.08"),
            ("9.999", "10.00"),
            ("0", "0.00"),
            ("0.001", "0.00"),
        ] {
            assert_eq!(
                round_decimal_string(input).as_deref(),
                Some(want),
                "{input} 应当变成 {want}"
            );
        }
        assert_eq!(round_decimal_string("abc"), None, "非数字串不编造数值");
        assert_eq!(round_decimal_string(""), None);
    }

    /// ⚠️ **这条走的是 `format_balance` 完整路径**（不是内部的
    /// `round_decimal_string`）—— 只测内部函数的话，把调用点换回
    /// `parse::<f64>()` 那条老路它照样绿（实测过，闸没有判别力）。
    #[test]
    fn format_balance_rounds_without_f64_on_the_real_path() {
        // `1.005` 经 f64 是 1.00，正确是 1.01 —— 这一分钱就是判别力。
        for (raw, want) in [
            ("1.005", "¥1.01"),
            ("2.675", "¥2.68"),
            ("0.015", "¥0.02"),
            ("9.999", "¥10.00"),
        ] {
            let body = format!(
                r#"{{"code":0,"msg":"","data":{{"biz_code":0,"biz_msg":"","biz_data":{{
                   "normal_wallets":[{{"currency":"CNY","balance":"{raw}","token_estimation":"0"}}],
                   "total_costs":[{{"currency":"CNY","amount":"0"}}]}}}}}}"#
            );
            let data: UserSummaryData = parse_envelope(&body, "余额").expect("解析");
            assert_eq!(
                format_balance(&data).expect("有余额").0,
                want,
                "{raw} 应当格式化成 {want}（经 f64 会偏一分钱）"
            );
        }
    }

    #[test]
    fn balance_is_absent_when_there_are_no_wallets() {
        let body = r#"{"code":0,"msg":"","data":{"biz_code":0,"biz_msg":"","biz_data":{
            "normal_wallets":[],"total_costs":[]}}}"#;
        let data: UserSummaryData = parse_envelope(body, "余额").expect("解析");
        assert!(
            format_balance(&data).is_none(),
            "拿不到余额时不显示，不是显示 0"
        );
    }

    /// ⭐ **[`config_for`] 的六个值必须仍与上游 preset 一致。**
    ///
    /// ## 为什么这条值得立闸
    ///
    /// 那六个 `(base_url, model)` 是 Rust 字面量，而**权威副本在上游的 preset 文件里**
    /// （`.ts`，编译器管不到）。上游 `8ae1ce85`（2026-07-31，fork 前 3 天）刚把 DeepSeek
    /// preset 切成 native Responses、加了 catalog 模板 —— 它正在**主动经营**这个厂商，
    /// 而 `codexProviderPresets.ts` 是全仓 churn 最高的文件之一。
    ///
    /// 漂移的症状：用户的 DeepSeek 账号配出**过期的模型名** ⇒ 切过去报「模型不存在」，
    /// 而我们这边零信号。属 `CLAUDE.md` §三点六 点名的那类（同一事实散在多处）。
    ///
    /// ## 判据
    ///
    /// 六个值逐个在对应的 preset 文件里找。**不比结构、只比值出现过** —— preset 的
    /// 数据形状各不相同（env 变量 / `baseUrl` / `base_url` / `endpointCandidates`），
    /// 硬解析会把闸绑死在上游的写法上，那反而更脆。
    ///
    /// 会红的改法：上游改 base_url 或把 `v4` 升成 `v5`；我们这边改任一个值。
    #[test]
    fn the_six_values_still_match_the_upstream_presets() {
        // (AppType, 该值该出现在哪个 preset 文件里)
        let cases: &[(AppType, &str, &str)] = &[
            (
                AppType::Codex,
                "codexProviderPresets.ts",
                include_str!("../../../src/config/codexProviderPresets.ts"),
            ),
            (
                AppType::Claude,
                "claudeProviderPresets.ts",
                include_str!("../../../src/config/claudeProviderPresets.ts"),
            ),
            (
                AppType::Hermes,
                "hermesProviderPresets.ts",
                include_str!("../../../src/config/hermesProviderPresets.ts"),
            ),
            (
                AppType::OpenClaw,
                "openclawProviderPresets.ts",
                include_str!("../../../src/config/openclawProviderPresets.ts"),
            ),
            (
                AppType::OpenCode,
                "opencodeProviderPresets.ts",
                include_str!("../../../src/config/opencodeProviderPresets.ts"),
            ),
        ];

        for (app, preset_name, preset_src) in cases {
            let (base_url, model) =
                config_for(app).unwrap_or_else(|| panic!("{app:?} 该有 DeepSeek 配置"));

            assert!(
                preset_src.contains(&format!("\"{base_url}\"")),
                "{app:?} 的 base_url ({base_url}) 在上游 {preset_name} 里找不到 —— \
                 要么上游改了端点（那我们得跟上，否则用户切过去连不上），\
                 要么我们这边写错了"
            );
            assert!(
                preset_src.contains(&format!("\"{model}\"")),
                "{app:?} 的 model ({model}) 在上游 {preset_name} 里找不到 —— \
                 上游很可能升了模型版本，我们这份字面量已经过期，\
                 用户切过去会报「模型不存在」"
            );
        }

        // Gemini / GrokBuild 有意返回 None（上游没有 DeepSeek preset）——
        // 钉住它，别哪天有人"顺手补全"两个猜出来的端点。
        for app in [AppType::Gemini, AppType::GrokBuild] {
            assert!(
                config_for(&app).is_none(),
                "{app:?} 该返回 None —— 上游没有 DeepSeek preset（Gemini CLI 认 Google 自家\
                 协议、GrokBuild 认 xAI 的），凭猜给一个端点会让用户切过去 401"
            );
        }
    }
}
