//! 官方供应商种子数据
//!
//! 启动时调用 `Database::init_default_official_providers` 把这些条目
//! 写入 `providers` 表，让所有用户都能看到一个"一键切回官方"的入口。
//!
//! 字段与前端预设保持一致，参见：
//! - `src/config/claudeProviderPresets.ts`（"Claude Official"）
//! - `src/config/codexProviderPresets.ts`（"OpenAI Official"）
//! - `src/config/geminiProviderPresets.ts`（"Google Official"）
//! - `src/components/providers/forms/GrokBuildProviderForm.tsx`（"Grok Official"）

use crate::app_config::AppType;

pub(crate) const CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID: &str = "claude-desktop-official";
pub(crate) const CODEX_OFFICIAL_PROVIDER_ID: &str = "codex-official";
pub(crate) const GROKBUILD_OFFICIAL_PROVIDER_ID: &str = "grokbuild-official";

/// 单条官方供应商种子定义。
pub(crate) struct OfficialProviderSeed {
    pub id: &'static str,
    pub app_type: AppType,
    pub name: &'static str,
    pub website_url: &'static str,
    pub icon: &'static str,
    pub icon_color: &'static str,
    /// settings_config 的 JSON 字符串，每个 app 结构不同。
    pub settings_config_json: &'static str,
}

/// Claude / Claude Desktop / Codex / Gemini 的官方预设。
///
/// id 固定，便于幂等检查；name 直接用英文原名（与前端预设一致），不做 i18n。
pub(crate) const OFFICIAL_SEEDS: &[OfficialProviderSeed] = &[
    OfficialProviderSeed {
        id: "claude-official",
        app_type: AppType::Claude,
        name: "Claude Official",
        website_url: "https://www.anthropic.com/claude-code",
        icon: "anthropic",
        icon_color: "#D4915D",
        // 空 env 让用户走 Claude CLI 默认认证流程
        settings_config_json: r#"{"env":{}}"#,
    },
    OfficialProviderSeed {
        id: CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
        app_type: AppType::ClaudeDesktop,
        name: "Claude Desktop Official",
        website_url: "https://claude.ai/download",
        icon: "anthropic",
        icon_color: "#D4915D",
        // 空 env 只是占位；切换该 provider 时会恢复 Claude Desktop 1P 模式
        settings_config_json: r#"{"env":{}}"#,
    },
    OfficialProviderSeed {
        id: CODEX_OFFICIAL_PROVIDER_ID,
        app_type: AppType::Codex,
        name: "OpenAI Official",
        website_url: "https://chatgpt.com/codex",
        icon: "openai",
        icon_color: "#00A67E",
        // 空 auth + 空 config 让用户走 ChatGPT Plus/Pro OAuth
        settings_config_json: r#"{"auth":{},"config":""}"#,
    },
    OfficialProviderSeed {
        id: "gemini-official",
        app_type: AppType::Gemini,
        name: "Google Official",
        website_url: "https://ai.google.dev/",
        icon: "gemini",
        icon_color: "#4285F4",
        // 空 env + 空 config 让用户走 Google OAuth
        settings_config_json: r#"{"env":{},"config":{}}"#,
    },
    OfficialProviderSeed {
        id: GROKBUILD_OFFICIAL_PROVIDER_ID,
        app_type: AppType::GrokBuild,
        name: "Grok Official",
        website_url: "https://x.ai/grok",
        icon: "grok",
        icon_color: "currentColor",
        // 空 config = 不写自定义模型表，Grok CLI 回落到自带的 xAI OAuth 登录
        settings_config_json: r#"{"config":""}"#,
    },
];

/// 判断给定的 provider id 是否属于内置官方种子。
///
/// 单一事实源：直接扫描 `OFFICIAL_SEEDS`，避免在多处重复维护 id 列表。
pub(crate) fn is_official_seed_id(id: &str) -> bool {
    OFFICIAL_SEEDS.iter().any(|seed| seed.id == id)
}

/// 这个 app 有没有官方 seed provider；有则返回它的固定 id。
///
/// 官方 seed 的「空 env / 空 config」就是该 CLI 刚装好时的默认认证状态（各 seed
/// 的注释原文），所以「切到官方」=「回默认认证」。强删中转站账号的收尾拿它把
/// 受影响 app 安置回官方（`commands::relay::switch_affected_apps_to_official`）。
///
/// 返回 `None` 的 app（codex-image 生图，及 additive 类）没有这个回落 —— 调用方
/// 维持悬空自愈。与 `is_official_seed_id` 同一份 `OFFICIAL_SEEDS`，别另列清单。
pub(crate) fn official_seed_id(app_type: &AppType) -> Option<&'static str> {
    OFFICIAL_SEEDS
        .iter()
        .find(|seed| seed.app_type == *app_type)
        .map(|seed| seed.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_seeds_include_claude_desktop() {
        let seed = OFFICIAL_SEEDS
            .iter()
            .find(|seed| seed.id == CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID)
            .expect("claude desktop official seed");

        assert_eq!(seed.app_type, AppType::ClaudeDesktop);
        assert!(is_official_seed_id(CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID));
    }

    #[test]
    fn official_seeds_include_grokbuild() {
        let seed = OFFICIAL_SEEDS
            .iter()
            .find(|seed| seed.id == GROKBUILD_OFFICIAL_PROVIDER_ID)
            .expect("grok build official seed");

        assert_eq!(seed.app_type, AppType::GrokBuild);
        assert!(is_official_seed_id(GROKBUILD_OFFICIAL_PROVIDER_ID));
        // 空 config = 官方登录态：切换时不注入自定义模型表
        assert_eq!(seed.settings_config_json, r#"{"config":""}"#);
    }

    /// 强删收尾靠这份映射决定「哪个 app 能回落官方」。**会红的改法**：给新 app
    /// 加官方 seed 却忘了这里（单源扫描，加 seed 即自动覆盖）；或有人把 codex-image
    /// 的回落写成依赖它 —— 它没有 seed，这里必须是 `None`。
    #[test]
    fn official_seed_id_maps_text_apps_and_denies_codex_image() {
        assert_eq!(official_seed_id(&AppType::Claude), Some("claude-official"));
        assert_eq!(
            official_seed_id(&AppType::ClaudeDesktop),
            Some(CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID)
        );
        assert_eq!(
            official_seed_id(&AppType::Codex),
            Some(CODEX_OFFICIAL_PROVIDER_ID)
        );
        assert_eq!(official_seed_id(&AppType::Gemini), Some("gemini-official"));
        assert_eq!(
            official_seed_id(&AppType::GrokBuild),
            Some(GROKBUILD_OFFICIAL_PROVIDER_ID)
        );
        // 生图没有「官方登录」可回；additive 类 app 永远进不了在用名单（current 恒空）
        assert_eq!(official_seed_id(&AppType::CodexImage), None);
        assert_eq!(official_seed_id(&AppType::OpenCode), None);
    }
}
