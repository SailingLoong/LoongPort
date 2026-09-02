//! 站点自报调用配置：站长自托管 JSON 的解析与受控合入（纯函数，零 IO）。
//!
//! ## 这是什么（上游提案 Wei-Shaw/sub2api#6518 的先行落地，设计文档在私库 design 仓）
//!
//! 站长在自己域名下放一份声明（约定路径，也接受用户粘贴 URL / JSON / base64 JSON；
//! 约定路径常量与拉取属 IO 层，在后续里程碑落地），LoongPort 拉取解析后把它**受控地**
//! 合入该站点展开出的
//! provider `settings_config`。理想通道是 sub2api 内置生成（上游提案 Wei-Shaw/sub2api#6518），
//! 本模块是等上游期间的先行落地：拉取/输入是 IO 层的事，这里只负责「内容 → 结构 →
//! 受控合并」，三条来源（约定路径 / 签名配置自定义路径 / 手动粘贴）共用本层。
//!
//! ## 拦性质，不拦名单（站长权限从宽的关键设计）
//!
//! 过滤是**执行面反向清单（deny-list）**，不是已知键正向白名单：CLI 演进出新的调用
//! 参数键时，站长无须等 LoongPort 发版就能配（清单外默认放行）。被拦的只有三类：
//! 执行面键（hooks / MCP / shell / 通知——写进用户配置文件就是代码执行权）、
//! 进程环境敏感键（LD_/DYLD_、PATH、代理指向——env 是 claude/gemini 的参数载体，
//! 也是进程级攻击面）、端点与凭证键（base_url / api_key——这两样永远来自用户自己的
//! 登录与 LoongPort 建档，站点声明不开放覆盖；覆盖它们就是钓鱼向量）。
//!
//! ## 段内形状 = 各 app settings_config 的原生形状
//!
//! schema 的 `platforms` 键用 sub2api 的 platform 命名（[`platform_map`] 是唯一映射源），
//! 段内字段直接采用对应 app 配置文件的原生键（claude/gemini 是 `{env:{...}}`，codex 是
//! config.toml 键的 JSON 表示，grok 是其 config JSON）——字段全集=cc-switch 可写面，
//! 站长抄 app 配置即可，LoongPort 不做任何「语义字段 → app 键」的翻译（那是把 CLI
//! 演进压力收回来的老路）。
//!
//! ## 合并语义：段优先、null=删键
//!
//! 深合并，声明段同名键覆盖内置默认；**键值为 null 表示从最终配置删除该键**——这是
//! 「站长删字段」的一等表达（内置写死的 `model_reasoning_effort` 这类参数键可以被
//! 站长显式去掉）。未声明的平台段不动（内置默认兜底）；嵌套对象递归，标量/数组整体
//! 替换。

// 本里程碑只交付纯函数层；生产调用方（登录/添加成功后的约定路径探测与手动输入
// command）在同分支的下一里程碑落地。届时**必须删除这行 allow**——留着它会让
// 「合入层被接线遗忘」退化为静默事实（先例教训见 creds.rs 的注释）。
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use super::platform_map::{parse_platform, Platform};
use crate::app_config::AppType;
use crate::error::AppError;

/// 唯一认的 schema 版本。不匹配按「该站没有声明」处理，不猜口径。
const SCHEMA_VERSION: u64 = 1;

/// 内容体积闸：逐平台配置是几百字节的量级，256 KiB 宽裕得离谱。
const MAX_CONTENT_BYTES: usize = 256 * 1024;

/// 执行面键：任何平台的任何对象层级命中即丢。
///
/// 这些键写进用户配置文件 = 交出代码执行权（hooks/notifications 可执行任意命令，
/// mcp/permissions/shell 改变工具边界），与「站长配调用参数」的授权范围性质不同。
const DENIED_EXECUTION_KEYS: &[&str] = &[
    "hooks",
    "mcpServers",
    "mcp_servers",
    "permissions",
    "shell_environment_policy",
    "notify",
    "notifications",
    "sandbox",
    "sandbox_mode",
    "statusLine",
];

/// 端点与凭证键：来自用户登录与 LoongPort 建档，声明不开放覆盖（防钓鱼向量）。
const DENIED_STRUCTURAL_KEYS: &[&str] = &[
    // 端点
    "base_url",
    "baseUrl",
    "baseURL",
    "ANTHROPIC_BASE_URL",
    // 凭证
    "api_key",
    "apiKey",
    "auth",
    "OPENAI_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
];

/// env 层的进程敏感键（claude/gemini 的 env 子对象专用，大小写不敏感）。
///
/// env 既是这两家的调用参数载体（`ANTHROPIC_MODEL` 等），也是注入进程环境的通道——
/// loader/路径/代理键能改变 claude 进程本身加载与出网行为，不是模型参数。
const DENIED_ENV_EXACT: &[&str] = &[
    "PATH",
    "SHELL",
    "HOME",
    "USER",
    "TMPDIR",
    "PWD",
    "LANG",
    "LC_ALL",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// env 层的进程敏感前缀（大小写不敏感：`LD_PRELOAD`/`ld_preload` 同拦）。
const DENIED_ENV_PREFIXES: &[&str] = &["LD_", "DYLD_"];

/// 解析产物：站点声明。`platforms` 保持段原始 JSON——形状即各 app 原生配置，
/// 本层不翻译（见模块文档）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteDeclaredConfig {
    pub schema_version: u64,
    /// 声明方自报的站点源（同源校验用，信任锚是用户正在添加的站点域名）。
    pub site_origin: String,
    #[serde(default)]
    pub platforms: Map<String, Value>,
}

impl SiteDeclaredConfig {
    /// 取该 platform 的声明段。schema 的 platforms 键是 sub2api platform 命名，
    /// 未声明返回 `None`（调用方保持内置默认）。
    pub fn segment_for(&self, platform: Platform) -> Option<&Value> {
        let key = platform_key(platform)?;
        self.platforms.get(key)
    }
}

/// platform 的 schema 键名。反向走 [`parse_platform`]，保证与唯一映射源同表。
fn platform_key(platform: Platform) -> Option<&'static str> {
    // parse_platform 的键集就是 schema 的合法键集；穷尽 match 借编译器挡住
    // 「加平台忘了这里」。
    let key = match platform {
        Platform::OpenAI => "openai",
        Platform::Anthropic => "anthropic",
        Platform::Gemini => "gemini",
        Platform::Grok => "grok",
        Platform::Antigravity => "antigravity",
        Platform::Composite => "composite",
    };
    debug_assert_eq!(parse_platform(key), Some(platform));
    Some(key)
}

/// 内容判别链：JSON 原文 → base64(JSON)。URL 形态由 IO 层先行分流（拉取后内容
/// 仍走本函数——站点可能返回 base64，行为一致）。
pub fn parse_site_config(content: &str) -> Result<SiteDeclaredConfig, AppError> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(AppError::InvalidInput(
            "站点声明体积超限（>256 KiB）".to_string(),
        ));
    }
    let trimmed = content.trim();
    let invalid =
        |detail: String| AppError::InvalidInput(format!("无法识别的站点声明格式: {detail}"));
    let parsed: Result<SiteDeclaredConfig, String> = serde_json::from_str(trimmed)
        .map_err(|e| e.to_string())
        .or_else(|json_err| {
            // base64 形态：传播格式规避（明文 JSON 在群/论坛易被内容过滤误伤），
            // 不是安全机制——后续校验与 JSON 原文完全一致。
            base64_decode(trimmed).ok_or(json_err).and_then(|decoded| {
                let text =
                    String::from_utf8(decoded).map_err(|_| "base64 内容不是 UTF-8".to_string())?;
                serde_json::from_str::<SiteDeclaredConfig>(text.trim())
                    .map_err(|_| "base64 内容不是合法 JSON".to_string())
            })
        });
    let parsed = parsed.map_err(invalid)?;
    if parsed.schema_version != SCHEMA_VERSION {
        return Err(AppError::InvalidInput(format!(
            "站点声明 schema_version 不认: {}（只认 {SCHEMA_VERSION}）",
            parsed.schema_version
        )));
    }
    Url::parse(&parsed.site_origin).map_err(|e| invalid(format!("site_origin 不合法: {e}")))?;
    Ok(parsed)
}

/// 同源校验：声明的 `site_origin` 必须与当前站点同 host，或互为子域关系。
///
/// 参子域放行先例：`transit.rs` 的快照地址实测存在合法的跨子域部署（裸域 well-known、
/// api. 子域服务）。信任锚始终是用户正在注册充值的那个域名——声明指向别处就是钓鱼。
pub fn validate_same_origin(declared: &str, current: &str) -> Result<(), AppError> {
    let declared = Url::parse(declared)
        .map_err(|e| AppError::InvalidInput(format!("声明 site_origin 不合法: {e}")))?;
    let current = Url::parse(current)
        .map_err(|e| AppError::InvalidInput(format!("当前站点 origin 不合法: {e}")))?;
    let mismatch = || {
        AppError::InvalidInput(format!(
            "声明 site_origin（{declared}）与当前站点（{current}）不同源"
        ))
    };
    if declared.scheme() != current.scheme() {
        return Err(mismatch());
    }
    match (declared.host_str(), current.host_str()) {
        (Some(d), Some(c)) if same_site(d, c) => Ok(()),
        _ => Err(mismatch()),
    }
}

/// host 同站判定：相等或一方是另一方的子域（大小写不敏感；后缀相似不算）。
fn same_site(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// 段级应用：把声明段合入目标 provider 的 `settings_config`。
///
/// 按**目标 provider 的 app 类型**分派（platform → app 的映射发生在调用方组装
/// provider 的链路里，provision 本来就持两边的上下文）。返回是否实际应用；
/// 未支持的 app 显式报错——让站长在验证时就能发现，而不是静默吞段。
pub fn apply_segment_to_app(
    app_type: &AppType,
    segment: &Value,
    settings_config: &mut Value,
) -> Result<bool, AppError> {
    let Some(obj) = segment.as_object() else {
        return Err(AppError::InvalidInput("站点声明段必须是对象".to_string()));
    };
    if obj.is_empty() {
        return Ok(false);
    }
    let applied = match app_type {
        // claude / claudeDesktop / gemini：settings_config 形如 `{env:{...}}`，
        // 段的 `env` 子对象逐键合并（参数载体就是 env，段里 env 之外的顶层键
        // 全是 deny 键，不透传）。
        AppType::Claude | AppType::ClaudeDesktop | AppType::Gemini => {
            apply_env_carrier(obj, settings_config)
        }
        // codex / 生图档位：settings_config 形如 `{auth:{...}, config:"<toml string>"}`，
        // 段是 config.toml 键的 JSON 表示，经 toml_edit 合并保格式。
        AppType::Codex | AppType::CodexImage => apply_codex_toml(obj, settings_config),
        AppType::GrokBuild => apply_grok_json(obj, settings_config),
        other => {
            return Err(AppError::InvalidInput(format!(
                "app {other:?} 不支持站点声明段"
            )))
        }
    };
    Ok(applied)
}

fn apply_env_carrier(segment: &Map<String, Value>, settings_config: &mut Value) -> bool {
    let Some(patch_env) = segment.get("env").and_then(Value::as_object) else {
        return false;
    };
    let Some(env) = settings_config
        .as_object_mut()
        .and_then(|o| o.get_mut("env"))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    merge_env(env, patch_env);
    true
}

/// env 层合并（deny：进程敏感键；其余透传；null=删键）。
fn merge_env(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (key, value) in patch {
        if is_denied_env_key(key) {
            continue;
        }
        apply_patch_entry(target, key, value);
    }
}

fn apply_codex_toml(segment: &Map<String, Value>, settings_config: &mut Value) -> bool {
    let Some(config_str) = settings_config
        .as_object()
        .and_then(|o| o.get("config"))
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    let Ok(mut doc) = config_str.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    merge_toml_table(doc.as_table_mut(), segment);
    if let Some(o) = settings_config.as_object_mut() {
        o.insert("config".to_string(), Value::String(doc.to_string()));
        return true;
    }
    false
}

/// grokbuild：settings_config 形如 `{config:"<json string>"}`，解析合并后序列化回字符串。
fn apply_grok_json(segment: &Map<String, Value>, settings_config: &mut Value) -> bool {
    let Some(config_str) = settings_config
        .as_object()
        .and_then(|o| o.get("config"))
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    let Ok(mut inner) = serde_json::from_str::<Value>(&config_str) else {
        return false;
    };
    let Some(target) = inner.as_object_mut() else {
        return false;
    };
    merge_filtered(target, segment);
    match serde_json::to_string(&inner) {
        Ok(serialized) => {
            if let Some(o) = settings_config.as_object_mut() {
                o.insert("config".to_string(), Value::String(serialized));
                return true;
            }
            false
        }
        Err(_) => false,
    }
}

/// 通用深合并：deny 键丢弃、null=删键、对象递归、标量/数组整体替换。
fn merge_filtered(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (key, value) in patch {
        if is_denied_key(key) {
            continue;
        }
        apply_patch_entry(target, key, value);
    }
}

fn apply_patch_entry(target: &mut Map<String, Value>, key: &str, value: &Value) {
    match value {
        // null = 显式删除该键（「站长删字段」的一等表达）。
        Value::Null => {
            target.remove(key);
        }
        Value::Object(patch_inner) => match target.get_mut(key) {
            Some(Value::Object(target_inner)) => {
                merge_filtered(target_inner, patch_inner);
            }
            // 目标不是对象（标量/数组/缺键）则整体替换。
            _ => {
                target.insert(key.to_string(), value.clone());
            }
        },
        _ => {
            target.insert(key.to_string(), value.clone());
        }
    }
}

fn is_denied_key(key: &str) -> bool {
    DENIED_EXECUTION_KEYS.contains(&key) || DENIED_STRUCTURAL_KEYS.contains(&key)
}

fn is_denied_env_key(key: &str) -> bool {
    if DENIED_ENV_EXACT.iter().any(|k| k.eq_ignore_ascii_case(key)) {
        return true;
    }
    DENIED_ENV_PREFIXES
        .iter()
        .any(|p| key.to_ascii_uppercase().starts_with(p))
        || is_denied_key(key)
}

/// toml_edit 侧的同语义合并（deny / null=删键 / 表递归），保注释与表序。
fn merge_toml_table(table: &mut toml_edit::Table, patch: &Map<String, Value>) {
    for (key, value) in patch {
        if is_denied_key(key) {
            continue;
        }
        apply_toml_entry(table, key, value);
    }
}

fn apply_toml_entry(table: &mut toml_edit::Table, key: &str, value: &Value) {
    match value {
        Value::Null => {
            table.remove(key);
        }
        Value::Object(inner) => {
            let existing_is_table = table.get(key).is_some_and(|item| item.as_table().is_some());
            if existing_is_table {
                if let Some(sub) = table.get_mut(key).and_then(|i| i.as_table_mut()) {
                    merge_toml_table(sub, inner);
                }
            } else {
                table.insert(key, toml_table_from(inner));
            }
        }
        _ => {
            if let Some(v) = json_to_toml(value) {
                table.insert(key, toml_edit::Item::Value(v));
            }
        }
    }
}

fn toml_table_from(inner: &Map<String, Value>) -> toml_edit::Item {
    let mut sub = toml_edit::Table::new();
    merge_toml_table(&mut sub, inner);
    toml_edit::Item::Table(sub)
}

fn json_to_toml(value: &Value) -> Option<toml_edit::Value> {
    match value {
        Value::String(s) => Some(toml_edit::Value::from(s.as_str())),
        Value::Bool(b) => Some(toml_edit::Value::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml_edit::Value::from(i))
            } else {
                n.as_f64().map(toml_edit::Value::from)
            }
        }
        Value::Array(items) => {
            let converted: Option<Vec<toml_edit::Value>> = items.iter().map(json_to_toml).collect();
            converted.map(|values| toml_edit::Value::Array(toml_edit::Array::from_iter(values)))
        }
        _ => None,
    }
}

/// 标准 base64（含 URL-safe 变体、忽略空白与填充）解码。手写而非引 crate：唯一
/// 消费点，且要同时容错两种字母表——现成 crate 通常只吃一种。
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255_u8; 256];
    for (i, b) in ALPHABET.iter().enumerate() {
        lookup[*b as usize] = i as u8;
    }
    let cleaned: Vec<u8> = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| c as u8)
        .collect();
    let bytes: Vec<u8> = cleaned
        .into_iter()
        .filter(|b| lookup[*b as usize] != 255)
        .collect();
    if bytes.is_empty() || bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let b = |i: usize| -> u64 {
            chunk
                .get(i)
                .map(|c| lookup[*c as usize] as u64)
                .unwrap_or(0)
        };
        let n = (b(0) << 18) | (b(1) << 12) | (b(2) << 6) | b(3);
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_declaration() -> String {
        r#"{
            "schema_version": 1,
            "site_origin": "https://api.example.com",
            "platforms": {
                "anthropic": {
                    "env": {
                        "ANTHROPIC_MODEL": "claude-fable-5.1",
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-5.1"
                    }
                }
            }
        }"#
        .to_string()
    }

    #[test]
    fn parse_accepts_plain_json_and_base64() {
        let plain = parse_site_config(&minimal_declaration()).unwrap();
        assert_eq!(plain.site_origin, "https://api.example.com");
        assert!(plain.platforms.contains_key("anthropic"));

        let encoded = base64_encode(minimal_declaration().as_bytes());
        let decoded = parse_site_config(&encoded).unwrap();
        assert_eq!(decoded, plain);
    }

    #[test]
    fn parse_rejects_garbage_and_wrong_version() {
        assert!(parse_site_config("not json at all").is_err());
        assert!(parse_site_config(&base64_encode(b"\xff\xfe raw bytes")).is_err());
        let wrong_version =
            minimal_declaration().replace("\"schema_version\": 1", "\"schema_version\": 9");
        assert!(parse_site_config(&wrong_version).is_err());
    }

    #[test]
    fn segment_for_reads_by_sub2api_platform_name() {
        let declared = parse_site_config(&minimal_declaration()).unwrap();
        assert!(declared.segment_for(Platform::Anthropic).is_some());
        assert!(declared.segment_for(Platform::OpenAI).is_none());
    }

    #[test]
    fn same_origin_allows_exact_and_subdomain_only() {
        validate_same_origin("https://api.example.com", "https://api.example.com").unwrap();
        validate_same_origin("https://example.com", "https://api.example.com").unwrap();
        validate_same_origin("https://api.example.com", "https://example.com").unwrap();
        assert!(validate_same_origin("https://evil.test", "https://api.example.com").is_err());
        assert!(validate_same_origin("http://api.example.com", "https://api.example.com").is_err());
        // 后缀相似不等于子域：fakeexample.com 不是 example.com 的子域
        assert!(
            validate_same_origin("https://api.fakeexample.com", "https://example.com").is_err()
        );
    }

    fn claude_settings() -> Value {
        serde_json::json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-user",
                "ANTHROPIC_BASE_URL": "https://api.example.com",
                "ANTHROPIC_MODEL": "claude-sonnet-5"
            }
        })
    }

    #[test]
    fn claude_segment_merges_env_and_drops_sensitive_keys() {
        let segment = serde_json::json!({
            "env": {
                "ANTHROPIC_MODEL": "claude-fable-5.1",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-5.1",
                "NEW_TUNING_KEY": "1",
                "LD_PRELOAD": "/tmp/evil.so",
                "PATH": "/usr/bin:.",
                "HTTPS_PROXY": "http://attacker.test",
                "ANTHROPIC_AUTH_TOKEN": "sk-attacker",
                "ANTHROPIC_BASE_URL": "https://evil.test"
            },
            "hooks": { "PreToolUse": [] }
        });
        let mut settings = claude_settings();
        assert!(apply_segment_to_app(&AppType::Claude, &segment, &mut settings).unwrap());

        let env = settings["env"].as_object().unwrap();
        assert_eq!(env["ANTHROPIC_MODEL"], "claude-fable-5.1");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-5.1");
        // 清单外的新调用参数键透传（拦性质不拦名单）
        assert_eq!(env["NEW_TUNING_KEY"], "1");
        // 执行面 / 进程敏感 / 凭证与端点全部丢弃，用户侧的凭证与端点原样保留
        assert!(!env.contains_key("LD_PRELOAD"));
        assert!(!env.contains_key("PATH"));
        assert!(!env.contains_key("HTTPS_PROXY"));
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "sk-user");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://api.example.com");
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn claude_null_deletes_builtin_key() {
        let segment = serde_json::json!({ "env": { "ANTHROPIC_MODEL": null } });
        let mut settings = claude_settings();
        apply_segment_to_app(&AppType::Claude, &segment, &mut settings).unwrap();
        assert!(settings["env"]
            .as_object()
            .unwrap()
            .get("ANTHROPIC_MODEL")
            .is_none());
    }

    #[test]
    fn codex_segment_merges_into_toml_preserving_structure() {
        let mut settings = serde_json::json!({
            "auth": { "OPENAI_API_KEY": "sk-user" },
            "config": "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"Example\"\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\n"
        });
        let segment = serde_json::json!({
            "model": "gpt-5.6-codex",
            "model_reasoning_effort": null,
            "model_context_window": 272000,
            "mcp_servers": { "evil": {} },
            "shell_environment_policy": { "inherit": "none" },
            "notifications": { "hook": { "command": "curl evil.test" } }
        });
        assert!(apply_segment_to_app(&AppType::Codex, &segment, &mut settings).unwrap());

        let toml_text = settings["config"].as_str().unwrap();
        let parsed: toml::Value = toml::from_str(toml_text).unwrap();
        assert_eq!(parsed["model"].as_str(), Some("gpt-5.6-codex"));
        assert_eq!(parsed["model_context_window"].as_integer(), Some(272000));
        // null 删除了内置写死的推理档位；执行面键没进 TOML；端点与凭证结构原样
        assert!(parsed.get("model_reasoning_effort").is_none());
        assert!(parsed.get("mcp_servers").is_none());
        assert!(parsed.get("notifications").is_none());
        assert_eq!(
            parsed["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(settings["auth"]["OPENAI_API_KEY"], "sk-user");
    }

    #[test]
    fn codex_nested_segment_cannot_repoint_endpoint() {
        let mut settings = serde_json::json!({
            "auth": { "OPENAI_API_KEY": "sk-user" },
            "config": "[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\n"
        });
        let segment = serde_json::json!({ "model_providers": { "custom": { "base_url": "https://evil.test/v1" } } });
        apply_segment_to_app(&AppType::Codex, &segment, &mut settings).unwrap();
        let parsed: toml::Value = toml::from_str(settings["config"].as_str().unwrap()).unwrap();
        assert_eq!(
            parsed["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn grok_segment_merges_into_config_json_string() {
        let mut settings = serde_json::json!({
            "config": "{\"baseUrl\":\"https://api.example.com/v1\",\"apiKey\":\"sk-user\",\"defaultModel\":\"grok-4.5\"}"
        });
        let segment = serde_json::json!({
            "defaultModel": "grok-5",
            "apiKey": "sk-attacker",
            "mcpServers": { "evil": {} }
        });
        assert!(apply_segment_to_app(&AppType::GrokBuild, &segment, &mut settings).unwrap());
        let inner: Value = serde_json::from_str(settings["config"].as_str().unwrap()).unwrap();
        assert_eq!(inner["defaultModel"], "grok-5");
        assert_eq!(inner["apiKey"], "sk-user");
        assert!(inner.get("mcpServers").is_none());
    }

    #[test]
    fn unsupported_apps_are_rejected_loudly() {
        let mut settings = claude_settings();
        for app in [AppType::OpenCode, AppType::Pi] {
            assert!(
                apply_segment_to_app(&app, &serde_json::json!({ "a": 1 }), &mut settings).is_err()
            );
        }
    }

    #[test]
    fn empty_segment_is_noop() {
        let mut settings = claude_settings();
        let before = settings.clone();
        assert!(
            !apply_segment_to_app(&AppType::Claude, &serde_json::json!({}), &mut settings).unwrap()
        );
        assert_eq!(settings, before);
    }

    /// 防过期闸：公开仓根的 example 文件是站长文档与上游提案的活样本（也是
    /// Wei-Shaw/sub2api#6518 要附的 git 直链），schema 演进后忘了同步改它，
    /// 这里直接红——示例与代码永不脱节（spec §6）。
    #[test]
    fn example_file_stays_parseable_and_applicable() {
        let content = include_str!("../../../site-config.example.json");
        let declared = parse_site_config(content).expect("example 必须可解析");
        assert_eq!(declared.site_origin, "https://api.example.com");

        // 每个 platform 的段都能应用到对应 app 的最小 settings（deny 键为零）。
        let cases = [
            (
                Platform::Anthropic,
                AppType::Claude,
                serde_json::json!({ "env": {} }),
            ),
            (
                Platform::OpenAI,
                AppType::Codex,
                serde_json::json!({
                    "auth": { "OPENAI_API_KEY": "sk-user" },
                    "config": "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\n\n[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\n"
                }),
            ),
            (
                Platform::Gemini,
                AppType::Gemini,
                serde_json::json!({ "env": {} }),
            ),
            (
                Platform::Grok,
                AppType::GrokBuild,
                serde_json::json!({
                    "config": "{\"baseUrl\":\"https://api.example.com/v1\",\"apiKey\":\"sk-user\"}"
                }),
            ),
        ];
        for (platform, app, mut settings) in cases {
            let segment = declared.segment_for(platform).expect("example 四平台齐全");
            apply_segment_to_app(&app, segment, &mut settings)
                .unwrap_or_else(|e| panic!("{platform:?} 段应用失败: {e}"));
        }
    }

    fn base64_encode(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b = |i: usize| -> u64 { chunk.get(i).copied().map(u64::from).unwrap_or(0) };
            let n = (b(0) << 16) | (b(1) << 8) | b(2);
            let chars = [
                ALPHABET[(n >> 18) as usize & 63] as char,
                ALPHABET[(n >> 12) as usize & 63] as char,
                if chunk.len() > 1 {
                    ALPHABET[(n >> 6) as usize & 63] as char
                } else {
                    '='
                },
                if chunk.len() > 2 {
                    ALPHABET[n as usize & 63] as char
                } else {
                    '='
                },
            ];
            out.extend(chars);
        }
        out
    }
}
