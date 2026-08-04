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

/// 一把 sk 展开成六条 `(app_type, settings_config)`。
///
/// 走 `operator::provision::settings_config_for` —— 它的非 codex 分支复用上游
/// `deeplink::build_provider_from_request`，而那个 match 覆盖全部 8 个平台
/// （`deeplink/provider.rs:147`）⇒ 我们要的六个都在里面，不需要新写分派。
pub fn provider_rows_for(vendor: Vendor, api_key: &str) -> Vec<(AppType, Value)> {
    let display = vendor.display_name();
    DEEPSEEK_APPS
        .iter()
        .filter_map(|app| {
            let (base_url, model) = deepseek::config_for(app)?;
            let cfg = crate::operator::provision::settings_config_for(
                app, api_key, display, base_url, model,
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
