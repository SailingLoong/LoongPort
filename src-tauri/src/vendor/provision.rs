//! 一把 sk → N 个 plan × 六个平台的 provider 记录。
//!
//! ## plan（接入变体）层
//!
//! 单 plan 厂商（DeepSeek / BigModel）就是旧世界：一个账号一个 endpoint，一个
//! bundle。多 plan 厂商（opencode 的 Zen / Go）**同一个账号展开出多个 bundle**：
//! 每档各自六条 provider 记录、各自的 provider id（按 [`provider_id_for`] 的段
//! 区分），同 app 下互斥可切 —— 与中转站「一个账号多档」的语义对齐，但**账号层**
//! （登录、key、余额）仍是一份，不因 plan 分叉。判据见 `vendor::opencode::Plan`。
//!
//! ## 为什么一个 bundle 的六条记录共用一个 provider id
//!
//! `providers` 表的主键是**复合的** `PRIMARY KEY (id, app_type)`（`schema.rs:42`）
//! ⇒ 同一个 id 在六个 `app_type` 下是六行，不冲突。这与 sub2api 的
//! `provider_id_for` 同构（它也不含 app_type）。
//!
//! 共用一个 id 是**有意的**：它表达「同一把 sk 在六个 CLI 上的六种落法」，
//! 删账号时逐 plan `DELETE WHERE id = ?`（不带 app_type 条件）删全。
//! ⚠️ 别顺手给 id 加平台段 —— 那会让「删一个账号」变成要遍历六个 app_type。
//!
//! ## key 生命周期：删了才建
//!
//! 1. **删了才建，不能反过来** —— 先建后删的话中途失败会留下两把都在，
//!    而本地只记得一把 ⇒ 下次又多一把。
//! 2. **删失败不阻断建** —— 删是清理，建是目的。与 `relay::provision`
//!    的「尽力而为 + 全量回报，不回滚」语义一致。
//! 3. **多 plan 共用同一把 key** —— plan 是配置层的派生物，不去官网建第二把
//!    （两档的 key 认领名字相同，天然一把通吃）。

use serde_json::Value;

use crate::app_config::AppType;
use crate::vendor::{key_name_for, PlanInfo, Vendor, VendorKey};

/// 官网厂商展开的全部平台。`Gemini` / `GrokBuild` 等不在其中
/// （vendor 的 `config_for` 对不支持的平台返回 `None`，这里只是候选集）。
pub const VENDOR_APPS: [AppType; 6] = [
    AppType::Codex,
    AppType::Claude,
    AppType::ClaudeDesktop,
    AppType::Hermes,
    AppType::OpenClaw,
    AppType::OpenCode,
];

/// 稳定派生的 provider id。**不含 app_type**（见模块文档）。
///
/// 第一个参数是**段**：单 plan 厂商 = `vendor_id`，多 plan 厂商 = plan 的
/// `id_segment`（[`crate::vendor::PlanInfo`]）。单 plan 厂商的调用结果与
/// 「拿 vendor_id 进来」的旧世界逐字节相同 —— 存量 id 靠这一点不动。
///
/// ⚠️ **分隔符不能省** —— 没有它 `(vendor="a", account="bc")` 与
/// `(vendor="ab", account="c")` 喂进哈希的字节流完全相同
/// （同型于 `relay::provision::provider_id_for` 那个闸）。
pub fn provider_id_for(id_segment: &str, account_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(id_segment.as_bytes());
    h.update(b"/");
    h.update(account_id.as_bytes());
    format!(
        "{}vendor-{}",
        crate::relay::managed::MANAGED_ID_PREFIX,
        &hex::encode(h.finalize())[..16]
    )
}

/// 从官网 key 列表里筛出「本客户端为**这个账号**建过的」那些。
///
/// ⚠️ **精确相等，不是 `starts_with`** —— 用前缀匹配会命中
/// `LoongPort专用/a123-old` 之类（同型于 `relay::provision::claim_key`
/// 那个「`.../42` 会被 `.../420` 命中」的坑）。
///
/// ⚠️ **必须带 `account_id`** —— 裸 `LoongPort专用/` 会把**别的账号**那把也删掉。
/// 而同一台机器上挂多个 DeepSeek 账号是这张表的唯一索引
/// （`(vendor_id, account_id)`）特意支持的场景。
///
/// ## 为什么不再按机器筛（2026-08-04 改）
///
/// 见 [`crate::vendor::key_name_for`]：按机器命名的理由被实测证伪，
/// 而代价是每台机器各堆一份（DeepSeek 上限 100 把）。改按账号之后，
/// 三台 Mac 共用同一把 —— 谁先 provision 谁建，其余两台**认领**它。
///
/// ⚠️ 连带效果：本函数现在会筛出「另一台机器建的那把」。那是**有意的** ——
/// 它与本机要用的是同一把（同账号同名字），删旧建新时本来就该一起处理。
pub fn keys_to_delete(all: &[VendorKey], account_id: &str) -> Vec<VendorKey> {
    let mine = key_name_for(account_id);
    all.iter().filter(|k| k.name == mine).cloned().collect()
}

/// 这个平台要不要按角色分档写 Claude 的模型别名。
///
/// 只有 Claude 系有那套角色别名（`ANTHROPIC_DEFAULT_*` / `CLAUDE_CODE_SUBAGENT_MODEL`），
/// 其余平台返回 `None`。分档的取值与理由见
/// [`crate::vendor::claude_role_models`]（**按 plan 取**：Go 目录里没有 claude 系）。
///
/// ## ⚠️ 生成配置与 `is_user_edited` 的基准**必须都走这个函数**
///
/// `is_user_edited` 靠「与重算的默认值整份比对」判断用户改没改过
/// （`relay::provision::is_user_edited`）。两边算法只要有一处不一致，
/// 结果就是**每个官网档位都显示「已手工维护」**，而用户一个字没改过。
///
/// 所以这个判断收在一个 pub 函数里，两个调用方（`plan_rows_for` 与
/// vendor 侧算 `user_edited` 那处）共用它，而不是各写一遍 `matches!(app, Claude | ..)`。
pub fn claude_roles_for(
    vendor: Vendor,
    id_segment: &str,
    app: &AppType,
) -> Option<crate::relay::provision::ClaudeRoleModels> {
    matches!(app, AppType::Claude | AppType::ClaudeDesktop)
        .then(|| crate::vendor::claude_role_models(vendor, id_segment))
}

/// 一个 plan 展开出的那组 provider 记录（同 bundle 共用一个 provider id）。
pub struct PlanRows {
    pub plan: PlanInfo,
    /// 这个 plan 的 provider id（六个平台共用，见模块文档）。
    pub provider_id: String,
    /// `(app_type, settings_config)`，本 plan 覆盖的平台。
    pub rows: Vec<(AppType, Value)>,
}

/// 一把 sk 按厂商的全部 plan 展开。单 plan 厂商返回一个 bundle（与旧世界的
/// `provider_rows_for` 等价）；多 plan 厂商（opencode）返回 Zen + Go 两个。
///
/// 走 `relay::provision::settings_config_with_roles_and_models` —— 它的非 codex
/// 分支复用上游 `deeplink::build_provider_from_request`，而那个 match 覆盖全部
/// 8 个平台（`deeplink/provider.rs:147`）⇒ 我们要的六个都在里面，不需要新写分派。
/// plan 的生成风格（鉴权字段 / wire）由 [`crate::vendor::plan_style`] 一并传下去。
pub fn plan_rows_for(vendor: Vendor, account_id: &str, api_key: &str) -> Vec<PlanRows> {
    crate::vendor::plans(vendor)
        .iter()
        .map(|plan| {
            // 模型目录与配置同源派生：没有目录的档位会被省心模式的偏好过滤静默排除。
            let catalog = crate::vendor::catalog_models(vendor, plan.id_segment);
            PlanRows {
                plan: *plan,
                provider_id: provider_id_for(plan.id_segment, account_id),
                rows: VENDOR_APPS
                    .iter()
                    .filter_map(|app| {
                        let (base_url, model) =
                            crate::vendor::config_for(vendor, plan.id_segment, app)?;
                        let cfg = crate::relay::provision::settings_config_with_roles_and_models(
                            app,
                            api_key,
                            plan.display_name,
                            &base_url,
                            &model,
                            claude_roles_for(vendor, plan.id_segment, app),
                            Some(catalog.as_slice()),
                            crate::vendor::plan_style(vendor, plan.id_segment),
                        )?;
                        Some((app.clone(), cfg))
                    })
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DeepSeek（单 plan 厂商）的那一个 bundle 的行 —— 旧世界 `provider_rows_for`
    /// 的等价物，存量断言全部照抄。
    fn deepseek_rows(api_key: &str) -> Vec<(AppType, Value)> {
        let mut bundles = plan_rows_for(Vendor::DeepSeek, "uuid-a", api_key);
        assert_eq!(bundles.len(), 1, "单 plan 厂商恰好一个 bundle");
        bundles.remove(0).rows
    }

    fn key(name: &str) -> VendorKey {
        VendorKey {
            name: name.to_string(),
            redacted_key: "sk-25c***122".to_string(),
            created_at: 1,
            tracking_id: "t".to_string(),
        }
    }

    #[test]
    fn one_key_expands_to_six_platforms() {
        let rows = deepseek_rows("sk-plaintext");
        assert_eq!(rows.len(), 6, "六个平台一次全补，漏一个都不行");
        let apps: Vec<String> = rows.iter().map(|(a, _)| a.as_str().to_string()).collect();
        for expect in [
            "codex",
            "claude",
            "claude-desktop",
            "hermes",
            "openclaw",
            "opencode",
        ] {
            assert!(apps.contains(&expect.to_string()), "缺平台 {expect}");
        }
    }

    /// ⭐ 每条行都带模型目录：无目录档位会被省心模式的模型偏好过滤**静默排除**
    /// （2026-08-17 真实 smoke 实测 DeepSeek 直连中招）。目录与配置同源派生、收基础名。
    #[test]
    fn rows_carry_model_catalog_for_easy_mode() {
        let rows = deepseek_rows("sk-x");
        for (app, cfg) in &rows {
            let models: Vec<&str> = cfg["modelCatalog"]["models"]
                .as_array()
                .unwrap_or_else(|| panic!("{} 行缺模型目录", app.as_str()))
                .iter()
                .filter_map(|m| m.get("model").and_then(|v| v.as_str()))
                .collect();
            assert!(
                models.contains(&"deepseek-v4-pro"),
                "{} 目录缺 pro：{models:?}",
                app.as_str()
            );
            assert!(
                models.contains(&"deepseek-v4-flash"),
                "{} 目录缺 flash：{models:?}",
                app.as_str()
            );
            assert!(
                models.iter().all(|m| !m.ends_with("[1M]")),
                "目录收基础名（[1M] 是角色 env 的变体后缀）：{models:?}"
            );
        }
    }

    #[test]
    fn gemini_and_grokbuild_get_no_row() {
        let rows = deepseek_rows("sk-plaintext");
        let apps: Vec<String> = rows.iter().map(|(a, _)| a.as_str().to_string()).collect();
        assert!(!apps.contains(&"gemini".to_string()));
        assert!(!apps.contains(&"grokbuild".to_string()));
    }

    #[test]
    fn the_plaintext_key_lands_in_every_platform_config() {
        let rows = deepseek_rows("sk-unique-marker");
        for (app, cfg) in &rows {
            let s = serde_json::to_string(cfg).expect("序列化");
            assert!(
                s.contains("sk-unique-marker"),
                "{} 的配置里没有那把 sk",
                app.as_str()
            );
        }
    }

    #[test]
    fn claude_config_uses_the_anthropic_base_url() {
        let rows = deepseek_rows("sk-x");
        let (_, cfg) = rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Claude))
            .expect("claude 那条");
        let s = serde_json::to_string(cfg).expect("序列化");
        assert!(
            s.contains("api.deepseek.com/anthropic"),
            "claude 走 Anthropic 兼容层（子路径挂载），写错直接 404：{s}"
        );
    }

    /// ⭐ **Claude 那条配置必须带齐五个角色键，且取值是分档后的。**
    ///
    /// 这是端到端的闸：`claude_roles_for` → `settings_config_with_roles_and_models` →
    /// 上游 `build_claude_settings` 三段都得通。中间任一段掉链子的症状都是
    /// 「配置里少了几个键」，而那要真机切过去用 sonnet 才发现。
    #[test]
    fn claude_config_carries_all_five_role_models() {
        let rows = deepseek_rows("sk-x");
        let (_, cfg) = rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Claude))
            .expect("claude 那条");
        let env = cfg.get("env").expect("claude 配置该有 env");

        let expect: &[(&str, &str)] = &[
            // 主模型不带 [1M]：它来自 `config_for`，同时被 codex 复用（codex 的
            // config.toml 模型名不能带后缀）。只有角色对齐带。
            ("ANTHROPIC_MODEL", "deepseek-v4-pro"),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", "deepseek-v4-pro[1M]"),
            ("ANTHROPIC_DEFAULT_FABLE_MODEL", "deepseek-v4-pro[1M]"),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", "deepseek-v4-flash[1M]"),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "deepseek-v4-flash[1M]"),
            // ⚠️ 这个键**不带 `ANTHROPIC_DEFAULT_` 前缀**。
            ("CLAUDE_CODE_SUBAGENT_MODEL", "deepseek-v4-flash[1M]"),
        ];
        for (key, want) in expect {
            assert_eq!(
                env.get(*key).and_then(|v| v.as_str()),
                Some(*want),
                "claude 配置的 {key} 该是 {want} —— 分档链路（claude_roles_for → \
                 settings_config_with_roles → 上游 build_claude_settings）断了一段"
            );
        }
    }

    /// 非 Claude 平台**不该**多出那两个新键。
    ///
    /// `claude_roles_for` 对它们返回 `None` ⇒ 生成侧不写
    /// fable / subagent。钉住它：那两个键是 Claude 专有的，写进 codex 的 TOML 或
    /// opencode 的配置里是噪音（上游那些分支压根不读）。
    #[test]
    fn non_claude_platforms_get_no_role_keys() {
        let rows = deepseek_rows("sk-x");
        for (app, cfg) in rows
            .iter()
            .filter(|(a, _)| !matches!(a, AppType::Claude | AppType::ClaudeDesktop))
        {
            let s = serde_json::to_string(cfg).expect("序列化");
            for key in [
                "ANTHROPIC_DEFAULT_FABLE_MODEL",
                "CLAUDE_CODE_SUBAGENT_MODEL",
            ] {
                assert!(
                    !s.contains(key),
                    "{} 的配置里不该有 {key} —— 那是 Claude 专有的角色别名",
                    app.as_str()
                );
            }
        }
    }

    /// 只有 Claude 系有角色分档。
    ///
    /// ⚠️ **`ClaudeDesktop` 属于 Claude 系**（它与 Claude 走同一个
    /// `build_claude_settings`、读同一批 `ANTHROPIC_*` env），所以它也分档。
    #[test]
    fn only_claude_family_gets_role_split() {
        for app in provision_apps() {
            let expected = matches!(app, AppType::Claude | AppType::ClaudeDesktop);
            assert_eq!(
                claude_roles_for(Vendor::DeepSeek, "deepseek", app).is_some(),
                expected,
                "{} 的角色分档判定错了 —— ClaudeDesktop 与 Claude 同形，两个都要有",
                app.as_str()
            );
        }
    }

    /// 本轮 provision 覆盖的那几个平台（测试辅助）。
    fn provision_apps() -> impl Iterator<Item = &'static AppType> {
        VENDOR_APPS.iter()
    }

    #[test]
    fn codex_config_omits_requires_openai_auth() {
        let rows = deepseek_rows("sk-x");
        let (_, cfg) = rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Codex))
            .expect("codex 那条");
        let s = serde_json::to_string(cfg).expect("序列化");
        assert!(
            !s.contains("requires_openai_auth"),
            "实测那行必须不写，否则 codex 判成 ChatGPT auth 模式去打 chatgpt.com 拿 403"
        );
    }

    #[test]
    fn provider_id_is_stable_and_separates_accounts() {
        let a = provider_id_for("deepseek", "uuid-a");
        assert_eq!(a, provider_id_for("deepseek", "uuid-a"), "必须稳定派生");
        assert_ne!(a, provider_id_for("deepseek", "uuid-b"));
        assert_ne!(a, provider_id_for("kimi", "uuid-a"));
    }

    #[test]
    fn provider_id_separator_cannot_be_dropped() {
        // 没有分隔符时这两组喂进哈希的字节流相同。
        assert_ne!(
            provider_id_for("a", "bc"),
            provider_id_for("ab", "c"),
            "分隔符不能省"
        );
    }

    #[test]
    fn provider_id_is_recognised_as_managed() {
        let id = provider_id_for("deepseek", "uuid-a");
        assert!(
            crate::relay::managed::is_managed(&id),
            "要命中 MANAGED_ID_PREFIX，守卫/前端过滤/托盘菜单才免费继承"
        );
    }

    // ─────────────────────── 多 plan（opencode Zen / Go）───────────────────────

    /// opencode 一个账号展开成两个 bundle，各自六条记录、各自的 provider id。
    #[test]
    fn opencode_expands_into_two_bundles_with_distinct_ids() {
        let bundles = plan_rows_for(Vendor::OpenCode, "wrk_x", "sk-x");
        assert_eq!(bundles.len(), 2, "Zen + Go");
        for bundle in &bundles {
            assert_eq!(
                bundle.rows.len(),
                6,
                "{} 六个平台",
                bundle.plan.display_name
            );
            assert!(
                crate::relay::managed::is_managed(&bundle.provider_id),
                "{} 的 id 也要命中托管前缀",
                bundle.plan.display_name
            );
        }
        assert_ne!(
            bundles[0].provider_id, bundles[1].provider_id,
            "两档 id 必须不同 —— 互斥可切就靠它"
        );
    }

    /// ⭐ **Zen bundle 与旧世界逐字节等价**：id 由 ("opencode", account) 派生、
    /// Claude 配置仍是 Bearer。这条失守 = 存量用户的 Zen 档位失联。
    #[test]
    fn zen_bundle_keeps_the_legacy_provider_id_and_bearer_auth() {
        let bundles = plan_rows_for(Vendor::OpenCode, "wrk_x", "sk-x");
        let zen = &bundles[0];
        assert_eq!(zen.plan.id_segment, "opencode");
        assert_eq!(zen.provider_id, provider_id_for("opencode", "wrk_x"));

        let (_, cfg) = zen
            .rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Claude))
            .expect("claude 那条");
        let env = cfg.get("env").expect("claude 配置该有 env");
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()),
            Some("sk-x"),
            "Zen 走 Bearer（旧行为一个字节都不能变）"
        );
        assert!(env.get("ANTHROPIC_API_KEY").is_none());
    }

    /// ⭐ **Go bundle 的 Claude 配置只写 x-api-key**。同写两个时 Claude Code 优先
    /// Bearer、被 Go 网关静默忽略 —— 那是一条真机切换后才炸的必 401 配置。
    #[test]
    fn go_bundle_auths_claude_via_api_key_only() {
        let bundles = plan_rows_for(Vendor::OpenCode, "wrk_x", "sk-x");
        let go = bundles
            .iter()
            .find(|b| b.plan.id_segment == "opencode-go")
            .unwrap();
        let (_, cfg) = go
            .rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Claude))
            .expect("claude 那条");
        let env = cfg.get("env").expect("claude 配置该有 env");
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").and_then(|v| v.as_str()),
            Some("sk-x"),
            "Go 只认 x-api-key"
        );
        assert!(
            env.get("ANTHROPIC_AUTH_TOKEN").is_none(),
            "绝不能同写 AUTH_TOKEN —— Claude Code 优先 Bearer，写了两边就静默 401"
        );
        assert!(
            serde_json::to_string(&cfg).unwrap().contains("/zen/go"),
            "Go 的 Claude 端点要落在 /zen/go 下"
        );
    }

    /// Go bundle 的 codex 配置走 chat wire（responses 目录只有带监控保留的模型）。
    #[test]
    fn go_bundle_codex_uses_chat_wire() {
        let bundles = plan_rows_for(Vendor::OpenCode, "wrk_x", "sk-x");
        let go = bundles
            .iter()
            .find(|b| b.plan.id_segment == "opencode-go")
            .unwrap();
        let (_, cfg) = go
            .rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Codex))
            .expect("codex 那条");
        let toml = cfg
            .get("config")
            .and_then(|v| v.as_str())
            .expect("codex 配置该有 config");
        assert!(
            toml.contains("wire_api = \"chat\""),
            "Go 的 codex 走 chat：{toml}"
        );

        // 对照：Zen 的 codex 仍是 responses（旧行为不变）。
        let (_, zen_cfg) = bundles[0]
            .rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Codex))
            .expect("zen codex 那条");
        let zen_toml = zen_cfg.get("config").and_then(|v| v.as_str()).unwrap();
        assert!(zen_toml.contains("wire_api = \"responses\""));
    }

    /// 两档共用同一把 key：所有 bundle 的所有配置里都是同一把 sk（不建第二把）。
    #[test]
    fn every_bundle_carries_the_same_single_key() {
        let bundles = plan_rows_for(Vendor::OpenCode, "wrk_x", "sk-the-one");
        for bundle in &bundles {
            for (app, cfg) in &bundle.rows {
                let s = serde_json::to_string(cfg).expect("序列化");
                assert!(
                    s.contains("sk-the-one"),
                    "{} / {} 的配置里没有那把 sk",
                    bundle.plan.display_name,
                    app.as_str()
                );
            }
        }
    }

    /// ⚠️ **精确相等，不是前缀** —— `a12` 不能命中 `a123`（同型于 relay 侧
    /// 那个「`.../42` 会被 `.../420` 命中」的坑）。
    #[test]
    fn keys_to_delete_matches_exactly_not_by_prefix() {
        let all = vec![
            key("LoongPort专用/a12"),
            key("LoongPort专用/a123"),
            key("LoongPort专用/a12-old"),
            key("我自己建的"),
        ];
        let got = keys_to_delete(&all, "12");
        assert_eq!(got.len(), 1, "只删精确相等的那把");
        assert_eq!(got[0].name, "LoongPort专用/a12");
    }

    /// ⭐ **不许碰别的账号那把。**
    ///
    /// 同一台机器上挂多个 DeepSeek 账号，是 `loongport_vendor` 的唯一索引
    /// `(vendor_id, account_id)` 特意支持的场景 —— 删错了另一个账号的 CLI 当场失效。
    #[test]
    fn keys_to_delete_never_touches_another_account() {
        let all = vec![key("LoongPort专用/a222"), key("LoongPort专用/a333")];
        assert!(keys_to_delete(&all, "111").is_empty());
    }

    /// ⭐ **同一个账号在别的机器上建的那把，会被筛出来 —— 那是有意的。**
    ///
    /// 按账号命名之后三台机器共用同一把（同账号 ⇒ 同名字），
    /// 所以「删旧建新」本来就该把它一起处理，不是跨机器误删。
    /// （初版按机器命名、要防的正是「跨机器互删」；那个设计已被实测推翻，
    /// 见 `crate::vendor::key_name_for` 的文档。）
    #[test]
    fn keys_to_delete_covers_the_same_account_from_any_machine() {
        let all = vec![key("LoongPort专用/a111")];
        assert_eq!(
            keys_to_delete(&all, "111").len(),
            1,
            "同账号同名字 ⇒ 三台机器共用一把，删旧建新要一起算"
        );
    }

    #[test]
    fn keys_to_delete_never_touches_user_created_keys() {
        let all = vec![key("慕豪"), key("ALLEN"), key("子墨")];
        assert!(
            keys_to_delete(&all, "111").is_empty(),
            "用户手建的 key 一把都不能碰"
        );
    }
}
