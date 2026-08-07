//! 一把 sk → 六个平台的 provider 记录。**无档位层**（官网一个账号就一个 endpoint）。
//!
//! ## 为什么六条记录共用一个 provider id
//!
//! `providers` 表的主键是**复合的** `PRIMARY KEY (id, app_type)`（`schema.rs:42`）
//! ⇒ 同一个 id 在六个 `app_type` 下是六行，不冲突。这与 sub2api 的
//! `provider_id_for` 同构（它也不含 app_type）。
//!
//! 共用一个 id 是**有意的**：它表达「同一把 sk 在六个 CLI 上的六种落法」，
//! 删账号时 `DELETE WHERE id = ?`（不带 app_type 条件）一次删全。
//! ⚠️ 别顺手给 id 加平台段 —— 那会让「删一个账号」变成要遍历六个 app_type。
//!
//! ## key 生命周期：删了才建
//!
//! 1. **删了才建，不能反过来** —— 先建后删的话中途失败会留下两把都在，
//!    而本地只记得一把 ⇒ 下次又多一把。
//! 2. **删失败不阻断建** —— 删是清理，建是目的。与 `operator::provision`
//!    的「尽力而为 + 全量回报，不回滚」语义一致。

use serde_json::Value;

use crate::app_config::AppType;
use crate::vendor::{deepseek, key_name_for, Vendor, VendorKey};

/// 六个平台。`Gemini` / `GrokBuild` 不在其中（上游无 DeepSeek preset）。
pub const DEEPSEEK_APPS: [AppType; 6] = [
    AppType::Codex,
    AppType::Claude,
    AppType::ClaudeDesktop,
    AppType::Hermes,
    AppType::OpenClaw,
    AppType::OpenCode,
];

/// 稳定派生的 provider id。**不含 app_type**（见模块文档）。
///
/// ⚠️ **分隔符不能省** —— 没有它 `(vendor="a", account="bc")` 与
/// `(vendor="ab", account="c")` 喂进哈希的字节流完全相同
/// （同型于 `operator::provision::provider_id_for` 那个闸）。
pub fn provider_id_for(vendor_id: &str, account_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(vendor_id.as_bytes());
    h.update(b"/");
    h.update(account_id.as_bytes());
    format!(
        "{}vendor-{:.16x}",
        crate::operator::managed::MANAGED_ID_PREFIX,
        h.finalize()
    )
}

/// 从官网 key 列表里筛出「本客户端为**这个账号**建过的」那些。
///
/// ⚠️ **精确相等，不是 `starts_with`** —— 用前缀匹配会命中
/// `LoongPort专用/a123-old` 之类（同型于 `operator::provision::claim_key`
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
/// [`deepseek::claude_role_models`]。
///
/// ## ⚠️ 生成配置与 `is_user_edited` 的基准**必须都走这个函数**
///
/// `is_user_edited` 靠「与重算的默认值整份比对」判断用户改没改过
/// （`operator::provision::is_user_edited`）。两边算法只要有一处不一致，
/// 结果就是**每个 DeepSeek 的 Claude 档位都显示「已手工维护」**，而用户一个字没改过。
///
/// 所以这个判断收在一个 pub 函数里，两个调用方（`provider_rows_for` 与
/// vendor 侧算 `user_edited` 那处）共用它，而不是各写一遍 `matches!(app, Claude | ..)`。
pub fn claude_roles_for(app: &AppType) -> Option<crate::operator::provision::ClaudeRoleModels> {
    matches!(app, AppType::Claude | AppType::ClaudeDesktop).then(deepseek::claude_role_models)
}

/// 一把 sk 展开成六条 `(app_type, settings_config)`。
///
/// 走 `operator::provision::settings_config_with_roles` —— 它的非 codex 分支复用上游
/// `deeplink::build_provider_from_request`，而那个 match 覆盖全部 8 个平台
/// （`deeplink/provider.rs:147`）⇒ 我们要的六个都在里面，不需要新写分派。
pub fn provider_rows_for(vendor: Vendor, api_key: &str) -> Vec<(AppType, Value)> {
    let display = vendor.display_name();
    DEEPSEEK_APPS
        .iter()
        .filter_map(|app| {
            let (base_url, model) = deepseek::config_for(app)?;
            let cfg = crate::operator::provision::settings_config_with_roles(
                app,
                api_key,
                display,
                base_url,
                model,
                claude_roles_for(app),
            )?;
            Some((app.clone(), cfg))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-plaintext");
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

    #[test]
    fn gemini_and_grokbuild_get_no_row() {
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-plaintext");
        let apps: Vec<String> = rows.iter().map(|(a, _)| a.as_str().to_string()).collect();
        assert!(!apps.contains(&"gemini".to_string()));
        assert!(!apps.contains(&"grokbuild".to_string()));
    }

    #[test]
    fn the_plaintext_key_lands_in_every_platform_config() {
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-unique-marker");
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
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
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
    /// 这是端到端的闸：`claude_roles_for` → `settings_config_with_roles` →
    /// 上游 `build_claude_settings` 三段都得通。中间任一段掉链子的症状都是
    /// 「配置里少了几个键」，而那要真机切过去用 sonnet 才发现。
    #[test]
    fn claude_config_carries_all_five_role_models() {
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
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
    /// `claude_roles_for` 对它们返回 `None` ⇒ `settings_config_with_roles` 不写
    /// fable / subagent。钉住它：那两个键是 Claude 专有的，写进 codex 的 TOML 或
    /// opencode 的配置里是噪音（上游那些分支压根不读）。
    #[test]
    fn non_claude_platforms_get_no_role_keys() {
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
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

    /// ⭐ **`is_user_edited` 在官网直连这条路上的三条基本行为。**
    ///
    /// ## 第 1 条为什么最要紧
    ///
    /// 「刚生成的配置算没改过」——这条挂了的症状是**每个 DeepSeek 的 Claude 档位都
    /// 显示「已手动维护」**，而用户一个字没改过。而它挂的原因通常很隐蔽：生成配置与
    /// 算基准两边的 `roles` 不一致（一边带 fable/subagent、一边不带）。
    ///
    /// 那是本轮加分档时最容易踩的坑，所以这条闸直接钉「两边走同一个
    /// `claude_roles_for`」的结果，而不是各自的实现。
    ///
    /// ## 第 3 条是「自愈」
    ///
    /// 用户把配置改回默认值 ⇒ 标记自动消失。这是「不存标记、靠比对」这个设计的
    /// 主要好处（存标记会留一个用户清不掉的永久假阳性）。
    #[test]
    fn user_edited_verdict_round_trips_for_the_claude_row() {
        use crate::operator::provision::is_user_edited_with_roles;

        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
        let (app, generated) = rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Claude))
            .expect("claude 那条");
        let (base_url, model) = deepseek::config_for(app).expect("claude 有配置");
        let verdict = |cfg: &Value| {
            is_user_edited_with_roles(
                cfg,
                app,
                Vendor::DeepSeek.display_name(),
                base_url,
                model,
                claude_roles_for(app),
            )
        };

        // 1. 刚生成的 ⇒ 没改过。
        assert_eq!(
            verdict(generated),
            Some(false),
            "刚生成的配置该算「没改过」—— 报 true 通常是生成与基准两边的 roles 不一致\
             （一边带 fable/subagent、一边不带），症状是全部 Claude 档位集体误报"
        );

        // 2. 改一个值 ⇒ 改过了。
        let mut edited = generated.clone();
        edited["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"] = Value::String("deepseek-v4-pro".into());
        assert_eq!(
            verdict(&edited),
            Some(true),
            "把 sonnet 改成 pro 是用户编辑，必须认出来 —— 认不出的后果是下次\
             「获取密钥」把他的改动整份冲掉"
        );

        // 3. 改回默认 ⇒ 标记自动消失（**自愈**）。
        let restored = generated.clone();
        assert_eq!(
            verdict(&restored),
            Some(false),
            "改回默认值后标记该自动消失 —— 那是「不存标记、靠比对」这个设计的主要好处"
        );
    }

    /// 分档的两个新键**参与**比对。
    ///
    /// 单独钉它是因为 `normalize_for_comparison` 只抹字符串首尾空白、
    /// **不动结构**（它的文档写了这条）：少一个键算改过、多一个键也算改过。
    /// 若哪天有人给它加上「忽略未知键」的宽松逻辑，用户删掉 subagent 那行就会
    /// 被判成「没改过」⇒ 下次 provision 静默把它加回去。
    #[test]
    fn removing_a_role_key_counts_as_edited() {
        use crate::operator::provision::is_user_edited_with_roles;

        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
        let (app, generated) = rows
            .iter()
            .find(|(a, _)| matches!(a, AppType::Claude))
            .expect("claude 那条");
        let (base_url, model) = deepseek::config_for(app).expect("claude 有配置");

        for key in [
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            let mut stripped = generated.clone();
            stripped["env"]
                .as_object_mut()
                .expect("env 是对象")
                .remove(key);
            assert_eq!(
                is_user_edited_with_roles(
                    &stripped,
                    app,
                    Vendor::DeepSeek.display_name(),
                    base_url,
                    model,
                    claude_roles_for(app),
                ),
                Some(true),
                "删掉 {key} 是用户编辑（结构变了），必须认出来"
            );
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
                claude_roles_for(app).is_some(),
                expected,
                "{} 的角色分档判定错了 —— ClaudeDesktop 与 Claude 同形，两个都要有",
                app.as_str()
            );
        }
    }

    /// ⚠️ **`is_user_edited` 目前只覆盖 codex / claude 系 / gemini。**
    ///
    /// hermes / openclaw / opencode 落到 `api_key_location` 的 `_ => None` ⇒
    /// 它们的 `user_edited` 恒为 `None`（「判不了」）⇒ **界面上永远不显示
    /// 「已手动维护」标记，「恢复默认」也用不了**（`extract_api_key` 读不出 sk）。
    ///
    /// 这条测试钉住**当前的真实行为**，不是钉住「这样是对的」——
    /// 缺口已记进代码仓 `TODO.md`（要改 `api_key_location` 的 `(section, field)`
    /// 两段结构才能表达 hermes / openclaw 那种 sk 在**顶层**的形状，
    /// 而那会动 `patch_api_key` / `extract_api_key` 的签名与 operator 侧全部调用方）。
    ///
    /// **哪天补上了，这条测试会红** —— 那时把它改成断言 `Some(false)`，
    /// 别以为是回归。
    #[test]
    fn user_edited_is_currently_undecidable_for_three_platforms() {
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
        for (app, cfg) in rows.iter() {
            let (base_url, model) = deepseek::config_for(app).expect("有配置");
            let verdict = crate::operator::provision::is_user_edited_with_roles(
                cfg,
                app,
                Vendor::DeepSeek.display_name(),
                base_url,
                model,
                claude_roles_for(app),
            );
            match app {
                // 接了的：刚生成的配置算「没改过」。
                AppType::Codex | AppType::Claude | AppType::ClaudeDesktop => assert_eq!(
                    verdict,
                    Some(false),
                    "{} 接了 api_key_location，刚生成的该算没改过",
                    app.as_str()
                ),
                // 没接的：判不了。**`None` 而不是 `Some(false)`** ——
                // 报「没改过」等于断言「刷新不会覆盖你的改动」，而事实是不知道。
                _ => assert_eq!(
                    verdict,
                    None,
                    "{} 还没接 api_key_location，该老实返回 None（见本测试文档）",
                    app.as_str()
                ),
            }
        }
    }

    /// 本轮 provision 覆盖的那几个平台（测试辅助）。
    fn provision_apps() -> impl Iterator<Item = &'static AppType> {
        DEEPSEEK_APPS.iter()
    }

    #[test]
    fn codex_config_omits_requires_openai_auth() {
        let rows = provider_rows_for(Vendor::DeepSeek, "sk-x");
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
            crate::operator::managed::is_managed(&id),
            "要命中 MANAGED_ID_PREFIX，守卫/前端过滤/托盘菜单才免费继承"
        );
    }

    /// ⚠️ **精确相等，不是前缀** —— `a12` 不能命中 `a123`（同型于 operator 侧
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
