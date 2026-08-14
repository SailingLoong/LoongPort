//! 远端配置：赞助中转站列表 + 注册邀请码，**Ed25519 签名，三层回落**。
//!
//! ## 为什么这份配置必须能远端更新（而邀请码曾被判为不必）
//!
//! 邀请码几乎不变，所以它单独存在时，「编译期常量」是对的
//! —— 远端拉取多一个网络依赖、一个失败模式、一个信任边界，换来的只是「改码不用发版」。
//!
//! **赞助中转站列表把这个权衡翻了过来**：谈成一家新的赞助商，不可能等下一个版本才让
//! 用户看到 —— 那不是便利，是一个**必须能远端更新的产品能力**。而一旦为它建了远端配置，
//! 邀请码顺路搭上去是**零成本**（同一个请求、同一个失败路径、同一份缓存）。
//! 这时把码留在二进制里反而是重复劳动。
//!
//! ⇒ 判据不是「远端拉取好不好」，而是「**这份数据的变更频率要不要求它能脱离发版**」。
//!
//! ## 三层回落：远端 > 本地缓存 > 编译期内置
//!
//! 「拉不到就当没有」对**新用户**无害，但对**已经在用的用户是回归** ——
//! 他今天有邀请码，我们的页面挂了他明天就没了。所以内置那份不能删，只能降级成兜底：
//!
//! | 层 | 何时用 | 存在的理由 |
//! |---|---|---|
//! | 远端 | 拉到且**验签通过** | 唯一能反映「今天的赞助商是谁」 |
//! | 本地缓存 | 拉不到 / 超时 / 验签不过 | 上一次成功的结果，比内置新 |
//! | 编译期内置（[`super::aff`]） | 前两层都没有 | 全新安装 + 首启就没网时仍有返利 |
//!
//! **任何一层失败都不是错误**，不弹 toast、不重试、不阻塞任何用户流程。
//!
//! ## ⚠️ 必须验签，这不是可选项
//!
//! 这份文件直接决定**用户被引到哪个站**、以及**邀请收益归谁**。它被换掉的后果是
//! 真实损失：改 `aff_code` 把维护者的返利转给攻击者，改赞助商 host 把用户引到钓鱼站。
//!
//! 只靠 HTTPS 不够 —— 域名过期被抢注、CDN 被投毒、托管方账号被盗，这些都绕过 TLS。
//! 所以：**维护者用私钥签，客户端硬编码公钥验，验不过就当拉不到**（走缓存/内置）。
//! 签名覆盖的是**原始字节**，不是解析后的结构 —— 那样连「解析歧义」都攻击不了。

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 远端配置的 URL。
///
/// 2026-08-03 接上真实端点（Cloudflare Pages 项目 `loongport-config`，
/// 源文件与签名脚本在本仓库的 `remote-config/`）。
///
/// ## ⚠️ 这个 URL 是不可逆的对外契约，别再改它
///
/// 已发出去的版本会**永远**打这个地址 —— 改它要发版，而且救不了老用户
/// （他们的二进制里烧着旧地址）。所以它归尺子1 而不是尺子2：
///
/// - **用独立子域**而不是官网的子路径：以后把后端从 Pages 换成 R2 / Worker
///   都只是改 DNS，客户端一个字节都不用动。
/// - **路径里带 `v1`**：留给将来 schema 的破坏性变更 —— 那时新客户端打 `/v2/`，
///   旧客户端继续吃 `/v1/`（那份要一直留着，不能删）。
/// - **有意不用 `bestapi.store` 的子域**：那会把配置与维护者自己的中转站绑在一起，
///   用户看到「工具去拉自家站的子域」会觉得这是在导流。
const CONFIG_URL: &str = "https://config.loongport.dev/v1/config.json";

/// 签名文件的 URL（与配置同目录、同名加 `.sig`）。
const SIGNATURE_URL: &str = "https://config.loongport.dev/v1/config.json.sig";

/// 占位标记。端点含它就说明还没配真实域名。
///
/// `.invalid` 是 RFC 2606 保留 TLD —— 占位期间万一判断失灵真发了请求，
/// 它也解析不到任何真实主机。**两个端点都配好之后这个常量仍然留着**：
/// [`is_endpoint_configured`] 是通用判据，而且它守着「别再退回占位」这条。
const UNCONFIGURED_MARKER: &str = ".invalid";

/// 验签用的 Ed25519 公钥（32 字节，hex）。
///
/// 2026-08-03 生成的真实密钥对的公钥。
///
/// ## ⚠️ 换这把公钥等于让所有老版本客户端失联
///
/// 它与 [`CONFIG_URL`] 一样是**不可逆**的：旧版本只认烧在自己二进制里的这把公钥。
/// 换了之后用新私钥签的配置，在老客户端那边**验不过** ⇒ 它们永久回落到缓存/内置那层。
///
/// ⇒ **私钥丢了或泄露是真正的麻烦事**（不是「重新生成一把就行」）。所以：
/// 私钥只存在于维护者本机（**绝不进仓库**），签名是离线做的。
///
/// ⚠️⚠️ **别以为「填错的公钥天然验不过任何东西」** —— 那是错的，我实测证伪过：
/// 低阶点（如全零、单位元编码）下某些 (消息, 签名) 组合会**验过**
/// （实测 `ring` 对 `(b"{ this is not json", [0u8; 64])` 返回 `Ok(())`）。
/// 所以 [`verify_with`] 开头有一道**显式**的低阶点拦截，不靠曲线的性质兜。
const PUBLIC_KEY_HEX: &str = "3e199ad0082b525fdf8edef5f7161270675e107fd81d31dbce1b71d83936a131";

/// Ed25519 的**低阶点编码**（cofactor 8 那批 + y = ±1）。
///
/// ## 为什么必须显式拒绝这些
///
/// ⭐ **这是 review 抓出的 P0，我原来的防护漏了它。**
///
/// 我原本只拦「公钥全零」，理由是实测发现全零会让某些 (消息,签名) 验过。
/// 但**全零只是这批点里的一个** —— reviewer 给的反例是**单位元**编码
/// `01 || 31×00`：它**非全零、正好 32 字节**，我那两道检查全都放过。
///
/// 实测确认（比 reviewer 说的更严重）：单位元公钥 + 全零签名
/// （`R = 01||31×00`, `S = 32×00`）对**任意正文**都验过 ——
/// `{}` / `{"sponsors":[]}` / 中文字节 三个都返回 `Ok(())`。
///
/// 后果：切生产时误填这个结构合法的 key ⇒ [`is_configured_with`] 为 true ⇒
/// 攻击者对任意配置附上那个固定签名即可完全控制邀请码与赞助商列表。
///
/// `ring` **不做 subgroup 检查**（它按 RFC 8032 的宽松验证方程算），
/// 所以这道防线必须我们自己建。
///
/// ⚠️ **黑名单是「列举已知坏值」，天生可能漏** —— 所以它只是第一道。
/// 真正的白名单是 [`verify_with`] 里那条 known-answer 自检（见那里的文档）。
const LOW_ORDER_PUBLIC_KEYS: &[&str] = &[
    // 全零（四阶点）—— 原来只拦这一个。
    "0000000000000000000000000000000000000000000000000000000000000000",
    // 单位元 y=1（reviewer 给的反例，实测对任意正文验过）。
    "0100000000000000000000000000000000000000000000000000000000000000",
    // 上面两个的 sign-bit 置位变体。
    "0000000000000000000000000000000000000000000000000000000000000080",
    "0100000000000000000000000000000000000000000000000000000000000080",
    // y = p-1 与 y = p（约简后同为 ±1 的编码）。
    "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    // 两个 8 阶点。
    "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
    "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
];

/// Ed25519 签名的字节数（定值，不是上限）。
const ED25519_SIGNATURE_LEN: usize = 64;

/// Ed25519 公钥的字节数。填错长度的公钥会表现成「验签永远失败」，
/// 与「服务器挂了」症状相同 —— 所以显式校验，见 [`verify_with`]。
const ED25519_PUBLIC_KEY_LEN: usize = 32;

/// 拉取超时。**短** —— 它在启动路径附近，卡住毫无价值，拉不到有两层兜底。
const FETCH_TIMEOUT_SECS: u64 = 8;

/// 配置体积上限（拉取时截断判断）。
///
/// 防的是「端点被换成一个无限流」把内存吃光。1 MiB 对一份站点清单来说宽裕得离谱，
/// 真超了说明拿到的不是我们的配置。
///
/// ⚠️ **网络与缓存两条路都要判，且必须在「读进内存之前/之中」判** ——
/// review 抓出过：原来先 `.bytes().await` 再看长度，上限形同虚设。
/// 网络侧现在流式累计（见 [`fetch_bytes`]），缓存侧先看文件元数据（见 [`load_cached`]）。
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

/// 一个赞助中转站。
///
/// 字段名进了**签名覆盖的契约**（发布的 JSON 是 snake_case），改名意味着
/// 旧版本客户端解不出新配置 —— 所以不要动它们。
///
/// ## ⚠️ 两个方向的命名有意不同
///
/// - **`Deserialize`（读配置）用 snake_case** —— 那是签名覆盖的对外契约，动不了。
/// - **`Serialize`（发给前端）用 camelCase** —— 那是本仓 TS 侧的惯例
///   （`commands/relay.rs` 里 5 处 DTO 都是）。
///
/// 分方向 rename 是安全的，因为**缓存存的是原始字节**（`write_cache`），
/// 不经过 `Serialize` ⇒ 它唯一的消费者就是 [`super::super::commands::relay`]
/// 那个 `relay_list_sponsors` 命令。改这里不会影响验签或缓存。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Sponsor {
    /// 站点 origin（如 `https://bestapi.store`）。
    pub site_origin: String,
    /// 展示名。**服务端给什么就显示什么** —— 我们不翻译、不美化。
    pub display_name: String,
    /// 一句话介绍（可空）。UI 那轮决定显不显示。
    #[serde(default)]
    pub tagline: String,
}

/// 一个 vendor/app 组合的默认档位配置。
///
/// `tier_configs` 的 key 是稳定的 `<vendor>/<app_type>`（例如
/// `deepseek/claude`）。未知 key 会被忽略，新增 vendor 不需要改客户端 schema。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RemoteTierConfig {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub claude_roles: Option<RemoteClaudeRoleModels>,
}

/// Claude 角色模型默认值。字段全部可选，允许远端只覆盖部分角色；调用方负责回落内置值。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RemoteClaudeRoleModels {
    pub opus: Option<String>,
    pub fable: Option<String>,
    pub sonnet: Option<String>,
    pub haiku: Option<String>,
    pub subagent: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct RelayDirectorySite {
    #[serde(default)]
    pub veridrop_host: Option<String>,
    #[serde(default)]
    pub entry_url: Option<String>,
    #[serde(default)]
    pub purchase_url: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct RelayDirectoryPolicy {
    #[serde(default)]
    pub blocked_hosts: Vec<String>,
    #[serde(default)]
    pub sites: std::collections::BTreeMap<String, RelayDirectorySite>,
}

/// 远端配置的全文。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct RemoteConfig {
    /// 赞助中转站，按维护者给的顺序（**不排序** —— 顺序是他的编排意图）。
    #[serde(default)]
    pub sponsors: Vec<Sponsor>,
    /// 站点 host → 邀请码。key 必须是**归一后的 host**（与 [`super::aff`] 同一套）。
    #[serde(default)]
    pub aff_codes: std::collections::BTreeMap<String, String>,
    /// 站点 host → 注册优惠码。key 同上（[`super::aff::lookup_host`] 归一后的 host）。
    ///
    /// ## 为什么与 `aff_codes` 分成两个 map，而不是一个 map 装两个码
    ///
    /// 它们是服务端**两个不同的字段**（`aff_code` / `promo_code`，见
    /// [`super::promo`] 的对照表），取值域不同、生效条件不同、一个站可以只有其中一个。
    /// 合成一个 map 就得给值加结构（`{"aff":…,"promo":…}`）—— 那会让**已发出去的
    /// 客户端读不懂新格式**（`aff_codes` 的值现在是裸字符串）。
    ///
    /// ## 两个兼容方向靠的是**不同的机制**，别混为一谈
    ///
    /// | 方向 | 谁保证的 |
    /// |---|---|
    /// | **老客户端读新配置**（它的结构体没有 `promo_codes`） | serde **默认忽略未知字段** —— 与本注解无关 |
    /// | **新客户端读旧配置**（线上那份现在还没有这个键） | **本行的 `#[serde(default)]`** |
    ///
    /// 第二个方向才是这个注解在管的事，而且它**现在就生效** ——
    /// 线上那份配置要等这版发出去才会加 `promo_codes`。忘了这个注解的后果不是
    /// 「优惠码拿不到」，而是**整份配置解不出** ⇒ 连赞助商列表与 aff 码一起
    /// 回落到内置 ⇒ 首启屏卡片全空。
    ///
    /// ⇒ 结论仍是「新增字段而不改老字段的形状」，只是两个方向各有各的靠山。
    #[serde(default)]
    pub promo_codes: std::collections::BTreeMap<String, String>,
    /// vendor/app → 默认 base URL、主模型和（可选）Claude 角色模型。
    ///
    /// `#[serde(default)]` 保证旧线上配置与新客户端双向兼容；未知 vendor/app key
    /// 由调用方忽略，远端配置只覆盖它明确提供且通过基本校验的值。
    #[serde(default)]
    pub tier_configs: std::collections::BTreeMap<String, RemoteTierConfig>,
    /// 中转站广场兼容策略。评分与排名不在这里，始终由 VeriDrop 提供。
    #[serde(default)]
    pub relay_directory: RelayDirectoryPolicy,
}

/// 端点与公钥都配好了没。任一没配就整条链路 no-op（走缓存/内置）。
///
/// **只有参数化这一个版本，没有零参数的 wrapper** ——
/// 生产路径由 [`refresh_and_cache`] 传生产常量进来，测试传自己的值。
/// 曾经那个 `is_configured()` 在生产路径改走 [`refresh_and_cache_with`] 之后
/// 就只剩测试在用了（clippy 的 `dead_code` 当场抓出），删掉比留一个
/// `#[allow(dead_code)]` 诚实。
///
/// ⚠️ **三项缺一不可**，且必须参数化才测得出来：用固定常量测的话，
/// 少判一项时其余两项仍为坏值会让整体照样 false，那种退化测不出来。
/// review mutation 抓出过这个盲区。
fn is_configured_with(config_url: &str, signature_url: &str, public_key_hex: &str) -> bool {
    is_endpoint_configured(config_url)
        && is_endpoint_configured(signature_url)
        && is_key_usable(public_key_hex)
}

/// 这个 URL 配成真实端点了没。
fn is_endpoint_configured(url: &str) -> bool {
    !url.contains(UNCONFIGURED_MARKER)
}

/// 这把公钥能用吗。
///
/// ⚠️ **与 [`verify_with`] 用同一个判据**（低阶点黑名单），不再各写一份 ——
/// review 指出原来这里只判「全不全零」，比 `verify_with` 弱：
/// 一把低阶但非全零的公钥会让 [`is_configured_with`] 返回 true ⇒ 整条拉取链路上线，
/// 而那把公钥下任意正文都能伪造签名。**两个调用点必须用同一条规则。**
fn is_key_usable(public_key_hex: &str) -> bool {
    let normalized = public_key_hex.trim().to_ascii_lowercase();
    !LOW_ORDER_PUBLIC_KEYS.contains(&normalized.as_str())
        && decode_hex(&normalized).is_some_and(|b| b.len() == ED25519_PUBLIC_KEY_LEN)
}

/// 用指定公钥验签。`body` 是配置的**原始字节**，`signature` 是那 64 字节裸签名。
///
/// ## 为什么签原始字节而不是解析后的结构
///
/// 签结构就要先解析再验 —— 那意味着**攻击者的输入已经过了我们的解析器**，
/// 而且「同一个结构的不同字节表示」（键顺序、空白、重复键）都成了可乘之机。
/// 签字节则是：验不过连解析都不做。
///
/// ## 公钥为什么是参数
///
/// 生产路径用的始终是 [`PUBLIC_KEY_HEX`]：[`load_cached`] 直接传它，
/// [`refresh_and_cache`] 经 [`refresh_and_cache_with`] 传它。
///
/// 参数化有两个理由：让「验签与解析的先后顺序」能用一把**形状合法的假公钥**测出来
/// （见 `a_bad_signature_is_rejected_before_parsing`），以及让**正路**能用现场生成的
/// 密钥对测出来（见 `a_correctly_signed_config_actually_verifies_and_parses` ——
/// 那条不可或缺，其余用例全在测「被拒」）。
fn verify_with(public_key_hex: &str, body: &[u8], signature: &[u8]) -> Result<(), AppError> {
    // ⚠️ **低阶公钥必须直接拒**，不能交给 ring 去判 —— 它不做 subgroup 检查。
    //
    // 这批点下存在「对任意正文都验过」的固定签名（见 [`LOW_ORDER_PUBLIC_KEYS`]
    // 的文档，含实测证据）。占位全零是其中之一，但**远不止它**：
    // 单位元编码 `01||31×00` 非全零、长度也对，只拦全零会放过它。
    //
    // 比较用小写归一 —— 公钥是人手粘贴的，大小写不该影响安全判断。
    let normalized = public_key_hex.trim().to_ascii_lowercase();
    if LOW_ORDER_PUBLIC_KEYS.contains(&normalized.as_str()) {
        return Err(AppError::Config(
            "配置公钥是 Ed25519 低阶点，拒绝校验（这类公钥下任意正文都能伪造签名）".into(),
        ));
    }

    let key_bytes =
        decode_hex(&normalized).ok_or_else(|| AppError::Config("配置公钥不是合法的 hex".into()))?;

    // 长度必须正好 32 字节。**这不是多余的严格** —— 少了这道检查，
    // 一个被截断或多粘了一个字符的公钥会「合法地」解出来、交给 ring，
    // 然后表现成「验签永远失败」= 与「服务器挂了」**完全一样的症状**。
    // 那会让填错公钥的人查很久。宁可在这里报一个说清楚的错。
    if key_bytes.len() != ED25519_PUBLIC_KEY_LEN {
        return Err(AppError::Config(format!(
            "配置公钥长度不对：要 {} 字节（{} 位 hex），实际 {} 字节",
            ED25519_PUBLIC_KEY_LEN,
            ED25519_PUBLIC_KEY_LEN * 2,
            key_bytes.len()
        )));
    }

    let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &key_bytes);
    key.verify(body, signature)
        // ⚠️ 错误信息**不带任何细节** —— 验签失败的原因对攻击者是有用信息，
        // 对我们只需要知道「不通过」。
        .map_err(|_| AppError::Config("配置签名校验失败".into()))
}

/// hex → bytes。只用在这一处（公钥），不引 crate。
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// 缓存文件（存**已验签通过的原文**，与 `settings.json` 同目录）。
///
/// 存原文而不是解析后的结构：那样下次读它时能**再验一次签**，
/// 而不是「相信一份自己写下的 JSON」—— 磁盘上的文件同样可以被改。
fn cache_path() -> std::path::PathBuf {
    cache_dir().join("remote-config-cache.json")
}

/// 缓存所在目录。
///
/// **测试可覆盖**（`#[cfg(test)]` 那条分支）—— 否则 `load_cached` 那条最重要的
/// 文档声明（「必须重新验签」）根本没法测：路径硬编码时测试碰不到那两个文件。
/// review mutation 验出过：把 `load_cached` 里的验签换成裸 `from_slice`，
/// 17 个测试**全绿** —— 那是零覆盖。
fn cache_dir() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(dir) = tests::cache_dir_override() {
        return dir;
    }
    crate::config::get_home_dir().join(crate::config::APP_DIR_NAME)
}

/// 签名缓存。与配置分开存，**两者都在**才算一份可用的缓存。
fn cache_sig_path() -> std::path::PathBuf {
    cache_dir().join("remote-config-cache.sig")
}

/// 读一个文件，**先看元数据、超限直接放弃**。
///
/// 存在的理由见 [`load_cached`]：缓存文件是磁盘上的东西，可以被换成任意大小，
/// 而验签发生在读之后 —— 防不了「读的时候就 OOM 了」。
fn read_capped(path: &std::path::Path, max: usize) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > max as u64 {
        log::warn!(
            "缓存文件 {} 体积异常（{} 字节 > 上限 {max}），忽略它",
            path.display(),
            meta.len()
        );
        return None;
    }
    std::fs::read(path).ok()
}

/// 落盘这次拉到的原文与签名。失败只记 log —— 缓存写不进去只是下次少一层兜底。
fn write_cache(body: &[u8], signature: &[u8]) {
    let (p, sp) = (cache_path(), cache_sig_path());
    if let Some(dir) = p.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::debug!("远端配置缓存目录建不了（跳过缓存）: {e}");
            return;
        }
    }
    if let Err(e) = std::fs::write(&p, body) {
        log::debug!("远端配置缓存写不进去（跳过）: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&sp, signature) {
        // 签名没写成 ⇒ 下次读缓存会因缺签名而拒（正确行为），
        // 但那份无签名的配置留在磁盘上是垃圾，删掉它。
        log::debug!("远端配置签名缓存写不进去，清掉配置缓存: {e}");
        let _ = std::fs::remove_file(&p);
    }
}

/// 读上次成功拉取的缓存，**并重新验签**。
///
/// ⚠️ 必须重新验签，不能因为「这是我们自己写的文件」就信它 ——
/// 磁盘上的文件同样可被改（另一个进程、同机器的其它用户）。验不过就当没缓存。
pub fn load_cached() -> Option<RemoteConfig> {
    load_cached_with(PUBLIC_KEY_HEX)
}

/// 参数化版本。**存在只为了让「写入 → 读回」这条往返可测** ——
/// 与 [`is_configured_with`] / [`verify_with`] / [`refresh_and_cache_with`] 同一个理由。
///
/// review mutation 抓出过这个盲区：把 [`write_cache`] 改成无条件 no-op、
/// 或在这里把 [`cache_path`] 与 [`cache_sig_path`] 互换，**169 条测试全绿** ——
/// 因为所有缓存用例都只断言「被拒」。而「缓存根本不工作」的线上症状同样静默：
/// 没网的用户失去第二层兜底、悄悄掉回编译期内置表。
///
/// 用生产公钥测不了这条：我们没有对应私钥，写不出一份能验过的缓存。
fn load_cached_with(public_key_hex: &str) -> Option<RemoteConfig> {
    // ⚠️ **先看文件大小再读**（review 抓出：缓存被换成数 GB 的文件时，
    // `std::fs::read` 会在验签开始之前就把进程撑爆 —— 验签防不了 OOM）。
    let body = read_capped(&cache_path(), MAX_CONFIG_BYTES)?;
    // 签名**恰好** 64 字节。它不是「上限」而是定值，所以判等而不是判小于 ——
    // 不符合就说明那不是我们写的签名文件。
    let signature = read_capped(&cache_sig_path(), ED25519_SIGNATURE_LEN)
        .filter(|s| s.len() == ED25519_SIGNATURE_LEN)?;
    match parse_verified(public_key_hex, &body, Some(&signature)) {
        Ok(config) => Some(config),
        Err(e) => {
            log::debug!("远端配置缓存验签不过，忽略它: {e}");
            None
        }
    }
}

/// 拉一次远端配置、验签、落盘缓存。**启动时调一次。**
///
/// 任何失败都返回 `None`（调用方回落到 [`load_cached`] / 内置）。
/// **绝不 propagate 错误** —— 这是后台动作，不该有任何用户可见的失败。
pub async fn refresh_and_cache() -> Option<RemoteConfig> {
    refresh_and_cache_with(CONFIG_URL, SIGNATURE_URL, PUBLIC_KEY_HEX).await
}

/// 参数化版本。**存在只为了让那道「未配置就早退」的守卫可测** ——
/// 与 [`is_configured_with`] / [`verify_with`] 同一个理由。
///
/// 端点配成真的之后（2026-08-03），用生产常量测不出「删掉早退会怎样」：
/// 那会让测试真去打生产端点（网络依赖 + 打自己的服务器）。
/// 传占位值进来则两条路径可分辨 —— 见
/// `refresh_makes_no_request_and_writes_no_cache_while_unconfigured`。
async fn refresh_and_cache_with(
    config_url: &str,
    signature_url: &str,
    public_key_hex: &str,
) -> Option<RemoteConfig> {
    if !is_configured_with(config_url, signature_url, public_key_hex) {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .ok()?;

    // 两个都必须拿到（`?` 而不是 `if let Ok`）—— 只有配置没有签名时**绝不能**
    // 「就先用着」，那正好是攻击者要的：删掉 .sig 文件即可绕过验签。
    let body = fetch_bytes(&client, config_url).await.ok()?;
    let signature = fetch_bytes(&client, signature_url).await.ok()?;

    match parse_verified(public_key_hex, &body, Some(&signature)) {
        Ok(config) => {
            write_cache(&body, &signature);
            Some(config)
        }
        Err(e) => {
            // 验签不过**绝不落盘** —— 那会把一份坏配置固化下来，
            // 之后每次启动都从缓存读到它。
            log::warn!("远端配置验签不过，已丢弃（不落盘）: {e}");
            None
        }
    }
}

/// 验签 + 解析。**把「拿到了什么」与「怎么判」分开**，好让后者可单测。
///
/// `signature` 是 `Option` 只为了**表达一个必须被拒绝的输入**：`None` 表示
/// 「配置拿到了但签名没拿到」。它必须报错而不是放行 —— 否则攻击者删掉 `.sig`
/// `refresh_and_cache` 那边用 `?` 保证不会真的传 `None`，
/// 而这里的显式拒绝让那条规则**有闸可守**（见 `a_missing_signature_is_rejected...`）。
fn parse_verified(
    public_key_hex: &str,
    body: &[u8],
    signature: Option<&[u8]>,
) -> Result<RemoteConfig, AppError> {
    let Some(signature) = signature else {
        return Err(AppError::Config(
            "配置缺少签名，拒绝使用（删掉签名文件不该能绕过校验）".into(),
        ));
    };
    verify_with(public_key_hex, body, signature)?;

    // 验签通过之后**才**解析 —— 顺序反了等于让攻击者的输入先过我们的解析器。
    serde_json::from_slice(body).map_err(|e| AppError::Config(format!("配置格式不对: {e}")))
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, AppError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::Config(format!("拉取配置失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Config(format!("拉取配置被拒: {}", resp.status())));
    }
    // ⚠️ **必须流式读 + 边读边判**，不能 `.bytes().await` 之后再看长度
    // （review 抓出：那样 500 MiB 的响应会**先整个进内存**，上限形同虚设；
    // 而且压缩响应可以用很小的传输体积膨胀成巨大正文）。
    //
    // 也**不能只信 `Content-Length`** —— 那是服务端说的，可以撒谎或干脆不给。
    // 它只能用来「快速拒绝」，不能替代累计上限。
    if let Some(len) = resp.content_length() {
        if len > MAX_CONFIG_BYTES as u64 {
            return Err(AppError::Config(format!(
                "配置声明的体积就超限（{len} 字节），直接拒绝"
            )));
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| AppError::Config(format!("读取配置失败: {e}")))?
    {
        buf.extend_from_slice(&chunk);
        // 超一个字节就停 —— 不再往下读，也不再分配。
        if buf.len() > MAX_CONFIG_BYTES {
            return Err(AppError::Config(format!(
                "配置体积超过 {MAX_CONFIG_BYTES} 字节上限，已中止读取"
            )));
        }
    }
    Ok(buf)
}

/// 读取签名目录为站点配置的购买入口。
///
/// 购买地址会直接承载用户的付款动作，因此只接受 HTTPS 且与站点 origin 完全同源的
/// 已签名值。目录没有该站或该字段为空时返回 `Ok(None)`；配置值不安全时返回错误，
/// 不猜测 `/purchase`、`/wallet` 或其它路径。
pub fn configured_purchase_url(
    config: &RemoteConfig,
    site_origin: &str,
) -> Result<Option<url::Url>, AppError> {
    let normalized_origin = super::api::normalize_site_origin(site_origin)?;
    let host = super::aff::lookup_host(&normalized_origin);
    let Some(configured) = config
        .relay_directory
        .sites
        .get(&host)
        .and_then(|site| site.purchase_url.as_deref())
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Ok(None);
    };

    let purchase_url = url::Url::parse(configured)
        .map_err(|e| AppError::Config(format!("购买入口地址不合法: {e}")))?;
    if purchase_url.scheme() != "https" {
        return Err(AppError::Config("购买入口必须使用 HTTPS".into()));
    }

    let relay_origin = url::Url::parse(&normalized_origin)
        .map_err(|e| AppError::Config(format!("归一化后的站点地址不合法: {e}")))?;
    let purchase_origin = (
        purchase_url.scheme(),
        purchase_url.host_str(),
        purchase_url.port_or_known_default(),
    );
    let expected_origin = (
        relay_origin.scheme(),
        relay_origin.host_str(),
        relay_origin.port_or_known_default(),
    );
    if purchase_origin != expected_origin {
        return Err(AppError::Config("购买入口必须与中转站同源".into()));
    }

    Ok(Some(purchase_url))
}

/// 按三层回落查一个站的邀请码：远端（已缓存的） > 编译期内置。
///
/// `cached` 传上一次成功拉取并缓存下来的配置（`None` = 没有缓存）。
///
/// ⚠️ **内置那层不能删** —— 见模块文档那张表：删了它，全新安装 + 首启没网的用户
/// 那次注册就拿不到返利。
pub fn resolve_aff_code(cached: Option<&RemoteConfig>, site_origin: &str) -> Option<String> {
    resolve_code(
        cached.map(|c| &c.aff_codes),
        site_origin,
        super::aff::aff_code_for,
    )
}

/// 按三层回落查一个站的**注册优惠码**：远端（已缓存的） > 编译期内置。
///
/// 与 [`resolve_aff_code`] 同一套语义（含「远端给空串 = 撤销，不回落」那条），
/// 只是查另一个 map 与另一张内置表 —— 两者共用 [`resolve_code`]。
pub fn resolve_promo_code(cached: Option<&RemoteConfig>, site_origin: &str) -> Option<String> {
    resolve_code(
        cached.map(|c| &c.promo_codes),
        site_origin,
        super::promo::promo_code_for,
    )
}

/// 「远端（含缓存） > 编译期内置」这条回落链，aff 与 promo 共用。
///
/// ## 为什么提成共用函数（而两张码表本身有意不合并）
///
/// 合并的是**规则**，不是**数据**。这里面唯一微妙的一条是
/// 「**远端给空串 = 维护者撤掉了这个码 ⇒ 不回落到内置**」——
/// 它不是显而易见的，各写一份的话迟早有一份漏掉，
/// 而漏掉的后果是**撤销静默失效**（维护者以为撤了，老客户端还在用内置那个）。
///
/// 数据仍分两张表（`aff_codes` / `promo_codes`）：那是服务端两个不同字段，见
/// [`RemoteConfig::promo_codes`] 的文档。
fn resolve_code(
    remote: Option<&std::collections::BTreeMap<String, String>>,
    site_origin: &str,
    builtin: fn(&str) -> Option<&'static str>,
) -> Option<String> {
    let host = super::aff::lookup_host(site_origin);

    // 第一层：远端（含缓存）。命中就用它 —— 它比内置新。
    if let Some(code) = remote.and_then(|m| m.get(&host)) {
        let trimmed = code.trim();
        // 空值当作「远端明确说这个站没有码」⇒ **不回落到内置**。
        // 那是维护者撤掉一个码的唯一手段，回落会让撤销失效。
        return if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    // 第二层：编译期内置。
    builtin(site_origin).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    /// 测试用的缓存目录覆盖。见 [`super::cache_dir`] 的文档。
    ///
    /// 用 `Mutex<Option<..>>` 而不是 `thread_local`：cargo 的测试是多线程跑的，
    /// 但需要覆盖的用例会自己拿 [`cache_lock`] 串行化。
    static CACHE_OVERRIDE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

    /// 碰缓存的用例必须互斥（它们共享那个全局 override）。
    fn cache_lock() -> &'static Mutex<()> {
        static L: Mutex<()> = Mutex::new(());
        &L
    }

    pub(super) fn cache_dir_override() -> Option<std::path::PathBuf> {
        CACHE_OVERRIDE.lock().ok()?.clone()
    }

    /// 把缓存目录指到一个临时目录，返回时自动复原。
    struct CacheDirGuard {
        dir: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl CacheDirGuard {
        fn new(tag: &str) -> Self {
            let lock = cache_lock().lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("lp-cache-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("mkdir");
            *CACHE_OVERRIDE.lock().unwrap() = Some(dir.clone());
            Self { dir, _lock: lock }
        }
    }

    impl Drop for CacheDirGuard {
        fn drop(&mut self) {
            *CACHE_OVERRIDE.lock().unwrap() = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// ⭐ **缓存的「写入 → 读回」往返** —— 这是第二层兜底唯一走通正路的用例。
    ///
    /// review mutation 抓出的零覆盖（比下面那条更彻底）：把 [`write_cache`] 改成
    /// 无条件 no-op、或在 [`load_cached_with`] 里把 [`cache_path`] 与
    /// [`cache_sig_path`] 互换，**169 条测试全绿** —— 因为所有缓存用例都只断言
    /// 「被拒」。「缓存根本不工作」于是完全不可见。
    ///
    /// 它的线上症状同样静默：拉不到配置的用户（没网 / 端点挂了）本该用上次的缓存，
    /// 实际悄悄掉回编译期内置表 —— 于是**新谈的赞助商和撤销过的码全部回退**，
    /// 而没有任何错误。
    #[test]
    fn a_freshly_written_cache_reads_back_through_the_verify_path() {
        let _g = CacheDirGuard::new("roundtrip");
        let (public_key_hex, pair) = generate_test_keypair();

        let body = br#"{"sponsors":[{"site_origin":"https://x.com","display_name":"X"}],"aff_codes":{"x.com":"ROUNDTRIP1"}}"#;
        let signature = pair.sign(body);

        write_cache(body, signature.as_ref());

        // 两个文件都得写出来 —— 只写一个的话读回时会因缺签名而拒（正确但不是这里要的）。
        assert!(cache_path().exists(), "配置缓存没写出来");
        assert!(cache_sig_path().exists(), "签名缓存没写出来");

        let cfg = load_cached_with(&public_key_hex)
            .expect("刚写下的缓存必须读得回来 —— 红了说明 write_cache 或 load_cached 坏了");

        assert_eq!(cfg.sponsors.len(), 1);
        assert_eq!(cfg.aff_codes.get("x.com").unwrap(), "ROUNDTRIP1");
    }

    /// ⭐ review mutation 抓出的**零覆盖**：`load_cached` 跳过验签时 17 个测试全绿。
    ///
    /// 那是本模块最强调的一条声明 ——「必须重新验签，不能因为『这是我们自己写的文件』
    /// 就信它」。而「验一份自己刚写的文件」看起来天经地义地多余，
    /// 所以那种"简化"极可能发生，且**没有任何东西会红**。
    #[test]
    fn a_tampered_cache_file_is_rejected_not_trusted_because_we_wrote_it() {
        let _g = CacheDirGuard::new("tamper");

        // 一份**合法 JSON**，但签名是错的 —— 模拟「别的进程改了缓存」。
        std::fs::write(
            cache_path(),
            br#"{"sponsors":[],"aff_codes":{"evil.com":"ATTACKER1234"}}"#,
        )
        .expect("write body");
        std::fs::write(cache_sig_path(), [0u8; ED25519_SIGNATURE_LEN]).expect("write sig");

        assert!(
            load_cached().is_none(),
            "签名对不上的缓存必须被拒 —— 磁盘上的文件同样可以被改"
        );
    }

    #[test]
    fn a_cache_without_its_signature_file_is_not_trusted() {
        let _g = CacheDirGuard::new("nosig");
        std::fs::write(cache_path(), b"{}").expect("write body");
        // 有意不写 .sig。
        assert!(
            load_cached().is_none(),
            "缺签名文件的缓存必须被拒（删掉 .sig 不该能绕过校验）"
        );
    }

    #[test]
    fn a_signature_file_of_the_wrong_length_is_rejected() {
        let _g = CacheDirGuard::new("shortsig");
        std::fs::write(cache_path(), b"{}").expect("write body");
        std::fs::write(cache_sig_path(), [0u8; 32]).expect("write short sig");
        assert!(
            load_cached().is_none(),
            "签名长度必须恰好 64 字节 —— 不符合说明那不是我们写的文件"
        );
    }

    /// 一把**合法形状、非占位**的公钥，只用于测「验签 vs 解析」的顺序。
    /// 它验不过任何真实签名（我们没有对应私钥），而那正是这些用例要的。
    const TEST_KEY_HEX: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    /// 现场生成一对密钥，返回 `(公钥 hex, 签名器)`。
    ///
    /// ## 为什么需要它：本模块所有其它用例都只测**负面**路径
    ///
    /// 它们断言的都是「被拒」（错签名 / 缺签名 / 长度不对 / 低阶点 / 超限）。
    /// 那留下一个**大缺口**：假如 [`verify_with`] 有 bug 导致它拒绝**一切**输入
    /// （包括合法签名），上面每一条断言照样绿 —— 而线上表现是
    /// 「配置永远拉不到」，静默回落到内置那层，**没有任何东西会报错**。
    ///
    /// 所以必须有一条走通正路的用例。用现场生成的密钥对而不是把真实私钥
    /// 放进测试（那绝不行）——验的是**机制**，与生产用哪把钥匙无关。
    fn generate_test_keypair() -> (String, ring::signature::Ed25519KeyPair) {
        use ring::signature::KeyPair;

        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("生成密钥对");
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("解析密钥对");
        let hex = pair
            .public_key()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        (hex, pair)
    }

    fn config_with_site(
        host: &str,
        entry_url: Option<&str>,
        purchase_url: Option<&str>,
    ) -> RemoteConfig {
        RemoteConfig {
            relay_directory: RelayDirectoryPolicy {
                blocked_hosts: vec![],
                sites: std::collections::BTreeMap::from([(
                    host.to_string(),
                    RelayDirectorySite {
                        veridrop_host: None,
                        entry_url: entry_url.map(str::to_string),
                        purchase_url: purchase_url.map(str::to_string),
                        display_name: None,
                    },
                )]),
            },
            ..RemoteConfig::default()
        }
    }

    #[test]
    fn purchase_url_requires_signed_https_same_origin() {
        let config = config_with_site(
            "api-top.com",
            Some("https://api-top.com/register"),
            Some("https://api-top.com/wallet"),
        );

        assert_eq!(
            configured_purchase_url(&config, "https://api-top.com")
                .unwrap()
                .unwrap()
                .as_str(),
            "https://api-top.com/wallet"
        );
        assert!(configured_purchase_url(&config, "https://unknown.example")
            .unwrap()
            .is_none());
    }

    #[test]
    fn purchase_url_rejects_http_and_cross_origin_entries() {
        for purchase_url in [
            "http://api-top.com/wallet",
            "https://payments.example/wallet",
        ] {
            let config = config_with_site(
                "api-top.com",
                Some("https://api-top.com/register"),
                Some(purchase_url),
            );
            assert!(configured_purchase_url(&config, "https://api-top.com").is_err());
        }
    }

    fn cfg_with(host: &str, code: &str) -> RemoteConfig {
        let mut aff_codes = std::collections::BTreeMap::new();
        aff_codes.insert(host.to_string(), code.to_string());
        RemoteConfig {
            sponsors: vec![],
            aff_codes,
            promo_codes: std::collections::BTreeMap::new(),
            tier_configs: std::collections::BTreeMap::new(),
            relay_directory: RelayDirectoryPolicy::default(),
        }
    }

    /// 只带优惠码的配置（aff 那份留空）。
    fn cfg_with_promo(host: &str, code: &str) -> RemoteConfig {
        let mut promo_codes = std::collections::BTreeMap::new();
        promo_codes.insert(host.to_string(), code.to_string());
        RemoteConfig {
            sponsors: vec![],
            aff_codes: std::collections::BTreeMap::new(),
            promo_codes,
            tier_configs: std::collections::BTreeMap::new(),
            relay_directory: RelayDirectoryPolicy::default(),
        }
    }

    #[test]
    fn old_config_without_tier_configs_remains_compatible() {
        let config: RemoteConfig = serde_json::from_str(
            r#"{"sponsors":[],"aff_codes":{"example.com":"CODE"},"promo_codes":{}}"#,
        )
        .expect("旧配置应能被新客户端读取");
        assert!(config.tier_configs.is_empty());
        assert_eq!(
            config.aff_codes.get("example.com"),
            Some(&"CODE".to_string())
        );
    }

    #[test]
    fn tier_config_contract_preserves_deepseek_defaults_and_partial_roles() {
        let config: RemoteConfig = serde_json::from_str(
            r#"{
                "tier_configs": {
                    "deepseek/claude": {
                        "base_url": "https://api.deepseek.com/anthropic",
                        "model": "deepseek-v4-pro",
                        "claude_roles": {"sonnet": "deepseek-v4-flash[1M]"}
                    }
                }
            }"#,
        )
        .expect("档位配置契约应能解析");
        let tier = config.tier_configs.get("deepseek/claude").unwrap();
        assert_eq!(tier.model, "deepseek-v4-pro");
        assert_eq!(
            tier.claude_roles.as_ref().unwrap().sonnet.as_deref(),
            Some("deepseek-v4-flash[1M]")
        );
        assert!(tier.claude_roles.as_ref().unwrap().opus.is_none());
    }

    #[test]
    fn endpoints_and_key_are_configured_for_production() {
        // 2026-08-03 端点与公钥都配成真的了（这条以前断言的是反面，是那时留的闸）。
        // 现在它守的是**别退回占位**，以及下面那三条「填错了会静默失效」的性质。
        assert!(
            is_configured_with(CONFIG_URL, SIGNATURE_URL, PUBLIC_KEY_HEX),
            "端点或公钥退回占位了？那整条拉取链路会 no-op（静默回落到内置表）"
        );

        for url in [CONFIG_URL, SIGNATURE_URL] {
            assert!(
                !url.contains(UNCONFIGURED_MARKER),
                "端点不该含占位标记: {url}"
            );
            // HTTPS 是硬要求：验签防篡改，但明文 HTTP 会泄漏「谁在用 LoongPort」。
            assert!(url.starts_with("https://"), "端点必须是 HTTPS: {url}");
        }

        // 签名文件必须是配置路径加 `.sig` —— 两者指向不同的东西时症状是
        // 「验签永远失败」，与「服务器挂了」完全一样，很难查。
        assert_eq!(
            SIGNATURE_URL,
            format!("{CONFIG_URL}.sig"),
            "签名 URL 必须是配置 URL 加 .sig"
        );

        // ⭐ 公钥填错（截断 / 多粘一个字符 / 误填低阶点）同样表现成「验签永远失败」。
        // `is_key_usable` 是 `is_configured_with` 用的那条判据，这里显式再钉一次意图。
        assert!(
            is_key_usable(PUBLIC_KEY_HEX),
            "PUBLIC_KEY_HEX 不可用：要么不是 32 字节 hex，要么是低阶点"
        );
    }

    /// 打**线上真实端点**，走生产路径（[`refresh_and_cache`]）验整条链路。
    /// **默认不跑**（`#[ignore]`）—— CI 不该依赖外网可达，
    /// 而这条要验的恰恰是「真的拉得到、线上那份真的验得过我们这把公钥」。
    ///
    /// 手动跑：`cargo test --lib live_remote_config -- --ignored --nocapture`
    ///
    /// ## 它守的是单测覆盖不到的那件事
    ///
    /// 上面那些用例用**现场生成的密钥对**验「机制」对不对，覆盖不了
    /// 「线上部署的那份文件，跟这里烧着的 [`PUBLIC_KEY_HEX`]，是不是配套的」。
    /// 后者失败时的表现最难查：客户端**静默丢弃**整份配置、回落到内置表、**不报错**，
    /// 看起来就是「改了配置没生效」。
    ///
    /// 常见触发原因：改了 `config.json` 忘跑 `sign.sh`、只发配置没发 `.sig`、
    /// 或换过密钥对但没同步这里的公钥。
    /// （本仓库 `remote-config/verify.sh` 从 shell 侧验同一件事，两者互为备份。）
    #[test]
    #[ignore = "需要外网；手动跑 --ignored"]
    fn live_remote_config_verifies_against_our_public_key() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建 runtime");

        let _g = CacheDirGuard::new("live");

        let cfg = rt.block_on(refresh_and_cache()).expect(
            "线上配置拉取或验签失败 —— 检查：\
             (1) config.json 改完是否重跑了 sign.sh；\
             (2) .sig 是否与配置一起发布；\
             (3) PUBLIC_KEY_HEX 是否与签名用的私钥配套",
        );

        // 至少得有内容 —— 一份全空的配置能验过，但那说明发布出了岔子。
        assert!(
            !cfg.sponsors.is_empty(),
            "线上配置没有赞助商？发布内容可能不对"
        );
        assert!(!cfg.aff_codes.is_empty(), "线上配置没有邀请码？同上");

        // 验签通过才该落盘缓存（模块文档：验不过绝不落盘）。
        assert!(
            cache_path().exists() && cache_sig_path().exists(),
            "拉取成功后应写下缓存（下次没网时的第二层兜底）"
        );

        // 顺带核对：远端那层确实盖住了内置表（同一个 host 两边都有）。
        //
        // ⚠️ **必须跳过空值** —— 空串是「维护者明确撤销这个站的码」，
        // `resolve_aff_code` 对它正确返回 `None`（那条语义由
        // `an_empty_remote_code_revokes_the_builtin_one` 钉着）。
        // review 抓出：原来这里对所有 key 一律断言 `is_some()` ⇒
        // **第一次真用撤销功能时，这条闸会对着一份完全正确的配置报红**。
        for (host, code) in &cfg.aff_codes {
            if code.trim().is_empty() {
                println!("  （{host} 的码已被维护者撤销，跳过）");
                continue;
            }
            let resolved = resolve_aff_code(Some(&cfg), &format!("https://{host}"));
            assert_eq!(
                resolved.as_deref(),
                Some(code.trim()),
                "线上有 {host} 的码却解不出来（或解出了别的值）"
            );
        }

        println!(
            "线上配置 OK：{} 家赞助商、{} 条邀请码",
            cfg.sponsors.len(),
            cfg.aff_codes.len()
        );
    }

    /// 把那个反直觉事实做成**可复现的证据**，而不是只留一句注释。
    ///
    /// 它断言的是 `ring` 的行为（不是我们的代码），所以标 `#[ignore]`：
    /// 平时不跑（那是上游的性质，我们没法也不该修），
    /// 但任何人怀疑「全零公钥真的会验过？」时可以
    /// `cargo test -- --ignored low_order` 亲眼看一次。
    ///
    /// ⚠️ 如果哪天 ring 收紧了低阶点检查、这条变红 —— **那是好事**，
    /// 但 `verify_with` 里那道显式拦截**仍然不该删**：安全不该依赖上游的实现细节。
    #[test]
    #[ignore = "断言的是 ring 的性质，不是我们的代码；怀疑那个结论时手动跑"]
    fn ring_really_does_accept_some_inputs_under_an_all_zero_key() {
        let zero_key = vec![0u8; 32];
        let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &zero_key);

        // 这个具体组合实测**验得过** —— 全零公钥是 Ed25519 的低阶点。
        assert!(
            key.verify(b"{ this is not json", &[0u8; 64]).is_ok(),
            "若这条红了：ring 收紧了低阶点检查。好事，但别删 verify_with 里那道拦截"
        );

        // 而我们自己的 verify_with 对**同一个输入**必须拒 —— 这才是重点。
        assert!(verify_with(PUBLIC_KEY_HEX, b"{ this is not json", &[0u8; 64]).is_err());
    }

    #[test]
    fn every_known_low_order_key_is_rejected_not_just_the_all_zero_one() {
        // ⭐ review 抓出的 P0 的闸：我原来只拦全零，而**单位元编码
        // `01||31×00` 非全零、长度也对** ⇒ 那两道检查全放过它。
        // 实测它 + 全零签名对**任意正文**都验过（`{}`、`{"sponsors":[]}`、中文都 Ok）。
        //
        // 逐个断言整批低阶点都被拒 —— 加新的进那个数组时这条自动覆盖它。
        for key in LOW_ORDER_PUBLIC_KEYS {
            let err = verify_with(key, b"{}", &[0u8; 64])
                .expect_err("低阶公钥必须被拒")
                .to_string();
            assert!(
                err.contains("低阶"),
                "{key} 的拒绝理由要说清是低阶点：{err}"
            );
        }

        // 那个具体反例单独再钉一次（它是 review 给的，最容易被「优化」掉）。
        let identity = "0100000000000000000000000000000000000000000000000000000000000000";
        let mut forged_sig = [0u8; 64];
        forged_sig[0] = 1;
        for body in [b"{}".as_slice(), br#"{"sponsors":[]}"#.as_slice()] {
            assert!(
                verify_with(identity, body, &forged_sig).is_err(),
                "单位元公钥 + 那个固定签名必须被拒 —— 它对任意正文都能骗过 ring"
            );
        }

        // 大小写与空白不该绕过（公钥是人手粘贴的）。
        assert!(verify_with(
            "  0100000000000000000000000000000000000000000000000000000000000000  ",
            b"{}",
            &[0u8; 64]
        )
        .is_err());
        assert!(verify_with(
            "0100000000000000000000000000000000000000000000000000000000000000"
                .to_ascii_uppercase()
                .as_str(),
            b"{}",
            &[0u8; 64]
        )
        .is_err());
    }

    #[test]
    fn an_unconfigured_public_key_is_rejected_explicitly_not_left_to_the_curve() {
        // ⭐ 这条挡的是一个**实测发现的反直觉事实**：
        // 全零公钥**不是**「验不过任何东西」—— 它是 Ed25519 的低阶点，
        // 某些 (消息, 签名) 组合在它下面会**验过**。
        // 实测：`ring` 对 `(b"{ this is not json", [0u8; 64])` 返回 `Ok(())`。
        //
        // 所以不能指望「忘了配公钥也天然安全」，必须在 `verify` 里显式拦。
        //
        // ⚠️ 这里传的是**字面量占位公钥**，不是 `PUBLIC_KEY_HEX`
        // （2026-08-03 那个常量已配成真公钥）。这条守的性质是
        // 「**万一有人把公钥退回占位**，必须显式因低阶点被拒」——
        // 用生产常量写就只在占位期间有效，而占位期已经过去了。
        const PLACEHOLDER_KEY: &str =
            "0000000000000000000000000000000000000000000000000000000000000000";

        for (body, label) in [
            (b"anything".as_slice(), "普通消息"),
            // 这个输入就是当初漏过去的那一个，钉住它。
            (b"{ this is not json".as_slice(), "低阶点会放过的那个消息"),
        ] {
            let err = verify_with(PLACEHOLDER_KEY, body, &[0u8; 64])
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("低阶"),
                "{label} 必须因「低阶点」被拒，而不是交给曲线去判：{err}"
            );
        }
    }

    #[test]
    fn a_wrong_length_public_key_is_a_loud_error_not_a_silent_verify_failure() {
        // 填错公钥（截断 / 多粘一个字符）若只表现成「验签失败」，
        // 症状与「服务器挂了 / 签名坏了」**一模一样** ⇒ 填错的人会查很久。
        // 所以要在这里就报清楚是长度问题。
        for bad in [
            // 少一个字节（62 位 hex —— **偶数长度**，所以过得了 hex 解码那关，
            // 正好落到长度检查上；奇数长度会先被判「不是合法 hex」，测不到这条）
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f70751",
            // 多一个字节
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a00",
        ] {
            let err = verify_with(bad, b"body", &[0u8; 64])
                .expect_err("长度不对的公钥必须被拒")
                .to_string();
            assert!(
                err.contains("长度"),
                "错误要说清是长度问题（否则与「验签失败」不可区分）：{err}"
            );
        }

        // 非 hex 字符也要报清楚（而不是当成 0）。
        let err = verify_with(&"z".repeat(64), b"body", &[0u8; 64])
            .expect_err("非 hex 必须被拒")
            .to_string();
        assert!(err.contains("hex"), "{err}");
    }

    #[test]
    fn remote_code_wins_over_the_builtin_one() {
        // 远端存在的全部意义：它比内置新。
        let cached = cfg_with("wawapii.com", "NEWCODE12345");
        assert_eq!(
            resolve_aff_code(Some(&cached), "https://wawapii.com").as_deref(),
            Some("NEWCODE12345")
        );
    }

    #[test]
    fn falls_back_to_the_builtin_code_when_there_is_no_cache() {
        // 全新安装 + 首启没网 ⇒ 仍然要有返利（这正是内置那层不能删的理由）。
        assert_eq!(
            resolve_aff_code(None, "https://wawapii.com").as_deref(),
            Some("4PAUD8SSZXG7"),
        );
    }

    #[test]
    fn falls_back_to_the_builtin_code_when_the_remote_lacks_that_host() {
        // 远端拉到了但没提这个站 ⇒ 用内置（不是"当作没有"）。
        let cached = cfg_with("someone-else.com", "OTHER1234567");
        assert_eq!(
            resolve_aff_code(Some(&cached), "https://wawapii.com").as_deref(),
            Some("4PAUD8SSZXG7"),
        );
    }

    #[test]
    fn an_empty_remote_code_revokes_the_builtin_one() {
        // ⭐ 这是维护者**撤掉一个码**的唯一手段（比如与那家终止合作）。
        // 回落到内置会让撤销失效 —— 那正是「远端可控」要避免的。
        let cached = cfg_with("wawapii.com", "");
        assert_eq!(resolve_aff_code(Some(&cached), "https://wawapii.com"), None);
        // 空白也算空（防维护者手滑录一个空格进去）。
        let spaces = cfg_with("wawapii.com", "   ");
        assert_eq!(resolve_aff_code(Some(&spaces), "https://wawapii.com"), None);
    }

    #[test]
    fn the_maintainers_own_site_stays_absent_through_both_layers() {
        // 自己邀请自己会被服务端拒。远端没提它、内置也没有 ⇒ 两层都该是 None。
        assert_eq!(resolve_aff_code(None, "https://bestapi.store"), None);
        let unrelated = cfg_with("wawapii.com", "X1234567890A");
        assert_eq!(
            resolve_aff_code(Some(&unrelated), "https://bestapi.store"),
            None
        );
    }

    #[test]
    fn the_promo_code_gets_the_same_three_layer_fallback() {
        // 远端赢过内置。
        let remote = cfg_with_promo("bestapi.store", "SUMMER2026");
        assert_eq!(
            resolve_promo_code(Some(&remote), "https://bestapi.store").as_deref(),
            Some("SUMMER2026"),
            "远端存在的全部意义：它比内置新"
        );
        // 没缓存 ⇒ 回落内置。
        assert_eq!(
            resolve_promo_code(None, "https://bestapi.store").as_deref(),
            Some("LOONGPORT"),
            "全新安装 + 首启没网仍要有赠额"
        );
        // 远端拉到了但没提这个站 ⇒ 用内置。
        let other = cfg_with_promo("someone-else.com", "THEIRS");
        assert_eq!(
            resolve_promo_code(Some(&other), "https://bestapi.store").as_deref(),
            Some("LOONGPORT")
        );
        // 归一同样生效（远端那层与内置那层共用 `lookup_host`）。
        assert_eq!(
            resolve_promo_code(Some(&remote), "https://WWW.BestApi.store:443").as_deref(),
            Some("SUMMER2026")
        );
    }

    /// ⭐ **远端给空串 = 撤销，不回落到内置** —— 优惠码这条也必须成立。
    ///
    /// 那是维护者停掉一个活动的唯一手段（活动结束了，码在服务端也删了）。
    /// 回落会让用户填一个已失效的码 ⇒ 注册页弹红框「优惠码无效」。
    ///
    /// 这条与 aff 那条共用 [`resolve_code`]，但**仍然各测一遍**：
    /// 共用是当下的实现，而这条语义属于两个功能各自的契约 ——
    /// 哪天有人把它们拆开重写，两边都该有闸拦住漏掉这条的那一版。
    #[test]
    fn an_empty_remote_promo_code_revokes_the_builtin_one() {
        for value in ["", "   "] {
            let cached = cfg_with_promo("bestapi.store", value);
            assert_eq!(
                resolve_promo_code(Some(&cached), "https://bestapi.store"),
                None,
                "远端给 {value:?} 时必须撤销内置那个，不能回落"
            );
        }
    }

    /// ⭐ **两个码互不干扰** —— 它们是服务端两个不同的字段，各有各的 map。
    ///
    /// 这条守的是「合成一个 map」那种"简化"：那会让一个站只能有其中一个码。
    #[test]
    fn the_two_code_kinds_resolve_independently() {
        // 只给 promo 的配置：aff 该回落到自己的内置表，不受影响。
        let promo_only = cfg_with_promo("wawapii.com", "PROMOX");
        assert_eq!(
            resolve_promo_code(Some(&promo_only), "https://wawapii.com").as_deref(),
            Some("PROMOX")
        );
        assert_eq!(
            resolve_aff_code(Some(&promo_only), "https://wawapii.com").as_deref(),
            Some("4PAUD8SSZXG7"),
            "aff 该走自己那条链，不该被 promo 的 map 影响"
        );

        // 反过来：只给 aff 的配置，promo 回落内置。
        let aff_only = cfg_with("bestapi.store", "AFFX12345678");
        assert_eq!(
            resolve_promo_code(Some(&aff_only), "https://bestapi.store").as_deref(),
            Some("LOONGPORT")
        );
    }

    #[test]
    fn host_lookup_is_normalized_in_the_remote_layer_too() {
        // 远端那层也必须走同一套归一，否则「表里明明有却查不到」会在两层里
        // 表现得不一样（更难查）。
        let cached = cfg_with("wawapii.com", "NEWCODE12345");
        for origin in [
            "https://WawaPii.com",
            "https://www.wawapii.com",
            "https://wawapii.com:8443",
        ] {
            assert_eq!(
                resolve_aff_code(Some(&cached), origin).as_deref(),
                Some("NEWCODE12345"),
                "{origin}"
            );
        }
    }

    #[test]
    fn config_parses_the_shape_the_maintainer_will_actually_publish() {
        // 用一份**真实形状**的 JSON 当 fixture（而不是逐字段构造结构体）——
        // 那样才验得到 serde 的字段名契约。
        // `r#".."#` 而不是 `br#".."#`：后者是 byte string，不能含非 ASCII，
        // 而站名本来就是中文 —— 这份 fixture 要长得像真实发布的那份。
        //
        // ⚠️ **必须带上 `issued_at`** —— 线上那份有它（见本仓库
        // `remote-config/public/v1/config.json`），而它是本结构体的**未知字段**。
        // fixture 不带它的话，「未知字段该被忽略」这条性质就是零覆盖。
        let raw = r#"{
          "issued_at": "2026-08-03T00:00:00Z",
          "sponsors": [
            {"site_origin": "https://bestapi.store", "display_name": "百适 BestApi", "tagline": "官方推荐"},
            {"site_origin": "https://wawapii.com", "display_name": "Wawa"}
          ],
          "aff_codes": {"wawapii.com": "NEWCODE12345"},
          "promo_codes": {"bestapi.store": "LOONGPORT"}
        }"#
        .as_bytes();
        // serde 默认按 Rust 字段名解 ⇒ **发布的 JSON 必须用 snake_case**。
        // 这条 fixture 就是那个契约（含中文站名 + 省略 tagline 两种真实情形）。
        let cfg: RemoteConfig = serde_json::from_slice(raw).expect("要能解");
        assert_eq!(cfg.sponsors.len(), 2);
        assert_eq!(
            cfg.sponsors[0].display_name, "百适 BestApi",
            "中文站名要原样解出"
        );
        assert_eq!(cfg.sponsors[0].tagline, "官方推荐");
        assert_eq!(cfg.sponsors[1].site_origin, "https://wawapii.com");
        assert_eq!(cfg.sponsors[1].display_name, "Wawa");
        assert_eq!(cfg.sponsors[1].tagline, "", "tagline 可省");
        assert_eq!(cfg.aff_codes.get("wawapii.com").unwrap(), "NEWCODE12345");
        assert_eq!(cfg.promo_codes.get("bestapi.store").unwrap(), "LOONGPORT");
    }

    /// ⭐ **老客户端必须能解「没有 `promo_codes` 字段」的那份配置。**
    ///
    /// 这条守的是向后兼容的另一半：`promo_codes` 是新加的，而**线上那份配置
    /// 现在还没有它**（等这版发出去才会加）。若忘了 `#[serde(default)]`，
    /// 后果是**整份配置解不出** ⇒ 连赞助商列表与 aff 码一起回落到内置
    /// ⇒ 首启屏的赞助商卡片全空。
    #[test]
    fn a_config_without_promo_codes_still_parses() {
        let raw = br#"{"sponsors": [], "aff_codes": {"wawapii.com": "X1234567890A"}}"#;
        let cfg: RemoteConfig = serde_json::from_slice(raw).expect("缺 promo_codes 也要能解");
        assert!(cfg.promo_codes.is_empty(), "缺字段 ⇒ 空 map，不是解析失败");
        assert_eq!(
            cfg.aff_codes.get("wawapii.com").unwrap(),
            "X1234567890A",
            "其余字段照常"
        );
    }

    /// ⭐ **两个方向的命名有意不同，别「统一」它们** —— 统一哪边都会坏。
    ///
    /// - 读配置（`Deserialize`）必须是 **snake_case**：那是签名覆盖的对外契约，
    ///   改了旧版本客户端解不出新配置。
    /// - 发给前端（`Serialize`）必须是 **camelCase**：本仓 TS 侧的惯例
    ///   （`src/lib/api/relay.ts` 里的类型全是），改了前端拿到 undefined。
    ///
    /// 这种「同一个结构两套命名」看起来像疏漏，**极可能被人顺手统一** ——
    /// 而两个方向的失败都静默：前者是配置解不出（回落内置表），
    /// 后者是首启屏卡片全空白。所以这条闸两个方向都钉。
    #[test]
    fn sponsor_reads_snake_case_but_serializes_camel_case() {
        // 读：发布的 JSON 是 snake_case。
        let s: Sponsor = serde_json::from_slice(
            br#"{"site_origin":"https://x.com","display_name":"X","tagline":"T"}"#,
        )
        .expect("必须能解 snake_case —— 那是签名覆盖的契约");
        assert_eq!(s.site_origin, "https://x.com");

        // 写：给前端的是 camelCase。断言**键名本身**，不是能不能 round-trip
        // （round-trip 在两边都改成同一套时照样过，测不出这条）。
        let json = serde_json::to_value(&s).expect("要能序列化");
        let obj = json.as_object().expect("是个对象");
        assert!(
            obj.contains_key("siteOrigin") && obj.contains_key("displayName"),
            "发给前端必须是 camelCase，实际键：{:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(
            !obj.contains_key("site_origin"),
            "序列化侧不该再出现 snake_case 键"
        );
    }

    /// ⭐ **未知字段必须被忽略，绝不能给 [`RemoteConfig`] 加
    /// `#[serde(deny_unknown_fields)]`。**
    ///
    /// 这条是实测踩出来的：加上那个属性会让**线上配置当场全线失效** ——
    /// 它带 `issued_at`（为将来防回滚攻击攒的历史），那是本结构体的未知字段
    /// ⇒ 验签通过但解析失败 ⇒ **整份配置被丢弃**、静默回落到编译期内置表。
    ///
    /// 最坏的部分：那次改动后**全部单测照样绿**（每一份 fixture 都不带 `issued_at`），
    /// 而线上表现只是「改了配置没生效」，不报任何错。实测过 serde 的两种行为差异：
    /// 严格版报 ``unknown field `issued_at` ``，宽容版正常解出。
    ///
    /// 宽容未知字段不是疏忽，是**前向兼容的必要条件**：新版本要能往配置里加字段
    /// 而不打断旧客户端 —— 那正是这套远端配置存在的意义（改配置不用发版）。
    #[test]
    fn unknown_fields_are_ignored_so_new_keys_never_break_old_clients() {
        // 一份「未来版本」的配置：除了 `issued_at`，还加了两个这个版本不认识的字段。
        let future = br#"{
          "issued_at": "2026-08-03T00:00:00Z",
          "schema_version": 7,
          "sponsors": [],
          "aff_codes": {"a.com": "CODE1"},
          "something_invented_later": {"nested": [1, 2, 3]}
        }"#;

        let cfg: RemoteConfig = serde_json::from_slice(future).expect(
            "未知字段必须被忽略 —— 红了说明给 RemoteConfig 加了 deny_unknown_fields，\
             那会让线上带 issued_at 的配置被整份丢弃",
        );

        // 认得的字段照常解出来。
        assert_eq!(cfg.aff_codes.get("a.com").unwrap(), "CODE1");
    }

    #[test]
    fn a_config_missing_every_field_is_still_valid() {
        // 维护者发一份空配置（比如临时撤掉所有赞助商）不该让客户端报错。
        let cfg: RemoteConfig = serde_json::from_slice(b"{}").expect("空配置也要能解");
        assert!(cfg.sponsors.is_empty());
        assert!(cfg.aff_codes.is_empty());
    }

    #[test]
    fn a_missing_signature_is_rejected_rather_than_trusted() {
        // ⭐ 本模块最危险的退化路径：「拿不到 .sig 就跳过验签」。
        //
        // 攻击者只要能删掉 / 让签名文件 404，就能让任意配置被接受 ——
        // 而那份配置决定用户被引到哪个站、邀请收益归谁。
        // 所以缺签名必须**报错**，不是「宽容处理」。
        let err = parse_verified(TEST_KEY_HEX, b"{}", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("签名"), "错误要说清是签名缺失：{err}");
    }

    #[test]
    fn a_bad_signature_is_rejected_before_parsing() {
        // 签名不对时**连解析都不该做**：否则攻击者的输入先过了我们的解析器，
        // 而 serde 的解析面就成了攻击面。
        //
        // ⚠️ 判据必须能区分顺序。**用一份签名错、且 JSON 也坏的输入**：
        // - 先验签（正确）⇒ 报「签名」
        // - 先解析（退化）⇒ 报「格式」
        //
        // 光用合法 JSON + 错签名测不出顺序（两条路都返回签名错），
        // 那样的断言是假闸 —— 实测过：把顺序反过来它照样绿。
        let err = parse_verified(TEST_KEY_HEX, b"{ this is not json", Some(&[0u8; 64]))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("签名"),
            "必须先验签再解析：坏 JSON + 错签名该报签名错，报格式错说明顺序反了：{err}"
        );
        assert!(
            !err.contains("格式"),
            "报了格式错 ⇒ 解析发生在验签之前：{err}"
        );
    }

    /// ⭐ **唯一走通正路的用例**：合法签名必须**验过**并解析出内容。
    ///
    /// 见 [`generate_test_keypair`] 的文档：其余 20 条全在测「被拒」，
    /// 所以「拒绝一切输入」这种退化在它们那儿是**零覆盖**，
    /// 而它的线上表现是最难查的一类 —— 配置永远拉不到、静默回落、不报错。
    #[test]
    fn a_correctly_signed_config_actually_verifies_and_parses() {
        let (public_key_hex, pair) = generate_test_keypair();

        // 一份**真实形状**的配置（与将来发布的那份同构，含中文站名）。
        let body = r#"{"sponsors":[{"site_origin":"https://790053500.com","display_name":"鑫旺","tagline":"中转"}],"aff_codes":{"790053500.com":"FQSPPFUYXSSS"}}"#.as_bytes();
        let signature = pair.sign(body);

        let cfg = parse_verified(&public_key_hex, body, Some(signature.as_ref()))
            .expect("合法签名必须验过并解析成功 —— 红了说明验签逻辑拒绝了一切输入");

        assert_eq!(cfg.sponsors.len(), 1);
        assert_eq!(cfg.sponsors[0].display_name, "鑫旺");
        assert_eq!(cfg.aff_codes.get("790053500.com").unwrap(), "FQSPPFUYXSSS");
    }

    /// 签名覆盖的是**原始字节**，所以改任何一个字节都必须失效 ——
    /// 包括**语义等价**的改动（空白、键顺序）。
    ///
    /// 这条钉住的是 `sign.sh` 那条纪律的另一面：**改完 config.json 必须重签**。
    /// 忘了重签的症状是「客户端整份丢弃」，而这里让那个因果关系有据可查。
    #[test]
    fn any_byte_changed_after_signing_invalidates_the_signature() {
        let (public_key_hex, pair) = generate_test_keypair();
        let original = br#"{"aff_codes":{"a.com":"CODE1"}}"#;
        let signature = pair.sign(original);

        // 先确认基线是过的，否则下面的「被拒」可能只是因为一切都被拒。
        assert!(parse_verified(&public_key_hex, original, Some(signature.as_ref())).is_ok());

        for (tampered, label) in [
            // 改了值 —— 攻击者要做的那件事。
            (br#"{"aff_codes":{"a.com":"EVIL2"}}"#.as_slice(), "改了码"),
            // ⭐ **语义完全等价**、只多一个空格。签结构的话这个会漏过去，
            // 签字节则不会 —— 这正是「签原始字节」的理由。
            (
                br#"{"aff_codes":{"a.com": "CODE1"}}"#.as_slice(),
                "只多一个空格（语义等价）",
            ),
        ] {
            assert!(
                parse_verified(&public_key_hex, tampered, Some(signature.as_ref())).is_err(),
                "{label}：改过的正文必须验不过（签名覆盖原始字节）"
            );
        }
    }

    #[test]
    fn an_oversized_cache_file_is_refused_before_being_read_into_memory() {
        // review 抓出：缓存是磁盘上的文件，可以被换成任意大小，
        // 而验签发生在**读之后** ⇒ 防不了「读的时候就 OOM」。
        // 所以要先看元数据。
        let dir = std::env::temp_dir().join(format!("lp-cap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let big = dir.join("big.json");
        // 只写「上限 + 1」字节就够验证判据（不必真造 GB 级文件）。
        std::fs::write(&big, vec![b'x'; MAX_CONFIG_BYTES + 1]).expect("write");

        assert!(
            read_capped(&big, MAX_CONFIG_BYTES).is_none(),
            "超上限的文件必须在读进内存之前就被拒"
        );

        // 恰好等于上限则该放行（不是 off-by-one 地拒掉合法的）。
        let exact = dir.join("exact.json");
        std::fs::write(&exact, vec![b'x'; MAX_CONFIG_BYTES]).expect("write");
        assert!(
            read_capped(&exact, MAX_CONFIG_BYTES).is_some(),
            "正好等于上限的文件是合法的"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_signature_file_must_be_exactly_64_bytes() {
        // 签名长度是**定值**不是上限 —— 不符合就说明那不是我们写的文件。
        assert_eq!(ED25519_SIGNATURE_LEN, 64);
    }

    /// ⚠️ 这条曾是**假闸**（review mutation 抓出）：原来只断言
    /// `refresh_and_cache().await == None`，而删掉那个 `is_configured()` 早退之后
    /// 它照样绿 —— 因为 `.invalid` 的 DNS 会失败、`.ok()?` 同样返回 `None`，
    /// **两条路径返回值相同**。
    ///
    /// 现在改成断言「守卫本身」+「没写缓存」，那才是它声称守的东西。
    #[tokio::test]
    async fn refresh_makes_no_request_and_writes_no_cache_while_unconfigured() {
        let _g = CacheDirGuard::new("noop");

        // ⚠️ 传**字面量占位**而不是生产常量（2026-08-03 后者已配成真端点）。
        // 用生产常量测这条会真去打自己的服务器 —— 那既是网络依赖，
        // 也测不出「删掉早退会怎样」（请求成功时返回 Some，与早退的 None 混不到一起）。
        const BAD_URL: &str = "https://config.invalid/v1/config.json";
        const BAD_SIG: &str = "https://config.invalid/v1/config.json.sig";
        const BAD_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

        // 判据一：守卫成立（端点/公钥任一没配就该早退）。
        assert!(
            !is_configured_with(BAD_URL, BAD_SIG, BAD_KEY),
            "占位端点/公钥下 is_configured_with 必须为 false —— 那是早退的依据"
        );

        // 判据二：**必须在 8 秒超时之前就返回** ——
        // 早退被删掉时它会真去打 `.invalid`（DNS 解析要时间），实测那种退化耗时
        // 从 ~0ms 涨到 8 秒级。用时间当判据在单测里通常是坏味道，
        // 但这里「有没有发起网络请求」本身就只能靠这个侧信道观察
        // （不注入 HTTP client 的话）。阈值取 2 秒：正常早退是微秒级，
        // 真发请求最快也要 DNS 往返，中间差着三个数量级。
        let started = std::time::Instant::now();
        assert_eq!(
            refresh_and_cache_with(BAD_URL, BAD_SIG, BAD_KEY).await,
            None
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "未配置时必须立刻早退（实测 {elapsed:?}）—— 花了这么久说明它真去发请求了"
        );

        // 判据三：一个缓存文件都不该产生。
        assert!(
            !cache_path().exists() && !cache_sig_path().exists(),
            "未配置时不该写任何缓存文件"
        );
    }

    #[test]
    fn is_configured_needs_all_three_terms_none_is_redundant() {
        const GOOD_URL: &str = "https://config.example.com/config.json";
        const GOOD_SIG: &str = "https://config.example.com/config.json.sig";
        const GOOD_KEY: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

        // ⚠️ 坏值用**字面量占位**，不借生产常量。
        // 2026-08-03 那三个常量都配成真的了 ⇒ 拿它们当「坏值」会让这条测试
        // 恰好因为「生产值是坏的」而通过，配好之后当场变红。
        // 而 `is_configured_with` 参数化的初衷本来就是**不依赖生产常量**。
        const BAD_URL: &str = "https://config.invalid/v1/config.json";
        const BAD_SIG: &str = "https://config.invalid/v1/config.json.sig";
        const BAD_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

        // 三项都好 ⇒ true（否则下面那些 false 断言可能只是因为基线就是 false）。
        assert!(
            is_configured_with(GOOD_URL, GOOD_SIG, GOOD_KEY),
            "基线该为 true"
        );

        // ⭐ 逐项拿掉，**每一项都必须让整体变 false**。
        // 这才是「缺一不可」的判据 —— review mutation 抓出过：
        // 三项里少判一项时，其余两项仍为坏值会让整体照样 false，测不出来。
        assert!(
            !is_configured_with(BAD_URL, GOOD_SIG, GOOD_KEY),
            "配置 URL 是占位 ⇒ 整体必须 false"
        );
        assert!(
            !is_configured_with(GOOD_URL, BAD_SIG, GOOD_KEY),
            "签名 URL 是占位 ⇒ 整体必须 false"
        );
        assert!(
            !is_configured_with(GOOD_URL, GOOD_SIG, BAD_KEY),
            "公钥是占位 ⇒ 整体必须 false（删掉那一项这条会红）"
        );

        // （「生产常量此刻是好的」由 `endpoints_and_key_are_configured_for_production`
        // 那条守，这里不重复断言 —— 本条只管「三项缺一不可」这一个性质。）

        // 公钥项与 `verify_with` 同一条规则：低阶点一律不可用。
        for bad in LOW_ORDER_PUBLIC_KEYS {
            assert!(
                !is_configured_with(GOOD_URL, GOOD_SIG, bad),
                "低阶公钥 {bad} 下整体必须 false —— 否则拉取链路会在一把可伪造的公钥下上线"
            );
        }
        assert!(!is_key_usable("d75a98"), "长度不对该判不可用");
        assert!(!is_key_usable(&"z".repeat(64)), "非 hex 该判不可用");
    }
}
