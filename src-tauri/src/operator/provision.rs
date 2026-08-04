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
//! LoongPort/a<account-id>/<platform>/<group-id>
//! ```
//!
//! 四段合起来表达「这个账号下、这个平台的、这个分组的、由 LoongPort 管理的一把 Key」。
//!
//! - **`account-id` 不能省**：见下面那节「为什么按账号而不按机器」。
//!   前缀 `a` 是分隔符的替代品 —— 纯数字段与 `group-id` 段形状相同，加个字母让人
//!   一眼看出哪段是账号（也顺带避免将来某天两段顺序写错还能解析成功）。
//! - **`platform` 不能省**：分组 id 只在平台内唯一，跨平台会撞号。第一版只展开 `openai`，
//!   但「站点 × 分组」页要按当前 tab 的平台展开 codex / claude / gemini —— 那时同号
//!   分组分属不同平台，靠三段名字认领会互相顶掉对方的 Key。
//! - **`group-id` 用数值 ID 不用分组名**：名字由运营商随时可改，改了就认领不到自己的 Key。
//! - **不带站点**：Key 天生就在某个站点名下（不同站点是不同的 sub2api 实例、
//!   各自一套数据库），名字里再写一遍是冗余。⚠️ 别与
//!   [`provider_id_for`] 混淆 —— **那个必须带 `site_origin`**，因为它是**本地** id，
//!   一个 DB 里要区分多个站点（曾经漏了 `account_id` 导致同站两账号互相覆盖，见那边的文档）。
//!
//! ## 为什么按账号而不按机器（2026-08-04 改）
//!
//! 原来第二段是 `device-id`，理由是「多台机器各认领自己那把，否则 A 机器改了 Key，
//! B 机器的配置就悄悄失效」。**那个理由站不住，而代价是 Key 爆炸**：
//!
//! - 实测维护者一个账号下堆了 **11 把**，其中只有 3 把在用 —— 剩下的分属一台已经不存在的
//!   机器、一种更早的命名格式、以及同机重复（那 3 把是 409 那轮留下的）。
//!   每接一台新机器 `+分组数` 把，而**旧机器的 Key 永远没人清**。
//! - 而它要防的「A 改了 B 失效」并不成立：`provision` **从不改动已有 Key**
//!   （认领到就直接用），能换掉 sk 的只有「用户去网页端手工删了重建」——
//!   那种情况下不论按机器还是按账号，其它机器都一样要重新 provision。
//!   用 device_id 换来的不是安全，只是**每台机器各堆一份**。
//!
//! 按账号之后，Key 总数 = 该账号的可用分组数（这里是 3），与机器数无关。
//!
//! ⚠️ **`account-id` 必须真的参与**：同一个站点上挂两个账号是本产品的核心能力，
//! 而 `list_keys` 是按用户隔离的 ⇒ 单看「认领」不带账号也不会串。但名字带上它是
//! **诊断需要**：用户在网页端看到一堆 Key 时，得能分清哪把属于哪个账号。
//!
//! ## ⚠️ 改名字的代价：旧 Key 认领不回来，会留一批孤儿
//!
//! 这个名字**进了服务端、是跨端可见的**，所以换命名格式等于「所有已建 Key 认领不回来」——
//! 下次 provision 会按新名字各建一把，旧的成为孤儿留在用户账号里。
//!
//! 本次改名是**知情选择**：孤儿是一次性的（每个账号 ≤ 已有机器数 × 分组数），
//! 而不改就是每加一台机器永久 +N。一次性清理由用户在网页端做，或将来加一个
//! 「清理其它机器的 Key」入口（见 `TODO.md`）。
//!
//! **别为了兼容旧名字做「读宽写窄」**：那意味着认领时要同时试 `device-id` 与
//! `account-id` 两种形状 ⇒ 认领到旧名字的 Key 之后它永远不会被换成新名字，
//! 于是两种格式长期共存、Key 爆炸的问题一点没解决 —— 那才是白改。
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
    /// 该写进这条档位配置的模型名。见 [`pick_model`]。
    ///
    /// **不是常量** —— 纯生图分组要写它自己的 `gpt-image-*`，写 [`DEFAULT_MODEL`]
    /// 会让它选中即 404。
    pub model: String,
    /// 服务端说这个分组允许生图（`allow_image_generation`）。
    ///
    /// 与「这是纯生图档位」是两件事，见 [`super::api::Group::allow_image_generation`]。
    pub allow_image_generation: bool,
}

/// 展开的整体结果。**失败项不阻断成功项**，两者都如实带出来。
#[derive(Debug, Default)]
pub struct ProvisionResult {
    /// 每个分组连带它该落到哪个 CLI（见 [`TargetedTier`]）。
    pub tiers: Vec<TargetedTier>,
    /// `(分组名, 失败原因)`。
    pub failures: Vec<(String, String)>,
}

/// 一个分组对应的 Key 名字。见模块文档「Key 命名契约」。
///
/// `account_id` 为 `None` 时用 `anon` —— 那是「登录了但还没回填账号 id」的窗口期
/// （`usable_operator` 会尽力补，但拉 profile 可能瞬时失败）。用一个固定值而不是
/// 跳过那一段：跳过会让名字少一段、与其它格式混起来更难认。
pub fn key_name_for(account_id: Option<i64>, platform: &str, group_id: i64) -> String {
    match account_id {
        Some(id) => format!("{MANAGED_PREFIX}/a{id}/{platform}/{group_id}"),
        None => format!("{MANAGED_PREFIX}/anon/{platform}/{group_id}"),
    }
}

/// 在已有 Key 里认领属于本账号 + 本分组的那把。
///
/// `list_keys` 的 `search` 是**子串匹配**（不是前缀），所以必须在客户端做精确比对 ——
/// 否则 `.../42` 会被 `.../420` 命中。
///
/// 命中多把时取 `id` 最大的那把（服务端 `name` 无唯一约束，同名可以无限建）。其余不自动删：
/// 删别人的东西要有更强的依据，这里只是「我认得出哪把是我的」。
pub fn claim_key<'a>(
    keys: &'a [ApiKey],
    account_id: Option<i64>,
    platform: &str,
    group_id: i64,
) -> Option<&'a ApiKey> {
    let want = key_name_for(account_id, platform, group_id);
    keys.iter()
        .filter(|k| k.name == want && k.is_usable())
        // 非 active 的不得认领：否则「认领到废 Key → 调用失败 → 再认领同一把」就是个环。
        .max_by_key(|k| k.id)
}

/// 为所有可用分组备好 sk。
/// 一个分组连带它该落到哪个 CLI。
///
/// 「哪个 CLI」由分组自己的 `platform` 决定（经 [`platform_map`](super::platform_map)），
/// **不是调用方指定的** —— 这是 2026-08-03 修的那个 bug 的核心：
/// 原来按「当前 tab」过滤分组，于是在 claude tab 点获取密钥会拉到 openai 分组、
/// 却写成 claude 的配置形状（openai 的 sk 配在 `ANTHROPIC_BASE_URL` 上，调用必失败）。
#[derive(Debug, Clone)]
pub struct TargetedTier {
    pub tier: Tier,
    /// 这个分组该落到哪个 CLI。
    pub app_type: AppType,
}

/// 为账号下**全部可用分组**备好 sk，各自带上它该落到的 CLI。
///
/// ## 语义：一次登录探全部平台，各归各的 tab
///
/// 用户原话：「注册/登录时默认探查所有可用分组下是否有对应的 sk，有则直接取、
/// 没有则自动创建，然后在对应的（codex / claude / …）下面展示这个站点的分组。」
///
/// 所以**不接受 app_type 参数** —— 分组落到哪个 CLI 由它自己的 `platform` 决定：
/// `openai → codex`、`anthropic → claude`、`gemini → gemini`、`grok → grokbuild`。
/// 认不出映射的（`antigravity` 还没接、`composite` 有意不做）直接跳过。
///
/// 这比「按当前 tab 拉」好在：用户在任何一个 tab 登录一次，全部平台的档位都备好了 ——
/// 不必为每个 tab 各点一次「获取密钥」，也不可能把某个平台的 sk 写成另一个平台的形状。
pub async fn provision(client: &Client) -> Result<ProvisionResult, AppError> {
    // 账号身份从 `client` 取，**不另收一个参数** —— 「用哪个账号建 Key」与
    // 「用哪个账号发请求」必须是同一个答案，两处各传一遍就可能不一致。
    let account_id = client.account_id();
    let groups = client.list_groups().await?;

    // 按分组自己的 platform 分派，认不出的跳过（不是错误：composite 是有意不做、
    // antigravity 是还没接，两者都不该让整个流程失败）。
    let usable: Vec<(Group, AppType)> = groups
        .into_iter()
        .filter_map(|g| {
            let app_type = super::platform_map::parse_platform(&g.platform)?.app_type()?;
            // 平台对上了还要过 is_usable_for 那几道（active、倍率不离谱）。
            g.is_usable_for(&app_type).then_some((g, app_type))
        })
        .collect();

    if usable.is_empty() {
        return Err(AppError::Config(
            "这个账号下没有本客户端支持的活跃分组".into(),
        ));
    }

    // 一次拉全量已有 Key，而不是每个分组各查一次：分组通常 1-5 个，一次拉回来在内存里比对
    // 更省请求，也避免撞面板的 240 次/分钟限流。
    let existing = client.list_keys(MANAGED_PREFIX).await?;
    // 认领的**上游输入**。它少了或空了，下面每个分组都会去新建 —— 而那是唯一
    // 会撞幂等冲突、会在用户账号里堆 sk 的路径，所以这个规模值得记一行。
    log::info!(
        "拉到 {} 把已有 Key（search={MANAGED_PREFIX}），待认领 {} 个分组",
        existing.len(),
        usable.len(),
    );

    let mut result = ProvisionResult::default();
    for (group, app_type) in usable {
        match ensure_key_for(client, account_id, &group, &existing).await {
            Ok(tier) => result.tiers.push(TargetedTier { tier, app_type }),
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
    account_id: Option<i64>,
    group: &Group,
    existing: &[ApiKey],
) -> Result<Tier, AppError> {
    let (api_key, created) = match claim_key(existing, account_id, &group.platform, group.id) {
        // 正常路径：认领到了就直接用，不发任何写请求。
        Some(k) => (k.key.clone(), false),
        None => {
            let name = key_name_for(account_id, &group.platform, group.id);
            // ⚠️ **「为什么没认领到」必须落日志**（维护者实测抓出）。
            //
            // 走到这一支就要发写请求（建 Key），而它是**唯一**会撞服务端幂等冲突、
            // 会在用户账号里堆 sk 的地方。可它原来一个字都不记 ⇒ 用户看到
            // 「创建密钥失败: HTTP 409」时，没人知道**本该认领的那把 Key 去哪了**。
            //
            // 那次定位（2026-08-03）花掉的正是这个信息：线上明明有一把同名的
            // active Key，而 `claim_key` 喂真实数据实测是认得出的 ⇒
            // 说明那一刻 `existing` 里没有它，而没有日志就查不出原因。
            //
            // 记 `existing` 的规模与**同前缀但没匹配上的那些名字** —— 后者是判据：
            // 若列表里压根没有同前缀的，是 `list_keys` 那步的问题（分页 / search /
            // 权限）；若有而没匹配上，是名字拼法或 `is_usable` 的问题。
            // **只记名字与 status，绝不记 `key` 字段**（那是明文 sk）。
            let same_prefix: Vec<String> = existing
                .iter()
                .filter(|k| k.name.starts_with(MANAGED_PREFIX))
                .map(|k| format!("{}[{}]", k.name, k.status))
                .collect();
            log::info!(
                "分组 {}（{}，platform={}）没认领到已有 Key，将新建。\
                 期望名字={name}；本次拉到 {} 把 Key，其中托管前缀的 {} 把：{:?}",
                group.id,
                group.name,
                group.platform,
                existing.len(),
                same_prefix.len(),
                same_prefix,
            );
            let created = client.create_key(&name, group.id).await?;
            if created.key.is_empty() {
                return Err(AppError::Config("服务端返回的密钥是空的".into()));
            }
            (created.key, true)
        }
    };

    // 拉这个分组能调哪些模型 —— 只为决定写什么模型名（纯生图分组必须写它自己的
    // `gpt-image-*`，写文本模型会 404）。
    //
    // ⚠️ **查失败不算错**：回落到 `DEFAULT_MODEL` = 本功能出现之前的行为。
    // 为一个「模型名可能不理想」中断整个分组的 provision 是把小问题放大成大问题
    // （用户会看到「获取密钥失败」而不是「某个档位模型名不对」）。
    let models = match super::api::list_models(client.site_origin(), &api_key).await {
        Ok(v) => v,
        Err(e) => {
            log::debug!(
                "分组 {}（{}）的模型列表拉不到，模型名回落默认值（不影响使用）: {e}",
                group.id,
                group.name,
            );
            None
        }
    };
    let model = pick_model(models.as_deref());
    if model != DEFAULT_MODEL {
        // 写了非默认模型是**要留痕的判断**：它决定这条档位能不能用，
        // 而判据（模型列表）是网络来的、事后无从复现。
        log::info!(
            "分组 {}（{}）是纯生图分组，模型名写 {model}（可选 {:?}）",
            group.id,
            group.name,
            models.as_deref().unwrap_or_default(),
        );
    }

    Ok(Tier {
        group_id: group.id,
        group_name: group.name.clone(),
        rate_multiplier: group.rate_multiplier,
        api_key,
        key_was_created: created,
        model,
        allow_image_generation: group.allow_image_generation,
    })
}

/// 档位排序：倍率从低到高（便宜的在前），同倍率按分组 id 稳定排序。
///
/// 稳定性是必要的：顺序抖动会让 UI 里的档位每次刷新都换位置。
pub fn sort_tiers(tiers: &mut [TargetedTier]) {
    tiers.sort_by(|a, b| {
        a.tier
            .rate_multiplier
            .partial_cmp(&b.tier.rate_multiplier)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.tier.group_id.cmp(&b.tier.group_id))
    });
}

/// 一条 codex provider 的稳定 id。
///
/// 由 `site_origin + account_id + group_id` 派生而不是随机生成：同一个分组重复
/// provision 必须得到同一个 provider（否则每次都新增一条，列表里堆满重复项）。
///
/// ## ⚠️ `account_id` 必须参与，否则同站多账号会互相覆盖
///
/// 曾经只用 `site_origin + group_id`，那是错的 —— **sub2api 的分组是站级实体**
/// （核对过上游 `backend/ent/schema/group.go`：`Group` 没有 `user_id` 字段，
/// 谁能用哪个分组由 `userallowedgroup` 表控制）⇒ 同一个站上两个账号看到的
/// `group_id` **必然重叠** ⇒ 两个账号算出**完全相同**的 provider id ⇒
/// 后 provision 的那个账号**静默覆盖**前一个的档位（连 sk 一起换掉）。
///
/// 那正好废掉「同站多账号」这个核心能力：库层面按 `(site_origin, account_id)`
/// 分得很干净，档位层面却退化成只按站点。
///
/// `None` = 那一行还没登录（`creds::Operator::account_id` 为 `NULL`）。
/// 未登录的行本来就 provision 不出档位（没有 token 拉不到分组），
/// 但签名要能表达这个状态 —— 用 `"anon"` 参与哈希而不是跳过，
/// 免得「未登录」与「account_id 恰好是某个值」撞到同一个 id 上。
///
/// 前缀取自 [`managed::MANAGED_ID_PREFIX`](super::managed::MANAGED_ID_PREFIX)，不在这里写
/// 字面量 —— 它同时是各入口守卫的判据，两处各写一遍就迟早失配（见 [`super::managed`] 模块文档）。
pub fn provider_id_for(site_origin: &str, account_id: Option<i64>, group_id: i64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(site_origin.as_bytes());
    h.update(b"/");
    // 分隔符不可省：没有它 `(account=1, group=23)` 与 `(account=12, group=3)`
    // 喂进哈希的字节流完全相同。
    match account_id {
        Some(id) => h.update(id.to_string().as_bytes()),
        None => h.update(b"anon"),
    }
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

/// 一条托管 provider 的 `settings_config`，**复用上游 deeplink 那套按 CLI 分派的构造**。
///
/// ## 为什么走 deeplink 而不是自己写一份
///
/// `deeplink::build_provider_from_request`（`deeplink/provider.rs:143`）**已经覆盖全部
/// 8 个 CLI**（claude / claudeDesktop / codex / gemini / grokbuild / opencode /
/// openclaw / hermes），而且是**上游维护的** —— 上游加第 9 个 CLI 时我们免费拿到。
///
/// 这条路是 sub2api 自己「导入到 cc-switch」那个按钮走的同一条（它生成
/// `ccswitch://v1/import?...`，见其前端 `KeysView` 的 `rs()` 函数），所以形状天然对齐。
/// 自己再写一份 match 等于把上游的维护责任接过来，而且会漏掉新增的 CLI。
///
/// ## ⚠️ 唯一要改的一行：`requires_openai_auth`
///
/// 上游 codex 那份（`build_codex_settings`，`:431`）写 `requires_openai_auth = true`，
/// 而我们实测那条**必须不写**：它是给「sk 走 auth.json」那条路准备的，而 LoongPort 走
/// 「sk 只进 config.toml 的 `experimental_bearer_token`、auth.json 全程不碰」——
/// 留着它 codex 会判成 ChatGPT 登录模式、去打 `chatgpt.com` 拿 403 报 credentials
/// incomplete（三组 `codex doctor` 对照见 [`codex_config_toml`] 的文档）。
///
/// 有意思的是**上游自己的实现就是这么设计的**（`codex_config.rs:2046` 的注释：
/// 「switching providers only needs to update config.toml; auth.json stays as the
/// user's long-lived ChatGPT login cache」）—— 那一行是 deeplink 与
/// `UniversalProvider` 两处的历史遗留，不是它的整体设计。
///
/// 所以 codex 仍走我们自己那份 [`codex_config_toml`]（测试
/// `config_toml_must_not_declare_requires_openai_auth` 钉着），
/// **其余 CLI 全部交给上游**。
///
/// ## 返回 `None` 的语义
///
/// 只在 `app_type` 是 codex 之外、而上游那套也构造失败时才返回 —— 实际上
/// `build_provider_from_request` 对 8 个 CLI 都有分支，所以这里几乎不会是 `None`。
/// 保留 `Option` 是让调用方那道闸有东西可判（"这个 CLI 接了没有"），
/// 而不必在两处硬编码同一份 CLI 清单。
pub fn settings_config_for(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
) -> Option<serde_json::Value> {
    // codex 例外：上游那份多一行 requires_openai_auth，见上面那段。
    if matches!(app_type, AppType::Codex) {
        return Some(serde_json::json!({
            "auth": { "OPENAI_API_KEY": api_key },
            "config": codex_config_toml(display_name, base_url, model),
        }));
    }

    // 其余交给上游 —— 构造一个等价于「导入到 cc-switch」那个 deeplink 的请求。
    let request = crate::deeplink::DeepLinkImportRequest {
        version: "v1".to_string(),
        resource: "provider".to_string(),
        app: Some(app_type.as_str().to_string()),
        name: Some(display_name.to_string()),
        endpoint: Some(base_url.to_string()),
        api_key: Some(api_key.to_string()),
        model: Some(model.to_string()),
        // ⚠️ **三个别名必须显式给**：上游只在请求里带了才写这几个 env
        // （`build_claude_settings` 的 `if let Some(haiku_model)`）。不给的话
        // Claude Code 会按 haiku/sonnet/opus 各自的默认名去请求，而运营商那边
        // 通常只认一个模型名 ⇒ 用户切到 sonnet 就报「模型不存在」。
        //
        // 全部指向同一个 model 而不是各给一个：运营商的分组是「一个 sk 一档价」，
        // 没有「便宜的 haiku、贵的 opus」这种分层，硬分会让用户以为能选。
        haiku_model: Some(model.to_string()),
        sonnet_model: Some(model.to_string()),
        opus_model: Some(model.to_string()),
        homepage: None,
        ..Default::default()
    };

    crate::deeplink::build_provider_from_request(app_type, &request)
        .ok()
        .map(|p| p.settings_config)
}

/// 这份 `settings_config` 是不是**被用户改过**（≠ 我们会生成的默认配置）。
///
/// ## 为什么是「跟默认值比对」，而不是存一个「用户编辑过」的标记
///
/// 需求是「用户手动保存过就算手动维护，进编辑页又取消不算」。存标记的话，置位时机
/// 只能挂在保存动作上，而保存走的是**上游** `updateProvider`（`App.tsx` 的
/// `handleEditProvider` → 上游 store）—— 要么改上游代码（扩大 merge 接触面，
/// 违反 CLAUDE.md §一），要么在前端猜「弹窗关闭是保存还是取消」（猜不准，
/// 而用户明确要求两者要分开）。
///
/// 比对没有这个问题，而且有两个额外好处：
///
/// 1. **自愈**：用户把配置手动改回默认值，标记自动消失。存标记会留一个永久假阳性
///    （显示「已手动维护」而其实跟默认一模一样），那种状态用户没法清除。
/// 2. **零存储**：不进 schema、不进 `ProviderMeta`（那个结构 `save_provider` 是
///    **全量覆盖**的，只保 `is_current` / `in_failover_queue` 两列 ——
///    标记要活下来就得指望每个写入方都原样带回 meta，多一个写入点就漏一处）。
///
/// ## 判据是「整份 JSON 相等」，而不是逐字段列白名单
///
/// 白名单要穷举「哪些字段算用户改动」，漏一个就是漏报（用户改了它却显示「默认」）。
/// 而默认配置的**全部**内容都由 [`settings_config_for`] 一个函数决定，
/// 拿它当基准做整份比对，新增字段自动纳入，不需要同步维护第二份清单。
///
/// **sk 不参与比对**：它每次 provision 都可能换（服务端重新签发），
/// 拿它比会让「刷新过密钥」被误报成「用户改过配置」。做法是把默认配置里的 sk
/// 换成现有配置里的那把，再比其余部分 —— 等价于「除 sk 之外都一样吗」。
///
/// ## 模型名同理：**换过默认模型不算用户改过**
///
/// `model` 是「当前版本的默认值」（`DEFAULT_MODEL`），而它的文档写着「运营商上新一代
/// 模型后该跟着调」。已存在的档位不会被 provision 改写模型名（只换 sk），所以那天一到，
/// **每一个现存档位的模型名都跟新基准不一致** ⇒ 全部集体显示「已手动维护」，
/// 而用户一个字都没改过。review 抓出的正是这条。
///
/// 解法与 sk 一致：**把已知的历史默认值也当作「未改过」**。
/// 见 [`HISTORICAL_DEFAULT_MODELS`] —— 改 `DEFAULT_MODEL` 时把旧值加进那个数组，
/// 就像 `LEGACY_OFFICIAL_PROXY_PROVIDER_IDS` 那样「读宽写窄」
/// （CLAUDE.md §三点五：识别认全部历史值、写入只产出当前值）。
///
/// 返回 `None` = **判不了**（这个 CLI 没有默认配置形状）。
/// 调用方应当据此**不显示任何标记**，而不是当成 `false`（那是在说「没改过」，
/// 而事实是「不知道」—— 断言一件不知道的事会让用户误信刷新不会覆盖）。
pub fn is_user_edited(
    settings_config: &serde_json::Value,
    app_type: &AppType,
    display_name: &str,
    base_url: &str,
    model: &str,
) -> Option<bool> {
    // ⚠️ **「这个 CLI 没接」与「sk 位置被改坏了」必须分开** —— review 抓出初版把两者
    // 混成同一个 `None`，而后者恰恰是「确定被改过」里最危险的一种：
    //
    // sk 读不出来 ⇒ 下次「获取密钥」时 `patch_api_key` 也会失败 ⇒ provision **回落到
    // 全量重写**（`do_provision` 里那段），用户的编辑被整份冲掉。而界面上因为是 `None`
    // 什么标记都没有 —— **恰好在编辑要被覆盖之前显示成「没改过」**。
    //
    // 所以判据分两层：
    // 1. 这个 CLI 有没有默认形状（`api_key_location` / `settings_config_for`）——
    //    没有才是真「判不了」，返回 `None`。
    // 2. 有形状但读不出 sk ⇒ 形状被动过了 ⇒ `Some(true)`（明确改过）。
    let (section, field) = api_key_location(app_type)?;

    let Some(api_key) = extract_api_key(settings_config, app_type) else {
        // 该放 sk 的位置不在了 / 类型不对 / 是空串。
        // 这是**用户动过配置**的确凿证据（生成的配置必然有它，
        // `settings_config_always_carries_the_auth_key` 钉着这条）。
        log::debug!(
            "配置里 {section}.{field} 读不出来，按「已手动维护」处理 —— \
             下次 provision 会把它整份重置"
        );
        return Some(true);
    };

    // 当前默认模型 + 全部历史默认值，任一匹配就算「没改过」。
    //
    // ⚠️ **`chain` 而不是只比 `model`** —— 见上面那段：`DEFAULT_MODEL` 改动那天，
    // 现存档位的模型名还是旧的（provision 只换 sk 不改模型），只比当前值会让
    // 全部档位集体误报「已手动维护」。
    let current = normalize_for_comparison(settings_config);
    let mut matched_any = false;
    for candidate in candidate_models(settings_config, model) {
        let defaults = settings_config_for(app_type, &api_key, display_name, base_url, &candidate)?;
        if current == normalize_for_comparison(&defaults) {
            matched_any = true;
            break;
        }
    }
    Some(!matched_any)
}

/// 比对基准里要试哪些模型名。
///
/// 顺序：调用方给的那个 → 全部历史默认值 → **配置里那个（仅当它是生图模型）**。
///
/// ## 最后那一项为什么必要，以及为什么必须限定条件
///
/// 生图档位的模型名由 [`pick_model`] 按**服务端的模型列表**定（如 `gpt-image-2`），
/// 那是一次网络查询的结果。而三个调用方里有两个拿不到它：
/// `list_operators_impl`（只读本地的首屏契约）与 `reset_tier_config`
/// （手上只有本地 `settings_config`）。它们只能传 [`DEFAULT_MODEL`] ⇒
/// **每个生图档位都会显示「已手动维护」**，而用户一个字没改过。
///
/// ⚠️ **绝不能无条件把配置里的模型名加进候选** —— 那样 `model` 这一行就
/// **结构上不可能不同**（它同时是被比对的值和生成基准的输入），于是
/// 「用户手改了模型名」永远检测不出来。`real_edits_are_detected` 的第一个 case
/// 钉的正是这条。
///
/// 所以限定 [`is_image_model`]：生图模型名是**我们自己按服务端数据写进去的**，
/// 用户手工填一个 `gpt-image-*` 进去当然也会被当成"没改过"—— 但那个值本就是
/// 这个档位的正解，把它判成「已手动维护」才是错的。而用户改成任何文本模型名
/// （`gpt-5.4` 之类）仍然照常检测得出来。
fn candidate_models(settings_config: &serde_json::Value, model: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(HISTORICAL_DEFAULT_MODELS.len() + 2);
    out.push(model.to_string());
    out.extend(HISTORICAL_DEFAULT_MODELS.iter().map(|m| m.to_string()));

    if let Some(in_config) = extract_model(settings_config) {
        if is_image_model(&in_config) && !out.contains(&in_config) {
            out.push(in_config);
        }
    }
    out
}

/// 从一份 `settings_config` 里读出 codex 的 `model`。
///
/// 与 [`extract_api_key`] 对称：都是「从一份配置里抠出一个我们自己写进去的值」。
///
/// 两个消费方：[`candidate_models`]（判「这是不是生图档位的模型名」）与
/// `list_operators_impl`（填 `TierInfo::is_image_model`，那条路只读本地）。
///
/// 只看 codex 的形状 —— 其它 CLI 的配置里没有 `gpt-image-*` 这回事。
///
/// **不引 toml 解析器**：要读的是 `key = "值"` 这种最简形状，由
/// [`codex_config_toml`] 生成，形状我们自己定的。
pub fn extract_model(settings_config: &serde_json::Value) -> Option<String> {
    let config = settings_config.get("config")?.as_str()?;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        // 严格相等：`model_provider` / `model_reasoning_effort` 都以 `model` 开头。
        if lhs.trim() != "model" {
            continue;
        }
        let value = rhs.trim();
        let unquoted = value.strip_prefix('"')?.strip_suffix('"')?;
        if unquoted.is_empty() {
            return None;
        }
        return Some(unquoted.to_string());
    }
    None
}

/// 比对前的归一化：抹掉「配置在系统里走一圈」会产生的无语义差异。
///
/// ## 为什么必须有这一步（实测出来的，review 抓出）
///
/// 切换档位会让配置**过一遍 `toml_edit`**：`ProviderService::switch` 往
/// `config.toml` 注入 `experimental_bearer_token`（live 写入），下一次切走时
/// 又把它摘掉写回 DB（`services/provider/mod.rs:3114` 的 backfill →
/// `codex_config::remove_codex_experimental_bearer_token`）。
///
/// 而 `DocumentMut::to_string()` **总会补一个尾换行**，我们的
/// [`codex_config_toml`] 生成的字符串却没有。实测：
///
/// ```text
/// orig len=219  back len=220   equal=false
/// orig tail="wire_api = \"responses\""
/// back tail="wire_api = \"responses\"\n"
/// ```
///
/// 不归一化的后果：**用户切过某个档位再切走，它就永久显示「已手动维护」**，
/// 而他从没打开过编辑页。更糟的是那个状态**清不掉** —— 点「恢复默认配置」
/// 能清一次，下次切走又脏，而标记宣称的「刷新不会覆盖你的改动」
/// 对一个没有任何改动的档位毫无意义。
///
/// ## 为什么是 `trim`，而不是「拿 toml_edit 再解析一遍」
///
/// 后者更彻底，但也更贵（每个档位每次列表刷新都解析一遍 TOML），而且它把
/// **本模块对 codex 格式的判断**换成了「toml_edit 认为等价就等价」——
/// 那会顺带忽略掉键序、空行、注释这些用户真改过的痕迹（用户加一行注释说明
/// 自己为什么改了某个值，是编辑过的证据）。
///
/// `trim` 只抹掉首尾空白：那是唯一**确定由系统产生、不由用户产生**的差异
/// （用户在编辑框里改配置不会只多一个尾换行 —— 而就算他真的只加了个换行，
/// 把那个当成「没改过」也是对的）。
///
/// ⚠️ **只归一化字符串叶子，不动结构**：多一个字段、少一个字段仍然算改过。
///
/// ## 已知不覆盖的一类
///
/// Claude 那条路上 `normalize_claude_models_in_value`（`mod.rs:2426`）会回填
/// `ANTHROPIC_DEFAULT_*` 三个键 —— 那是**结构变化**，`trim` 管不到。
/// 当前不受影响：`settings_config_for` 生成 claude 配置时已经显式写了那三个键
/// （见它里面 `haiku_model` / `sonnet_model` / `opus_model` 那段说明），
/// 所以回填是个空操作。真要变的那天，`the_verdict_survives_a_json_string_round_trip`
/// 那类闸抓不到它，得靠 claude 档位的实测 —— 记在这里免得将来当成新 bug 查一遍。
fn normalize_for_comparison(settings_config: &serde_json::Value) -> serde_json::Value {
    match settings_config {
        serde_json::Value::String(s) => serde_json::Value::String(s.trim().to_string()),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalize_for_comparison(v)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(normalize_for_comparison).collect())
        }
        other => other.clone(),
    }
}

/// 曾经作为默认模型写进档位配置的值。**只增不删。**
///
/// 每一项都对应「某个版本的用户机器上可能存在的默认配置」——
/// 与上游 `LEGACY_OFFICIAL_PROXY_PROVIDER_IDS` 同一个模式（CLAUDE.md §三点五
/// 「读宽写窄」：识别认全部历史值，写入只产出当前值）。
///
/// ## 改 [`DEFAULT_MODEL`](crate::commands::operator) 时必须把旧值加到这里
///
/// 不加的后果不是报错，是**静默误报**：已存在的档位不会被 provision 改写模型名
/// （只换 sk），于是它们的配置仍带着旧模型 ⇒ 跟新基准比对不上 ⇒
/// 界面上**每一个档位**都显示「已手动维护」+ 一个 amber 的恢复按钮，
/// 而用户一个字都没改过。他会以为是 bug，或者去点「恢复默认」把真默认值又写一遍。
///
/// 当前为空：`DEFAULT_MODEL` 自引入起没变过（`gpt-5.6-sol`）。
/// 空数组不是占位 —— 它让上面那个循环退化成「只比当前值」，与没有这个机制等价，
/// 而第一次改模型时加一项就自动生效。
pub(crate) const HISTORICAL_DEFAULT_MODELS: &[&str] = &[];

/// codex 的默认模型（**文本对话**用）。
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
///
/// ⚠️ **它不再无条件套给每条 codex 档位** —— 纯生图分组（`/v1/models` 里只有
/// `gpt-image-*`）写它就是必定 404。选哪个模型走 [`pick_model`]，本常量是那里的回落值。
///
/// ## ⚠️ 改这个值时必须把旧值加进 [`HISTORICAL_DEFAULT_MODELS`]
///
/// 已存在的档位**不会**被 provision 改写模型名（只换 sk），所以改了这个常量之后，
/// 它们的配置里还是旧模型 ⇒ 与「用户改过没有」的比对基准对不上 ⇒
/// **界面上每一个档位都显示「已手动维护」**，而用户一个字都没改过。
///
/// 那个数组就是为此存在的（「读宽写窄」：认全部历史值，写入只产出当前值）。
pub const DEFAULT_MODEL: &str = "gpt-5.6-sol";

/// 生图模型的名字前缀。
///
/// 与上游 sub2api 的 `isOpenAIImageGenerationModel`（`service/openai_images.go`：
/// `strings.HasPrefix(model, "gpt-image-")`）**逐字一致** —— 那是判据的来源，
/// 别在这里自造一套（如加上 `dall-e`：sub2api 不认它，我们认了只会写出转发不了的配置）。
const IMAGE_MODEL_PREFIX: &str = "gpt-image-";

/// 该给这条档位的 `config.toml` 写什么模型名。
///
/// ## 判据：这个分组有没有非生图模型
///
/// - 有文本模型（或问不出来）⇒ [`DEFAULT_MODEL`]，即本函数出现之前的行为
/// - **一个文本模型都没有**（全是 `gpt-image-*`）⇒ 其中排序最前的那个
///
/// ⚠️ **取真实值而不是硬编码 `"gpt-image-2"`**：运营商上 `gpt-image-3` 那天自动跟上，
/// 不必改代码。这与 [`DEFAULT_MODEL`] 那条「读宽写窄」的取舍不同 —— 那个值我们无从
///查证（要跨所有站点都可用），而这个值服务端刚刚告诉了我们。
///
/// ## 为什么排序后取第一个而不是随手取一个
///
/// `/v1/models` 的顺序**不保证稳定**（实测同一分组两次请求顺序不同）。不排序的话，
/// 同一个档位每次 provision 可能写进不同的模型名 ⇒ [`is_user_edited`] 的基准跟着抖 ⇒
/// 界面上「已手动维护」标记会随机出现又消失。排序让它成为该分组的一个确定函数。
///
/// ## 为什么「问不出来」回落到 [`DEFAULT_MODEL`] 而不是报错
///
/// `list_models` 可能因为站点没这个端点、权限不够、或临时故障而返回 `None`。
/// 那时回落到旧行为：**最坏情况是退化成现状**（纯生图分组写错模型名、选中 404），
/// 而报错会让整个「获取密钥」失败，把一个「某个档位模型名不对」放大成「一个档位都没有」。
pub fn pick_model(available: Option<&[String]>) -> String {
    let Some(models) = available else {
        return DEFAULT_MODEL.to_string();
    };
    // 有任何一个非生图模型 ⇒ 这不是纯生图分组，照旧写默认文本模型。
    if models.iter().any(|m| !m.starts_with(IMAGE_MODEL_PREFIX)) {
        return DEFAULT_MODEL.to_string();
    }
    models
        .iter()
        .min()
        .cloned()
        // 空列表在 `list_models` 里已经归成 `None` 了，走不到这里；
        // 真走到也回落默认值而不是 panic。
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

/// 这个模型名是生图模型吗（[`IMAGE_MODEL_PREFIX`] 前缀）。
///
/// UI 据此显示「生图档位」标记。判据放在**模型名**而不是「拉一次 `/v1/models` 看看」，
/// 是因为 `operator_list_operators` 那条路**只读本地不发网络**（首屏契约）——
/// 而模型名就在本地 `settings_config` 里，两条路都拿得到，无需异步填空。
pub fn is_image_model(model: &str) -> bool {
    model.starts_with(IMAGE_MODEL_PREFIX)
}

/// sk 在各 CLI 的 `settings_config` 里的位置。[`patch_api_key`] 与
/// [`extract_api_key`] 共用这一处定义 —— 两处各写一遍迟早分叉（一处改了另一处没改 ⇒
/// 写进去的和读出来的不是同一个字段）。
///
/// 返回 `None` = 这个 CLI 还没接。
fn api_key_location(app_type: &AppType) -> Option<(&'static str, &'static str)> {
    match app_type {
        AppType::Codex => Some(("auth", "OPENAI_API_KEY")),
        AppType::Claude => Some(("env", "ANTHROPIC_AUTH_TOKEN")),
        AppType::Gemini => Some(("env", "GEMINI_API_KEY")),
        _ => None,
    }
}

/// 从一份 `settings_config` 里读出 sk。
///
/// 供「恢复默认配置」用：那个操作要保留 sk 不变，所以得先把它取出来。
/// 返回 `None` 表示配置形状里找不到 sk（被改坏了 / 这个 CLI 还没接）——
/// 调用方应当报错而不是继续，生成一份没有 sk 的配置是条必定 401 的记录。
pub fn extract_api_key(settings_config: &serde_json::Value, app_type: &AppType) -> Option<String> {
    let (section, field) = api_key_location(app_type)?;
    settings_config
        .get(section)?
        .get(field)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 把新 sk 塞进一份**已存在的** `settings_config`，其余部分原样保留。
///
/// ## 为什么需要它：重复 provision 不该冲掉用户的编辑
///
/// `save_provider` 是**全量覆盖** `settings_config` 的（只保住 `is_current` 与
/// `in_failover_queue` 两列）。所以如果每次 provision 都写完整默认配置，用户在
/// cc-switch 的编辑页改过的模型名、reasoning effort、自定义端点，**点一次「获取密钥」
/// 就全没了** —— 而他很可能只是想刷新一下档位列表。
///
/// 于是分两种时机（用户定的）：
///
/// | 时机 | 行为 |
/// |---|---|
/// | 首次导入（provider 不存在） | 写 [`settings_config_for`] 的完整默认配置 |
/// | 重复 provision（已存在） | **只换 sk**，走本函数 |
/// | 「恢复默认」（用户显式点） | 再走一次完整默认配置 |
///
/// 编辑走 cc-switch 现成的编辑页，我们不自己做 —— 那页已经支持全部字段。
///
/// ## sk 在两种形状里的位置不同
///
/// - codex：`auth.OPENAI_API_KEY`
/// - claude：`env.ANTHROPIC_AUTH_TOKEN`
/// - gemini：`env.GEMINI_API_KEY`
///
/// 返回 `false` 表示**没找到该放 sk 的位置**（形状被用户改坏了，或这个 CLI 还没接）——
/// 调用方据此回落到「全量重写」，否则用户会拿着一把旧 sk 却以为刷新成功了。
pub fn patch_api_key(
    settings_config: &mut serde_json::Value,
    app_type: &AppType,
    api_key: &str,
) -> bool {
    let Some((section, field)) = api_key_location(app_type) else {
        return false;
    };

    // 只在那个 section 本来就是对象时改 —— 不存在就说明形状不对，让调用方全量重写，
    // 别在这里凭空造一个 section（那会拼出一份半新半旧的配置）。
    let Some(map) = settings_config
        .get_mut(section)
        .and_then(serde_json::Value::as_object_mut)
    else {
        return false;
    };
    map.insert(field.to_string(), serde_json::json!(api_key));
    true
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
    fn key_name_is_four_segments_with_account_and_platform() {
        assert_eq!(
            key_name_for(Some(13), "openai", 42),
            "LoongPort/a13/openai/42"
        );
        // 还没回填账号 id 的窗口期用固定的 `anon`，不省掉那一段
        // （少一段会让名字与其它格式混起来更难认）。
        assert_eq!(key_name_for(None, "openai", 42), "LoongPort/anon/openai/42");
    }

    /// ⭐ **Key 总数只跟分组数走，与机器数无关** —— 这条守的是「Key 爆炸」那个缺陷。
    ///
    /// 原来第二段是 `device-id`，于是每接一台新机器就在用户账号里多建一整套
    /// （实测维护者一个账号堆了 11 把、只有 3 把在用，其余分属一台已不存在的机器、
    /// 一种更早的命名格式、以及同机重复）。
    ///
    /// 判据就是「名字不含任何机器相关的东西」：同一个账号 + 同一个分组，
    /// 无论在哪台机器上算，都必须得到**同一个名字** —— 那样第二台机器
    /// 认领得到第一台建的那把，不会再建。
    ///
    /// 会红的改法：为了「多机隔离」把 device_id 加回名字里。
    #[test]
    fn the_key_name_does_not_depend_on_the_machine() {
        // 这个函数的入参里**压根没有**机器相关的东西 —— 这就是保证本身。
        // 同账号同分组连算两次必然相同（纯函数），所以真正要钉的是
        // 「名字里不出现 device / machine / host 这类段」。
        let name = key_name_for(Some(13), "openai", 42);
        assert_eq!(name.split('/').count(), 4, "四段：前缀/账号/平台/分组");
        for seg in name.split('/') {
            assert!(
                !seg.contains('-') || seg == "LoongPort",
                "段 {seg:?} 看着像 uuid/device_id —— 机器标识不该进 Key 名字，                 否则每台机器各建一套（Key 爆炸）"
            );
        }
        // 不同账号必须分开（同站多账号是核心能力）。
        assert_ne!(name, key_name_for(Some(60), "openai", 42));
    }

    #[test]
    fn claim_matches_exactly_not_by_prefix() {
        // 子串/前缀匹配会让 .../42 命中 .../420。服务端的 search 就是子串匹配，
        // 所以这道精确比对是唯一防线。
        let keys = vec![
            key(1, "LoongPort/a13/openai/420", "active"),
            key(2, "LoongPort/a13/openai/42", "active"),
        ];
        assert_eq!(claim_key(&keys, Some(13), "openai", 42).unwrap().id, 2);
    }

    #[test]
    fn claim_never_crosses_accounts() {
        // 「绝不动别的账号那把 Key」的正面测点。
        //
        // `list_keys` 本身按用户隔离 ⇒ 正常情况下别的账号那把压根不会出现在列表里。
        // 但名字带账号是**诊断需要**（用户在网页端要能分清哪把属于哪个账号），
        // 而既然带了，认领就必须严格比对 —— 否则那一段等于装饰。
        let keys = vec![key(1, "LoongPort/a60/openai/42", "active")];
        assert!(claim_key(&keys, Some(13), "openai", 42).is_none());
    }

    #[test]
    fn a_second_machine_claims_what_the_first_one_created() {
        // ⭐ **这条是「Key 不再爆炸」的行为测点**（上面那条守名字，这条守认领）。
        //
        // 场景：用户在 Mac 上 provision 过（建了 a13/openai/2），
        // 然后在 Windows 上登同一个账号 —— 必须**认领到那把**，而不是新建。
        // 原来按 device_id 命名时这里会认领不到，于是 Windows 各建一套（实测复现过）。
        let from_the_first_machine = vec![key(7, "LoongPort/a13/openai/2", "active")];
        let claimed = claim_key(&from_the_first_machine, Some(13), "openai", 2)
            .expect("第二台机器必须认领到第一台建的那把，否则每台机器各堆一套");
        assert_eq!(claimed.id, 7);
    }

    #[test]
    fn claim_never_crosses_platforms() {
        // platform 段存在的全部理由：分组 id 只在平台内唯一，跨平台会撞号。少了这一段，
        // codex 页与 claude 页的同号分组会互相顶掉对方的 Key（认领到别的平台那把 →
        // 写进 config 的 sk 属于错平台 → 调用失败）。
        let keys = vec![key(1, "LoongPort/a13/anthropic/42", "active")];
        assert!(claim_key(&keys, Some(13), "openai", 42).is_none());
    }

    #[test]
    fn claim_accepts_keys_with_empty_status_so_sk_never_piles_up() {
        // ⚠️ **这条防的是「sk 爆炸」**：`status` 带 serde(default)，运营商不返回该字段时
        // 它是空串。若判成不可用 ⇒ 认领必然失败 ⇒ **每次 provision 都新建一把**，
        // 而下次认领同样失败 ⇒ 用户账号里的 sk 单调增长，只能去网页端手工删。
        //
        // 两种误判的代价不对称（见 `ApiKey::is_usable` 的文档）：
        // 把废 Key 当好的 → 调用 401、点一次重建即可；
        // 把好 Key 当废的 → 反复新建，不可自愈。
        //
        // 实测 sub2api 会返回 status，这条是为别的运营商（如 new-api）字段不同时兜底。
        let keys = vec![key(1, "LoongPort/a13/openai/42", "")];
        assert!(
            claim_key(&keys, Some(13), "openai", 42).is_some(),
            "空 status 必须认领得到 —— 否则每次 provision 都会新建 sk"
        );
    }

    #[test]
    fn claim_skips_unusable_keys() {
        // 认领到废 Key 会形成环：调用失败 → 重新认领 → 又是同一把。
        let keys = vec![key(1, "LoongPort/a13/openai/42", "disabled")];
        assert!(claim_key(&keys, Some(13), "openai", 42).is_none());
    }

    #[test]
    fn claim_takes_the_newest_when_duplicated() {
        // 服务端 name 无唯一约束，同名可以无限建。
        let keys = vec![
            key(1, "LoongPort/a13/openai/42", "active"),
            key(9, "LoongPort/a13/openai/42", "active"),
            key(5, "LoongPort/a13/openai/42", "active"),
        ];
        assert_eq!(claim_key(&keys, Some(13), "openai", 42).unwrap().id, 9);
    }

    /// 纯生图分组（`/v1/models` 全是 `gpt-image-*`）要拿到它自己那个模型名。
    ///
    /// 写 `DEFAULT_MODEL` 的后果是**选中即 404** —— 那个分组根本没挂文本模型。
    /// 这是本功能存在的理由，必须钉住。
    #[test]
    fn an_image_only_group_gets_its_own_image_model() {
        let models = vec!["gpt-image-2".to_string()];
        assert_eq!(pick_model(Some(&models)), "gpt-image-2");
    }

    /// 取真实值而不是硬编码 `gpt-image-2` —— 运营商上新一代时要自动跟上。
    #[test]
    fn a_newer_image_model_is_picked_up_without_a_code_change() {
        let models = vec!["gpt-image-3".to_string()];
        assert_eq!(
            pick_model(Some(&models)),
            "gpt-image-3",
            "硬编码了 gpt-image-2，运营商上新一代就跟不上了"
        );
    }

    /// 有文本模型 ⇒ 这不是纯生图分组，照旧写默认文本模型。
    ///
    /// ⚠️ 混合分组（既有文本又有生图，如实测的 `pro池`）**必须走这条** ——
    /// 它的生图靠 sub2api 的 codex 生图桥（给文本请求注入 `image_generation` tool），
    /// 主模型写成 `gpt-image-*` 反而会把对话能力弄坏。
    #[test]
    fn a_group_with_text_models_keeps_the_default_text_model() {
        let mixed = vec![
            "gpt-image-2".to_string(),
            "gpt-5.6-sol".to_string(),
            "gpt-5.4".to_string(),
        ];
        assert_eq!(pick_model(Some(&mixed)), DEFAULT_MODEL);
    }

    /// 问不出模型列表 ⇒ 回落默认值（= 本功能出现之前的行为），不 panic 不报错。
    #[test]
    fn an_unknown_model_list_falls_back_to_the_default() {
        assert_eq!(pick_model(None), DEFAULT_MODEL);
        assert_eq!(
            pick_model(Some(&[])),
            DEFAULT_MODEL,
            "空列表也要回落 —— 否则会写一个空 model 出去"
        );
    }

    /// 多个生图模型时取值**稳定**。
    ///
    /// `/v1/models` 的顺序不保证稳定（实测同一分组两次请求顺序不同）。不排序的话同一个
    /// 档位每次 provision 可能写进不同模型名 ⇒ `is_user_edited` 的基准跟着抖 ⇒
    /// 界面上「已手动维护」标记随机出现又消失。
    #[test]
    fn picking_among_several_image_models_is_deterministic() {
        let a = vec![
            "gpt-image-2".to_string(),
            "gpt-image-1".to_string(),
            "gpt-image-1.5".to_string(),
        ];
        // 同一集合、不同顺序，必须得到同一个答案。
        let b = vec![
            "gpt-image-1.5".to_string(),
            "gpt-image-2".to_string(),
            "gpt-image-1".to_string(),
        ];
        assert_eq!(pick_model(Some(&a)), pick_model(Some(&b)));
    }

    /// 生图档位**不该**显示「已手动维护」。
    ///
    /// 两个调用方（`list_operators_impl` / `reset_tier_config`）拿不到分组数据，
    /// 只能传 `DEFAULT_MODEL` 当基准。不认配置里那个生图模型名的话，
    /// **每个生图档位都会挂上 amber 标记**而用户一个字没改过。
    #[test]
    fn an_image_only_tier_is_not_reported_as_user_edited() {
        let app = AppType::Codex;
        let base = "https://api.x.dev/v1";
        // provision 按 `pick_model` 写出来的那份配置。
        let cfg = settings_config_for(&app, "sk-1", "生图档", base, "gpt-image-2")
            .expect("codex 必须有默认形状");

        assert_eq!(
            // 调用方只能给 DEFAULT_MODEL —— 它拿不到分组的模型列表。
            is_user_edited(&cfg, &app, "生图档", base, DEFAULT_MODEL),
            Some(false),
            "生图档位被误报成「已手动维护」—— 每个生图档位都会挂 amber 标记"
        );
    }

    /// 但**用户真改了模型名**仍然要检测得出来。
    ///
    /// ⚠️ 这是上一条那个放宽的边界：若无条件把配置里的模型名当基准，
    /// `model` 那一行就结构上不可能不同，这条会静默失效。
    #[test]
    fn changing_the_model_to_another_text_model_is_still_detected() {
        let app = AppType::Codex;
        let base = "https://api.x.dev/v1";
        let mut cfg = settings_config_for(&app, "sk-1", "普通档", base, DEFAULT_MODEL)
            .expect("codex 必须有默认形状");
        // 用户在编辑页把模型换成了另一个文本模型。
        let config = cfg["config"].as_str().unwrap().replace(
            &format!("model = \"{DEFAULT_MODEL}\""),
            "model = \"gpt-4o\"",
        );
        cfg["config"] = config.into();

        assert_eq!(
            is_user_edited(&cfg, &app, "普通档", base, DEFAULT_MODEL),
            Some(true),
            "用户改了模型名却没被检测出来 —— 那个放宽条件放得太松了"
        );
    }

    /// `extract_model` 别把 `model_provider` 当成 `model`。
    ///
    /// 抠错了会拿 `"custom"` 去判 `is_image_model`（恒 false，于是上面那条放宽失效，
    /// 生图档位又开始误报），而没有任何东西会报错。
    #[test]
    fn extract_model_matches_the_whole_key_not_a_prefix() {
        let cfg = settings_config_for(
            &AppType::Codex,
            "sk",
            "t",
            "https://x.dev/v1",
            "gpt-image-2",
        )
        .expect("codex 必须有默认形状");
        assert_eq!(extract_model(&cfg).as_deref(), Some("gpt-image-2"));
    }

    /// `is_image_model` 是 UI 判据（显示「生图档位」标记），别把文本模型认成生图的。
    #[test]
    fn is_image_model_only_matches_the_image_prefix() {
        assert!(is_image_model("gpt-image-2"));
        assert!(is_image_model("gpt-image-3-turbo"));
        assert!(!is_image_model(DEFAULT_MODEL));
        assert!(!is_image_model("gpt-5.4-mini"));
        assert!(!is_image_model(""));
    }

    #[test]
    fn tiers_sort_cheapest_first_and_are_stable() {
        let mk = |id: i64, rate: f64| Tier {
            group_id: id,
            group_name: format!("g{id}"),
            rate_multiplier: rate,
            api_key: "sk".into(),
            key_was_created: false,
            // 排序只看倍率与 group_id，模型名与生图开关都不参与。
            model: DEFAULT_MODEL.into(),
            allow_image_generation: false,
        };
        let targeted = |id: i64, rate: f64| TargetedTier {
            tier: mk(id, rate),
            app_type: AppType::Codex,
        };
        let mut tiers = vec![targeted(3, 2.0), targeted(1, 1.0), targeted(2, 1.0)];
        sort_tiers(&mut tiers);
        assert_eq!(
            tiers.iter().map(|t| t.tier.group_id).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "同倍率要按 id 稳定排序，否则 UI 里档位每次刷新都换位置"
        );
    }

    #[test]
    fn provider_id_is_stable_and_scoped_to_site() {
        let a = provider_id_for("https://bestapi.store", Some(1), 42);
        // 稳定：重复 provision 必须得到同一个 id，否则列表里堆满重复项。
        assert_eq!(a, provider_id_for("https://bestapi.store", Some(1), 42));
        assert_ne!(a, provider_id_for("https://bestapi.store", Some(1), 43));
        // 不同站的同号分组必须不同 id。
        assert_ne!(a, provider_id_for("https://other.dev", Some(1), 42));
        // 走**真判据**而不是 `starts_with(前缀)` —— 判据已收紧成「前缀 + 16 位小写
        // hex」，只验前缀的断言比它弱，改坏格式时不会红（形状那条由
        // `generated_ids_always_have_exactly_sixteen_lowercase_hex_chars` 专门钉）。
        assert!(crate::operator::is_managed(&a), "id: {a}");
    }

    /// ⭐ **同一个站上两个账号的同号分组必须是不同的 provider。**
    ///
    /// 这是实测踩到的那类：sub2api 的分组是**站级实体**（上游
    /// `backend/ent/schema/group.go` 里 `Group` 没有 `user_id`，可用性由
    /// `userallowedgroup` 控制）⇒ 同站两个账号看到的 `group_id` **必然重叠**。
    /// 少了账号维度，两者算出同一个 id ⇒ 后 provision 的**静默覆盖**前一个的档位与 sk。
    #[test]
    fn provider_id_separates_two_accounts_on_the_same_site() {
        let site = "https://bestapi.store";
        let acct7 = provider_id_for(site, Some(7), 42);
        let acct9 = provider_id_for(site, Some(9), 42);
        assert_ne!(
            acct7, acct9,
            "同站不同账号的同号分组必须是两条 provider —— 否则后来的会覆盖先来的"
        );

        // 未登录（`None`）也要与任何已登录账号区分开。
        let anon = provider_id_for(site, None, 42);
        assert_ne!(anon, acct7);
        assert_ne!(anon, acct9);

        // ⚠️ 分隔符不能省：没有它 `(account=1, group=23)` 与 `(account=12, group=3)`
        // 喂进哈希的字节流完全相同 ⇒ 两个不相关的档位撞成一条。
        assert_ne!(
            provider_id_for(site, Some(1), 23),
            provider_id_for(site, Some(12), 3),
            "拼接必须有分隔符，否则 (1,23) 与 (12,3) 会撞号"
        );
    }

    /// ⭐ **生成的 id 恒是「前缀 + 16 位小写 hex」—— 那是托管判据的地基。**
    ///
    /// ## 为什么这条闸必须有
    ///
    /// `managed::is_managed` 从「只判前缀」收紧成「前缀 + 恰好 16 位小写 hex」之后，
    /// **这个格式成了契约**：某个 hash 值若产出 15 位或带大写，那条记录当场脱管 ——
    /// 守卫全线失效（能从托盘直接切、能被删）、且下次 provision 会为同一分组
    /// 再插一条新 id（旧的永远留在库里，不可见也不可删）。
    ///
    /// ## 它验的具体是什么
    ///
    /// `format!("{:.16x}", h.finalize())` 里的 `.16` 是**精度**，而精度对不同类型
    /// 语义不同：对**整数**它被忽略（`format!("{:.16x}", 1u128)` 得到 `"1"`，不补零
    /// 也不截断），对**字符串式 Display** 才是截断。sha256 的 `finalize()` 返回
    /// `GenericArray`，它的 `LowerHex` 按字节逐个输出两位十六进制（含前导零）⇒
    /// 全长恒 64 位 ⇒ 截断到 16 位恒成立。
    ///
    /// 这个推理链依赖第三方 crate 的 impl 细节（`generic-array` / `sha2`），
    /// 所以不能只靠读文档 —— 扫一批真实输入把它钉住。bump 那两个 crate 时若语义变了，
    /// 本条会红，而那正是需要被通知的时刻。
    #[test]
    fn generated_ids_always_have_exactly_sixteen_lowercase_hex_chars() {
        let prefix = crate::operator::managed::MANAGED_ID_PREFIX;

        // 扫一批输入：不同站点、账号（含未登录的 `None`）、分组号。
        // 2000 组足够覆盖「首字节为 0」这类前导零情形（概率 1/256，期望约 8 次）。
        for i in 0..2000i64 {
            for (site, account) in [("https://bestapi.store", Some(i)), ("https://x.dev", None)] {
                let id = provider_id_for(site, account, i);
                let hex = id
                    .strip_prefix(prefix)
                    .expect("id 必须带托管前缀，否则守卫认不出它");

                assert_eq!(
                    hex.len(),
                    16,
                    "hex 段不是 16 位 ⇒ 这条记录会脱管（守卫失效 + 重复插记录）：{id}"
                );
                // 大小写敏感是判据的一部分（`{:x}` 恒小写，放行大写会把判据
                // 重新放宽到用户填得出的形状上）。
                assert!(
                    hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                    "hex 段含非小写十六进制字符：{id}"
                );
                // 端到端：判据本身必须认它。
                assert!(
                    crate::operator::is_managed(&id),
                    "生成的 id 没被判据认出来：{id}"
                );
            }
        }

        // vendor 那支形状不同（多一段 `vendor-`），同样钉住。
        for i in 0..500 {
            let id = crate::vendor::provision::provider_id_for("deepseek", &format!("acct-{i}"));
            let hex = id
                .strip_prefix(prefix)
                .and_then(|r| r.strip_prefix("vendor-"))
                .unwrap_or_else(|| panic!("vendor id 形状变了：{id}"));
            assert_eq!(hex.len(), 16, "vendor 的 hex 段不是 16 位：{id}");
            assert!(
                crate::operator::is_managed(&id),
                "vendor id 没被认出来：{id}"
            );
        }
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

    /// ⭐ **我们那份 codex 模板必须等于上游那份减掉 `requires_openai_auth` 那一行。**
    ///
    /// ## 为什么负向闸不够
    ///
    /// 上面那条 `config_toml_must_not_declare_requires_openai_auth` 只断言「不含那个词」。
    /// 它挡得住「照抄上游把那行加回来」，但对**另一个方向完全无感**：
    /// 上游给模板加一个新键（或改某个键的值）时，我们这份手抄的副本不会跟上，
    /// 而两条闸都是绿的 ⇒ **静默漂移**。
    ///
    /// 上游那个文件（`deeplink/provider.rs`）近期改过 12 次，且这份模板同时供着
    /// operator 档位与 vendor 账号两条链（`vendor/provision.rs` 的 `provider_rows_for`
    /// 也走 `settings_config_for`）—— 漂移会一次打中两处。
    ///
    /// ## 判据
    ///
    /// 调上游的真函数拿它那份，逐行比对：**唯一允许的差异就是少了那一行**。
    /// 这样「我们有意偏离一行」从注释里的口头约定变成可执行断言。
    ///
    /// 会红的改法（两个方向都覆盖）：上游给模板加/改任何一个键；
    /// 我们这边改任何一个键的值或顺序。
    #[test]
    fn our_codex_toml_is_upstreams_minus_exactly_one_line() {
        const DISPLAY: &str = "Pro tier";
        const BASE_URL: &str = "https://ops.example.dev/v1";
        const MODEL: &str = "gpt-5-codex";

        // 上游那份 —— 走它自己的入口，不复制它的代码。
        let request = crate::deeplink::DeepLinkImportRequest {
            version: "v1".to_string(),
            resource: "provider".to_string(),
            app: Some("codex".to_string()),
            name: Some(DISPLAY.to_string()),
            endpoint: Some(BASE_URL.to_string()),
            api_key: Some("sk-test".to_string()),
            model: Some(MODEL.to_string()),
            ..Default::default()
        };
        let upstream = crate::deeplink::build_provider_from_request(&AppType::Codex, &request)
            .expect("上游必须能构造 codex provider");
        let upstream_toml = upstream.settings_config["config"]
            .as_str()
            .expect("上游那份要有 config 字符串");

        let ours = codex_config_toml(DISPLAY, BASE_URL, MODEL);

        // 逐行比：上游的行去掉 `requires_openai_auth` 那一行之后，必须与我们的逐行相同。
        let upstream_lines: Vec<&str> = upstream_toml
            .lines()
            .filter(|l| !l.contains("requires_openai_auth"))
            .map(|l| l.trim_end())
            .collect();
        let our_lines: Vec<&str> = ours.lines().map(|l| l.trim_end()).collect();

        // 上游那份结尾多一个换行（raw string 里带了），我们的没有 —— 掐掉尾随空行再比，
        // 免得这道闸被一个尾随空行绊住（那不是漂移）。
        fn strip_trailing_blanks(mut v: Vec<&str>) -> Vec<&str> {
            while v.last().is_some_and(|l| l.is_empty()) {
                v.pop();
            }
            v
        }
        let upstream_lines = strip_trailing_blanks(upstream_lines);
        let our_lines = strip_trailing_blanks(our_lines);

        assert_eq!(
            our_lines,
            upstream_lines,
            "codex 模板与上游漂移了。\n  \
             我们的:\n{ours}\n  \
             上游的（已滤掉 requires_openai_auth 行）:\n{}\n  \
             —— 要么上游加了新键我们没跟上（那要判断是否该跟），\
             要么我们改了不该改的地方。有意偏离请在这里写清并调整断言。",
            upstream_lines.join("\n")
        );

        // 顺带钉住「差异恰好是那一行」这件事本身：上游哪天自己删了它，
        // 我们这个例外就没有存在理由了，该收到通知。
        assert!(
            upstream_toml.contains("requires_openai_auth = true"),
            "上游那份不再声明 requires_openai_auth —— 我们那条例外的前提消失了，\
             可以直接复用上游模板，去掉 provision.rs 里这段手抄的副本"
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
        let sc = settings_config_for(&AppType::Codex, "sk-abc", "n", "https://x.dev/v1", "m")
            .expect("codex 必须有形状");
        assert_eq!(sc["auth"]["OPENAI_API_KEY"].as_str().unwrap(), "sk-abc");
        assert!(sc["config"].as_str().unwrap().contains("model_provider"));
    }

    #[test]
    fn patch_api_key_replaces_only_the_key_and_keeps_user_edits() {
        // 用户编辑过的配置：改了模型、加了自定义字段。
        let mut sc = settings_config_for(&AppType::Codex, "sk-old", "n", "https://x.dev/v1", "m")
            .expect("codex 必须有形状");
        sc["config"] = serde_json::json!("model = \"用户改过的模型\"\n自定义 = 1");
        sc["auth"]["用户加的字段"] = serde_json::json!("保留我");

        assert!(patch_api_key(&mut sc, &AppType::Codex, "sk-new"));

        // sk 换了。
        assert_eq!(sc["auth"]["OPENAI_API_KEY"], "sk-new");
        // **用户的编辑必须还在** —— 这条是这个函数存在的全部理由：
        // 重复 provision 走全量覆盖会把它们冲掉，而用户点「获取密钥」通常只想刷新列表。
        assert_eq!(sc["config"], "model = \"用户改过的模型\"\n自定义 = 1");
        assert_eq!(sc["auth"]["用户加的字段"], "保留我");
    }

    /// 刚生成的配置**不算**用户编辑过 —— 这是基线，红了说明比对基准跟生成器分叉了。
    #[test]
    fn a_freshly_generated_config_is_not_user_edited() {
        for app in [AppType::Codex, AppType::Claude, AppType::Gemini] {
            let sc = settings_config_for(&app, "sk-1", "名字", "https://x.dev/v1", "m")
                .unwrap_or_else(|| panic!("{} 应该有形状", app.as_str()));
            assert_eq!(
                is_user_edited(&sc, &app, "名字", "https://x.dev/v1", "m"),
                Some(false),
                "{} 的默认配置被判成用户改过了 —— 比对基准与生成器不是同一份？",
                app.as_str()
            );
        }
    }

    /// ⭐⭐ **切换过档位不算「用户改过」**（review 抓出的 P0，实测确认）。
    ///
    /// ## 这条闸走的是真实的 `toml_edit` 往返，不是手写一个尾换行
    ///
    /// 手写差异测的是「我以为漂移长什么样」；调真实函数测的是**它实际会怎么变**。
    /// 后者才守得住 —— `toml_edit` 升版换了格式化行为时这条会红。
    ///
    /// ## 那条链
    ///
    /// 切档位 → `ProviderService::switch` 往 `config.toml` 注入
    /// `experimental_bearer_token`（live 写入）→ 下次切走时 backfill 把它摘掉写回 DB
    /// （`services/provider/mod.rs:3114` → `remove_codex_experimental_bearer_token`）。
    /// 那一步过 `DocumentMut`，而 `to_string()` **总会补一个尾换行**，
    /// 我们生成的字符串没有 ⇒ 219 字节变 220。
    ///
    /// 不修的后果：**用户切过某档位再切走，它就永久显示「已手动维护」**，
    /// 而他从没打开过编辑页；点「恢复默认」能清一次，下次切走又脏。
    #[test]
    fn switching_to_a_tier_and_away_again_does_not_count_as_a_user_edit() {
        let name = "测试站 · pro池";
        let base_url = "https://x.dev/v1";
        let sc = settings_config_for(&AppType::Codex, "sk-1", name, base_url, "m")
            .expect("codex 必须有形状");

        // live 写入：往 `[model_providers.custom]` 里加 bearer token
        // （形状与 `set_codex_experimental_bearer_token` 一致，那个函数是私有的）。
        let config_text = sc["config"].as_str().expect("config 是字符串");
        let with_token = config_text.replace(
            r#"wire_api = "responses""#,
            "wire_api = \"responses\"\nexperimental_bearer_token = \"sk-1\"",
        );

        // backfill：摘掉 token 写回 DB。**这一步过 `DocumentMut`，漂移就在这儿产生。**
        let backfilled =
            crate::codex_config::remove_codex_experimental_bearer_token_if(&with_token, |_| true)
                .expect("摘除 token");

        // 先确认漂移**真的发生了** —— 否则这条测试是空转的（将来 toml_edit 不再加
        // 尾换行时它会提醒我们，那时归一化就成了没必要的代码）。
        assert_ne!(
            backfilled, config_text,
            "toml_edit 往返不再产生漂移了？那 `normalize_for_comparison` 可以简化"
        );

        // 而判据必须看穿它。
        let mut after_switch = sc.clone();
        after_switch["config"] = serde_json::json!(backfilled);
        assert_eq!(
            is_user_edited(&after_switch, &AppType::Codex, name, base_url, "m"),
            Some(false),
            "切换过的档位被误判成「用户改过」—— 用户从没打开编辑页，\
             而这个标记会永久挂着（点恢复默认能清一次，下次切走又脏）"
        );
    }

    /// 归一化**只抹首尾空白，不放过真实改动** —— 否则它会把缺陷藏起来。
    #[test]
    fn normalization_does_not_hide_real_edits() {
        let name = "n";
        let url = "https://x.dev/v1";
        let sc =
            settings_config_for(&AppType::Codex, "sk-1", name, url, "m").expect("codex 必须有形状");
        let text = sc["config"].as_str().expect("是字符串").to_string();

        // 内部的改动（不在首尾）必须仍算改过。
        let mut inner = sc.clone();
        inner["config"] = serde_json::json!(text.replace(
            "model_reasoning_effort = \"high\"",
            "model_reasoning_effort = \"low\""
        ));
        assert_eq!(
            is_user_edited(&inner, &AppType::Codex, name, url, "m"),
            Some(true),
            "改了 reasoning effort 被归一化吃掉了"
        );

        // 结构变化（多一个字段）也必须仍算改过 —— 归一化只碰字符串叶子。
        let mut extra = sc.clone();
        extra["自定义"] = serde_json::json!("x");
        assert_eq!(
            is_user_edited(&extra, &AppType::Codex, name, url, "m"),
            Some(true),
            "多一个字段被归一化吃掉了"
        );
    }

    /// ⭐ **换过默认模型的档位不算「用户改过」**（review 抓出的）。
    ///
    /// 场景：某版把 `DEFAULT_MODEL` 从 A 改成 B。已存在的档位配置里还是 A
    /// （provision 只换 sk、不改模型名），而基准用 B 重算 ⇒ 只比当前值的话
    /// **每一个现存档位都会显示「已手动维护」**，用户一个字没改过。
    ///
    /// 修法是 [`HISTORICAL_DEFAULT_MODELS`]「读宽写窄」。这条测试**不依赖那个数组
    /// 当前有没有内容** —— 它把「旧值」当参数传进去验机制本身，所以数组现在是空的
    /// 也测得到，而将来加了值这条依然成立。
    #[test]
    fn a_tier_still_on_a_previous_default_model_is_not_user_edited() {
        const OLD: &str = "gpt-5.5";
        const NEW: &str = "gpt-5.6-sol";

        // 档位是用旧默认模型生成的（用户从没动过它）。
        let existing = settings_config_for(&AppType::Codex, "sk-1", "n", "https://x.dev/v1", OLD)
            .expect("codex 必须有形状");

        // 直接比新基准 ⇒ 报「改过」。这是**没有历史值机制时的行为**，
        // 也就是 review 指出的那个缺陷 —— 先钉住它确实会发生。
        let naive = settings_config_for(&AppType::Codex, "sk-1", "n", "https://x.dev/v1", NEW)
            .expect("codex 必须有形状");
        assert_ne!(
            existing, naive,
            "换模型名确实会让配置不同 —— 所以必须有历史值兜底"
        );

        // 而**认历史值**之后就对了：把 OLD 当候选之一，判据必须是「没改过」。
        //
        // 这里直接验 `is_user_edited` 在「当前默认 = OLD」下的结果，等价于
        // HISTORICAL_DEFAULT_MODELS 里有 OLD 时的那条分支 ——
        // 循环对每个候选做的就是这件事。
        assert_eq!(
            is_user_edited(&existing, &AppType::Codex, "n", "https://x.dev/v1", OLD),
            Some(false),
            "配置与某个（历史）默认模型完全一致时必须判成「没改过」"
        );
    }

    /// [`HISTORICAL_DEFAULT_MODELS`] 里的每一项都必须**真的能生成出配置**。
    ///
    /// 手滑写错一个模型名（多空格、拼错）不会有任何报错 —— 那一项只是永远匹配不上，
    /// 于是「读宽」少读了一个历史值，而症状仍是那批档位集体误报「已手动维护」。
    #[test]
    fn every_historical_default_model_is_usable_as_a_baseline() {
        for old in HISTORICAL_DEFAULT_MODELS {
            assert!(
                !old.trim().is_empty() && old.trim() == *old,
                "历史模型名 {old:?} 有空白问题 —— 那一项会永远匹配不上"
            );
            let sc = settings_config_for(&AppType::Codex, "sk-1", "n", "https://x.dev/v1", old)
                .unwrap_or_else(|| panic!("历史模型 {old:?} 生成不出配置"));
            assert_eq!(
                is_user_edited(&sc, &AppType::Codex, "n", "https://x.dev/v1", "别的模型"),
                Some(false),
                "用历史模型 {old:?} 生成的配置必须被认成「没改过」"
            );
        }
    }

    /// ⚠️ **[`settings_config_for`] 必须是确定性的** —— 同输入必须同输出。
    ///
    /// 整个「跟默认值比对」的方案建立在这条性质上：基准每次都得重算，
    /// 里面只要掺进一个时间戳 / 随机数 / HashMap 迭代序，`is_user_edited` 就会
    /// **恒为 true** ⇒ 全部档位集体显示「已手动维护」，而没有任何东西会报错。
    ///
    /// 这个风险不是假想的：非 codex 的形状是委托上游
    /// `deeplink::build_provider_from_request` 造的，而**同一个文件里**
    /// （`deeplink/provider.rs:102`）就有一处 `Utc::now().timestamp_millis()`。
    /// 那处写的是 `provider.id`（在 `import_provider` 里，不在我们调的那个函数里），
    /// 所以当前是安全的 —— 但上游哪天把类似的东西挪进 `settings_config`，
    /// 这条测试会红，而没有它就只能靠用户报「怎么全都显示手动维护了」。
    ///
    /// 连比三次而不是两次：两次相同还可能是同一毫秒内的巧合。
    #[test]
    fn generating_the_same_config_twice_yields_identical_json() {
        for app in [AppType::Codex, AppType::Claude, AppType::Gemini] {
            let make = || {
                settings_config_for(&app, "sk-1", "名字", "https://x.dev/v1", "m")
                    .unwrap_or_else(|| panic!("{} 应该有形状", app.as_str()))
            };
            let (a, b, c) = (make(), make(), make());
            assert_eq!(a, b, "{} 的默认配置两次生成不一致", app.as_str());
            assert_eq!(b, c, "{} 的默认配置三次生成不一致", app.as_str());

            // 序列化后的**字节**也要一致 —— `Value` 相等但键序不同的话，
            // 比对本身没问题（`Value` 的 Eq 不看 Map 顺序），但落库再读回来
            // 会经过一轮字符串往返，那时顺序就参与了。
            assert_eq!(
                serde_json::to_string(&a).expect("可序列化"),
                serde_json::to_string(&c).expect("可序列化"),
                "{} 的默认配置序列化后不稳定（键序在变？）",
                app.as_str()
            );
        }
    }

    /// ⚠️ **`settings_config` 经过一轮「落库 → 读回」之后判据仍要成立。**
    ///
    /// 真实路径上，比对的一边是刚生成的 `Value`，另一边是**从 SQLite 的 TEXT 列
    /// 解析回来的** `Value` —— 中间过了 `to_string` / `from_str` 一个往返。
    /// 数字在那个往返里最容易变形（`1.0` ⇄ `1`、整数被解析成 `f64`），
    /// 而 `serde_json::Value` 的 `PartialEq` 对 `Number` 是按内部表示比的：
    /// `json!(1)` 与 `json!(1.0)` **不相等**。
    ///
    /// 当前的配置形状全是字符串，所以往返是安全的 —— 这条测试钉住那个事实，
    /// 将来谁往配置里加个数值字段（超时秒数、重试次数）就会在这里红，
    /// 而不是在用户那里表现成「全部档位莫名显示已手动维护」。
    #[test]
    fn the_verdict_survives_a_json_string_round_trip() {
        for app in [AppType::Codex, AppType::Claude, AppType::Gemini] {
            let fresh = settings_config_for(&app, "sk-1", "名字", "https://x.dev/v1", "m")
                .unwrap_or_else(|| panic!("{} 应该有形状", app.as_str()));

            // 模拟落库再读回。
            let stored = serde_json::to_string(&fresh).expect("可序列化");
            let reloaded: serde_json::Value = serde_json::from_str(&stored).expect("可解析");

            assert_eq!(
                is_user_edited(&reloaded, &app, "名字", "https://x.dev/v1", "m"),
                Some(false),
                "{} 的配置经过一轮 JSON 往返后被判成用户改过了 —— \
                 是不是往配置里加了数值字段？（Value 对 1 与 1.0 不相等）",
                app.as_str()
            );
        }
    }

    /// 换了 sk **不算**用户编辑。
    ///
    /// 这条最要紧：sk 每次 provision 都可能被服务端重新签发，拿它参与比对会让
    /// 「刷新了一次密钥」全部档位集体显示成「已手动维护」—— 而那会让用户以为
    /// 自己改过配置，进而去点「恢复默认」把真正的默认配置又重写一遍。
    #[test]
    fn a_rotated_api_key_is_not_a_user_edit() {
        let mut sc = settings_config_for(&AppType::Codex, "sk-old", "n", "https://x.dev/v1", "m")
            .expect("codex 必须有形状");
        assert!(patch_api_key(&mut sc, &AppType::Codex, "sk-brand-new"));

        assert_eq!(
            is_user_edited(&sc, &AppType::Codex, "n", "https://x.dev/v1", "m"),
            Some(false),
            "换 sk 被误判成用户编辑"
        );
    }

    /// 用户改过的每一类内容都要认出来。
    #[test]
    fn real_edits_are_detected() {
        let base = || {
            settings_config_for(&AppType::Codex, "sk-1", "n", "https://x.dev/v1", "m")
                .expect("codex 必须有形状")
        };

        // 改模型 / 删掉那三条硬要求 / 改 base_url —— 都是实际会出问题的改动。
        let mut changed_model = base();
        changed_model["config"] = serde_json::json!("model = \"别的模型\"");
        // 只删一行（`disable_response_storage`）也要认出来 —— 缺它 sub2api 会 400。
        let mut dropped_flag = base();
        dropped_flag["config"] = serde_json::json!(codex_config_toml("n", "https://x.dev/v1", "m")
            .replace("disable_response_storage = true\n", ""));
        // 加了自定义字段。
        let mut extra_field = base();
        extra_field["自定义"] = serde_json::json!(1);

        for (sc, label) in [
            (changed_model, "改了模型"),
            (dropped_flag, "删了 disable_response_storage"),
            (extra_field, "加了自定义字段"),
        ] {
            assert_eq!(
                is_user_edited(&sc, &AppType::Codex, "n", "https://x.dev/v1", "m"),
                Some(true),
                "{label}：没被认出来"
            );
        }
    }

    /// ⚠️ **判不了要返回 `None`，绝不能回落成 `false`。**
    ///
    /// `false` 是在断言「没改过、刷新不会覆盖你的东西」，而事实是「不知道」——
    /// 用户据此以为配置安全，下次 provision 才发现不是那样。
    ///
    /// `None` 只留给**这个 CLI 还没接**（没有默认形状可比）。
    #[test]
    fn an_unjudgeable_config_returns_none_not_false() {
        // 这个 CLI 还没接（`api_key_location` / `settings_config_for` 都没有它的形状）。
        let sc = serde_json::json!({"env": {"SOME_KEY": "sk-1"}});
        assert_eq!(
            is_user_edited(&sc, &AppType::OpenCode, "n", "https://x.dev/v1", "m"),
            None,
            "没接的 CLI 必须返回 None（真的判不了）"
        );
    }

    /// ⭐ **sk 位置被改坏 ⇒ `Some(true)`，不是 `None`。**（review 抓出的）
    ///
    /// 这两种情况初版混成了同一个 `None`，而它们的后果完全相反：
    ///
    /// 「CLI 没接」是真不知道；而「该放 sk 的位置不在了」是**用户动过配置的确凿证据**
    /// —— 生成的配置必然带着它（`settings_config_always_carries_the_auth_key` 钉着）。
    ///
    /// 更要紧的是这种档位的下一步：`patch_api_key` 同样读不出位置 ⇒ 下次「获取密钥」
    /// 时 provision **回落到全量重写**，用户的编辑被整份冲掉。而 `None` 让界面
    /// 什么标记都不显示 —— **恰好在编辑要被覆盖之前显示成「没改过」**。
    #[test]
    fn a_broken_api_key_slot_counts_as_edited_because_provision_will_wipe_it() {
        let base = || {
            settings_config_for(&AppType::Codex, "sk-1", "n", "https://x.dev/v1", "m")
                .expect("codex 必须有形状")
        };

        // 四种「读不出 sk」的形态，全都是用户动过的证据。
        let mut section_wrong_type = base();
        section_wrong_type["auth"] = serde_json::json!("不是对象");
        let mut section_gone = base();
        section_gone
            .as_object_mut()
            .expect("是对象")
            .remove("auth")
            .expect("原本有 auth");
        let mut field_gone = base();
        field_gone["auth"] = serde_json::json!({});
        let mut field_empty = base();
        field_empty["auth"]["OPENAI_API_KEY"] = serde_json::json!("");

        for (sc, label) in [
            (section_wrong_type, "auth 变成字符串"),
            (section_gone, "auth 段被删了"),
            (field_gone, "OPENAI_API_KEY 被删了"),
            (field_empty, "OPENAI_API_KEY 是空串"),
        ] {
            assert_eq!(
                is_user_edited(&sc, &AppType::Codex, "n", "https://x.dev/v1", "m"),
                Some(true),
                "{label}：必须算「已手动维护」—— 这种配置下次 provision 会被整份重置，\
                 而 None 会让界面在那之前显示成「没改过」"
            );
        }
    }

    #[test]
    fn patch_api_key_refuses_broken_shapes_instead_of_inventing_one() {
        // 该放 sk 的 section 不见了（用户改坏了）⇒ 返回 false 让调用方全量重写。
        // **不能凭空造一个 auth 段** —— 那会拼出半新半旧的配置，比重写更难查。
        let mut no_auth = serde_json::json!({ "config": "model = \"m\"" });
        assert!(!patch_api_key(&mut no_auth, &AppType::Codex, "sk-new"));
        assert!(no_auth.get("auth").is_none(), "不该凭空造出 auth 段");

        // section 存在但不是对象。
        let mut wrong_type = serde_json::json!({ "auth": "不是对象" });
        assert!(!patch_api_key(&mut wrong_type, &AppType::Codex, "sk-new"));

        // 还没接的 CLI。
        let mut sc = serde_json::json!({ "env": {} });
        assert!(!patch_api_key(&mut sc, &AppType::OpenCode, "sk-new"));
    }

    #[test]
    fn extract_api_key_round_trips_for_every_supported_cli() {
        // 「恢复默认」要先把 sk 读出来再塞回去 —— 读写必须认同一个字段。
        // 两处各写一遍字段名迟早分叉，所以它们共用 api_key_location；这条测试守住往返。
        for app_type in [AppType::Codex, AppType::Claude, AppType::Gemini] {
            let sc = settings_config_for(&app_type, "sk-abc", "n", "https://x.dev/v1", "m")
                .unwrap_or_else(|| panic!("{app_type:?} 必须有形状"));
            assert_eq!(
                extract_api_key(&sc, &app_type).as_deref(),
                Some("sk-abc"),
                "{app_type:?} 的 sk 写进去又读不出来 —— patch 与 extract 的字段对不上了"
            );
        }

        // 空 sk 当作「没有」：一份 sk 为空串的配置恢复默认后仍然不可用，
        // 该让调用方报错让用户走「获取密钥」。
        let mut blank =
            settings_config_for(&AppType::Codex, "", "n", "https://x.dev/v1", "m").unwrap();
        assert_eq!(extract_api_key(&blank, &AppType::Codex), None);
        blank["auth"] = serde_json::json!({});
        assert_eq!(extract_api_key(&blank, &AppType::Codex), None);
    }

    #[test]
    fn settings_config_shapes_match_upstream_for_claude_and_gemini() {
        // claude / gemini 有意与上游 `UniversalProvider::to_*_provider()` 一致 ——
        // 上游加第 9 个 CLI 时我们照它抄，这条钉住「现在是抄来的」这个事实。
        let claude =
            settings_config_for(&AppType::Claude, "sk-c", "n", "https://a.dev", "m").unwrap();
        assert_eq!(claude["env"]["ANTHROPIC_BASE_URL"], "https://a.dev");
        assert_eq!(claude["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-c");
        // 三个默认模型也照上游给：不给的话 Claude Code 会按各自默认名请求，
        // 而运营商通常只认一个模型名。
        assert_eq!(claude["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "m");

        let gemini =
            settings_config_for(&AppType::Gemini, "sk-g", "n", "https://g.dev", "m").unwrap();
        assert_eq!(gemini["env"]["GEMINI_API_KEY"], "sk-g");

        // codex **不能**照上游抄：上游那份写 requires_openai_auth = true，
        // 而我们实测那会让 codex 去打 chatgpt.com（见 codex_config_toml 的文档）。
        let codex =
            settings_config_for(&AppType::Codex, "sk-x", "n", "https://x.dev/v1", "m").unwrap();
        assert!(
            !codex["config"]
                .as_str()
                .unwrap()
                .contains("requires_openai_auth"),
            "codex 那份不能照上游抄 —— 那个字段会让它去打 chatgpt.com 拿 403"
        );

        // 改用上游 deeplink 那套之后，**8 个 CLI 全都有形状了** —— 这是复用带来的：
        // `build_provider_from_request` 覆盖 claude/claudeDesktop/codex/gemini/
        // grokbuild/opencode/openclaw/hermes。所以那道「这个 CLI 接了没有」的闸
        // （`do_provision` 里）现在实际上不会拦下任何 CLI。
        //
        // 留着 Option 与那道闸**不是多余**：它让「上游哪天新增一个 app_type
        // 而我们还没验证过它」这件事有地方表达，而不是静默生成一份没验证过的配置。
        for app_type in AppType::all() {
            assert!(
                settings_config_for(&app_type, "k", "n", "https://x.dev", "m").is_some(),
                "{app_type:?} 没有配置形状 —— 上游 build_provider_from_request 该覆盖它"
            );
        }
    }

    #[test]
    fn extract_api_key_is_the_only_way_to_read_sk_across_clis() {
        // 这条钉住一个**真踩过的坑**：`operator_list_tier_rates` 原本硬编码
        // `settings_config.auth.OPENAI_API_KEY` 去抠 sk —— 那是 codex 的位置，
        // claude/gemini 的 sk 不在那儿 ⇒ 那两个平台**永远查不到倍率**，
        // 而且是静默的（filter_map 直接跳过，用户只看到「倍率未知」）。
        //
        // 所以：任何要读 sk 的地方都必须走 extract_api_key。
        let codex_path = |sc: &serde_json::Value| {
            sc.get("auth")
                .and_then(|a| a.get("OPENAI_API_KEY"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };

        for app_type in [AppType::Claude, AppType::Gemini] {
            let sc = settings_config_for(&app_type, "sk-real", "n", "https://x.dev", "m")
                .unwrap_or_else(|| panic!("{app_type:?} 必须有形状"));

            // 硬编码 codex 路径读不到 —— 这正是原来那个 bug。
            assert_eq!(
                codex_path(&sc),
                None,
                "{app_type:?} 的 sk 不在 auth.OPENAI_API_KEY —— 硬编码那条路径会静默失败"
            );
            // 走 extract_api_key 就读得到。
            assert_eq!(extract_api_key(&sc, &app_type).as_deref(), Some("sk-real"));
        }
    }

    #[test]
    fn display_name_falls_back_to_group_when_site_name_is_blank() {
        assert_eq!(provider_display_name("BestApi", "Pro"), "BestApi · Pro");
        assert_eq!(provider_display_name("", "Pro"), "Pro");
    }
}
