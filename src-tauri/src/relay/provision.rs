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
//! - **`group-id` 用数值 ID 不用分组名**：名字由中转站随时可改，改了就认领不到自己的 Key。
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

use std::collections::HashSet;

use crate::app_config::AppType;
use crate::claude_desktop_config::ONE_M_CONTEXT_MARKER;
use crate::error::AppError;
use crate::relay::api::{ApiKey, Client, Group};

/// Key 名字的前缀，也是「这把 Key 由本客户端管理」的识别标志。
const MANAGED_PREFIX: &str = "LoongPort";

/// 从托管 key 名 `LoongPort/a<account_id>/<platform>/<group_id>` 里解析出
/// `(platform, group_id)`。解析不出（名字被外部改过 / 不是托管前缀）返回 `None`。
///
/// ⚠️ `parts[1]` 可能是 `a<account_id>` 或 `anon`（未登录时的兜底名字），
/// 两者都正好占一段，不影响后面取 `platform` / `group_id`。
fn parse_managed_key_name(name: &str) -> Option<(String, i64)> {
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() != 4 || parts[0] != MANAGED_PREFIX {
        return None;
    }
    let group_id = parts[3].parse::<i64>().ok()?;
    Some((parts[2].to_string(), group_id))
}

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
    /// claude 平台各角色模型（由 [`pick_tier_models`] 按该分组模型列表挑出）。
    ///
    /// 其余平台 `None`（它们的配置没有 haiku/sonnet/opus 这套角色别名）。
    pub roles: Option<ClaudeRoleModels>,
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
/// （`usable_relay` 会尽力补，但拉 profile 可能瞬时失败）。用一个固定值而不是
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

    // 「当前还存在哪些 (platform, group_id)」—— 用来判断哪些托管 key 成了孤儿。
    // ⚠️ 用**完整**分组列表而不是 usable：临时不可用的分组（维护中）key 还在被别的
    // 机器用，不该删；只有分组真的从列表里消失才算被删除。
    let current_groups: HashSet<(String, i64)> =
        groups.iter().map(|g| (g.platform.clone(), g.id)).collect();

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
        match ensure_key_for(client, account_id, &app_type, &group, &existing).await {
            Ok(tier) => {
                // **纯生图分组落到生图那一栏**，不是 codex。
                //
                // 判据用 `tier.model`（= [`pick_model`] 的产物）而不是分组的
                // `allow_image_generation`：后者只说「这个分组允许生图」，而**允许生图的
                // 混合分组仍然能聊天**（它有文本模型）—— 那种该留在 codex 栏。真正
                // 只能生图的是「一个文本模型都没有」，而那正是 `pick_model` 写出
                // `gpt-image-*` 的唯一条件。
                //
                // 分栏的理由见 [`AppType::CodexImage`](crate::app_config::AppType::CodexImage)：
                // 挤在 codex 栏里会让两者抢同一个 `is_current`，且 switch 的回填互相污染。
                let app_type = image_tier_app_type(&app_type, &tier.model);
                result.tiers.push(TargetedTier { tier, app_type })
            }
            // 一个分组失败不影响其它分组 —— 部分可用优于全部不可用。
            Err(e) => result.failures.push((group.name.clone(), e.to_string())),
        }
    }

    // 分组被删除 ⇒ 它的 sk 在服务端成了孤儿，顺手删掉（**含服务端那把**）。
    //
    // 已有分组的 key 只是认领、绝不重建/轮换（见 `ensure_key_for`）；这里只处理
    // 「名字能解析出 (platform, group_id) 且当前分组列表里已不存在」的 key。
    for key in &existing {
        let Some((platform, group_id)) = parse_managed_key_name(&key.name) else {
            continue;
        };
        if current_groups.contains(&(platform, group_id)) {
            continue;
        }
        match client.delete_key(key.id).await {
            Ok(()) => log::info!("删除已下架分组的密钥：{}", key.name),
            Err(e) => log::warn!("删除已下架分组的密钥 {} 失败: {e}", key.name),
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
    app_type: &AppType,
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
    let picked = pick_tier_models(app_type, models.as_deref());
    if picked.main != DEFAULT_MODEL {
        // 写了非默认模型是**要留痕的判断**：它决定这条档位能不能用，
        // 而判据（模型列表）是网络来的、事后无从复现。
        log::info!(
            "分组 {}（{}）模型名写 {}（来自模型列表，可选 {:?}）",
            group.id,
            group.name,
            picked.main,
            models.as_deref().unwrap_or_default(),
        );
    }

    Ok(Tier {
        group_id: group.id,
        group_name: group.name.clone(),
        rate_multiplier: group.rate_multiplier,
        api_key,
        key_was_created: created,
        model: picked.main,
        roles: picked.claude_roles,
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
/// `None` = 那一行还没登录（`creds::Relay::account_id` 为 `NULL`）。
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
        crate::relay::managed::MANAGED_ID_PREFIX,
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
///    | 无 `requires_openai_auth` + bearer token | provider auth | 中转站 `/v1`（200，0 fail） |
///    | `requires_openai_auth = true` + auth.json | API key auth | 中转站 `/v1`（200，0 fail） |
///
///    留着它 + 不写 auth.json 是唯一跑不通的组合：codex 会判成 ChatGPT 登录模式，去打
///    `chatgpt.com/backend-api` 然后报 credentials incomplete。
///
/// 3. **`disable_response_storage = true`**：不写它 codex 会发 `previous_response_id` 续接，
///    而 sub2api 的 HTTP 路径对非空 `previous_response_id` **直接 400**（只有 WebSocket v2
///    支持），不是静默忽略。
///
/// 4. **`base_url` 必须带 `/v1`**，见 [`crate::relay::api::codex_base_url`]。
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
/// Claude 各模型角色分别用哪个模型名。
///
/// ## 为什么需要这个，而不是让三个别名都等于主模型
///
/// [`settings_config_for`] 默认把 haiku / sonnet / opus 全指向同一个 `model`，
/// 理由写在那里：**中转站的分组是「一个 sk 一档价」**，没有「便宜的 haiku、贵的 opus」
/// 这种分层，硬分会让用户以为能选。
///
/// ⭐ **那条对中转站成立，对官网直连不成立。** DeepSeek 官方的 `deepseek-v4-pro`
/// 与 `-flash` 是**真实的两档模型、两个价格**，用户按角色分档是有意义的
/// （见 [`crate::vendor::deepseek::claude_role_models`]）。
///
/// ## 为什么必须走这个参数，不能在 vendor 层「后置 patch」
///
/// ⚠️ [`is_user_edited`] 内部调 [`settings_config_for`] **重算比对基准**。
/// 在 vendor 层生成完再补两个键的话，基准里没有它们 ⇒ **每个 DeepSeek 的 Claude
/// 档位都会误报「已手工维护」**，而用户一个字没改过。生成与基准必须走同一条路。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeRoleModels {
    pub haiku: String,
    pub sonnet: String,
    pub opus: String,
    pub fable: String,
    /// 写进 `CLAUDE_CODE_SUBAGENT_MODEL`。
    ///
    /// ⚠️ **这个键不在 `ANTHROPIC_DEFAULT_*` 系列里**，照抄前缀会写出一个
    /// Claude Code 不认的名字。
    pub subagent: String,
}

pub fn settings_config_for(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
) -> Option<serde_json::Value> {
    settings_config_with_roles(app_type, api_key, display_name, base_url, model, None)
}

/// [`settings_config_for`] 加一个「Claude 角色分档」的入口。
///
/// `roles = None` ⇒ 与 [`settings_config_for`] 完全等价（三别名全指主模型，
/// 不写 fable / subagent）。官网直连传 `Some(..)`，见 [`ClaudeRoleModels`]
/// 那段「为什么不能在 vendor 层后置 patch」。
///
/// `roles` 只对 Claude 系生效 —— 其余 CLI 没有这套角色别名，传了也无处可写。
pub fn settings_config_with_roles(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
    roles: Option<ClaudeRoleModels>,
) -> Option<serde_json::Value> {
    // codex 例外：上游那份多一行 requires_openai_auth，见上面那段。
    //
    // ⚠️ **生图栏必须与 codex 走同一条**（测试 `the_image_column_shares_the_codex_config_shape`
    // 钉着）：生图 MCP 按 codex 的形状去读 sk 与 base_url。掉进下面那条上游分支会
    // 得到一份 claude/gemini 形状的配置 ⇒ 生图在运行时读不出密钥，而那是只有真机
    // 才发现得了的失败。
    if matches!(app_type, AppType::Codex | AppType::CodexImage) {
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
        // ⚠️ **别名必须显式给**：上游只在请求里带了才写这几个 env
        // （`build_claude_settings` 的 `if let Some(haiku_model)`）。不给的话
        // Claude Code 会按 haiku/sonnet/opus 各自的默认名去请求，而中转站那边
        // 通常只认一个模型名 ⇒ 用户切到 sonnet 就报「模型不存在」。
        //
        // 默认（`roles = None`）全部指向同一个 model：中转站的分组是「一个 sk
        // 一档价」，没有「便宜的 haiku、贵的 opus」这种分层，硬分会让用户以为能选。
        // 官网直连例外 —— 见 [`ClaudeRoleModels`]。
        haiku_model: Some(
            roles
                .as_ref()
                .map_or(model, |r| r.haiku.as_str())
                .to_string(),
        ),
        sonnet_model: Some(
            roles
                .as_ref()
                .map_or(model, |r| r.sonnet.as_str())
                .to_string(),
        ),
        opus_model: Some(
            roles
                .as_ref()
                .map_or(model, |r| r.opus.as_str())
                .to_string(),
        ),
        // 这两个**只在分档时写**：`roles = None`（中转站）那条路保持原样，
        // 不给已有档位凭空多两个键 —— 那会让全部存量档位的整份比对失配，
        // 集体误报「已手工维护」。
        fable_model: roles.as_ref().map(|r| r.fable.clone()),
        subagent_model: roles.as_ref().map(|r| r.subagent.clone()),
        homepage: None,
        ..Default::default()
    };

    crate::deeplink::build_provider_from_request(app_type, &request)
        .ok()
        .map(|p| {
            let mut config = p.settings_config;
            // 只有 Claude Code（CLI）默认带 `language: chinese`：维护者要求 LoongPort
            // 生成的所有 Claude Code 配置默认中文（Claude Desktop 形状不同、不带这个键）。
            //
            // ⚠️ **加在生成侧，并让 `normalize_for_comparison` 在比对时剥掉它**（见
            // `is_user_edited` 文档）：language 是 LoongPort 托管的常开字段，不是用户编辑
            // 信号 —— 存量档位（生成时还没有这键）不该因此集体误报「已手动维护」。
            if matches!(app_type, AppType::Claude) {
                config["language"] = serde_json::json!("chinese");
            }
            config
        })
}

/// 从一份 `settings_config` 里读出 codex 的 `model`。
///
/// 与 [`extract_api_key`] 对称：都是「从一份配置里抠出一个我们自己写进去的值」。
///
/// 两个消费方：[`candidate_models`]（判「这是不是生图档位的模型名」）与
/// 数据库迁移（`database::loongport_schema` 里判「这条 codex 档位该搬去生图栏吗」）。
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
/// **这是「默认值」不是「唯一值」**：用户在 provider 编辑里能改，中转站上新一代模型后也该
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
/// 判据来自上游 sub2api 的 `IsGPTImageGenerationModel`（`service/openai_images.go`）——
/// **它先 `strings.ToLower` + `strings.TrimSpace` 再比前缀**，所以本仓的比较也必须
/// 归一化，见 [`is_image_model`]。别在这里自造一套（如加上 `dall-e`：sub2api 不认它，
/// 我们认了只会写出转发不了的配置）。
///
/// ⚠️ 有意**只对齐 GPT 那一族**，不含上游 `isOpenAIImageGenerationModel` 另外认的三个
/// grok 别名（`grok-imagine` / `-edit` / `-image*`）—— 生图工具只装在 codex 档位上
/// （openai 平台），grok 档位落的是另一个 CLI。
const IMAGE_MODEL_PREFIX: &str = "gpt-image-";

/// 一条档位该落到哪一栏：纯生图的进 [`AppType::CodexImage`]，其余原样返回。
///
/// ## 判据是模型名，不是分组的 `allow_image_generation`
///
/// 后者只说「这个分组**允许**生图」，而允许生图的**混合**分组仍然能聊天（它有文本模型）——
/// 那种该留在 codex 栏，用户既能用它对话也能用它出图。真正只能生图的是「一个文本模型都
/// 没有」，而那正是 [`pick_model`] 写出 `gpt-image-*` 的唯一条件 ⇒ 直接看它的产物。
///
/// ## 只对 codex 生效
///
/// `gpt-image-*` 是 openai 平台的事。claude / gemini / grok 的档位即便模型名恰好以
/// 那个前缀开头（不会发生，但判据不该依赖「不会发生」），也不该被搬去生图栏 ——
/// 生图工具走的是 `/v1/images/generations`，那是 openai 的端点。
///
/// ## 为什么不在 [`AppType`] 上做成方法
///
/// 它需要两个输入（当前 app_type + 模型名），而 `AppType` 是上游的类型 ——
/// 给它加一个只有 LoongPort 用得上的方法会扩大与上游的接触面（CLAUDE.md §一）。
pub fn image_tier_app_type(app_type: &AppType, model: &str) -> AppType {
    if matches!(app_type, AppType::Codex) && is_image_model(model) {
        AppType::CodexImage
    } else {
        app_type.clone()
    }
}

/// 该给这条档位的 `config.toml` 写什么模型名。
///
/// ## 判据：这个分组有没有非生图模型
///
/// - 有文本模型（或问不出来）⇒ [`DEFAULT_MODEL`]，即本函数出现之前的行为
/// - **一个文本模型都没有**（全是 `gpt-image-*`）⇒ 其中排序最前的那个
///
/// ⚠️ **取真实值而不是硬编码 `"gpt-image-2"`**：中转站上 `gpt-image-3` 那天自动跟上，
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
    // 走 `is_image_model` 而不是裸 `starts_with` —— 归一化在那个函数里，见它的文档。
    if !models.iter().all(|m| is_image_model(m)) {
        return DEFAULT_MODEL.to_string();
    }
    // 取**最新的那一代**，见 `image_model_rank`。
    // 空列表在 `list_models` 里已经归成 `None` 了，走不到这里；真走到也回落默认值。
    models
        .iter()
        .max_by(|a, b| {
            image_model_rank(a)
                .cmp(&image_model_rank(b))
                // 同代时按名字定序，让结果是该分组的一个确定函数（不随
                // `/v1/models` 的返回顺序抖动 —— 那个顺序实测不稳定，而抖动会让
                // `is_user_edited` 的基准跟着抖 ⇒ 「已手动维护」标记随机出现又消失）。
                .then_with(|| a.as_str().cmp(b.as_str()))
        })
        .cloned()
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

/// 一条档位该写进配置的模型：主模型 + claude 平台的角色模型。
///
/// `main` 是 codex 的 `config.toml` `model` / claude 的 `ANTHROPIC_MODEL`；
/// `claude_roles` 只在 claude 平台是 `Some`（其余 CLI 没有这套角色别名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierModels {
    pub main: String,
    pub claude_roles: Option<ClaudeRoleModels>,
}

/// claude 平台「最强档」（opus / fable / main）的候选模型，从高到低。
///
/// 中转站的分组是「一个 sk 一档价」，`/v1/models` 返回该 sk 可调的全部模型，
/// 家族由分组决定（claude 分组返回 claude-*、瓜子内部 api 的 gpt 分组返回 gpt-*）。
/// 同一档位里跨家族按优先级列出 —— 列表是 claude 家族就用 claude 模型、
/// 是 gpt 家族就按「opus↔sol」对齐。见 [`pick_tier_models`]。
const CLAUDE_OPUS_CANDIDATES: &[&str] = &["claude-opus-5", "gpt-5.6-sol", "deepseek-v4-pro"];

/// claude 平台「次强档」（sonnet / subagent）的候选模型。
const CLAUDE_SONNET_CANDIDATES: &[&str] =
    &["claude-sonnet-5", "gpt-5.6-terra", "deepseek-v4-flash"];

/// claude 平台「弱档」（haiku）的候选模型。
const CLAUDE_HAIKU_CANDIDATES: &[&str] = &["claude-haiku-4-5", "gpt-5.6-luna", "deepseek-v4-flash"];

/// codex 平台主模型的候选（从高到低）。
///
/// 首位是 [`DEFAULT_MODEL`]（查证过「全部可调度账号都支持」）。列表里没有它时
/// **顺延**而不是照旧写一个列表里不存在的模型 —— 那是选中即 404。
const CODEX_MAIN_CANDIDATES: &[&str] = &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.4"];

/// 一条档位的模型名：**按平台**从该分组模型列表里挑，而不是写死 [`DEFAULT_MODEL`]。
///
/// ## 为什么必须有（2026-08-08 修复）
///
/// 原 `pick_model` 只区分「纯生图 vs 有文本模型」，有文本一律写 `DEFAULT_MODEL`（openai
/// 专属值）。于是 claude 分组明明返回 12 个 claude 模型，档位却写 gpt-5.6-sol ——
/// 切过去 Claude Code 调一个中转站根本不提供的模型。修根：按 `app_type` 走各自的档位表。
///
/// ## 各平台规则
///
/// - **claude**：按角色档位（opus/fable/main、sonnet/subagent、haiku）从候选里取第一个
///   命中的；取不到顺延低档。列表是 gpt 家族时按「opus↔sol、sonnet↔terra、haiku↔luna」对齐。
///   挑出的模型名对支持 1M 的新一代模型附 `[1M]` 后缀声明（见 [`maybe_one_m`]）。
/// - **codex**：主模型候选顺延（首位 `DEFAULT_MODEL` 命中即保持现状）。
/// - **其它平台**：模型列表第一个文本模型。
/// - **纯生图分组**（只有 `gpt-image-*`）：仍写最新的生图模型（复用作 `pick_model`）。
/// - **列表拉不到**：回落 `DEFAULT_MODEL`（旧行为，最坏退化）。
pub fn pick_tier_models(app_type: &AppType, models: Option<&[String]>) -> TierModels {
    let Some(models) = models else {
        return TierModels {
            main: DEFAULT_MODEL.to_string(),
            claude_roles: None,
        };
    };
    // 纯生图分组（只有 gpt-image-*）：写它自己的生图模型，不分角色。
    if models.iter().all(|m| is_image_model(m)) {
        return TierModels {
            main: pick_model(Some(models)),
            claude_roles: None,
        };
    }
    match app_type {
        AppType::Claude => pick_claude_tier_models(models),
        AppType::Codex | AppType::CodexImage => TierModels {
            main: first_hit(CODEX_MAIN_CANDIDATES, models).unwrap_or_else(|| models[0].clone()),
            claude_roles: None,
        },
        _ => TierModels {
            main: models[0].clone(),
            claude_roles: None,
        },
    }
}

/// claude 平台的角色档位挑选：各角色取候选里第一个命中的，取不到顺延相邻档位。
///
/// 主模型（`ANTHROPIC_MODEL`）用最高档（opus）；某个角色 miss 时指向 main，保证
/// 结果可用且确定（同一列表每次挑出同一个值 —— `is_user_edited` 的比对基准不抖）。
fn pick_claude_tier_models(models: &[String]) -> TierModels {
    let first_text = models.iter().find(|m| !is_image_model(m)).cloned();
    let opus = first_hit(CLAUDE_OPUS_CANDIDATES, models);
    let sonnet = first_hit(CLAUDE_SONNET_CANDIDATES, models);
    let haiku = first_hit(CLAUDE_HAIKU_CANDIDATES, models);
    let main = opus
        .clone()
        .or_else(|| sonnet.clone())
        .or_else(|| first_text.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let opus = opus.unwrap_or_else(|| main.clone());
    let sonnet = sonnet.unwrap_or_else(|| main.clone());
    let haiku = haiku.unwrap_or_else(|| main.clone());
    // claude 平台档位声明 1M 上下文：对支持 1M 的模型附 `[1M]` 后缀。
    //
    // `[1M]` 是 Claude Code 认的本地能力声明（转发到上游前剥掉），
    // codex 档位的 config.toml 不认后缀 —— 所以只在这里（claude 平台）加。
    let one_m = |m: String| maybe_one_m(&m);
    TierModels {
        claude_roles: Some(ClaudeRoleModels {
            opus: one_m(opus.clone()),
            fable: one_m(opus),
            sonnet: one_m(sonnet.clone()),
            subagent: one_m(sonnet),
            haiku: one_m(haiku),
        }),
        main: one_m(main),
    }
}

/// 从候选列表里取第一个在模型列表里**精确存在**的模型名。
fn first_hit(candidates: &[&str], models: &[String]) -> Option<String> {
    candidates
        .iter()
        .find(|c| models.iter().any(|m| m == *c))
        .map(|c| c.to_string())
}

/// 该模型是否支持 1M 上下文 —— claude 平台档位给它附 `[1M]` 后缀声明。
///
/// 名单是**新一代旗舰模型**：Anthropic 官方这些模型统一 1M 窗口（opus-5 / sonnet-5 /
/// haiku-4-5 / fable-5）、gpt-5.6 是 OpenAI 新一代（用户确认瓜子 api 支持 1M）、
/// deepseek-v4 官网直连已确认（`vendor/deepseek.rs` 的 `PRO_1M`）。
///
/// ⚠️ `[1M]` 是**本地能力声明**：Claude Code 认它、转发到上游前剥掉
/// （`proxy/model_mapper.rs::strip_one_m_suffix_for_upstream`）。声明错了不会报错、
/// 只是让 Claude Code 按更大的窗口跑 —— 所以名单宁保守、别乱扩。
///
/// ⚠️ 只匹配**带子模型后缀**的形态（`gpt-5.6-sol` 等）：裸 `gpt-5.6` 不是可访问的
/// 模型 id（中转站只认 luna / sol / terra 这些子模型），不该被当成「它支持 1M」。
fn supports_one_m(model: &str) -> bool {
    const ONE_M_MODEL_PREFIXES: &[&str] = &[
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-4-5",
        "claude-fable-5",
        // 这两个家族的裸名（`gpt-5.6` / `deepseek-v4`）**不是可访问的模型 id** ——
        // 必须带子模型后缀（luna/sol/terra、pro/flash）。前缀带尾连字符，
        // 只命中子模型形态，不会把裸名误判成「它支持 1M」。
        "gpt-5.6-",
        "deepseek-v4-",
    ];
    let m = model.trim();
    ONE_M_MODEL_PREFIXES.iter().any(|p| m.starts_with(p))
}

/// claude 平台档位用：模型支持 1M 则附 `[1M]` 后缀，否则原样返回。
///
/// 后缀复用 [`crate::claude_desktop_config::ONE_M_CONTEXT_MARKER`]（小写 `[1m]`，
/// Claude Code 匹配大小写不敏感）。
fn maybe_one_m(model: &str) -> String {
    if supports_one_m(model) {
        format!("{model}{ONE_M_CONTEXT_MARKER}")
    } else {
        model.to_string()
    }
}

/// 生图模型的「代」，用于在多个 `gpt-image-*` 里挑最新的那个。
///
/// ## 为什么不能直接比字符串（review 抓出）
///
/// 原来这里是 `models.iter().min()`，而它选的是**字典序最小**的 ——
/// `min(["gpt-image-2", "gpt-image-3"])` 得到 `gpt-image-2`，即**最老的那一代**。
/// 而文档写着「中转站上 `gpt-image-3` 那天自动跟上」，正好相反。
/// （原来那条测试只喂了单元素列表，所以 `min` / `max` 都能过 —— 假绿。）
///
/// 换成 `max()` 也不对：字典序下 `"gpt-image-10" < "gpt-image-2"`。
///
/// 所以按数字段比：`gpt-image-1.5` → `[1, 5]`、`gpt-image-2` → `[2]`、
/// `gpt-image-10` → `[10]`。逐段比较，段数不同时短的算小（`1` < `1.5`）。
/// 认不出数字的排最后（那种名字我们无从判断新旧，让它输给能判的）。
fn image_model_rank(model: &str) -> Vec<u32> {
    let normalized = model.trim().to_ascii_lowercase();
    let Some(rest) = normalized.strip_prefix(IMAGE_MODEL_PREFIX) else {
        return Vec::new();
    };
    // `1.5-mini` → 只取前面连续的数字与点，后缀（`-mini` 之类）不参与比较。
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    version
        .split('.')
        .filter(|seg| !seg.is_empty())
        .filter_map(|seg| seg.parse::<u32>().ok())
        .collect()
}

/// 这个模型名是生图模型吗（[`IMAGE_MODEL_PREFIX`] 前缀）。
///
/// UI 据此显示「生图档位」标记。判据放在**模型名**而不是「拉一次 `/v1/models` 看看」，
/// 是因为 `relay_list_relays` 那条路**只读本地不发网络**（首屏契约）——
/// 而模型名就在本地 `settings_config` 里，两条路都拿得到，无需异步填空。
///
/// ## ⚠️ 必须先归一化再比前缀（review 抓出）
///
/// 上游 `IsGPTImageGenerationModel` 是 `ToLower` + `TrimSpace` 之后才比的。裸比前缀会在
/// **危险的方向**上失败：某个中转站的 `/v1/models` 若返回 `GPT-Image-2` 或
/// `" gpt-image-2"`，我们判它**不是**生图模型 ⇒ [`pick_model`] 以为「这个分组有文本
/// 模型」⇒ 写 [`DEFAULT_MODEL`] ⇒ **正是这套代码要修的那个 404 又回来了**。
///
/// 连带的第二个后果：本函数也是 [`candidate_models`] 那个放宽的闸，认不出来会让
/// 「已手动维护」的误报一起回来。
///
/// **只归一化比较，不改写要写入的值** —— 服务端给什么名字就照原样写进配置，
/// 那是它认得的形式。
pub fn is_image_model(model: &str) -> bool {
    model
        .trim()
        .to_ascii_lowercase()
        .starts_with(IMAGE_MODEL_PREFIX)
}

/// sk 在各 CLI 的 `settings_config` 里的字段路径。[`patch_api_key`]、
/// [`extract_api_key`] 与 [`ensure_api_key`] 共用这一处定义，避免读写逻辑各自维护一份。
///
/// 一个 CLI 可能有多个兼容字段（例如 Claude 的 token / api key）；它们都存在时会
/// 一起更新，避免运行时和倍率查询读到不同的凭据。
///
/// 返回 `None` = 这个 CLI 还没接。
fn api_key_locations(app_type: &AppType) -> Option<&'static [&'static [&'static str]]> {
    const CODEX: &[&str] = &["auth", "OPENAI_API_KEY"];
    const CLAUDE_AUTH_TOKEN: &[&str] = &["env", "ANTHROPIC_AUTH_TOKEN"];
    const CLAUDE_API_KEY: &[&str] = &["env", "ANTHROPIC_API_KEY"];
    const GEMINI: &[&str] = &["env", "GEMINI_API_KEY"];
    const HERMES: &[&str] = &["api_key"];
    const OPENCLAW: &[&str] = &["apiKey"];
    const OPENCODE: &[&str] = &["options", "apiKey"];

    match app_type {
        // 生图栏与 codex 同形（见 `settings_config_for`），sk 在同一个位置。
        // 漏了它的后果是**静默的**：缺少路径会让 `is_user_edited` 对每条生图档位
        // 都返回「判不了」，`extract_api_key` 也读不出 sk ⇒ 生图工具起不来。
        AppType::Codex | AppType::CodexImage => Some(&[CODEX]),
        // ⚠️ **ClaudeDesktop 与 Claude 同形，两个都要在这里**（2026-08-05 补）。
        //
        // 它们走同一个 `deeplink::build_claude_settings`（`provider.rs:165` 的
        // `AppType::Claude | AppType::ClaudeDesktop =>`），sk 都落在
        // `env.ANTHROPIC_AUTH_TOKEN`。漏掉 ClaudeDesktop 的后果与漏掉生图栏那条
        // 完全一样、而且**同样是静默的**：
        // `is_user_edited` 恒为「判不了」⇒ 界面上永远不显示「已手动维护」标记；
        // `extract_api_key` 读不出 sk ⇒ 「恢复默认配置」直接报错。
        AppType::Claude | AppType::ClaudeDesktop => Some(&[CLAUDE_AUTH_TOKEN, CLAUDE_API_KEY]),
        AppType::Gemini => Some(&[GEMINI]),
        AppType::Hermes => Some(&[HERMES]),
        AppType::OpenClaw => Some(&[OPENCLAW]),
        AppType::OpenCode => Some(&[OPENCODE]),
        AppType::GrokBuild => None,
    }
}

fn value_at_path<'a>(root: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    path.iter().try_fold(root, |value, key| value.get(*key))
}

fn object_at_parent_path<'a>(
    root: &'a mut serde_json::Value,
    path: &[&str],
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let (_, parent) = path.split_last()?;
    let parent = parent
        .iter()
        .try_fold(root, |value, key| value.get_mut(*key))?;
    parent.as_object_mut()
}

fn ensure_object_at_parent_path<'a>(
    root: &'a mut serde_json::Value,
    path: &[&str],
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let (_, parent) = path.split_last()?;
    let mut current = root.as_object_mut()?;
    for key in parent {
        let value = current
            .entry((*key).to_string())
            .or_insert_with(|| serde_json::json!({}));
        current = value.as_object_mut()?;
    }
    Some(current)
}

/// 从一份 `settings_config` 里读出 sk。
///
/// 供「恢复默认配置」用：那个操作要保留 sk 不变，所以得先把它取出来。
/// 返回 `None` 表示配置形状里找不到 sk（被改坏了 / 这个 CLI 还没接）——
/// 调用方应当报错而不是继续，生成一份没有 sk 的配置是条必定 401 的记录。
pub fn extract_api_key(settings_config: &serde_json::Value, app_type: &AppType) -> Option<String> {
    api_key_locations(app_type)?.iter().find_map(|path| {
        value_at_path(settings_config, path)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
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
    let Some(locations) = api_key_locations(app_type) else {
        return false;
    };

    // 所有已经存在的候选字段都改成同一把 key。Claude 配置若意外同时含两种字段，
    // 只改一个会让运行时与倍率查询各读到不同的凭据。
    let mut patched = false;
    for path in locations {
        if let Some(map) = object_at_parent_path(settings_config, path) {
            let field = *path.last().expect("API key path is not empty");
            if map.contains_key(field) {
                map.insert(field.to_string(), serde_json::json!(api_key));
                patched = true;
            }
        }
    }
    if patched {
        return true;
    }

    // section 存在但 key 被用户删掉时，补回默认字段，避免下一次倍率查询丢凭据。
    let Some(map) = object_at_parent_path(settings_config, locations[0]) else {
        return false;
    };
    let field = *locations[0].last().expect("API key path is not empty");
    map.insert(field.to_string(), serde_json::json!(api_key));
    true
}

/// 确保手工编辑后的托管档位仍带有由 LoongPort 管理的 sk。
///
/// 与 [`patch_api_key`] 的区别是：编辑器可能把整个认证 section 删掉；此时仍应把
/// 托管凭据补回去，而不是让倍率/连通检测静默失效。若根配置不是对象，返回 `false`
/// 交给调用方报出明确错误。
pub fn ensure_api_key(
    settings_config: &mut serde_json::Value,
    app_type: &AppType,
    api_key: &str,
) -> bool {
    if patch_api_key(settings_config, app_type, api_key) {
        return true;
    }

    let Some(path) = api_key_locations(app_type).and_then(|locations| locations.first().copied())
    else {
        return false;
    };
    let Some(map) = ensure_object_at_parent_path(settings_config, path) else {
        return false;
    };
    let field = *path.last().expect("API key path is not empty");
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

    /// ⭐ `parse_managed_key_name` 是「删下架分组服务端 sk」的判据 —— 解析错了
    /// 会把还在用的 key 删掉。与 [`key_name_for`] 对偶：生成什么就解得回什么。
    #[test]
    fn managed_key_name_round_trips_platform_and_group_id() {
        assert_eq!(
            parse_managed_key_name(&key_name_for(Some(13), "openai", 42)),
            Some(("openai".to_string(), 42))
        );
        assert_eq!(
            parse_managed_key_name(&key_name_for(None, "anthropic", 9)),
            Some(("anthropic".to_string(), 9))
        );
        // 解析不出的一律跳过（宁可不删也不能误删）。
        assert_eq!(
            parse_managed_key_name("LoongPort/a13/openai/not-a-number"),
            None
        );
        assert_eq!(parse_managed_key_name("bestapi-xxx"), None);
        assert_eq!(parse_managed_key_name("LoongPort/a13/openai"), None);
        assert_eq!(parse_managed_key_name("LoongPort/a13"), None);
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
        // ⚠️ **这条防的是「sk 爆炸」**：`status` 带 serde(default)，中转站不返回该字段时
        // 它是空串。若判成不可用 ⇒ 认领必然失败 ⇒ **每次 provision 都新建一把**，
        // 而下次认领同样失败 ⇒ 用户账号里的 sk 单调增长，只能去网页端手工删。
        //
        // 两种误判的代价不对称（见 `ApiKey::is_usable` 的文档）：
        // 把废 Key 当好的 → 调用 401、点一次重建即可；
        // 把好 Key 当废的 → 反复新建，不可自愈。
        //
        // 实测 sub2api 会返回 status，这条是为别的中转站（如 new-api）字段不同时兜底。
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

    /// 取真实值而不是硬编码 `gpt-image-2` —— 中转站上新一代时要自动跟上。
    ///
    /// ⚠️ **必须喂多元素列表**：单元素时 `min` 与 `max` 都能过，那样这条测试就是假绿
    /// （review 抓出 —— 原来它只喂一个，而实现是 `min()`，即选**最老**的那一代）。
    #[test]
    fn a_newer_image_model_is_picked_up_without_a_code_change() {
        // 中转站加了新一代、同时留着老的 —— 最现实的情形。
        let both = vec!["gpt-image-2".to_string(), "gpt-image-3".to_string()];
        assert_eq!(
            pick_model(Some(&both)),
            "gpt-image-3",
            "选了旧的那一代 —— 「中转站上新一代自动跟上」这个承诺没兑现"
        );
    }

    /// 版本号按**数字**比，不按字典序 —— 否则 `gpt-image-10 < gpt-image-2`。
    #[test]
    fn image_model_versions_compare_numerically_not_lexically() {
        let two_vs_ten = vec!["gpt-image-2".to_string(), "gpt-image-10".to_string()];
        assert_eq!(
            pick_model(Some(&two_vs_ten)),
            "gpt-image-10",
            "字典序把 gpt-image-10 排在 gpt-image-2 前面了"
        );
        // 小数段：1.5 比 1 新、比 2 旧。
        let minor = vec!["gpt-image-1".to_string(), "gpt-image-1.5".to_string()];
        assert_eq!(pick_model(Some(&minor)), "gpt-image-1.5");
        let across = vec!["gpt-image-1.5".to_string(), "gpt-image-2".to_string()];
        assert_eq!(pick_model(Some(&across)), "gpt-image-2");
    }

    /// 认不出版本号的排最后 —— 让能判新旧的那些先被选中。
    #[test]
    fn an_unparsable_image_model_loses_to_a_versioned_one() {
        let mixed = vec!["gpt-image-preview".to_string(), "gpt-image-2".to_string()];
        assert_eq!(pick_model(Some(&mixed)), "gpt-image-2");
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

    /// **纯生图分组落到生图栏，混合分组留在 codex 栏。**
    ///
    /// 这是分栏的核心判据。搞错的两个方向都有具体代价：
    /// - 混合分组被搬进生图栏 ⇒ 用户少了一个能聊天的档位（它有文本模型，本该能聊）。
    /// - 纯生图分组留在 codex 栏 ⇒ 回到本轮要修的病根（抢同一个 `is_current`、
    ///   switch 回填互相污染 ⇒ 界面显示「已手动维护」）。
    #[test]
    fn only_image_only_tiers_move_to_the_image_column() {
        // 纯生图：`pick_model` 写出 gpt-image-* ⇒ 进生图栏。
        assert_eq!(
            image_tier_app_type(&AppType::Codex, "gpt-image-2"),
            AppType::CodexImage,
        );
        // 混合分组（有文本模型）：`pick_model` 写出 DEFAULT_MODEL ⇒ 留在 codex。
        // ⚠️ 这条分组的 `allow_image_generation` 可能是 true —— 判据不看它，
        // 看的是「有没有文本模型能聊天」。
        assert_eq!(
            image_tier_app_type(&AppType::Codex, DEFAULT_MODEL),
            AppType::Codex,
        );
    }

    /// 其它 CLI 不受影响 —— 即便模型名恰好带那个前缀。
    ///
    /// `/v1/images/generations` 是 openai 的端点，把一条 claude 档位搬进生图栏
    /// 会让生图工具拿 anthropic 形状的配置去打那个端点。
    #[test]
    fn non_codex_apps_never_move_to_the_image_column() {
        for app in [AppType::Claude, AppType::Gemini, AppType::GrokBuild] {
            assert_eq!(
                image_tier_app_type(&app, "gpt-image-2"),
                app,
                "{app:?} 被搬进生图栏了 —— 生图只走 openai 平台"
            );
        }
    }

    /// 已经在生图栏的档位不会被再搬一次（幂等）。
    #[test]
    fn the_image_column_is_a_fixed_point() {
        assert_eq!(
            image_tier_app_type(&AppType::CodexImage, "gpt-image-2"),
            AppType::CodexImage,
        );
    }

    /// 生图栏与 codex 栏的配置形状必须**逐字节相同**。
    ///
    /// 生图 MCP 按 codex 的形状去读 sk 与 base_url（`extract_api_key` /
    /// `extract_codex_base_url`）。两边形状一分叉，生图就在运行时读不出密钥，
    /// 而那是一个只有真机能发现的失败。
    #[test]
    fn the_image_column_shares_the_codex_config_shape() {
        let base = "https://api.x.dev/v1";
        let codex = settings_config_for(&AppType::Codex, "sk-1", "档", base, "gpt-image-2")
            .expect("codex 必须有形状");
        let image = settings_config_for(&AppType::CodexImage, "sk-1", "档", base, "gpt-image-2")
            .expect("生图栏必须有形状");
        assert_eq!(
            codex, image,
            "生图栏的配置形状与 codex 分叉了 —— 生图 MCP 会读不出 sk"
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

    /// **大小写与空白要归一化后再比前缀** —— 上游 `IsGPTImageGenerationModel` 就是
    /// `ToLower` + `TrimSpace` 之后比的。
    ///
    /// 裸比前缀会在危险方向失败：中转站返回 `GPT-Image-2` 时我们判它不是生图模型 ⇒
    /// `pick_model` 以为这个分组有文本模型 ⇒ 写 `DEFAULT_MODEL` ⇒ **404 又回来了**，
    /// 正是这套代码要修的那个 bug。（review 抓出）
    #[test]
    fn the_image_model_predicate_normalizes_case_and_whitespace() {
        assert!(is_image_model("GPT-Image-2"), "大写没被归一化");
        assert!(is_image_model("  gpt-image-2  "), "空白没被裁掉");
        assert!(is_image_model("GPT-IMAGE-1.5"));
        // 归一化不该把无关的名字也放进来。
        assert!(!is_image_model("gpt-5.6-sol"));
        assert!(!is_image_model("image-gpt-2"));
    }

    /// 同一条归一化要贯穿到 `pick_model`，否则大写的纯生图列表会被判成「有文本模型」。
    #[test]
    fn pick_model_handles_non_lowercase_model_ids() {
        let shouty = vec!["GPT-Image-2".to_string()];
        assert_eq!(
            pick_model(Some(&shouty)),
            "GPT-Image-2",
            "大写的纯生图分组被误判成有文本模型 ⇒ 写回了默认文本模型"
        );
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
            // 排序只看倍率与 group_id，模型名 / 角色模型 / 生图开关都不参与。
            model: DEFAULT_MODEL.into(),
            roles: None,
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

    /// 造一个模型名列表（测试 helper，省得每处 `.map(String::from).collect()`）。
    fn models(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// claude 平台档位按该分组模型列表挑角色模型 —— 修复「Anthropic 档位写 gpt-5.6-sol」的回归测试。
    #[test]
    fn claude_tier_picks_role_models_from_an_anthropic_list() {
        let picked = pick_tier_models(
            &AppType::Claude,
            Some(&models(&[
                "claude-fable-5",
                "claude-haiku-4-5",
                "claude-opus-4-5",
                "claude-opus-5",
                "claude-sonnet-5",
            ])),
        );
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        assert_eq!(picked.main, "claude-opus-5[1m]");
        assert_eq!(roles.opus, "claude-opus-5[1m]");
        assert_eq!(roles.fable, "claude-opus-5[1m]");
        assert_eq!(roles.sonnet, "claude-sonnet-5[1m]");
        assert_eq!(roles.subagent, "claude-sonnet-5[1m]");
        assert_eq!(roles.haiku, "claude-haiku-4-5[1m]");
    }

    /// 列表是 gpt 家族时按「opus↔sol、sonnet↔terra、haiku↔luna」对齐（瓜子内部 api 的 GPT 分组）。
    #[test]
    fn claude_tier_maps_a_gpt_list_by_equivalence() {
        let picked = pick_tier_models(
            &AppType::Claude,
            Some(&models(&[
                "gpt-5.4",
                "gpt-5.4-mini",
                "gpt-5.6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
            ])),
        );
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        assert_eq!(picked.main, "gpt-5.6-sol[1m]");
        assert_eq!(roles.opus, "gpt-5.6-sol[1m]");
        assert_eq!(roles.sonnet, "gpt-5.6-terra[1m]");
        assert_eq!(roles.haiku, "gpt-5.6-luna[1m]");
    }

    /// 列表是国产家族（deepseek/kimi）时按 pro/flash 分档。
    #[test]
    fn claude_tier_maps_a_deepseek_list() {
        let picked = pick_tier_models(
            &AppType::Claude,
            Some(&models(&[
                "deepseek-v4-flash",
                "deepseek-v4-pro",
                "kimi-for-coding",
            ])),
        );
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        assert_eq!(picked.main, "deepseek-v4-pro[1m]");
        assert_eq!(roles.opus, "deepseek-v4-pro[1m]");
        assert_eq!(roles.sonnet, "deepseek-v4-flash[1m]");
        assert_eq!(roles.haiku, "deepseek-v4-flash[1m]");
    }

    /// 高档位取不到时主模型顺延到低档（用户指定：取不到就写相邻档位可取的模型）。
    #[test]
    fn claude_tier_falls_to_lower_tier_when_top_is_absent() {
        let picked = pick_tier_models(&AppType::Claude, Some(&models(&["claude-sonnet-5"])));
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        assert_eq!(picked.main, "claude-sonnet-5[1m]");
        assert_eq!(roles.opus, "claude-sonnet-5[1m]");
        assert_eq!(roles.sonnet, "claude-sonnet-5[1m]");
        assert_eq!(roles.haiku, "claude-sonnet-5[1m]");
    }

    /// codex：列表里有 `DEFAULT_MODEL` 保持现状；没有则顺延，不写一个不存在的模型。
    #[test]
    fn codex_main_uses_default_when_present_otherwise_shifts() {
        let with_default =
            pick_tier_models(&AppType::Codex, Some(&models(&["gpt-5.4", "gpt-5.6-sol"])));
        assert_eq!(with_default.main, "gpt-5.6-sol");
        assert!(with_default.claude_roles.is_none());

        let without_default = pick_tier_models(&AppType::Codex, Some(&models(&["gpt-5.6-terra"])));
        assert_eq!(without_default.main, "gpt-5.6-terra");
    }

    /// 模型列表拉不到时回落旧行为（DEFAULT_MODEL），不因为「模型名不理想」中断 provision。
    #[test]
    fn tier_models_fall_back_to_default_when_list_unavailable() {
        let picked = pick_tier_models(&AppType::Claude, None);
        assert_eq!(picked.main, DEFAULT_MODEL);
        assert!(picked.claude_roles.is_none());
    }

    /// 纯生图分组各平台都写生图模型（claude 平台不会真的纯生图，但判据不该依赖「不会」）。
    #[test]
    fn image_only_group_keeps_an_image_model_across_platforms() {
        let image = models(&["gpt-image-2"]);
        assert_eq!(
            pick_tier_models(&AppType::Codex, Some(&image)).main,
            "gpt-image-2"
        );
        assert_eq!(
            pick_tier_models(&AppType::Claude, Some(&image)).main,
            "gpt-image-2"
        );
    }

    /// 无档位表的平台（gemini 等）：主模型 = 列表第一个文本模型，不分角色。
    #[test]
    fn other_platforms_take_the_first_text_model() {
        let picked = pick_tier_models(
            &AppType::Gemini,
            Some(&models(&["gemini-3-flash", "gemini-3-pro"])),
        );
        assert_eq!(picked.main, "gemini-3-flash");
        assert!(picked.claude_roles.is_none());
    }

    /// 同一列表两次调用必须挑出同一个结果 —— 否则「已手工维护」的比对基准跟着抖。
    #[test]
    fn claude_tier_pick_is_deterministic() {
        let list = models(&["gpt-5.4", "gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"]);
        assert_eq!(
            pick_tier_models(&AppType::Claude, Some(&list)),
            pick_tier_models(&AppType::Claude, Some(&list)),
        );
    }

    /// 端到端：瓜子 Anthropic 分组（模型列表全是 claude）→ 生成配置必须写 claude 模型。
    ///
    /// `[1M]` 只声明给支持 1M 的新一代模型，且裸名（gpt-5.6 / deepseek-v4）不算数。
    #[test]
    fn one_m_suffix_only_for_supported_generations() {
        assert_eq!(maybe_one_m("claude-opus-5"), "claude-opus-5[1m]");
        assert_eq!(maybe_one_m("claude-sonnet-5"), "claude-sonnet-5[1m]");
        assert_eq!(maybe_one_m("claude-haiku-4-5"), "claude-haiku-4-5[1m]");
        assert_eq!(maybe_one_m("gpt-5.6-sol"), "gpt-5.6-sol[1m]");
        assert_eq!(maybe_one_m("gpt-5.6-terra"), "gpt-5.6-terra[1m]");
        assert_eq!(maybe_one_m("deepseek-v4-flash"), "deepseek-v4-flash[1m]");
        // 旧代 / 裸名 / 其它家族：不声明。
        assert_eq!(maybe_one_m("gpt-5.4"), "gpt-5.4");
        assert_eq!(maybe_one_m("claude-sonnet-4-5"), "claude-sonnet-4-5");
        assert_eq!(maybe_one_m("gemini-3-pro"), "gemini-3-pro");
        // 裸 gpt-5.6 不是可访问的模型 id，不该被当成「支持 1M」。
        assert!(!supports_one_m("gpt-5.6"));
        assert!(!supports_one_m("deepseek-v4"));
        assert!(supports_one_m("gpt-5.6-luna"));
    }

    /// 这是「claude 档位写 gpt-5.6-sol」bug 的直接回归：挑模型 + 生成配置整条链路。
    #[test]
    fn claude_tier_flows_into_generated_settings_config() {
        let list = models(&[
            "claude-fable-5",
            "claude-haiku-4-5",
            "claude-opus-5",
            "claude-sonnet-5",
        ]);
        let picked = pick_tier_models(&AppType::Claude, Some(&list));
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        let cfg = settings_config_with_roles(
            &AppType::Claude,
            "sk-1",
            "瓜子内部 api · Anthropic 模型-导入 Claude Code",
            "https://api.guazi.shop/v1",
            &picked.main,
            Some(roles),
        )
        .expect("claude 必须有形状");
        let env = &cfg["env"];
        // 支持 1M 的 claude 新一代模型自动带 `[1m]` 后缀声明（转发时剥掉）。
        assert_eq!(env["ANTHROPIC_MODEL"], "claude-opus-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "claude-opus-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_FABLE_MODEL"], "claude-opus-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "claude-sonnet-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-4-5[1m]");
        // 修复前这里是 gpt-5.6-sol —— 模型列表明明全 claude，档位却写 openai 模型。
        assert_ne!(env["ANTHROPIC_MODEL"], "gpt-5.6-sol");
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
        assert!(crate::relay::is_managed(&a), "id: {a}");
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
        let prefix = crate::relay::managed::MANAGED_ID_PREFIX;

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
                    crate::relay::is_managed(&id),
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
            assert!(crate::relay::is_managed(&id), "vendor id 没被认出来：{id}");
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
        // 删掉它才走 provider auth 打中转站的 /v1（实测 0 fail）。
        let toml = codex_config_toml("n", "https://x.dev/v1", "m");
        assert!(
            !toml.contains("requires_openai_auth"),
            "声明了 requires_openai_auth 会让 codex 去打 chatgpt.com 而不是中转站: {toml}"
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
    /// relay 档位与 vendor 账号两条链（`vendor/provision.rs` 的 `provider_rows_for`
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

    #[test]
    fn claude_api_key_field_is_supported_by_read_and_patch() {
        let mut sc = serde_json::json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-old",
                "ANTHROPIC_MODEL": "用户改过的模型"
            }
        });

        assert_eq!(
            extract_api_key(&sc, &AppType::Claude).as_deref(),
            Some("sk-old")
        );
        assert!(patch_api_key(&mut sc, &AppType::Claude, "sk-new"));
        assert_eq!(sc["env"]["ANTHROPIC_API_KEY"], "sk-new");
        assert!(sc["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert_eq!(sc["env"]["ANTHROPIC_MODEL"], "用户改过的模型");

        sc["env"]["ANTHROPIC_AUTH_TOKEN"] = serde_json::json!("sk-stale");
        assert!(patch_api_key(&mut sc, &AppType::Claude, "sk-unified"));
        assert_eq!(sc["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-unified");
        assert_eq!(sc["env"]["ANTHROPIC_API_KEY"], "sk-unified");
    }

    #[test]
    fn ensure_api_key_recreates_a_missing_auth_section() {
        let mut sc = serde_json::json!({
            "config": "model = \"用户改过的模型\""
        });

        assert!(ensure_api_key(&mut sc, &AppType::Codex, "sk-managed"));
        assert_eq!(sc["auth"]["OPENAI_API_KEY"], "sk-managed");
        assert_eq!(sc["config"], "model = \"用户改过的模型\"");
    }

    /// Claude Code 的默认配置带 `language: chinese`（维护者要求所有 LoongPort 生成的
    /// Claude Code 配置默认中文）；**Claude Desktop 不带** —— 维护者指定「只在 claudecode」。
    #[test]
    fn claude_default_carries_language_chinese_but_desktop_does_not() {
        let claude = settings_config_for(&AppType::Claude, "sk-1", "n", "https://x.dev/v1", "m")
            .expect("claude 必须有形状");
        assert_eq!(
            claude["language"], "chinese",
            "Claude Code 默认配置该带 language: chinese"
        );

        let desktop = settings_config_for(
            &AppType::ClaudeDesktop,
            "sk-1",
            "n",
            "https://x.dev/v1",
            "m",
        )
        .expect("claude-desktop 必须有形状");
        assert!(
            desktop.get("language").is_none(),
            "Claude Desktop 不带 language —— 维护者指定只在 claudecode"
        );
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
        for app_type in [
            AppType::Codex,
            AppType::Claude,
            AppType::Gemini,
            AppType::Hermes,
            AppType::OpenClaw,
            AppType::OpenCode,
        ] {
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
    fn patch_api_key_supports_top_level_and_nested_additive_configs() {
        for app_type in [AppType::Hermes, AppType::OpenClaw, AppType::OpenCode] {
            let mut settings =
                settings_config_for(&app_type, "sk-old", "n", "https://x.dev/v1", "m")
                    .unwrap_or_else(|| panic!("{app_type:?} 必须有形状"));

            assert!(patch_api_key(&mut settings, &app_type, "sk-new"));
            assert_eq!(
                extract_api_key(&settings, &app_type).as_deref(),
                Some("sk-new")
            );
        }

        let mut missing_options = serde_json::json!({ "models": {} });
        assert!(!patch_api_key(
            &mut missing_options,
            &AppType::OpenCode,
            "sk-new"
        ));
        assert!(missing_options.get("options").is_none());

        assert!(ensure_api_key(
            &mut missing_options,
            &AppType::OpenCode,
            "sk-new"
        ));
        assert_eq!(
            extract_api_key(&missing_options, &AppType::OpenCode).as_deref(),
            Some("sk-new")
        );
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
        // 而中转站通常只认一个模型名。
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
        // 这条钉住一个**真踩过的坑**：`relay_list_tier_rates` 原本硬编码
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
