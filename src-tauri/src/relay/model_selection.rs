//! Deterministic model policy shared by protocol provisioning and client configuration.
//! Callers supply a policy snapshot. This module does not load caches, fetch data or schedule work.

use crate::app_config::AppType;
use crate::claude_desktop_config::ONE_M_CONTEXT_MARKER;

/// Trim, sort and deduplicate model identifiers from a protocol response.
pub fn normalize_model_names(models: Vec<String>) -> Vec<String> {
    let mut normalized: Vec<String> = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

/// Explicit Claude role assignments, shared by selection and configuration generation.
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

/// Built-in text-model fallback when no usable catalog is available.
pub const DEFAULT_MODEL: &str = "gpt-5.6-sol";

/// Image families in preference order; detection and ranking share this list.
const IMAGE_MODEL_FAMILIES: &[&str] = &["gpt-image-", "nano-banana"];

/// Route image-only Codex tiers to the image client.
pub fn image_tier_app_type(app_type: &AppType, model: &str) -> AppType {
    if matches!(app_type, AppType::Codex) && is_image_model(model) {
        AppType::CodexImage
    } else {
        app_type.clone()
    }
}

/// Built-in selection helper for image-model tests.
#[cfg(test)]
fn pick_model(available: Option<&[String]>) -> String {
    pick_model_with(available, &ModelSelectionTables::builtin())
}

/// Choose the newest image model for image-only groups, otherwise the configured text default.
pub fn pick_model_with(available: Option<&[String]>, tables: &ModelSelectionTables) -> String {
    let Some(models) = available else {
        return tables.default_model.clone();
    };
    // 有任何一个非生图模型 ⇒ 这不是纯生图分组，照旧写默认文本模型。
    // 纯生图的判据见 [`is_pure_image_group`]（唯源），归一化在 [`is_image_model`] 里。
    if !is_pure_image_group(models) {
        return tables.default_model.clone();
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
                // 选型结果跟着抖 ⇒ 同一分组两次 provision 写出不同的模型名）。
                .then_with(|| a.as_str().cmp(b.as_str()))
        })
        .cloned()
        .unwrap_or_else(|| tables.default_model.clone())
}

/// Selected primary model and optional Claude roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierModels {
    pub main: String,
    pub claude_roles: Option<ClaudeRoleModels>,
}

/// Built-in main/opus/fable preferences, in priority order.
const CLAUDE_OPUS_CANDIDATES: &[&str] = &["claude-opus-5", "gpt-5.6-sol", "deepseek-v4-pro"];

/// Built-in sonnet/subagent preferences, in priority order.
const CLAUDE_SONNET_CANDIDATES: &[&str] =
    &["claude-sonnet-5", "gpt-5.6-terra", "deepseek-v4-flash"];

/// Built-in haiku preferences, in priority order.
const CLAUDE_HAIKU_CANDIDATES: &[&str] = &["claude-haiku-4-5", "gpt-5.6-luna", "deepseek-v4-flash"];

/// Built-in Codex preferences, matched against advertised models.
const CODEX_MAIN_CANDIDATES: &[&str] = &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.4"];

/// Built-in context-capability prefixes; signed remote rules may override them.
const ONE_M_MODEL_PREFIXES: &[&str] = &[
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "claude-fable-5",
    "gpt-5.6-",
    "deepseek-v4-",
];

/// An immutable policy snapshot consumed by model selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectionTables {
    /// 目录拉不到时写的主模型回落值（内置 [`DEFAULT_MODEL`]）。
    pub default_model: String,
    /// codex 主模型候选（优先级序，内置 `CODEX_MAIN_CANDIDATES`）。
    pub codex_main: Vec<String>,
    /// claude 平台「最强档」候选（内置 `CLAUDE_OPUS_CANDIDATES`）。
    pub claude_opus: Vec<String>,
    /// claude 平台「次强档」候选（内置 `CLAUDE_SONNET_CANDIDATES`）。
    pub claude_sonnet: Vec<String>,
    /// claude 平台「弱档」候选（内置 `CLAUDE_HAIKU_CANDIDATES`）。
    pub claude_haiku: Vec<String>,
    /// 支持 1M 上下文的模型前缀（内置 [`ONE_M_MODEL_PREFIXES`]）。远端覆盖的动机：
    /// 新一代旗舰发布时免发版跟进。⚠️ 与候选表不同，前缀**没有** `first_hit` 式的
    /// 「坏值只敢不生效」保护 —— 多认一个前缀 = 给不支持的模型声明 1M（Claude Code
    /// 按更大窗口跑），所以远端这张表由维护者签名发布、同样宁保守。
    pub one_m_prefixes: Vec<String>,
}

impl ModelSelectionTables {
    /// 内置表。测试与「无远端覆盖」的回落共用这一份 —— 也因此选型测试不碰真实缓存。
    pub fn builtin() -> Self {
        Self {
            default_model: DEFAULT_MODEL.to_string(),
            codex_main: CODEX_MAIN_CANDIDATES
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
            claude_opus: CLAUDE_OPUS_CANDIDATES
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
            claude_sonnet: CLAUDE_SONNET_CANDIDATES
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
            claude_haiku: CLAUDE_HAIKU_CANDIDATES
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
            one_m_prefixes: ONE_M_MODEL_PREFIXES
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
        }
    }
}

/// Select client models from the advertised catalog and an explicit policy snapshot.
pub fn pick_tier_models_with(
    app_type: &AppType,
    models: Option<&[String]>,
    tables: &ModelSelectionTables,
) -> TierModels {
    let Some(models) = models else {
        return TierModels {
            main: tables.default_model.clone(),
            claude_roles: None,
        };
    };
    // 纯生图分组（只有生图模型）：写它自己的生图模型，不分角色。
    if is_pure_image_group(models) {
        return TierModels {
            main: pick_model_with(Some(models), tables),
            claude_roles: None,
        };
    }
    match app_type {
        AppType::Claude => pick_claude_tier_models(models, tables),
        AppType::Codex | AppType::CodexImage => TierModels {
            main: first_hit(&tables.codex_main, models).unwrap_or_else(|| models[0].clone()),
            claude_roles: None,
        },
        AppType::Gemini => TierModels {
            main: models
                .iter()
                .find(|model| model.to_ascii_lowercase().starts_with("gemini-"))
                .cloned()
                .unwrap_or_else(|| models[0].clone()),
            claude_roles: None,
        },
        AppType::GrokBuild => TierModels {
            // 版本号排新而不是照抄列表首项（`normalize_model_names` 已排序，字典序会
            // 把 grok-4.5 排在 grok-4.6 前面 —— 选旧弃新）。与 codex 的候选表不同，
            // 这里不写死代名：grok 家族没有「跨全部站点查证过」的候选，而版本号是
            // 模型名自带的数据，新一代自动跟上，不必改代码。
            main: models
                .iter()
                .filter(|model| model.to_ascii_lowercase().starts_with("grok"))
                .max_by(|a, b| {
                    grok_model_rank(a)
                        .cmp(&grok_model_rank(b))
                        .then_with(|| a.as_str().cmp(b.as_str()))
                })
                .cloned()
                .unwrap_or_else(|| models[0].clone()),
            claude_roles: None,
        },
        _ => TierModels {
            main: models[0].clone(),
            claude_roles: None,
        },
    }
}

/// Select models using built-in policy only.
pub fn pick_tier_models(app_type: &AppType, models: Option<&[String]>) -> TierModels {
    pick_tier_models_with(app_type, models, &ModelSelectionTables::builtin())
}

/// Choose each Claude role from the catalog, falling back to an available text model.
fn pick_claude_tier_models(models: &[String], tables: &ModelSelectionTables) -> TierModels {
    let first_text = models.iter().find(|m| !is_image_model(m)).cloned();
    let opus = first_hit(&tables.claude_opus, models);
    let sonnet = first_hit(&tables.claude_sonnet, models);
    let haiku = first_hit(&tables.claude_haiku, models);
    let main = opus
        .clone()
        .or_else(|| sonnet.clone())
        .or_else(|| first_text.clone())
        .unwrap_or_else(|| tables.default_model.clone());
    let opus = opus.unwrap_or_else(|| main.clone());
    let sonnet = sonnet.unwrap_or_else(|| main.clone());
    let haiku = haiku.unwrap_or_else(|| main.clone());
    // claude 平台档位声明 1M 上下文：对支持 1M 的模型附 `[1M]` 后缀。
    //
    // `[1M]` 是 Claude Code 认的本地能力声明（转发到上游前剥掉），
    // codex 档位的 config.toml 不认后缀 —— 所以只在这里（claude 平台）加。
    let one_m = |m: String| maybe_one_m(tables, &m);
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

/// Return the first exact candidate match in the advertised catalog.
fn first_hit(candidates: &[String], models: &[String]) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| models.iter().any(|model| model == *candidate))
        .map(|candidate| candidate.to_string())
}

/// Check context capability against the supplied policy snapshot.
fn supports_one_m(tables: &ModelSelectionTables, model: &str) -> bool {
    let m = model.trim();
    tables
        .one_m_prefixes
        .iter()
        .any(|p| m.starts_with(p.as_str()))
}

/// Append the canonical Claude context marker for supported models.
pub(super) fn maybe_one_m(tables: &ModelSelectionTables, model: &str) -> String {
    if supports_one_m(tables, model) {
        format!("{model}{ONE_M_CONTEXT_MARKER}")
    } else {
        model.to_string()
    }
}

/// Rank image models by family preference, then numeric version segments.
fn image_model_rank(model: &str) -> (u32, Vec<u32>) {
    let normalized = model.trim().to_ascii_lowercase();
    for (index, prefix) in IMAGE_MODEL_FAMILIES.iter().enumerate() {
        let Some(rest) = normalized.strip_prefix(prefix) else {
            continue;
        };
        let precedence = (IMAGE_MODEL_FAMILIES.len() - index - 1) as u32;
        // `gpt-image-1.5-mini` / `nano-banana-2` → 只取连续的数字与点，
        // 前导 `-`（家族前缀不带尾连字符时）与后缀（`-mini`）不参与比较。
        let version: String = rest
            .trim_start_matches('-')
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let segments = version
            .split('.')
            .filter(|seg| !seg.is_empty())
            .filter_map(|seg| seg.parse::<u32>().ok())
            .collect();
        return (precedence, segments);
    }
    if is_grok_image_model(&normalized) {
        let rest = normalized.strip_prefix("grok-imagine-image").unwrap_or("");
        let version: String = rest
            .trim_start_matches('-')
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let segments = version
            .split('.')
            .filter(|seg| !seg.is_empty())
            .filter_map(|seg| seg.parse::<u32>().ok())
            .collect();
        return (0, segments);
    }
    (0, Vec::new())
}

/// Extract numeric Grok version segments for generation ordering.
fn grok_model_rank(model: &str) -> Vec<u32> {
    model
        .rsplit('-')
        .next()
        .map(|tail| {
            tail.split('.')
                .filter(|seg| !seg.is_empty())
                .filter_map(|seg| seg.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Recognize supported image families after normalizing case and whitespace.
pub fn is_image_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    IMAGE_MODEL_FAMILIES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
        || is_grok_image_model(&normalized)
}

/// Recognize the supported Grok image names, excluding video models.
fn is_grok_image_model(normalized: &str) -> bool {
    normalized == "grok-imagine"
        || normalized == "grok-imagine-edit"
        || normalized.starts_with("grok-imagine-image")
}

/// Whether every advertised model belongs to an image family.
pub fn is_pure_image_group(models: &[String]) -> bool {
    models.iter().all(|model| is_image_model(model))
}

/// The same platform filtering is used by configuration generation and availability snapshots.
pub(crate) fn filter_models(app: &AppType, models: &[String]) -> Vec<String> {
    models
        .iter()
        .filter(|model| {
            if matches!(app, AppType::Gemini) {
                model.to_ascii_lowercase().starts_with("gemini-")
            } else {
                !is_image_model(model)
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn models(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_image_only_group_gets_its_own_image_model() {
        let models = vec!["gpt-image-2".to_string()];
        assert_eq!(pick_model(Some(&models)), "gpt-image-2");
    }

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

    #[test]
    fn an_unparsable_image_model_loses_to_a_versioned_one() {
        let mixed = vec!["gpt-image-preview".to_string(), "gpt-image-2".to_string()];
        assert_eq!(pick_model(Some(&mixed)), "gpt-image-2");
    }

    #[test]
    fn a_group_with_text_models_keeps_the_default_text_model() {
        let mixed = vec![
            "gpt-image-2".to_string(),
            "gpt-5.6-sol".to_string(),
            "gpt-5.4".to_string(),
        ];
        assert_eq!(pick_model(Some(&mixed)), DEFAULT_MODEL);
    }

    #[test]
    fn an_unknown_model_list_falls_back_to_the_default() {
        assert_eq!(pick_model(None), DEFAULT_MODEL);
        assert_eq!(
            pick_model(Some(&[])),
            DEFAULT_MODEL,
            "空列表也要回落 —— 否则会写一个空 model 出去"
        );
    }

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

    #[test]
    fn the_image_column_is_a_fixed_point() {
        assert_eq!(
            image_tier_app_type(&AppType::CodexImage, "gpt-image-2"),
            AppType::CodexImage,
        );
    }

    #[test]
    fn the_image_model_predicate_normalizes_case_and_whitespace() {
        assert!(is_image_model("GPT-Image-2"), "大写没被归一化");
        assert!(is_image_model("  gpt-image-2  "), "空白没被裁掉");
        assert!(is_image_model("GPT-IMAGE-1.5"));
        // 归一化不该把无关的名字也放进来。
        assert!(!is_image_model("gpt-5.6-sol"));
        assert!(!is_image_model("image-gpt-2"));
    }

    #[test]
    fn pick_model_handles_non_lowercase_model_ids() {
        let shouty = vec!["GPT-Image-2".to_string()];
        assert_eq!(
            pick_model(Some(&shouty)),
            "GPT-Image-2",
            "大写的纯生图分组被误判成有文本模型 ⇒ 写回了默认文本模型"
        );
    }

    #[test]
    fn is_image_model_only_matches_the_image_families() {
        assert!(is_image_model("gpt-image-2"));
        assert!(is_image_model("gpt-image-3-turbo"));
        // GPT Image 2.5（2026-09-10 准入：sub2api 在 /v1/models 广播的 flare/sunburst
        // 双变体，前缀表天然覆盖，见 IMAGE_MODEL_FAMILIES 的文档）。
        assert!(is_image_model("gpt-image-2.5-sunburst"));
        assert!(is_image_model("gpt-image-2.5-flare"));
        // nano-banana 家族（2026-09-05 实测准入，见 IMAGE_MODEL_FAMILIES 的文档）。
        assert!(is_image_model("nano-banana-2"));
        assert!(is_image_model("Nano-Banana-Pro"));
        assert!(!is_image_model(DEFAULT_MODEL));
        assert!(!is_image_model("gpt-5.4-mini"));
        assert!(!is_image_model(""));
    }

    #[test]
    fn pick_model_prefers_the_newest_gpt_image_generation() {
        let models = vec![
            "gpt-image-2".to_string(),
            "gpt-image-2.5-flare".to_string(),
            "gpt-image-2.5-sunburst".to_string(),
        ];
        assert_eq!(pick_model(Some(&models)), "gpt-image-2.5-sunburst");
    }

    #[test]
    fn grok_image_family_matches_the_upstream_allowlist_exactly() {
        // 三条白名单。
        assert!(is_image_model("grok-imagine"));
        assert!(is_image_model("grok-imagine-edit"));
        assert!(is_image_model("grok-imagine-image"));
        assert!(is_image_model("grok-imagine-image-quality"));
        // 上游不认的近亲：video 是视频模型（写了转发不了），grok-4 是文本模型。
        assert!(!is_image_model("grok-imagine-video"));
        assert!(!is_image_model("grok-4"));
        assert!(!is_image_model("grok-code-4.6"));
        // 归一化与 GPT 族同一待遇。
        assert!(is_image_model("  Grok-Imagine-Image  "));
    }

    #[test]
    fn a_grok_only_group_picks_its_own_model() {
        let only = vec![
            "grok-imagine-image".to_string(),
            "grok-imagine-image-2".to_string(),
        ];
        assert!(is_pure_image_group(&only));
        assert_eq!(pick_model(Some(&only)), "grok-imagine-image-2");

        // 跨家族混编：gpt-image 靠表内家族优先级胜出。
        let mixed = vec!["grok-imagine-image".to_string(), "gpt-image-2".to_string()];
        assert!(is_pure_image_group(&mixed));
        assert_eq!(pick_model(Some(&mixed)), "gpt-image-2");
    }

    #[test]
    fn gpt_image_wins_over_nano_banana_when_both_families_exist() {
        let mixed = vec!["nano-banana-2".to_string(), "gpt-image-2".to_string()];
        assert!(
            is_pure_image_group(&mixed),
            "两个都是生图模型，该判纯生图分组"
        );
        assert_eq!(
            pick_model(Some(&mixed)),
            "gpt-image-2",
            "跨家族并存时 gpt-image 家族必须胜出"
        );
    }

    #[test]
    fn a_nano_banana_only_group_gets_its_own_newest_model() {
        let only = vec!["nano-banana-2".to_string()];
        assert!(is_pure_image_group(&only));
        assert_eq!(
            pick_model(Some(&only)),
            "nano-banana-2",
            "独家生图模型必须被原样写出 —— 写 DEFAULT_MODEL 是选中即 404"
        );
        let newer = vec!["nano-banana-2".to_string(), "nano-banana-3".to_string()];
        assert_eq!(pick_model(Some(&newer)), "nano-banana-3");
    }

    #[test]
    fn pick_tier_models_with_honors_remote_candidates() {
        let tables = ModelSelectionTables {
            codex_main: vec!["gpt-6-astra".into(), "gpt-5.6-sol".into()],
            ..ModelSelectionTables::builtin()
        };
        // 远端首位不在目录 → 顺延到次位（内置同款纪律：不写目录里不存在的模型）。
        let catalog = vec!["gpt-5.6-sol".to_string(), "gpt-5.6-terra".to_string()];
        assert_eq!(
            pick_tier_models_with(&AppType::Codex, Some(&catalog), &tables).main,
            "gpt-5.6-sol"
        );
        // 目录里有远端首位 → 命中。
        let with_new = vec!["gpt-6-astra".to_string()];
        assert_eq!(
            pick_tier_models_with(&AppType::Codex, Some(&with_new), &tables).main,
            "gpt-6-astra"
        );

        // claude 角色表覆盖；挑出的名字照常过 `[1M]` 声明规则（opus-6 不在名单 → 无后缀）。
        let claude_tables = ModelSelectionTables {
            claude_opus: vec!["claude-opus-6".into()],
            ..ModelSelectionTables::builtin()
        };
        let claude_catalog = vec!["claude-opus-6".to_string(), "claude-haiku-4-5".to_string()];
        let picked = pick_tier_models_with(&AppType::Claude, Some(&claude_catalog), &claude_tables);
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        assert_eq!(picked.main, "claude-opus-6");
        assert_eq!(roles.opus, "claude-opus-6");

        // default_model 覆盖：目录拉不到时的回落值跟着换。
        let fallback = ModelSelectionTables {
            default_model: "gpt-6-astra".into(),
            ..ModelSelectionTables::builtin()
        };
        assert_eq!(
            pick_tier_models_with(&AppType::Codex, None, &fallback).main,
            "gpt-6-astra"
        );
        // 纯生图分组不受选型表影响（它写自己的生图模型，不走候选）。
        let image_group = vec!["gpt-image-2".to_string()];
        assert_eq!(
            pick_tier_models_with(&AppType::CodexImage, Some(&image_group), &fallback).main,
            "gpt-image-2"
        );
    }

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

    #[test]
    fn claude_tier_falls_to_lower_tier_when_top_is_absent() {
        let picked = pick_tier_models(&AppType::Claude, Some(&models(&["claude-sonnet-5"])));
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        assert_eq!(picked.main, "claude-sonnet-5[1m]");
        assert_eq!(roles.opus, "claude-sonnet-5[1m]");
        assert_eq!(roles.sonnet, "claude-sonnet-5[1m]");
        assert_eq!(roles.haiku, "claude-sonnet-5[1m]");
    }

    #[test]
    fn codex_main_uses_default_when_present_otherwise_shifts() {
        let with_default =
            pick_tier_models(&AppType::Codex, Some(&models(&["gpt-5.4", "gpt-5.6-sol"])));
        assert_eq!(with_default.main, "gpt-5.6-sol");
        assert!(with_default.claude_roles.is_none());

        let without_default = pick_tier_models(&AppType::Codex, Some(&models(&["gpt-5.6-terra"])));
        assert_eq!(without_default.main, "gpt-5.6-terra");
    }

    #[test]
    fn empty_model_catalog_uses_the_configured_default() {
        let mut tables = ModelSelectionTables::builtin();
        tables.default_model = "configured-default".into();
        for app in [
            AppType::Codex,
            AppType::Claude,
            AppType::Gemini,
            AppType::GrokBuild,
            AppType::OpenCode,
        ] {
            let picked = pick_tier_models_with(&app, Some(&[]), &tables);
            assert_eq!(picked.main, "configured-default", "{}", app.as_str());
            assert!(picked.claude_roles.is_none());
        }
    }

    #[test]
    fn tier_models_fall_back_to_default_when_list_unavailable() {
        let picked = pick_tier_models(&AppType::Claude, None);
        assert_eq!(picked.main, DEFAULT_MODEL);
        assert!(picked.claude_roles.is_none());
    }

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

    #[test]
    fn other_platforms_take_the_first_text_model() {
        let picked = pick_tier_models(
            &AppType::Gemini,
            Some(&models(&["gemini-3-flash", "gemini-3-pro"])),
        );
        assert_eq!(picked.main, "gemini-3-flash");
        assert!(picked.claude_roles.is_none());
    }

    #[test]
    fn claude_tier_pick_is_deterministic() {
        let list = models(&["gpt-5.4", "gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"]);
        assert_eq!(
            pick_tier_models(&AppType::Claude, Some(&list)),
            pick_tier_models(&AppType::Claude, Some(&list)),
        );
    }

    #[test]
    fn one_m_suffix_only_for_supported_generations() {
        let builtin = ModelSelectionTables::builtin();
        assert_eq!(maybe_one_m(&builtin, "claude-opus-5"), "claude-opus-5[1m]");
        assert_eq!(
            maybe_one_m(&builtin, "claude-sonnet-5"),
            "claude-sonnet-5[1m]"
        );
        assert_eq!(
            maybe_one_m(&builtin, "claude-haiku-4-5"),
            "claude-haiku-4-5[1m]"
        );
        assert_eq!(maybe_one_m(&builtin, "gpt-5.6-sol"), "gpt-5.6-sol[1m]");
        assert_eq!(maybe_one_m(&builtin, "gpt-5.6-terra"), "gpt-5.6-terra[1m]");
        assert_eq!(
            maybe_one_m(&builtin, "deepseek-v4-flash"),
            "deepseek-v4-flash[1m]"
        );
        // 旧代 / 裸名 / 其它家族：不声明。
        assert_eq!(maybe_one_m(&builtin, "gpt-5.4"), "gpt-5.4");
        assert_eq!(
            maybe_one_m(&builtin, "claude-sonnet-4-5"),
            "claude-sonnet-4-5"
        );
        assert_eq!(maybe_one_m(&builtin, "gemini-3-pro"), "gemini-3-pro");
        // 裸 gpt-5.6 不是可访问的模型 id，不该被当成「支持 1M」。
        assert!(!supports_one_m(&builtin, "gpt-5.6"));
        assert!(!supports_one_m(&builtin, "deepseek-v4"));
        assert!(supports_one_m(&builtin, "gpt-5.6-luna"));

        // 远端覆盖：新一代（claude-opus-6）免发版跟进，内置不在名单的旧代被替换掉。
        let next_gen = ModelSelectionTables {
            one_m_prefixes: vec!["claude-opus-6".into()],
            ..builtin.clone()
        };
        assert_eq!(
            maybe_one_m(&next_gen, "claude-opus-6"),
            "claude-opus-6[1m]",
            "远端名单让新代际立即拿到 1M 声明"
        );
        assert_eq!(
            maybe_one_m(&next_gen, "claude-opus-5"),
            "claude-opus-5",
            "整表替换而不是追加 —— 维护者显式复述想保留的条目"
        );
    }

    #[test]
    fn grok_default_picks_the_newest_generation() {
        let models = vec![
            "grok-4.5".to_string(),
            "grok-code-4.5".to_string(),
            "grok-4.6".to_string(),
        ];
        let picked = pick_tier_models(&AppType::GrokBuild, Some(&models));
        assert_eq!(picked.main, "grok-4.6");
        assert!(picked.claude_roles.is_none());

        // 没有该家族 → 回落列表首项，不报错（目录是异源数据，宁可用也不炸）
        let no_family = vec!["gpt-5.6-sol".to_string(), "gpt-5.6-terra".to_string()];
        let picked = pick_tier_models(&AppType::GrokBuild, Some(&no_family));
        assert_eq!(picked.main, "gpt-5.6-sol");
    }

    #[test]
    fn grok_model_rank_extracts_the_trailing_version() {
        assert_eq!(grok_model_rank("grok-4.5"), vec![4, 5]);
        assert_eq!(grok_model_rank("grok-4"), vec![4]);
        assert_eq!(grok_model_rank("grok-code-4.6"), vec![4, 6]);
        assert!(grok_model_rank("grok-4.10") > grok_model_rank("grok-4.6"));
        assert!(grok_model_rank("grok-custom").is_empty());
        assert!(grok_model_rank("grok-4.5") > grok_model_rank("grok-custom"));
    }

    #[test]
    fn fetched_model_names_are_stable_and_unique() {
        assert_eq!(
            normalize_model_names(vec![
                " gpt-b ".into(),
                "".into(),
                "gpt-a".into(),
                "gpt-b".into(),
            ]),
            vec!["gpt-a".to_string(), "gpt-b".to_string()]
        );
    }
}
