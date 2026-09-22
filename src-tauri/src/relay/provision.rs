//! Sub2api group provisioning: claim or create managed keys and expand groups into client tiers.
//! Model policy and client configuration belong to sibling modules; persistence belongs to commands.
//! Key names use `LoongPort/a<account-id>/<platform>/<group-id>` across devices.
//! Partial failures preserve successful tiers and report failed groups for an idempotent retry.

use super::model_selection::{
    image_tier_app_type, is_pure_image_group, normalize_model_names, pick_tier_models_with,
    ClaudeRoleModels, ModelSelectionTables,
};
use crate::app_config::AppType;
use crate::error::AppError;
use crate::relay::sub2api::{ApiKey, Client, Group};
use std::collections::HashSet;

const MANAGED_PREFIX: &str = "LoongPort";

fn parse_managed_key_name(name: &str) -> Option<(String, i64)> {
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() != 4 || parts[0] != MANAGED_PREFIX {
        return None;
    }
    let group_id = parts[3].parse::<i64>().ok()?;
    Some((parts[2].to_string(), group_id))
}

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
    /// The model identifiers returned by this tier's `/v1/models` endpoint.
    /// `None` means the endpoint was unavailable, so the UI must not claim a
    /// complete supported-model list.
    pub models: Option<Vec<String>>,
    /// claude 平台各角色模型（由 [`pick_tier_models`] 按该分组模型列表挑出）。
    ///
    /// 其余平台 `None`（它们的配置没有 haiku/sonnet/opus 这套角色别名）。
    pub roles: Option<ClaudeRoleModels>,
    /// 服务端说这个分组允许生图（`allow_image_generation`）。
    ///
    /// 与「这是纯生图档位」是两件事，见 [`super::sub2api::Group::allow_image_generation`]。
    pub allow_image_generation: bool,
    /// 订阅限额的重置窗口（分组限额 × 这把 key 的用量），见 [`super::tier_windows`]。
    ///
    /// composite 拆档的各档共享同一把 key ⇒ 窗口相同（同一个事实，各档各存一份投影）。
    /// 非订阅分组为空。
    pub windows: Vec<super::tier_windows::SubscriptionWindow>,
}

#[derive(Debug, Default)]
pub struct ProvisionResult {
    /// 每个分组连带它该落到哪个 CLI（见 [`TargetedTier`]）。
    pub tiers: Vec<TargetedTier>,
    /// `(分组名, 失败原因)`。
    pub failures: Vec<(String, String)>,
}

pub fn key_name_for(account_id: Option<i64>, platform: &str, group_id: i64) -> String {
    match account_id {
        Some(id) => format!("{MANAGED_PREFIX}/a{id}/{platform}/{group_id}"),
        None => format!("{MANAGED_PREFIX}/anon/{platform}/{group_id}"),
    }
}

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

#[derive(Debug, Clone)]
pub struct TargetedTier {
    pub tier: Tier,
    /// 这个分组该落到哪个 CLI。
    pub app_type: AppType,
}

pub async fn provision(
    client: &Client,
    tables: &ModelSelectionTables,
) -> Result<ProvisionResult, AppError> {
    // 账号身份从 `client` 取，**不另收一个参数** —— 「用哪个账号建 Key」与
    // 「用哪个账号发请求」必须是同一个答案，两处各传一遍就可能不一致。
    let account_id = client.account_id();
    let groups = client.list_groups().await?;

    // 「当前还存在哪些 (platform, group_id)」—— 用来判断哪些托管 key 成了孤儿。
    // ⚠️ 用**完整**分组列表而不是 usable：临时不可用的分组（维护中）key 还在被别的
    // 机器用，不该删；只有分组真的从列表里消失才算被删除。
    let current_groups: HashSet<(String, i64)> =
        groups.iter().map(|g| (g.platform.clone(), g.id)).collect();

    // composite 先分流出去（它映射不到单一 app，走拆档），其余照旧按平台归位。
    let (composite, platform_groups): (Vec<Group>, Vec<Group>) =
        groups.into_iter().partition(|g| {
            super::platform_map::parse_platform(&g.platform)
                == Some(super::platform_map::Platform::Composite)
        });
    // 按分组自己的 platform 分派，认不出的跳过（不是错误：antigravity 是还没接，
    // 不该让整个流程失败）。
    let usable: Vec<(Group, AppType)> = platform_groups
        .into_iter()
        .filter_map(|g| {
            let app_type = super::platform_map::parse_platform(&g.platform)?.app_type()?;
            // 平台对上了还要过 is_usable_for 那几道（active、倍率不离谱）。
            g.is_usable_for(&app_type).then_some((g, app_type))
        })
        .collect();
    let composite: Vec<Group> = composite
        .into_iter()
        // 同一道 active/倍率闸（只是没有平台维），不过闸的连拆档都不进。
        .filter(|g| g.is_usable_ignoring_platform())
        .collect();

    if usable.is_empty() && composite.is_empty() {
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

    // 订阅行是订阅分组三窗用量/起点的真源（api_keys 的 usage 字段只管 key 级限额）。
    // 拉不到按「没有订阅」处理——非订阅站点 / 老版本服务端本来就没有，不阻断。
    let subscriptions = client.list_subscriptions().await.unwrap_or_else(|e| {
        log::debug!("查询用户订阅失败（订阅窗口回落 key 级字段）: {e}");
        Default::default()
    });
    let mut result = ProvisionResult::default();
    for (group, app_type) in usable {
        match ensure_key_for(
            client,
            account_id,
            &app_type,
            &group,
            &existing,
            tables,
            &subscriptions,
        )
        .await
        {
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

    // composite 拆档：一把 Key 扇出多 CLI 档（见 [`ensure_composite_tiers`]）。
    // 失败语义与上面一致 —— 单组失败只进 failures，不拖垮别的分组。
    for group in composite {
        match ensure_composite_tiers(
            client,
            account_id,
            &group,
            &existing,
            tables,
            &subscriptions,
        )
        .await
        {
            Ok(mut tiers) => result.tiers.append(&mut tiers),
            Err(e) => result.failures.push((group.name.clone(), e.to_string())),
        }
    }

    // 把「用户专属倍率」并进来 —— sub2api 把倍率拆成两半，这是第二半。
    //
    // `/groups/available` 给的是**分组默认**倍率，中转站可以给某个用户单独设更低的
    // （`user_group_rates` 表），那份要另查 `/groups/rates`。sub2api 自己的前端就是把
    // 这两条 join 起来显示的（`KeysView.vue` 拉 `getAvailable()` + `getUserGroupRates()`
    // 交给 `GroupBadge`），我们照它做。
    //
    // ⚠️ **失败当作「没有专属倍率」**，不是错误：拿不到它只是显示的数字回落成分组默认值，
    // 而为它中断整次 provision 会让用户连密钥都拿不到。
    //
    // ⚠️ **有意在这里覆盖，而不是在 `ensure_key_for` 里** —— `Group::is_usable_for`
    // 那道探针池过滤（惩罚性倍率）必须继续用**分组默认倍率**判：它判的是「这个分组
    // 是不是不给人用的」，与某个用户拿到什么折扣无关。
    let user_rates = client.user_group_rates().await.unwrap_or_else(|e| {
        log::debug!("查询用户专属倍率失败（回落到分组默认倍率）: {e}");
        Default::default()
    });
    if !user_rates.is_empty() {
        for targeted in &mut result.tiers {
            if let Some(rate) = user_rates.get(&targeted.tier.group_id) {
                targeted.tier.rate_multiplier = *rate;
            }
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

async fn claim_or_create_key(
    client: &Client,
    account_id: Option<i64>,
    group: &Group,
    existing: &[ApiKey],
) -> Result<(String, bool, ApiKey), AppError> {
    match claim_key(existing, account_id, &group.platform, group.id) {
        // 正常路径：认领到了就直接用，不发任何写请求。
        Some(k) => Ok((k.key.clone(), false, k.clone())),
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
            // 新建 key 的服务端响应自带用量字段（零值、窗口未开始）——原样透传。
            Ok((created.key.clone(), true, created))
        }
    }
}

async fn ensure_key_for(
    client: &Client,
    account_id: Option<i64>,
    app_type: &AppType,
    group: &Group,
    existing: &[ApiKey],
    tables: &ModelSelectionTables,
    subscriptions: &[super::sub2api::UserSubscription],
) -> Result<Tier, AppError> {
    let (api_key, created, key_meta) =
        claim_or_create_key(client, account_id, group, existing).await?;

    // 拉这个分组能调哪些模型 —— 只为决定写什么模型名（纯生图分组必须写它自己的
    // `gpt-image-*`，写文本模型会 404）。
    //
    // ⚠️ **查失败不算错**：回落到 `DEFAULT_MODEL` = 本功能出现之前的行为。
    // 为一个「模型名可能不理想」中断整个分组的 provision 是把小问题放大成大问题
    // （用户会看到「获取密钥失败」而不是「某个档位模型名不对」）。
    let models = match super::sub2api::list_models(client.site_origin(), &api_key).await {
        Ok(v) => v.map(normalize_model_names),
        Err(e) => {
            log::debug!(
                "分组 {}（{}）的模型列表拉不到，模型名回落默认值（不影响使用）: {e}",
                group.id,
                group.name,
            );
            None
        }
    };
    let picked = pick_tier_models_with(app_type, models.as_deref(), tables);
    if picked.main != tables.default_model {
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
        models,
        roles: picked.claude_roles,
        allow_image_generation: group.allow_image_generation,
        windows: super::tier_windows::windows_for(
            group,
            &key_meta,
            subscriptions.iter().find(|sub| sub.group_id == group.id),
        ),
    })
}

fn composite_app_targets(models: &[String]) -> Vec<AppType> {
    if is_pure_image_group(models) {
        return vec![AppType::CodexImage];
    }
    let mut targets = vec![AppType::Codex, AppType::Claude];
    if models
        .iter()
        .any(|m| m.to_ascii_lowercase().starts_with("gemini-"))
    {
        targets.push(AppType::Gemini);
    }
    if models
        .iter()
        .any(|m| m.to_ascii_lowercase().starts_with("grok"))
    {
        targets.push(AppType::GrokBuild);
    }
    targets
}

async fn ensure_composite_tiers(
    client: &Client,
    account_id: Option<i64>,
    group: &Group,
    existing: &[ApiKey],
    tables: &ModelSelectionTables,
    subscriptions: &[super::sub2api::UserSubscription],
) -> Result<Vec<TargetedTier>, AppError> {
    let (api_key, created, key_meta) =
        claim_or_create_key(client, account_id, group, existing).await?;
    let models = super::sub2api::list_models(client.site_origin(), &api_key)
        .await
        .map_err(|e| AppError::Config(format!("拉不到模型目录，无法判定该落到哪些 CLI: {e}")))?
        .map(normalize_model_names)
        // 归一化后为空（全是空白 id）视同没有目录：空列表会让
        // `is_pure_image_group` 真空真、误判成纯生图分组。
        .filter(|models: &Vec<String>| !models.is_empty())
        .ok_or_else(|| AppError::Config("模型目录为空，无法判定该落到哪些 CLI".into()))?;

    let windows = super::tier_windows::windows_for(
        group,
        &key_meta,
        subscriptions.iter().find(|sub| sub.group_id == group.id),
    );
    let mut tiers = Vec::new();
    for (index, app_type) in composite_app_targets(&models).into_iter().enumerate() {
        let picked = pick_tier_models_with(&app_type, Some(&models), tables);
        if picked.main != tables.default_model {
            log::info!(
                "composite 分组 {}（{}）在 {} 落档，模型名写 {}（目录 {:?}）",
                group.id,
                group.name,
                app_type.as_str(),
                picked.main,
                models,
            );
        }
        tiers.push(TargetedTier {
            tier: Tier {
                group_id: group.id,
                group_name: group.name.clone(),
                rate_multiplier: group.rate_multiplier,
                api_key: api_key.clone(),
                key_was_created: created && index == 0,
                model: picked.main,
                models: Some(models.clone()),
                roles: picked.claude_roles,
                allow_image_generation: group.allow_image_generation,
                windows: windows.clone(),
            },
            app_type,
        });
    }
    Ok(tiers)
}

pub fn sort_tiers(tiers: &mut [TargetedTier]) {
    tiers.sort_by(|a, b| {
        a.tier
            .rate_multiplier
            .partial_cmp(&b.tier.rate_multiplier)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.tier.group_id.cmp(&b.tier.group_id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::model_selection::DEFAULT_MODEL;

    fn key(id: i64, name: &str, status: &str) -> ApiKey {
        ApiKey {
            id,
            key: format!("sk-{id}"),
            name: name.into(),
            status: status.into(),
            ..ApiKey::default()
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
        assert_eq!(parse_managed_key_name("example-relay-xxx"), None);
        assert_eq!(parse_managed_key_name("LoongPort/a13/openai"), None);
        assert_eq!(parse_managed_key_name("LoongPort/a13"), None);
    }

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

    #[test]
    fn composite_targets_follow_model_families() {
        // 国产模型订阅（glm/deepseek/kimi…，无 gemini/grok 家族）→ 只落 codex + claude。
        let domestic = [
            "glm-5.3",
            "glm-5.3-flash",
            "deepseek-v4-pro",
            "kimi-k3",
            "minimax-m3",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            composite_app_targets(&domestic),
            vec![AppType::Codex, AppType::Claude]
        );

        // 有 gemini / grok 家族 → 四个档全落。
        let mixed = ["glm-5.3", "gemini-3-pro", "grok-4.6", "deepseek-v4-flash"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            composite_app_targets(&mixed),
            vec![
                AppType::Codex,
                AppType::Claude,
                AppType::Gemini,
                AppType::GrokBuild
            ]
        );

        // 纯生图 → 只出生图档（与 newapi 扇出同判据同源）。
        let image_only = ["gpt-image-2", "gpt-image-2-mini"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            composite_app_targets(&image_only),
            vec![AppType::CodexImage]
        );

        // 家族判据不认大小写（站点目录的大小写不由我们保证）。
        let upper = ["GEMINI-3-PRO".to_string(), "glm-5.3".to_string()];
        assert!(composite_app_targets(&upper).contains(&AppType::Gemini));
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
            models: None,
            roles: None,
            allow_image_generation: false,
            windows: Vec::new(),
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
}
