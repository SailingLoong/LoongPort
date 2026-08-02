//! 分组 → sk → codex provider 的展开。「用户无感地拿到密钥」这步就在这里。
//!
//! ## 流程
//!
//! ```text
//! 拉分组（只留 platform == openai 且 active）
//!   └→ 对每个分组：
//!        ├→ 在已有 Key 里按名字精确认领   ← 正常路径，不发写请求
//!        └→ 认领不到才 POST 建一把
//!             └→ 拿到明文 sk → 组装成一条 codex provider 写库
//! ```
//!
//! ## Key 命名契约
//!
//! ```text
//! LoongPort/<device-id>/<platform>/<group-id>
//! ```
//!
//! 四段合起来表达「这台机器上、这个平台的、这个分组的、由 LoongPort 管理的一把 Key」。
//!
//! - **`device-id` 不能省**：多台机器共用一个账号时，各自认领自己那把 —— 否则 A 机器改了
//!   Key，B 机器的配置就悄悄失效。
//! - **`platform` 不能省**：分组 id 只在平台内唯一，跨平台会撞号。第一版只展开 `openai`，
//!   但下一步「站点 × 分组」页要按当前 tab 的平台展开 codex / claude / gemini —— 那时同号
//!   分组分属不同平台，靠三段名字认领会互相顶掉对方的 Key。
//! - **`group-id` 用数值 ID 不用分组名**：名字由运营商随时可改，改了就认领不到自己的 Key。
//! - **与 V1 同为四段**：V1 那四段是对的，V2 第一版曾砍成三段（理由是「只有一个 platform，
//!   那段恒定即冗余」），多平台之后该理由不成立，2026-08-02 改回四段。
//!
//! ⚠️ **这个名字进了服务端、是跨端可见的**。改它等于所有已建 Key 认领不回来，于是给用户
//! 账号里堆一批重复 sk —— 属不可逆决定，别顺手改。
//!
//! ## 批量失败的语义：尽力而为 + 全量回报，不回滚
//!
//! N 个分组里第 3 个建 Key 失败了，前 2 个**保留**。理由：每个分组的 provider 各自独立可用，
//! 部分可用优于全部不可用；而回滚本身也可能失败，还得再处理回滚失败。失败项在返回值里如实
//! 报出来，用户可以重试 —— 重试是幂等的（认领优先，已建的那些直接命中）。

use crate::app_config::AppType;
use crate::error::AppError;
use crate::operator::api::{ApiKey, Client, Group};

/// Key 名字的前缀，也是「这把 Key 由本客户端管理」的识别标志。
const MANAGED_PREFIX: &str = "LoongPort";

/// 一个分组的展开结果。
#[derive(Debug, Clone)]
pub struct Tier {
    pub group_id: i64,
    pub group_name: String,
    /// 计费倍率，越小越便宜。
    pub rate_multiplier: f64,
    /// 明文 sk。
    pub api_key: String,
    /// 这把 Key 是刚建的还是认领到的（只用于日志与 UI 提示，不参与逻辑）。
    pub key_was_created: bool,
}

/// 展开的整体结果。**失败项不阻断成功项**，两者都如实带出来。
#[derive(Debug, Default)]
pub struct ProvisionResult {
    pub tiers: Vec<Tier>,
    /// `(分组名, 失败原因)`。
    pub failures: Vec<(String, String)>,
}

/// 一个分组对应的 Key 名字。
pub fn key_name_for(device_id: &str, platform: &str, group_id: i64) -> String {
    format!("{MANAGED_PREFIX}/{device_id}/{platform}/{group_id}")
}

/// 在已有 Key 里认领属于本机 + 本分组的那把。
///
/// `list_keys` 的 `search` 是**子串匹配**（不是前缀），所以必须在客户端做精确比对 ——
/// 否则 `.../42` 会被 `.../420` 命中。
///
/// 命中多把时取 `id` 最大的那把（服务端 `name` 无唯一约束，同名可以无限建）。其余不自动删：
/// 删别人的东西要有更强的依据，这里只是「我认得出哪把是我的」。
pub fn claim_key<'a>(
    keys: &'a [ApiKey],
    device_id: &str,
    platform: &str,
    group_id: i64,
) -> Option<&'a ApiKey> {
    let want = key_name_for(device_id, platform, group_id);
    keys.iter()
        .filter(|k| k.name == want && k.is_usable())
        // 非 active 的不得认领：否则「认领到废 Key → 调用失败 → 再认领同一把」就是个环。
        .max_by_key(|k| k.id)
}

/// 为所有可用分组备好 sk。
pub async fn provision(client: &Client, device_id: &str) -> Result<ProvisionResult, AppError> {
    let groups = client.list_groups().await?;
    // 本轮 provision 仍只展开 codex（写入路径上还有四处 codex 硬编码，见 spec §一），
    // 所以这里把 app_type 写死成 `Codex` 而不是加参数 —— 签名吃 platform 是下一轮的事。
    let usable: Vec<Group> = groups
        .into_iter()
        .filter(|g| g.is_usable_for(&AppType::Codex))
        .collect();

    if usable.is_empty() {
        return Err(AppError::Config(
            "这个账号下没有可用的 codex 分组（需要 platform 为 openai 的活跃分组）".into(),
        ));
    }

    // 一次拉全量已有 Key，而不是每个分组各查一次：分组通常 1-5 个，一次拉回来在内存里比对
    // 更省请求，也避免撞面板的 240 次/分钟限流。
    let existing = client.list_keys(MANAGED_PREFIX).await?;

    let mut result = ProvisionResult::default();
    for group in usable {
        match ensure_key_for(client, device_id, &group, &existing).await {
            Ok(tier) => result.tiers.push(tier),
            // 一个分组失败不影响其它分组 —— 部分可用优于全部不可用。
            Err(e) => result.failures.push((group.name.clone(), e.to_string())),
        }
    }

    if result.tiers.is_empty() {
        let detail = result
            .failures
            .iter()
            .map(|(g, e)| format!("{g}: {e}"))
            .collect::<Vec<_>>()
            .join("；");
        return Err(AppError::Config(format!(
            "所有分组都没能备好密钥（{detail}）"
        )));
    }
    Ok(result)
}

async fn ensure_key_for(
    client: &Client,
    device_id: &str,
    group: &Group,
    existing: &[ApiKey],
) -> Result<Tier, AppError> {
    let (api_key, created) = match claim_key(existing, device_id, &group.platform, group.id) {
        // 正常路径：认领到了就直接用，不发任何写请求。
        Some(k) => (k.key.clone(), false),
        None => {
            let name = key_name_for(device_id, &group.platform, group.id);
            let created = client.create_key(&name, group.id).await?;
            if created.key.is_empty() {
                return Err(AppError::Config("服务端返回的密钥是空的".into()));
            }
            (created.key, true)
        }
    };

    Ok(Tier {
        group_id: group.id,
        group_name: group.name.clone(),
        rate_multiplier: group.rate_multiplier,
        api_key,
        key_was_created: created,
    })
}

/// 档位排序：倍率从低到高（便宜的在前），同倍率按分组 id 稳定排序。
///
/// 稳定性是必要的：顺序抖动会让 UI 里的档位每次刷新都换位置。
pub fn sort_tiers(tiers: &mut [Tier]) {
    tiers.sort_by(|a, b| {
        a.rate_multiplier
            .partial_cmp(&b.rate_multiplier)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.group_id.cmp(&b.group_id))
    });
}

/// 一条 codex provider 的稳定 id。
///
/// 由 `site_origin + group_id` 派生而不是随机生成：同一个分组重复 provision 必须得到同一个
/// provider（否则每次都新增一条，列表里堆满重复项）。
///
/// 前缀取自 [`managed::MANAGED_ID_PREFIX`](super::managed::MANAGED_ID_PREFIX)，不在这里写
/// 字面量 —— 它同时是各入口守卫的判据，两处各写一遍就迟早失配（见 [`super::managed`] 模块文档）。
pub fn provider_id_for(site_origin: &str, group_id: i64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(site_origin.as_bytes());
    h.update(b"/");
    h.update(group_id.to_string().as_bytes());
    // 取前 16 个 hex 字符：够避免碰撞，又不至于让 id 长得没法读。
    format!(
        "{}{:.16x}",
        crate::operator::managed::MANAGED_ID_PREFIX,
        h.finalize()
    )
}

/// provider 的展示名。
pub fn provider_display_name(site_name: &str, group_name: &str) -> String {
    if site_name.is_empty() {
        group_name.to_string()
    } else {
        format!("{site_name} · {group_name}")
    }
}

/// 生成 codex 的 `config.toml` 片段。
///
/// ## 四条硬要求，每条漏了都会静默走错
///
/// 1. **`model_provider = "custom"`**：它是 cc-switch 的会话历史桶标识 —— 所有 provider 都写
///    `custom`，切换分组后历史才在同一个列表里（需求里「聊天记录合并」靠的就是这个，不是
///    某个设置开关）。绝不能照抄 sub2api 面板给的模板（它写 `model_provider = "OpenAI"`）：
///    `openai` 在 cc-switch 的保留 id 列表里且比对**大小写不敏感**，照抄会让 bearer token
///    落到顶层而不是 provider 作用域，并且把桶从 `custom` 变成 `OpenAI`，历史就此分家。
///
/// 2. **不写 `requires_openai_auth`**（实测出来的，与上游预设相反）。上游第三方模板与
///    sub2api 面板模板都写 `requires_openai_auth = true`，那是给「sk 写进 auth.json」那条路
///    准备的。而 LoongPort 走的是「sk 只进 config.toml 的 `experimental_bearer_token`、
///    auth.json 全程不碰」——`codex doctor` 实测三组对照：
///
///    | 配置 | reachability mode | 实际打到哪 |
///    |---|---|---|
///    | `requires_openai_auth = true` + bearer token | **ChatGPT auth** | chatgpt.com（403，1 fail） |
///    | 无 `requires_openai_auth` + bearer token | provider auth | 运营商 `/v1`（200，0 fail） |
///    | `requires_openai_auth = true` + auth.json | API key auth | 运营商 `/v1`（200，0 fail） |
///
///    留着它 + 不写 auth.json 是唯一跑不通的组合：codex 会判成 ChatGPT 登录模式，去打
///    `chatgpt.com/backend-api` 然后报 credentials incomplete。
///
/// 3. **`disable_response_storage = true`**：不写它 codex 会发 `previous_response_id` 续接，
///    而 sub2api 的 HTTP 路径对非空 `previous_response_id` **直接 400**（只有 WebSocket v2
///    支持），不是静默忽略。
///
/// 4. **`base_url` 必须带 `/v1`**，见 [`crate::operator::api::codex_base_url`]。
pub fn codex_config_toml(display_name: &str, base_url: &str, model: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"model_provider = "custom"
model = {}
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = {}
base_url = {}
wire_api = "responses""#,
        q(model),
        q(display_name),
        q(base_url)
    )
}

/// 一条 codex provider 的 `settings_config`。
///
/// 形状必须是 `{"auth": {...}, "config": "<toml 字符串>"}` —— **`auth` 键缺失会让切换直接
/// 失败**（`write_live_snapshot` 的 Codex 分支硬要求它在）。
pub fn settings_config_for(
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
) -> serde_json::Value {
    serde_json::json!({
        "auth": { "OPENAI_API_KEY": api_key },
        "config": codex_config_toml(display_name, base_url, model),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: i64, name: &str, status: &str) -> ApiKey {
        ApiKey {
            id,
            key: format!("sk-{id}"),
            name: name.into(),
            status: status.into(),
        }
    }

    #[test]
    fn key_name_is_four_segments_with_platform() {
        assert_eq!(
            key_name_for("dev-1", "openai", 42),
            "LoongPort/dev-1/openai/42"
        );
    }

    #[test]
    fn claim_matches_exactly_not_by_prefix() {
        // 子串/前缀匹配会让 .../42 命中 .../420。服务端的 search 就是子串匹配，
        // 所以这道精确比对是唯一防线。
        let keys = vec![
            key(1, "LoongPort/dev-1/openai/420", "active"),
            key(2, "LoongPort/dev-1/openai/42", "active"),
        ];
        assert_eq!(claim_key(&keys, "dev-1", "openai", 42).unwrap().id, 2);
    }

    #[test]
    fn claim_never_crosses_devices() {
        // 「绝不动别的设备那把 Key」的正面测点。
        let keys = vec![key(1, "LoongPort/other-device/openai/42", "active")];
        assert!(claim_key(&keys, "dev-1", "openai", 42).is_none());
    }

    #[test]
    fn claim_never_crosses_platforms() {
        // platform 段存在的全部理由：分组 id 只在平台内唯一，跨平台会撞号。少了这一段，
        // codex 页与 claude 页的同号分组会互相顶掉对方的 Key（认领到别的平台那把 →
        // 写进 config 的 sk 属于错平台 → 调用失败）。
        let keys = vec![key(1, "LoongPort/dev-1/anthropic/42", "active")];
        assert!(claim_key(&keys, "dev-1", "openai", 42).is_none());
    }

    #[test]
    fn claim_skips_unusable_keys() {
        // 认领到废 Key 会形成环：调用失败 → 重新认领 → 又是同一把。
        let keys = vec![key(1, "LoongPort/dev-1/openai/42", "disabled")];
        assert!(claim_key(&keys, "dev-1", "openai", 42).is_none());
    }

    #[test]
    fn claim_takes_the_newest_when_duplicated() {
        // 服务端 name 无唯一约束，同名可以无限建。
        let keys = vec![
            key(1, "LoongPort/dev-1/openai/42", "active"),
            key(9, "LoongPort/dev-1/openai/42", "active"),
            key(5, "LoongPort/dev-1/openai/42", "active"),
        ];
        assert_eq!(claim_key(&keys, "dev-1", "openai", 42).unwrap().id, 9);
    }

    #[test]
    fn tiers_sort_cheapest_first_and_are_stable() {
        let mk = |id: i64, rate: f64| Tier {
            group_id: id,
            group_name: format!("g{id}"),
            rate_multiplier: rate,
            api_key: "sk".into(),
            key_was_created: false,
        };
        let mut tiers = vec![mk(3, 2.0), mk(1, 1.0), mk(2, 1.0)];
        sort_tiers(&mut tiers);
        assert_eq!(
            tiers.iter().map(|t| t.group_id).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "同倍率要按 id 稳定排序，否则 UI 里档位每次刷新都换位置"
        );
    }

    #[test]
    fn provider_id_is_stable_and_scoped_to_site() {
        let a = provider_id_for("https://bestapi.store", 42);
        // 稳定：重复 provision 必须得到同一个 id，否则列表里堆满重复项。
        assert_eq!(a, provider_id_for("https://bestapi.store", 42));
        assert_ne!(a, provider_id_for("https://bestapi.store", 43));
        // 不同站的同号分组必须不同 id。
        assert_ne!(a, provider_id_for("https://other.dev", 42));
        assert!(a.starts_with("loongport-"));
    }

    #[test]
    fn config_toml_uses_custom_provider_id_never_openai() {
        let toml = codex_config_toml("BestApi · Pro", "https://bestapi.store/v1", "gpt-5.6-sol");
        assert!(toml.contains(r#"model_provider = "custom""#));
        assert!(toml.contains("[model_providers.custom]"));
        // 这条钉住那个陷阱：sub2api 面板模板写的是 "OpenAI"，照抄会让 token 落到顶层
        // 且会话桶分家。
        assert!(!toml.contains("OpenAI\""), "{toml}");
        assert!(!toml.contains("[model_providers.OpenAI]"), "{toml}");
    }

    #[test]
    fn config_toml_has_the_mandatory_flags() {
        let toml = codex_config_toml("n", "https://x.dev/v1", "m");
        // 漏 disable_response_storage → codex 发 previous_response_id → sub2api 直接 400。
        assert!(toml.contains("disable_response_storage = true"));
        // sub2api 的 openai 网关原生走 responses，chat 是错的。
        assert!(toml.contains(r#"wire_api = "responses""#));
    }

    #[test]
    fn config_toml_must_not_declare_requires_openai_auth() {
        // 这条是 `codex doctor` 实测出来的，方向与上游预设**相反**，所以特别容易被
        // 「照抄上游模板」改回去。
        //
        // LoongPort 把 sk 放在 config.toml 的 experimental_bearer_token 里、不碰 auth.json。
        // 那种情况下声明 requires_openai_auth 会让 codex 判成 ChatGPT 登录模式，去打
        // chatgpt.com/backend-api 拿 403 并报 credentials incomplete —— 实测 1 fail。
        // 删掉它才走 provider auth 打运营商的 /v1（实测 0 fail）。
        let toml = codex_config_toml("n", "https://x.dev/v1", "m");
        assert!(
            !toml.contains("requires_openai_auth"),
            "声明了 requires_openai_auth 会让 codex 去打 chatgpt.com 而不是运营商: {toml}"
        );
    }

    #[test]
    fn config_toml_quotes_values_so_names_cannot_break_toml() {
        // 分组名来自服务端，含引号或反斜杠时不转义就会写出坏 TOML，切换时解析失败。
        let toml = codex_config_toml(r#"Pro "special" \ tier"#, "https://x.dev/v1", "m");
        let parsed: toml::Value = toml.parse().expect("生成的 TOML 必须可解析");
        assert_eq!(
            parsed["model_providers"]["custom"]["name"]
                .as_str()
                .unwrap(),
            r#"Pro "special" \ tier"#
        );
    }

    #[test]
    fn settings_config_always_carries_the_auth_key() {
        // auth 键缺失会让 write_live_snapshot 的 Codex 分支直接报错。
        let sc = settings_config_for("sk-abc", "n", "https://x.dev/v1", "m");
        assert_eq!(sc["auth"]["OPENAI_API_KEY"].as_str().unwrap(), "sk-abc");
        assert!(sc["config"].as_str().unwrap().contains("model_provider"));
    }

    #[test]
    fn display_name_falls_back_to_group_when_site_name_is_blank() {
        assert_eq!(provider_display_name("BestApi", "Pro"), "BestApi · Pro");
        assert_eq!(provider_display_name("", "Pro"), "Pro");
    }
}
