//! sub2api 的窄 DTO 与鉴权 HTTP 客户端。
//!
//! 只覆盖 V2 用到的五个端点，字段只取用得上的那几个（sub2api 的 `Group` 有 40+ 字段、
//! `APIKey` 有 30+，全解出来等于把上游的字段变更面全接进来）：
//!
//! | 端点 | 用途 | 鉴权 |
//! |---|---|---|
//! | `GET /api/v1/settings/public` | 域名探测（是不是 sub2api 站） | 无 |
//! | `GET /api/v1/groups/available` | 拉可用分组 | Bearer |
//! | `GET /api/v1/keys` | 认领已有 sk（明文返回） | Bearer |
//! | `POST /api/v1/keys` | 建新 sk | Bearer |
//! | `GET /api/v1/user/profile` | 余额 | Bearer |
//!
//! ## 三条会静默出错的约定
//!
//! 1. **响应是信封** `{code, message, data}`，业务数据在 `data` 里。`code` 为
//!    `"success"` 才算成功——HTTP 200 不代表业务成功。
//! 2. **`/groups/available` 返回的是平数组**，不是分页信封；`/keys` 才是分页
//!    （`{items, total, page, page_size, pages}`）。两者形状不同，别复用同一个解析。
//! 3. **`base_url` 必须归一到带 `/v1`**：sub2api 后台的 `api_base_url` 可能是空串
//!    （bestapi.store 实测就是），而它前端的 codex 分支不做补 `/v1` 的处理。见
//!    [`normalize_api_base`]。

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// sub2api 业务响应信封。
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    code: String,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

impl<T> Envelope<T> {
    /// 取出 `data`，`code != "success"` 或 `data` 缺失时报可见错误。
    fn into_data(self, what: &str) -> Result<T, AppError> {
        if self.code != "success" {
            let msg = if self.message.is_empty() {
                self.code
            } else {
                format!("{} ({})", self.message, self.code)
            };
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
    /// 站点展示名。V2 用它当运营商名字。
    #[serde(default)]
    pub site_name: String,
    /// 服务端注入的版本号（非 DB 设置项），存在即说明是 sub2api。
    #[serde(default)]
    pub version: String,
    /// 后台配置的 API 基址。**可能是空串**，不可盲信，见 [`normalize_api_base`]。
    #[serde(default)]
    pub api_base_url: String,
    /// 是否开放注册。关闭时 `/register` 是死页（只显示黄条），所以登录窗一律加载 `/login`。
    #[serde(default)]
    pub registration_enabled: bool,
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
}

impl Group {
    /// codex 能用的分组：platform 必须是 `openai`。
    ///
    /// 服务端**不按 platform 过滤**（`api_key_service.go` 的 `GetAvailableGroups` 只判
    /// 「活跃 + 可绑」），所以这个过滤必须客户端做。`composite` 分组一把 Key 跨多平台，
    /// 与「一分组一 provider」的展开模型不对齐，由本判定一并排除。
    pub fn is_codex_usable(&self) -> bool {
        self.platform == "openai" && self.status == "active"
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
    pub fn is_usable(&self) -> bool {
        self.status == "active"
    }
}

/// 余额（`GET /api/v1/user/profile`）的窄子集。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Balance {
    #[serde(default)]
    pub balance: f64,
    #[serde(default)]
    pub frozen_balance: f64,
}

/// 把用户输入的域名归一成面板 origin（`https://host`，无尾斜杠、无路径）。
///
/// 用户可能输入 `bestapi.store` / `https://bestapi.store/` / `http://bestapi.store/login`，
/// 全部归一到 `https://bestapi.store`。**一律升到 https**：sub2api 站点都跑 TLS，而登录
/// 页要过 WebView，明文 http 会被拦。
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

/// 由面板 origin 与后台声明的 `api_base_url` 算出 codex `base_url`（必须以 `/v1` 结尾）。
///
/// 为什么不能直接用 `api_base_url`：它可能是空串（bestapi.store 实测），而 sub2api 前端
/// 生成 codex 配置时**偏偏不对它做补 `/v1` 的处理**（grok / gemini 分支都做了），等于把
/// 责任推给后台配置。所以这里自己兜：空则回落面板 origin，然后统一补 `/v1`。
pub fn codex_base_url(site_origin: &str, api_base_url: &str) -> String {
    let base = api_base_url.trim();
    let root = if base.is_empty() { site_origin } else { base };
    let root = root.trim_end_matches('/');
    if root.ends_with("/v1") {
        root.to_string()
    } else {
        format!("{root}/v1")
    }
}

/// 未鉴权探测：这个域名是不是一个 sub2api 站。
///
/// `GET /api/v1/settings/public` 只挂公开 IP 限流，无 JWT、无 backend-mode 守卫，所以
/// 探测不需要任何凭据。
pub async fn probe_site(site_origin: &str) -> Result<PublicSettings, AppError> {
    let url = format!("{site_origin}/api/v1/settings/public");
    let client = build_client()?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Config(format!("连不上 {site_origin}: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::Config(format!(
            "{site_origin} 返回 HTTP {}，可能不是 sub2api 站点",
            resp.status().as_u16()
        )));
    }
    let env: Envelope<PublicSettings> = resp
        .json()
        .await
        .map_err(|e| AppError::Config(format!("{site_origin} 的响应不是 sub2api 格式: {e}")))?;
    let settings = env.into_data("探测站点")?;

    // 指纹：version 由服务端注入，任何 sub2api 都有；site_name 可能被运营商留空，不作硬判据。
    if settings.version.is_empty() {
        return Err(AppError::Config(format!(
            "{site_origin} 看起来不是 sub2api 站点（响应缺 version）"
        )));
    }
    Ok(settings)
}

/// 带 Bearer token 的 sub2api 客户端。
///
/// **User-Agent 必须与登录 WebView 一致**：sub2api 有可选的会话绑定
/// （`session_binding_enabled`，默认关），开启后 access token 里带
/// `SHA256(clientIP + "\n" + UA)[:16]`，每个请求重算比对，不符即**撤销整个会话家族**
/// 并返 401 `SESSION_BINDING_MISMATCH`。UA 不一致的后果不是单次失败，是连网页登录态
/// 一起被踢掉。
pub struct Client {
    http: reqwest::Client,
    site_origin: String,
    token: String,
}

impl Client {
    pub fn new(site_origin: impl Into<String>, token: impl Into<String>) -> Result<Self, AppError> {
        Ok(Self {
            http: build_client()?,
            site_origin: site_origin.into(),
            token: token.into(),
        })
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
        let resp = req
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| AppError::Config(format!("{what}失败: {e}")))?;

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

    /// 建一把新 Key。
    ///
    /// **一律带 `Idempotency-Key`**：建 Key 走服务端的幂等协调器，当前
    /// `idempotency.observe_only` 默认 true（即不强制），但运营商可以关掉它，届时不带头
    /// 就是 400。现在带上的成本是零，将来不会静默变 400。值用 name 的哈希，重试时天然复用
    /// 同一个 key。
    pub async fn create_key(&self, name: &str, group_id: i64) -> Result<ApiKey, AppError> {
        let body = serde_json::json!({ "name": name, "group_id": group_id });
        let req = self
            .http
            .post(self.url("/keys"))
            .header("Idempotency-Key", idempotency_key_for(name))
            .json(&body);
        self.send(req, "创建密钥").await
    }

    /// 余额。
    pub async fn balance(&self) -> Result<Balance, AppError> {
        self.send(self.http.get(self.url("/user/profile")), "获取余额")
            .await
    }
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
        .map_err(|e| AppError::Config(format!("续期失败: {e}")))?;

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
            "{what}失败: 登录态已失效（{code}），请重新登录运营商账号"
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

/// `Idempotency-Key` 的取值：name 的 SHA-256 十六进制。
///
/// 服务端要求 ≤128 字节且全 ASCII，64 个 hex 字符正好在范围内。
fn idempotency_key_for(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    format!("{:x}", h.finalize())
}

fn build_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(crate::operator::login::WEBVIEW_USER_AGENT)
        .build()
        .map_err(|e| AppError::Config(format!("创建 HTTP 客户端失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn group_usable_only_for_active_openai() {
        let mk = |platform: &str, status: &str| Group {
            id: 1,
            name: "t".into(),
            platform: platform.into(),
            rate_multiplier: 1.0,
            status: status.into(),
        };
        assert!(mk("openai", "active").is_codex_usable());
        // composite 一把 Key 跨多平台，与「一分组一 provider」不对齐，必须排除。
        assert!(!mk("composite", "active").is_codex_usable());
        assert!(!mk("anthropic", "active").is_codex_usable());
        assert!(!mk("openai", "disabled").is_codex_usable());
    }

    #[test]
    fn envelope_rejects_non_success_code() {
        let env: Envelope<Group> =
            serde_json::from_str(r#"{"code":"RATE_LIMITED","message":"too many","data":null}"#)
                .unwrap();
        let err = env.into_data("测试").unwrap_err().to_string();
        assert!(err.contains("too many"), "{err}");
        assert!(err.contains("RATE_LIMITED"), "{err}");
    }

    #[test]
    fn envelope_rejects_success_without_data() {
        // 服务端说成功却没给 data，属契约破裂，必须报错而不是当成空结果。
        let env: Envelope<Vec<Group>> =
            serde_json::from_str(r#"{"code":"success","message":"","data":null}"#).unwrap();
        assert!(env.into_data("测试").is_err());
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
            r#"{"code":"success","data":{"access_token":"a2","refresh_token":"r2","expires_at":1800000000000}}"#,
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
            serde_json::from_str(r#"{"code":"success","data":{"access_token":"a2"}}"#).unwrap();
        let d = not_rotated.into_data("测试").unwrap();
        assert_eq!(d.access_token.as_deref(), Some("a2"));
        assert!(d.refresh_token.is_none());
    }

    #[test]
    fn idempotency_key_is_ascii_hex_within_128_bytes() {
        let k = idempotency_key_for("LoongPort/dev-1/42");
        assert_eq!(k.len(), 64);
        assert!(k.is_ascii());
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        // 同名必须得到同一个 key，否则重试就不是幂等重试了。
        assert_eq!(k, idempotency_key_for("LoongPort/dev-1/42"));
        assert_ne!(k, idempotency_key_for("LoongPort/dev-1/43"));
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
}
