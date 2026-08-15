//! 托盘菜单管理模块
//!
//! 负责系统托盘图标和菜单的创建、更新和事件处理。

use once_cell::sync::Lazy;
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem, Submenu, SubmenuBuilder};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::events::{PROFILE_APPLIED, PROVIDER_SWITCHED};
use crate::store::AppState;

use crate::config::OFFICIAL_WEBSITE;

const TEMPLATE_TYPE_OFFICIAL_SUBSCRIPTION: &str = "official_subscription";
const H_TIER_NAMES: &[&str] = &[crate::services::subscription::TIER_FIVE_HOUR];
const W_TIER_NAMES: &[&str] = &[
    crate::services::subscription::TIER_WEEKLY_LIMIT,
    crate::services::subscription::TIER_SEVEN_DAY,
    crate::services::subscription::TIER_SEVEN_DAY_OPUS,
    crate::services::subscription::TIER_SEVEN_DAY_SONNET,
];
// 月窗口分组：火山方舟 Agent/Coding Plan 的月窗口（5h/周/月 三档），
// 以及 Codex 免费方案的 30 天窗口（#3651）——两者都归入 "m" 档，避免免费
// Codex 账号在托盘里空白（前端 footer 能看到、托盘却不显示的不对称）。
const M_TIER_NAMES: &[&str] = &[
    crate::services::subscription::TIER_MONTHLY,
    crate::services::subscription::TIER_THIRTY_DAY,
];
// Grok credit 额度的兜底窗口（重置距离能识别为周/月时归入 w/m 组）
const CREDITS_TIER_NAMES: &[&str] = &[crate::services::subscription::TIER_CREDITS];
const GEMINI_PRO_TIER_NAMES: &[&str] = &[crate::services::subscription::TIER_GEMINI_PRO];
const GEMINI_FLASH_TIER_NAMES: &[&str] = &[crate::services::subscription::TIER_GEMINI_FLASH];
const GEMINI_FLASH_LITE_TIER_NAMES: &[&str] =
    &[crate::services::subscription::TIER_GEMINI_FLASH_LITE];
const TIER_LABEL_GROUPS: &[(&str, &[&str])] = &[
    ("h", H_TIER_NAMES),
    ("w", W_TIER_NAMES),
    ("m", M_TIER_NAMES),
    ("c", CREDITS_TIER_NAMES),
    ("p", GEMINI_PRO_TIER_NAMES),
    ("f", GEMINI_FLASH_TIER_NAMES),
    ("l", GEMINI_FLASH_LITE_TIER_NAMES),
];

/// 每个 app 分区的子菜单句柄，用于 usage 更新时就地改 label 而非整菜单重建。
/// `create_tray_menu` 每次重建都会整表覆盖写入，保证句柄始终指向当前活跃菜单。
static TRAY_SECTION_SUBMENUS: Lazy<
    std::sync::Mutex<std::collections::HashMap<AppType, Submenu<tauri::Wry>>>,
> = Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// 托盘菜单文本（国际化）
#[derive(Clone, Copy)]
pub struct TrayTexts {
    pub show_main: &'static str,
    pub open_website: &'static str,
    pub no_providers_label: &'static str,
    pub lightweight_mode: &'static str,
    pub quit: &'static str,
    pub _auto_label: &'static str,
    pub projects_label: &'static str,
    pub no_project_label: &'static str,
    /// 托盘「模型」子菜单标题（挂在 Codex 当前托管档位下）。
    pub tier_model_label: &'static str,
    /// 托盘切档位前的确认对话框标题。
    pub tier_switch_confirm_title: &'static str,
    /// 确认对话框正文，`{}` 处填目标显示名。
    pub tier_switch_confirm_body: &'static str,
    /// 确认对话框「退出并切换」按钮。
    pub tier_quit_and_switch: &'static str,
    /// 确认对话框「取消」按钮。
    pub cancel_label: &'static str,
    /// 托盘切档位失败时的错误对话框标题。
    pub tier_switch_failed_title: &'static str,
}

/// 将系统区域标识映射为托盘支持的语言码。
///
/// 镜像前端 `i18n/getInitialLanguage` 的判定顺序，确保首次安装
/// （`settings.language` 尚未写入）时托盘语言与界面语言一致：
/// 繁中系统（zh-TW/HK/MO/Hant）→ `zh-TW`，其余 zh → `zh`，
/// 日文 → `ja`，英文 → `en`，未知区域回退到 `zh`（与前端默认一致）。
fn map_locale_to_tray_language(locale: &str) -> &'static str {
    let locale = locale.to_lowercase();
    if locale == "zh" {
        "zh"
    } else if locale.starts_with("zh-tw")
        || locale.starts_with("zh-hk")
        || locale.starts_with("zh-mo")
        || locale.starts_with("zh-hant")
    {
        "zh-TW"
    } else if locale.starts_with("zh") {
        "zh"
    } else if locale.starts_with("ja") {
        "ja"
    } else if locale.starts_with("en") {
        "en"
    } else {
        "zh"
    }
}

/// 读取系统区域并映射为托盘语言码；取不到区域时回退到 `zh`。
fn detect_system_tray_language() -> &'static str {
    sys_locale::get_locale()
        .as_deref()
        .map(map_locale_to_tray_language)
        .unwrap_or("zh")
}

/// 解析托盘当前该用的语言码：用户显式设置的 `settings.language` 优先，
/// 未设置（首次安装）按系统区域回退。
///
/// 菜单构建（`create_tray_menu`）和点击后的确认对话框（`confirm_quit_chatgpt`）
/// 都从这里取 —— 两处各读一遍 settings 会长出两套判定顺序。
fn tray_language() -> String {
    match crate::settings::get_settings().language {
        Some(lang) => lang,
        None => detect_system_tray_language().to_string(),
    }
}

impl TrayTexts {
    pub fn from_language(language: &str) -> Self {
        match language {
            "en" => Self {
                show_main: "Open main window",
                open_website: "Open Official Website",
                no_providers_label: "(no providers)",
                lightweight_mode: "Lightweight Mode",
                quit: "Quit",
                _auto_label: "Auto (Failover)",
                projects_label: "Projects",
                no_project_label: "No project",
                tier_model_label: "Model",
                tier_switch_confirm_title: "Switch tier",
                tier_switch_confirm_body: "Switching to \u{201C}{}\u{201D} requires quitting \
                     the ChatGPT desktop app first. It will be reopened automatically \
                     after the switch.",
                tier_quit_and_switch: "Quit & Switch",
                cancel_label: "Cancel",
                tier_switch_failed_title: "Switch failed",
            },
            "ja" => Self {
                show_main: "メインウィンドウを開く",
                open_website: "公式サイトを開く",
                no_providers_label: "(プロバイダーなし)",
                lightweight_mode: "軽量モード",
                quit: "終了",
                _auto_label: "自動 (フェイルオーバー)",
                projects_label: "プロジェクト",
                no_project_label: "プロジェクトを使用しない",
                tier_model_label: "モデル",
                tier_switch_confirm_title: "プランを切り替え",
                tier_switch_confirm_body: "「{}」への切り替えには、先に ChatGPT デスクトップ\
                     アプリを終了する必要があります。切り替え後に自動で再起動します。",
                tier_quit_and_switch: "終了して切り替え",
                cancel_label: "キャンセル",
                tier_switch_failed_title: "切り替えに失敗しました",
            },
            "zh-TW" => Self {
                show_main: "開啟主介面",
                open_website: "開啟官方網站",
                no_providers_label: "(無供應商)",
                lightweight_mode: "輕量模式",
                quit: "退出",
                _auto_label: "自動 (故障轉移)",
                projects_label: "專案",
                no_project_label: "不使用專案",
                tier_model_label: "模型",
                tier_switch_confirm_title: "切換檔位",
                tier_switch_confirm_body: "切換到「{}」需要先退出 ChatGPT 桌面版，\
                     切換後會自動重新開啟。",
                tier_quit_and_switch: "退出並切換",
                cancel_label: "取消",
                tier_switch_failed_title: "切換失敗",
            },
            _ => Self {
                show_main: "打开主界面",
                open_website: "打开官方网站",
                no_providers_label: "(无供应商)",
                lightweight_mode: "轻量模式",
                quit: "退出",
                _auto_label: "自动 (故障转移)",
                projects_label: "项目",
                no_project_label: "不使用项目",
                tier_model_label: "模型",
                tier_switch_confirm_title: "切换档位",
                tier_switch_confirm_body: "切换到「{}」需要先退出 ChatGPT 桌面版，\
                     切换后会自动重新打开。",
                tier_quit_and_switch: "退出并切换",
                cancel_label: "取消",
                tier_switch_failed_title: "切换失败",
            },
        }
    }
}

/// 托盘应用分区配置。
///
/// 机器部分（事件 id 前缀、空提示 id）全部从 [`AppType::as_str`] 派生，
/// 不写字面量 —— 10 个分区 × 3 份手写 id 迟早漂移，而漂移的表现是
/// 「菜单项点上去没反应」（事件 id 对不上），编译器和测试都抓不到。
#[derive(Clone)]
pub struct TrayAppSection {
    pub app_type: AppType,
    /// 展示名（子菜单标题 + 日志用），与主界面 `appConfig` 的 label 一致。
    pub label: &'static str,
}

impl TrayAppSection {
    /// 该分区菜单项的事件 id 前缀：`<as_str>_`。
    pub fn event_prefix(&self) -> String {
        format!("{}_", self.app_type.as_str())
    }

    /// 「无供应商」占位项的事件 id：`<as_str>_empty`。
    pub fn empty_id(&self) -> String {
        format!("{}_empty", self.app_type.as_str())
    }
}

/// Auto 菜单项后缀
pub const AUTO_SUFFIX: &str = "auto";
pub const TRAY_ID: &str = "cc-switch";

/// 托盘覆盖全部 provider 型 app（顺序对齐主界面 `appConfig`），实际显示哪些
/// 由 `settings.visible_apps` 过滤 —— 主界面藏掉的 tab 托盘也不出现。
pub const TRAY_SECTIONS: [TrayAppSection; 10] = [
    TrayAppSection {
        app_type: AppType::Claude,
        label: "Claude",
    },
    TrayAppSection {
        app_type: AppType::ClaudeDesktop,
        label: "Claude Desktop",
    },
    TrayAppSection {
        app_type: AppType::Codex,
        label: "Codex",
    },
    TrayAppSection {
        app_type: AppType::CodexImage,
        label: "Codex Images",
    },
    TrayAppSection {
        app_type: AppType::Gemini,
        label: "Gemini",
    },
    TrayAppSection {
        app_type: AppType::GrokBuild,
        label: "Grok Build",
    },
    TrayAppSection {
        app_type: AppType::OpenCode,
        label: "OpenCode",
    },
    TrayAppSection {
        app_type: AppType::OpenClaw,
        label: "OpenClaw",
    },
    TrayAppSection {
        app_type: AppType::Hermes,
        label: "Hermes",
    },
    TrayAppSection {
        app_type: AppType::Pi,
        label: "Pi",
    },
];

/// 配色阈值（与前端 `utilizationColor` 语义一致）。
const UTIL_WARN_PCT: f64 = 70.0;
const UTIL_DANGER_PCT: f64 = 90.0;

fn emoji_for_utilization(pct: f64) -> &'static str {
    if pct >= UTIL_DANGER_PCT {
        "\u{1F534}" // 🔴
    } else if pct >= UTIL_WARN_PCT {
        "\u{1F7E0}" // 🟠
    } else {
        "\u{1F7E2}" // 🟢
    }
}

fn format_subscription_summary(
    quota: &crate::services::subscription::SubscriptionQuota,
) -> Option<String> {
    if !quota.success {
        return None;
    }

    let entries: Vec<(&str, f64)> = quota
        .tiers
        .iter()
        .map(|tier| (tier.name.as_str(), tier.utilization))
        .collect();
    let parts = labeled_tier_parts(&entries);

    if parts.is_empty() {
        return None;
    }

    // 色标取所有已选 tier 里最高的利用率——用户更关心"离上限多近"。
    let worst = parts
        .iter()
        .map(|(_, u)| *u)
        .fold(f64::NEG_INFINITY, f64::max);
    if !worst.is_finite() {
        return None;
    }

    let emoji = emoji_for_utilization(worst);
    let body = parts
        .iter()
        .map(|(label, u)| format!("{label}{}%", u.round() as i64))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("{emoji} {body}"))
}

fn labeled_tier_parts(entries: &[(&str, f64)]) -> Vec<(&'static str, f64)> {
    let mut parts = Vec::new();
    for &(label, tier_names) in TIER_LABEL_GROUPS {
        let max_utilization = entries
            .iter()
            .filter(|(name, _)| tier_names.contains(name))
            .map(|(_, utilization)| *utilization)
            .filter(|utilization| utilization.is_finite())
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(utilization) = max_utilization {
            parts.push((label, utilization));
        }
    }
    parts
}

fn tier_pct(data: &crate::provider::UsageData) -> Option<f64> {
    match (data.used, data.total) {
        (Some(used), Some(total)) if total > 0.0 => Some(used / total * 100.0),
        _ => None,
    }
}

fn format_script_summary(result: &crate::provider::UsageResult) -> Option<String> {
    if !result.success {
        return None;
    }
    let data = result.data.as_ref()?;
    if data.is_empty() {
        return None;
    }

    // commands::provider 的 token_plan / official_subscription 分支都会把
    // SubscriptionQuota 的每个 tier 扁平化为一条 UsageData（plan_name 承载
    // tier 名），所以这里按 plan_name 恢复托盘短标签。其余 usage 结果
    //（Copilot / balance / 自定义脚本）走 fallback。
    let entries: Vec<(&str, f64)> = data
        .iter()
        .filter_map(|d| Some((d.plan_name.as_deref()?, tier_pct(d)?)))
        .collect();
    let parts = labeled_tier_parts(&entries);
    if !parts.is_empty() {
        let worst = parts
            .iter()
            .map(|(_, u)| *u)
            .fold(f64::NEG_INFINITY, f64::max);
        let emoji = emoji_for_utilization(worst);
        let body = parts
            .iter()
            .map(|(label, u)| format!("{label}{}%", u.round() as i64))
            .collect::<Vec<_>>()
            .join(" ");
        return Some(format!("{emoji} {body}"));
    }

    let first = data.first()?;
    let pct = tier_pct(first)?;
    let emoji = emoji_for_utilization(pct);
    let plan = first.plan_name.as_deref().unwrap_or("");
    let rounded = pct.round() as i64;
    if plan.is_empty() {
        Some(format!("{} {}%", emoji, rounded))
    } else {
        Some(format!("{} {} {}%", emoji, plan, rounded))
    }
}

fn provider_uses_official_subscription(provider: &crate::provider::Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|m| m.usage_script.as_ref())
        .map(|script| {
            script.enabled
                && script.template_type.as_deref() == Some(TEMPLATE_TYPE_OFFICIAL_SUBSCRIPTION)
        })
        .unwrap_or(false)
}

fn format_usage_suffix(
    app_state: &AppState,
    app_type: &AppType,
    provider: &crate::provider::Provider,
    provider_id: &str,
) -> Option<String> {
    // 当前脚本是否启用：禁用/删除时不再沿用旧 UsageCache 结果，
    // 并顺手 invalidate，防止后续重建继续命中过期数据。
    let is_official_provider = provider.category.as_deref() == Some("official");
    let can_use_script = provider.has_usage_script_enabled()
        && (!is_official_provider || provider_uses_official_subscription(provider));
    if can_use_script {
        // 脚本缓存优先（覆盖 Copilot/coding_plan/balance/自定义脚本），借用访问避免克隆整条 UsageResult。
        if let Some(Some(s)) =
            app_state
                .usage_cache
                .with_script(app_type, provider_id, format_script_summary)
        {
            return Some(format!(" · {s}"));
        }
        if provider_uses_official_subscription(provider) {
            if let Some(Some(s)) = app_state
                .usage_cache
                .with_subscription(app_type, format_subscription_summary)
            {
                return Some(format!(" · {s}"));
            }
        }
    } else {
        app_state
            .usage_cache
            .invalidate_script(app_type, provider_id);
    }

    if !provider_uses_official_subscription(provider) {
        app_state.usage_cache.invalidate_subscription(app_type);
    }
    None
}

/// 对供应商列表排序：sort_index → created_at → name
fn sort_providers(
    providers: &indexmap::IndexMap<String, crate::provider::Provider>,
) -> Vec<(&String, &crate::provider::Provider)> {
    let mut sorted: Vec<_> = providers.iter().collect();
    sorted.sort_by(|(_, a), (_, b)| {
        match (a.sort_index, b.sort_index) {
            (Some(idx_a), Some(idx_b)) => return idx_a.cmp(&idx_b),
            (Some(_), None) => return std::cmp::Ordering::Less,
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            _ => {}
        }

        match (a.created_at, b.created_at) {
            (Some(time_a), Some(time_b)) => return time_a.cmp(&time_b),
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            _ => {}
        }

        a.name.cmp(&b.name)
    });
    sorted
}

/// 托盘子菜单里该列出哪些供应商：按上游规则排序，**托管档位与自建 provider 混排**。
///
/// 历史上这里曾把托管项整个剔除（`filter_unmanaged`），因为托盘点击直连
/// `ProviderService::switch`，绕过 `relay_switch_tier` 的「退出 ChatGPT → 切换 → 重开」
/// 编排。那等于把中转站用户最核心的快速切换入口整个藏掉。2026-08-15 起改为：
/// **托管档位进菜单，但点击必须路由到 [`crate::commands::relay::switch_tier_command`]
/// 那条编排**（见 `handle_provider_click` 的 is_managed 分支）—— 列表和路由是一对
/// 不变量，动其中一个必须同时动另一个。
///
/// 单独抽成函数是为了可测：`create_tray_menu` 要真的 `AppHandle` 才跑得起来
/// （`MenuItem::with_id` 拿的是 Tauri 运行时），单测只能测到「列表怎么算出来的」这一层，
/// 测不到「菜单项真的建了出来」。所以这个函数必须是菜单构建的**唯一**列表来源。
fn tray_menu_providers(
    providers: &indexmap::IndexMap<String, crate::provider::Provider>,
) -> Vec<(&String, &crate::provider::Provider)> {
    sort_providers(providers)
}

/// 「模型」子菜单的数据来源：当前档位是**托管 Codex 项**且落库模型目录非空时，
/// 返回 `(当前模型, 目录)`；其余情况 `None`（不挂子菜单）。
///
/// 与主界面 `TierInfo.models` 同一份 `modelCatalog`（`codex_models_from_settings`），
/// 非托管 provider / 非 Codex app 没有「选模型」这个概念，不预埋。
fn tier_model_choices(
    provider: &crate::provider::Provider,
    app_type: &AppType,
) -> Option<(Option<String>, Vec<String>)> {
    if !matches!(app_type, AppType::Codex) || !crate::relay::is_managed(&provider.id) {
        return None;
    }
    let models = crate::commands::codex_models_from_settings(&provider.settings_config);
    if models.is_empty() {
        return None;
    }
    Some((
        crate::relay::provision::extract_model(&provider.settings_config),
        models,
    ))
}

/// 处理项目 Profile 托盘事件，返回是否已处理
///
/// 事件 id 形如 `profile_<scope>_<uuid>`（同一项目在各分组子菜单里各有一项，
/// 应用时只作用于该分组）；`profile_none_<scope>` 表示某分组"不使用项目"
/// （只清该分组标记，不动配置）。
pub fn handle_profile_tray_event(app: &tauri::AppHandle, event_id: &str) -> bool {
    let Some(suffix) = event_id.strip_prefix("profile_") else {
        return false;
    };

    if let Some(scope_str) = suffix.strip_prefix("none_") {
        let Ok(scope) = crate::services::profile::ProfileScope::parse(scope_str) else {
            log::error!("未知的项目分组托盘事件: {event_id}");
            return true;
        };
        if let Some(app_state) = app.try_state::<AppState>() {
            if let Err(e) = app_state.db.set_current_profile_id(scope.as_str(), None) {
                log::error!("清除当前项目失败: {e}");
            }
        }
        // 通知主窗口刷新（profileId=null 表示该分组已清除当前项目）
        if let Err(e) = app.emit(
            PROFILE_APPLIED,
            serde_json::json!({ "profileId": null, "scope": scope.as_str() }),
        ) {
            log::error!("发射 {PROFILE_APPLIED} 事件失败: {e}");
        }
        refresh_tray_menu(app);
        return true;
    }

    // scope 是固定枚举字符串（不含下划线），uuid 只含连字符，首个下划线即分界
    let Some((scope_str, profile_id)) = suffix.split_once('_') else {
        log::error!("无法解析项目托盘事件: {event_id}");
        return true;
    };
    let Ok(scope) = crate::services::profile::ProfileScope::parse(scope_str) else {
        log::error!("未知的项目分组托盘事件: {event_id}");
        return true;
    };

    log::info!("应用项目: {profile_id}（{scope_str} 组）");
    let app_handle = app.clone();
    let profile_id = profile_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(app_state) = app_handle.try_state::<AppState>() else {
            return;
        };
        match crate::services::profile::ProfileService::apply(app_state.inner(), &profile_id, scope)
        {
            Ok((warnings, should_stop_proxy)) => {
                for warning in &warnings {
                    log::warn!("[Profile] 应用项目 {profile_id} 警告: {warning}");
                }

                if should_stop_proxy {
                    let app_handle2 = app_handle.clone();
                    let proxy_service = app_state.proxy_service.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = proxy_service.stop().await {
                            log::warn!("托盘切换项目后停止代理服务失败: {e}");
                        }
                        if let Some(state) = app_handle2.try_state::<AppState>() {
                            crate::commands::emit_profile_apply_events(
                                &app_handle2,
                                state.inner(),
                                &profile_id,
                                scope,
                            );
                        }
                    });
                } else {
                    crate::commands::emit_profile_apply_events(
                        &app_handle,
                        app_state.inner(),
                        &profile_id,
                        scope,
                    );
                }
            }
            Err(e) => {
                log::error!("应用项目 {profile_id} 失败: {e}");
                refresh_tray_menu(&app_handle);
            }
        }
    });
    true
}

/// 托盘「模型」子菜单项的事件 id 后缀：`{section.event_prefix()}model_{model}`。
/// 自建 provider 的 id 是 uuid / `<category>-<uuid>`（连字符分隔），不会以
/// `model_` 开头，与供应商事件不冲突。
const TIER_MODEL_EVENT_PREFIX: &str = "model_";

/// 处理供应商托盘事件
pub fn handle_provider_tray_event(app: &tauri::AppHandle, event_id: &str) -> bool {
    for section in TRAY_SECTIONS.iter() {
        if let Some(suffix) = event_id.strip_prefix(&section.event_prefix()) {
            // 处理 Auto 点击
            if suffix == AUTO_SUFFIX {
                log::info!("切换到{} Auto模式", section.label);
                let app_handle = app.clone();
                let app_type = section.app_type.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(e) = handle_auto_click(&app_handle, &app_type) {
                        log::error!("切换{}Auto模式失败: {e}", section.label);
                    }
                });
                return true;
            }

            // 处理「模型」子菜单点击（只挂在 Codex 当前托管档位下）
            if let Some(model) = suffix.strip_prefix(TIER_MODEL_EVENT_PREFIX) {
                log::info!("切换{}档位模型: {model}", section.label);
                let app_handle = app.clone();
                let app_type = section.app_type.clone();
                let model = model.to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(e) = handle_tier_model_click(&app_handle, &app_type, &model) {
                        log::error!("切换{}档位模型失败: {e}", section.label);
                        show_tier_switch_error(&app_handle, &e);
                    }
                });
                return true;
            }

            // 处理供应商点击
            log::info!("切换到{}供应商: {suffix}", section.label);
            let app_handle = app.clone();
            let provider_id = suffix.to_string();
            let app_type = section.app_type.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = handle_provider_click(&app_handle, &app_type, &provider_id) {
                    log::error!("切换{}供应商失败: {e}", section.label);
                    if crate::relay::is_managed(&provider_id) {
                        show_tier_switch_error(&app_handle, &e);
                    }
                }
            });
            return true;
        }
    }
    false
}

/// 处理 Auto 点击：启用 proxy 和 auto_failover
fn handle_auto_click(app: &tauri::AppHandle, app_type: &AppType) -> Result<(), AppError> {
    if let Some(app_state) = app.try_state::<AppState>() {
        let app_type_str = app_type.as_str();

        // 强一致语义：Auto 模式开启后立即切到队列 P1（P1→P2→...）
        // 若队列为空，则尝试把“当前供应商”自动加入队列作为 P1，避免用户陷入无法开启的死锁。
        let mut queue = app_state.db.get_failover_queue(app_type_str)?;
        if queue.is_empty() {
            let current_id =
                crate::settings::get_effective_current_provider(&app_state.db, app_type)?;
            let Some(current_id) = current_id else {
                return Err(AppError::Message(
                    "故障转移队列为空，且未设置当前供应商，无法启用 Auto 模式".to_string(),
                ));
            };
            app_state
                .db
                .add_to_failover_queue(app_type_str, &current_id)?;
            queue = app_state.db.get_failover_queue(app_type_str)?;
        }

        let p1_provider_id = queue
            .first()
            .map(|item| item.provider_id.clone())
            .ok_or_else(|| AppError::Message("故障转移队列为空，无法启用 Auto 模式".to_string()))?;

        // 真正启用 failover：启动代理服务 + 执行接管 + 开启 auto_failover
        let proxy_service = &app_state.proxy_service;

        // 1) 确保代理服务运行（会自动设置 proxy_enabled = true）
        let is_running = futures::executor::block_on(proxy_service.is_running());
        if !is_running {
            log::info!("[Tray] Auto 模式：启动代理服务");
            if let Err(e) = futures::executor::block_on(proxy_service.start()) {
                log::error!("[Tray] 启动代理服务失败: {e}");
                return Err(AppError::Message(format!("启动代理服务失败: {e}")));
            }
        }

        // 2) 执行 Live 配置接管（确保该 app 被代理接管）
        log::info!("[Tray] Auto 模式：对 {app_type_str} 执行接管");
        if let Err(e) =
            futures::executor::block_on(proxy_service.set_takeover_for_app(app_type_str, true))
        {
            log::error!("[Tray] 执行接管失败: {e}");
            return Err(AppError::Message(format!("执行接管失败: {e}")));
        }

        // 3) 设置 auto_failover_enabled = true
        app_state
            .db
            .set_proxy_flags_sync(app_type_str, true, true)?;

        // 3.1) 立即切到队列 P1（热切换：不写 Live，仅更新 DB/settings/备份）
        if let Err(e) = futures::executor::block_on(
            proxy_service.switch_proxy_target(app_type_str, &p1_provider_id),
        ) {
            log::error!("[Tray] Auto 模式切换到队列 P1 失败: {e}");
            return Err(AppError::Message(format!(
                "Auto 模式切换到队列 P1 失败: {e}"
            )));
        }

        // 4) 更新托盘菜单
        if let Ok(new_menu) = create_tray_menu(app, app_state.inner()) {
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                let _ = tray.set_menu(Some(new_menu));
            }
        }

        // 5) 发射事件到前端
        let event_data = serde_json::json!({
            "appType": app_type_str,
            "proxyEnabled": true,
            "autoFailoverEnabled": true,
            "providerId": p1_provider_id
        });
        if let Err(e) = app.emit("proxy-flags-changed", event_data.clone()) {
            log::error!("发射 proxy-flags-changed 事件失败: {e}");
        }
        // 发射 provider-switched 事件（保持向后兼容，Auto 切换也算一种切换）
        if let Err(e) = app.emit(PROVIDER_SWITCHED, event_data) {
            log::error!("发射 {PROVIDER_SWITCHED} 事件失败: {e}");
        }
    }
    Ok(())
}

/// 处理供应商点击：关闭 auto_failover + 切换供应商
fn handle_provider_click(
    app: &tauri::AppHandle,
    app_type: &AppType,
    provider_id: &str,
) -> Result<(), AppError> {
    // **托管档位走 relay 编排，不走这条直连路径**：主界面那条「退 ChatGPT → 切 → 重开」
    // 的 `switch_tier_command` 在这里复用，确认对话框见 `confirm_quit_chatgpt`。
    // 这是把托管档位放回托盘的前提 —— 直连 `ProviderService::switch` 绕过编排，
    // 切完 codex 还连着旧分组而托盘已勾上新档位（旧防线 `filter_unmanaged` 防的正是这个）。
    if crate::relay::is_managed(provider_id) {
        return handle_managed_tier_click(app, app_type, provider_id);
    }

    if let Some(app_state) = app.try_state::<AppState>() {
        let app_type_str = app_type.as_str();

        // 获取当前 proxy 状态，保持 enabled 不变，只关闭 auto_failover
        let (proxy_enabled, _) = app_state.db.get_proxy_flags_sync(app_type_str);
        app_state
            .db
            .set_proxy_flags_sync(app_type_str, proxy_enabled, false)?;

        // 切换供应商。需要本地路由的供应商也不在这里自动启动代理，
        // 由用户在页面/设置中手动开启。
        crate::services::ProviderService::switch(app_state.inner(), app_type.clone(), provider_id)?;

        // 更新托盘菜单
        if let Ok(new_menu) = create_tray_menu(app, app_state.inner()) {
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                let _ = tray.set_menu(Some(new_menu));
            }
        }

        // 发射事件到前端
        let event_data = serde_json::json!({
            "appType": app_type_str,
            "proxyEnabled": proxy_enabled,
            "autoFailoverEnabled": false,
            "providerId": provider_id
        });
        if let Err(e) = app.emit("proxy-flags-changed", event_data.clone()) {
            log::error!("发射 proxy-flags-changed 事件失败: {e}");
        }
        // 发射 provider-switched 事件（保持向后兼容）
        if let Err(e) = app.emit(PROVIDER_SWITCHED, event_data) {
            log::error!("发射 {PROVIDER_SWITCHED} 事件失败: {e}");
        }
    }
    Ok(())
}

/// 托盘点托管档位：与主界面 `relay_switch_tier` 命令走**同一个** `switch_tier_command`
/// （含 ChatGPT 确认闸、「退 → 切 → 重开」编排、`provider-switched` 事件、托盘刷新）。
///
/// 注意**不在这里关 auto_failover**：relay 命令层本来就不动 failover 标志，
/// 托盘这条入口镜像它而不是镜像下面自建 provider 那条 —— 两类入口各自的语义
/// 以命令层为准，别在托盘长出第三套。
fn handle_managed_tier_click(
    app: &tauri::AppHandle,
    app_type: &AppType,
    provider_id: &str,
) -> Result<(), AppError> {
    let mut user_choice = None;
    loop {
        let outcome = futures::executor::block_on(crate::commands::switch_tier_command(
            app,
            provider_id,
            app_type.clone(),
            user_choice,
        ))?;
        match outcome {
            crate::commands::SwitchTierCommandResult::ConfirmationRequired { target_name } => {
                // 取消 = 干净中止（配置未动，菜单勾选也还停在旧档位上）。
                if !confirm_quit_chatgpt(app, &target_name) {
                    return Ok(());
                }
                user_choice = Some(true);
            }
            crate::commands::SwitchTierCommandResult::Switched { result } => {
                for warning in &result.warnings {
                    log::warn!("[Tray] 切换档位后警告: {warning}");
                }
                return Ok(());
            }
        }
    }
}

/// 托盘点「模型」子菜单：对**当前**托管 Codex 档位执行 `switch_tier_model_command`
/// （校验模型 ∈ 落库目录 → 更新 provider → 走切档位编排，失败回滚）。
/// 模型子菜单只在当前档位是托管 Codex 项时才会挂出来，这里再验一遍是防
/// 菜单陈旧（刚切走、菜单还没重建）时点了个已不属于当前档位的模型。
fn handle_tier_model_click(
    app: &tauri::AppHandle,
    app_type: &AppType,
    model: &str,
) -> Result<(), AppError> {
    if !matches!(app_type, AppType::Codex) {
        return Ok(());
    }
    let Some(app_state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    let Some(provider_id) =
        crate::settings::get_effective_current_provider(&app_state.db, app_type)?
    else {
        return Ok(());
    };
    if !crate::relay::is_managed(&provider_id) {
        return Ok(());
    }

    // 点的就是当前模型 → 无操作（与主界面 `handleSelectTierModel` 的守卫对齐，
    // 否则每次误点都会走一遍「退 ChatGPT」确认）。
    let current = app_state
        .db
        .get_provider_by_id(&provider_id, app_type.as_str())?;
    if let Some(provider) = current {
        if crate::relay::provision::extract_model(&provider.settings_config).as_deref()
            == Some(model)
        {
            return Ok(());
        }
    }

    let mut user_choice = None;
    loop {
        let outcome = futures::executor::block_on(crate::commands::switch_tier_model_command(
            app,
            &provider_id,
            app_type.clone(),
            model,
            user_choice,
        ))?;
        match outcome {
            crate::commands::SwitchTierCommandResult::ConfirmationRequired { target_name } => {
                if !confirm_quit_chatgpt(app, &target_name) {
                    return Ok(());
                }
                user_choice = Some(true);
            }
            crate::commands::SwitchTierCommandResult::Switched { result } => {
                for warning in &result.warnings {
                    log::warn!("[Tray] 切换档位模型后警告: {warning}");
                }
                return Ok(());
            }
        }
    }
}

/// 托盘侧的 ChatGPT 退出确认：`switch_tier_command` 返回 `ConfirmationRequired`
/// 时弹原生对话框，`true` = 替用户退出并重开，`false` = 取消本次切换。
///
/// 主界面那个三按钮弹窗（取消 / 只切换 / 退出并切换）走 `SwitchTierConfirmDialog`；
/// 原生对话框只放得下两个自定义按钮，这里给的是「退出并切换 / 取消」——
/// **dismiss 必须是取消**（关闭/X/Esc 都落到 false），绝不能映射到任何会执行
/// 动作的选项。「只切换」留给主界面。
fn confirm_quit_chatgpt(app: &tauri::AppHandle, target_name: &str) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    let texts = TrayTexts::from_language(&tray_language());
    app.dialog()
        .message(texts.tier_switch_confirm_body.replace("{}", target_name))
        .title(texts.tier_switch_confirm_title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            texts.tier_quit_and_switch.to_string(),
            texts.cancel_label.to_string(),
        ))
        .blocking_show()
}

/// 托盘切档位失败时把错误摆到用户眼前。托盘点击没有页面可 toast，
/// 只写日志的话用户看到的就是「勾选没变、什么都没发生」—— 那是坏掉的样子。
fn show_tier_switch_error(app: &tauri::AppHandle, error: &AppError) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    let texts = TrayTexts::from_language(&tray_language());
    let _ = app
        .dialog()
        .message(error.to_string())
        .title(texts.tier_switch_failed_title)
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::Ok)
        .blocking_show();
}

/// 创建动态托盘菜单
pub fn create_tray_menu(
    app: &tauri::AppHandle,
    app_state: &AppState,
) -> Result<Menu<tauri::Wry>, AppError> {
    let app_settings = crate::settings::get_settings();
    // 用户未显式设置语言（首次安装）时，按系统区域回退而非硬编码简体，
    // 否则繁中系统的托盘会固定显示简体直到用户手动切换一次。
    let tray_texts = TrayTexts::from_language(&tray_language());

    // Get visible apps setting, default to all visible
    let visible_apps = app_settings.visible_apps.unwrap_or_default();

    let mut menu_builder = MenuBuilder::new(app);
    let mut section_handles: std::collections::HashMap<AppType, Submenu<tauri::Wry>> =
        std::collections::HashMap::new();

    // 顶部：打开主界面 / 打开官方网站
    let show_main_item =
        MenuItem::with_id(app, "show_main", tray_texts.show_main, true, None::<&str>)
            .map_err(|e| AppError::Message(format!("创建打开主界面菜单失败: {e}")))?;
    let open_website_item = MenuItem::with_id(
        app,
        "open_website",
        tray_texts.open_website,
        true,
        None::<&str>,
    )
    .map_err(|e| AppError::Message(format!("创建打开官方网站菜单失败: {e}")))?;
    menu_builder = menu_builder
        .item(&show_main_item)
        .item(&open_website_item)
        .separator();

    // Pre-compute proxy running state (used to disable official providers in tray menu)
    let is_proxy_running = futures::executor::block_on(app_state.proxy_service.is_running());

    // 每个应用类型折叠为子菜单，避免供应商过多时菜单过长。
    // 分区数已扩到 10 个 app，分隔符整组只放一个 —— 每分区一条的话菜单会被
    // 分隔符撑到两倍长。
    let mut any_section_added = false;
    for section in TRAY_SECTIONS.iter() {
        if !visible_apps.is_visible(&section.app_type) {
            continue;
        }
        any_section_added = true;

        let app_type_str = section.app_type.as_str();
        let providers = app_state.db.get_all_providers(app_type_str)?;

        let current_id =
            crate::settings::get_effective_current_provider(&app_state.db, &section.app_type)?
                .unwrap_or_default();

        let menu_providers = tray_menu_providers(&providers);

        if menu_providers.is_empty() {
            // 空供应商：显示禁用的菜单项（否则会挂出一个点开什么都没有的空子菜单）
            let label = format!("{} {}", section.label, tray_texts.no_providers_label);
            let empty_item =
                MenuItem::with_id(app, section.empty_id(), &label, false, None::<&str>).map_err(
                    |e| AppError::Message(format!("创建{}空提示失败: {e}", section.label)),
                )?;
            menu_builder = menu_builder.item(&empty_item);
        } else {
            let current_provider = providers.get(&current_id);
            let submenu_label = match current_provider {
                Some(p) => {
                    let suffix = format_usage_suffix(app_state, &section.app_type, p, &current_id)
                        .unwrap_or_default();
                    format!("{} · {}{}", section.label, p.name, suffix)
                }
                None => section.label.to_string(),
            };
            let submenu_id = format!("submenu_{}", app_type_str);
            let event_prefix = section.event_prefix();

            // Check if this app is under proxy takeover (for disabling official providers)
            let is_app_taken_over = is_proxy_running
                && (futures::executor::block_on(app_state.db.get_live_backup(app_type_str))
                    .ok()
                    .flatten()
                    .is_some()
                    || app_state
                        .proxy_service
                        .detect_takeover_in_live_config_for_app(&section.app_type));

            let mut submenu_builder = SubmenuBuilder::with_id(app, &submenu_id, &submenu_label);

            for (id, provider) in menu_providers {
                let is_current = current_id == *id;
                let is_official_blocked = is_app_taken_over
                    && provider.category.as_deref() == Some("official")
                    && !crate::services::provider::official_provider_supports_proxy_takeover(
                        &section.app_type,
                        provider,
                    );
                let label = if is_official_blocked {
                    format!("{} \u{26D4}", &provider.name) // ⛔ emoji
                } else {
                    provider.name.clone()
                };
                let item = CheckMenuItem::with_id(
                    app,
                    format!("{event_prefix}{id}"),
                    &label,
                    !is_official_blocked, // disabled when blocked
                    is_current,
                    None::<&str>,
                )
                .map_err(|e| AppError::Message(format!("创建{}菜单项失败: {e}", section.label)))?;
                submenu_builder = submenu_builder.item(&item);
            }

            // 「模型」二级子菜单：只挂在 Codex 分区、且当前档位是托管项且有模型目录时。
            // 点击走 `handle_tier_model_click` → `switch_tier_model_command`（选模型即
            // 激活该档位，与主界面模型按钮组同一条路径）。
            if let Some((current_model, models)) =
                current_provider.and_then(|p| tier_model_choices(p, &section.app_type))
            {
                let mut models_builder = SubmenuBuilder::with_id(
                    app,
                    format!("submenu_{app_type_str}_models"),
                    tray_texts.tier_model_label,
                );
                for model in &models {
                    let item = CheckMenuItem::with_id(
                        app,
                        format!("{event_prefix}{TIER_MODEL_EVENT_PREFIX}{model}"),
                        model,
                        true,
                        current_model.as_deref() == Some(model.as_str()),
                        None::<&str>,
                    )
                    .map_err(|e| {
                        AppError::Message(format!("创建{}模型菜单项失败: {e}", section.label))
                    })?;
                    models_builder = models_builder.item(&item);
                }
                let models_submenu = models_builder.build().map_err(|e| {
                    AppError::Message(format!("构建{}模型子菜单失败: {e}", section.label))
                })?;
                submenu_builder = submenu_builder.separator().item(&models_submenu);
            }

            let submenu = submenu_builder
                .build()
                .map_err(|e| AppError::Message(format!("构建{}子菜单失败: {e}", section.label)))?;
            section_handles.insert(section.app_type.clone(), submenu.clone());
            menu_builder = menu_builder.item(&submenu);
        }
    }

    if any_section_added {
        menu_builder = menu_builder.separator();
    }

    // 项目 Profile 子菜单：项目列表全应用共享，按分组嵌套子菜单各自勾选/应用
    // （组内应用可见且存在项目时才显示该组）
    {
        use crate::services::profile::ProfileScope;

        let any_scope_visible = ProfileScope::ALL.iter().any(|scope| {
            scope
                .apps()
                .iter()
                .any(|app_type| visible_apps.is_visible(app_type))
        });
        let profiles = if any_scope_visible {
            app_state.db.get_all_profiles()?
        } else {
            Vec::new()
        };

        let mut scope_submenus = Vec::new();
        for scope in ProfileScope::ALL {
            if profiles.is_empty()
                || !scope
                    .apps()
                    .iter()
                    .any(|app_type| visible_apps.is_visible(app_type))
            {
                continue;
            }
            let current_profile_id = app_state
                .db
                .get_current_profile_id(scope.as_str())?
                .unwrap_or_default();
            // 分组标签用产品名，不进 i18n
            let scope_label = match scope {
                ProfileScope::Claude => "Claude Code",
                ProfileScope::ClaudeDesktop => "Claude Desktop",
                ProfileScope::Codex => "Codex",
            };
            let mut scope_builder = SubmenuBuilder::with_id(
                app,
                format!("submenu_profiles_{}", scope.as_str()),
                scope_label,
            );
            for profile in &profiles {
                let item = CheckMenuItem::with_id(
                    app,
                    format!("profile_{}_{}", scope.as_str(), profile.id),
                    &profile.name,
                    true,
                    current_profile_id == profile.id,
                    None::<&str>,
                )
                .map_err(|e| AppError::Message(format!("创建项目菜单项失败: {e}")))?;
                scope_builder = scope_builder.item(&item);
            }
            let none_item = CheckMenuItem::with_id(
                app,
                format!("profile_none_{}", scope.as_str()),
                tray_texts.no_project_label,
                true,
                current_profile_id.is_empty(),
                None::<&str>,
            )
            .map_err(|e| AppError::Message(format!("创建不使用项目菜单项失败: {e}")))?;
            let scope_submenu = scope_builder
                .separator()
                .item(&none_item)
                .build()
                .map_err(|e| AppError::Message(format!("构建项目分组子菜单失败: {e}")))?;
            scope_submenus.push(scope_submenu);
        }

        if !scope_submenus.is_empty() {
            let mut profiles_builder =
                SubmenuBuilder::with_id(app, "submenu_profiles", tray_texts.projects_label);
            for scope_submenu in &scope_submenus {
                profiles_builder = profiles_builder.item(scope_submenu);
            }
            let profiles_submenu = profiles_builder
                .build()
                .map_err(|e| AppError::Message(format!("构建项目子菜单失败: {e}")))?;
            menu_builder = menu_builder.item(&profiles_submenu).separator();
        }
    }

    let lightweight_item = CheckMenuItem::with_id(
        app,
        "lightweight_mode",
        tray_texts.lightweight_mode,
        true,
        crate::lightweight::is_lightweight_mode(),
        None::<&str>,
    )
    .map_err(|e| AppError::Message(format!("创建轻量模式菜单失败: {e}")))?;

    menu_builder = menu_builder.item(&lightweight_item).separator();

    let quit_item = MenuItem::with_id(app, "quit", tray_texts.quit, true, None::<&str>)
        .map_err(|e| AppError::Message(format!("创建退出菜单失败: {e}")))?;

    menu_builder = menu_builder.item(&quit_item);

    let menu = menu_builder
        .build()
        .map_err(|e| AppError::Message(format!("构建菜单失败: {e}")))?;

    *TRAY_SECTION_SUBMENUS
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = section_handles;

    Ok(menu)
}

/// 就地更新各 app 分区子菜单的标题（usage 后缀变化时走这条），
/// 避免 `set_menu` 导致用户打开中的菜单被关闭。
/// 句柄由上一次 `create_tray_menu` 填充；为空（从未构建过菜单）时无事发生。
fn update_tray_usage_labels(app: &tauri::AppHandle) {
    let Some(app_state) = app.try_state::<AppState>() else {
        return;
    };
    let handles = match TRAY_SECTION_SUBMENUS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    for section in TRAY_SECTIONS.iter() {
        let Some(submenu) = handles.get(&section.app_type) else {
            continue;
        };
        let Ok(providers) = app_state.db.get_all_providers(section.app_type.as_str()) else {
            continue;
        };
        let Ok(Some(current_id)) =
            crate::settings::get_effective_current_provider(&app_state.db, &section.app_type)
        else {
            continue;
        };
        let Some(provider) = providers.get(&current_id) else {
            continue;
        };
        let suffix = format_usage_suffix(&app_state, &section.app_type, provider, &current_id)
            .unwrap_or_default();
        let new_label = format!("{} · {}{}", section.label, provider.name, suffix);
        if let Err(e) = submenu.set_text(&new_label) {
            log::debug!("[Tray] 更新{}子菜单标题失败: {e}", section.label);
        }
    }
}

pub fn refresh_tray_menu(app: &tauri::AppHandle) {
    use crate::store::AppState;

    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(new_menu) = create_tray_menu(app, state.inner()) {
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                if let Err(e) = tray.set_menu(Some(new_menu)) {
                    log::error!("刷新托盘菜单失败: {e}");
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub fn apply_tray_policy(app: &tauri::AppHandle, dock_visible: bool) {
    use tauri::ActivationPolicy;

    let desired_policy = if dock_visible {
        ActivationPolicy::Regular
    } else {
        ActivationPolicy::Accessory
    };

    if let Err(err) = app.set_dock_visibility(dock_visible) {
        log::warn!("设置 Dock 显示状态失败: {err}");
    }

    if let Err(err) = app.set_activation_policy(desired_policy) {
        log::warn!("设置激活策略失败: {err}");
    }
}

/// 处理托盘菜单事件
pub fn handle_tray_menu_event(app: &tauri::AppHandle, event_id: &str) {
    log::info!("处理托盘菜单事件: {event_id}");

    match event_id {
        "show_main" => {
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "windows")]
                {
                    let _ = window.set_skip_taskbar(false);
                }
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                #[cfg(target_os = "linux")]
                {
                    crate::linux_fix::nudge_main_window(window.clone());
                }
                #[cfg(target_os = "macos")]
                {
                    apply_tray_policy(app, true);
                }
            } else if crate::lightweight::is_lightweight_mode() {
                if let Err(e) = crate::lightweight::exit_lightweight_mode(app) {
                    log::error!("退出轻量模式重建窗口失败: {e}");
                }
            }
        }
        "open_website" => {
            if let Err(e) = app.opener().open_url(OFFICIAL_WEBSITE, None::<String>) {
                log::error!("打开官方网站失败: {e}");
            }
        }
        "lightweight_mode" => {
            if crate::lightweight::is_lightweight_mode() {
                if let Err(e) = crate::lightweight::exit_lightweight_mode(app) {
                    log::error!("退出轻量模式失败: {e}");
                }
            } else if let Err(e) = crate::lightweight::enter_lightweight_mode(app) {
                log::error!("进入轻量模式失败: {e}");
            }
        }
        "quit" => {
            log::info!("退出应用");
            app.exit(0);
        }
        _ => {
            if handle_profile_tray_event(app, event_id) {
                return;
            }
            if handle_provider_tray_event(app, event_id) {
                return;
            }
            log::warn!("未处理的菜单事件: {event_id}");
        }
    }
}

static LAST_TRAY_USAGE_REFRESH: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);
const MIN_TRAY_USAGE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// 合并多次快速触发的"usage 标题软更新"：批量刷新期间多个 usage 命令
/// 同时成功时，只会产生一次就地 `set_text` 批量调用。走软更新而不是
/// `refresh_tray_menu` 整建，避免用户打开中的菜单被 macOS 系统关闭。
static TRAY_REBUILD_SCHEDULED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn schedule_tray_refresh(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if TRAY_REBUILD_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 50ms 合窗：让同一轮 React Query / 托盘批量刷新触发的多个写入
        // 共享一次标题更新。
        std::thread::sleep(std::time::Duration::from_millis(50));
        TRAY_REBUILD_SCHEDULED.store(false, Ordering::Release);
        update_tray_usage_labels(&app);
    });
}

/// 并行刷新每个可见 app "当前 provider" 的用量；成功 / 失败结果都通过各
/// command 的 write-through 逻辑写入 `UsageCache`，单次重建菜单由
/// `schedule_tray_refresh` 做合并。内部 10 秒节流防止鼠标悬停反复进出时
/// 雪崩请求；互斥锁被毒化时以上次状态为准继续推进，不会永久阻塞。
///
/// 刷新面与 `format_usage_suffix` 的展示面严格对齐 —— 每次悬停最多发
/// `TRAY_SECTIONS.len()` 次外部请求；只有显式启用的用量查询（含官方订阅、
/// coding_plan / balance / Copilot / 自定义脚本）才会发请求。
pub(crate) async fn refresh_all_usage_in_tray(app: &tauri::AppHandle) {
    use crate::commands::CopilotAuthState;
    use futures::future::join_all;

    {
        let mut guard = LAST_TRAY_USAGE_REFRESH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = std::time::Instant::now();
        if let Some(last) = *guard {
            if now.duration_since(last) < MIN_TRAY_USAGE_REFRESH_INTERVAL {
                return;
            }
        }
        *guard = Some(now);
    }

    let Some(app_state) = app.try_state::<AppState>() else {
        return;
    };

    // 与 `create_tray_menu` 保持一致：用户隐藏的 app 不参与外部 API 查询，
    // 避免在未使用的 app 上浪费请求、撞 rate limit 或反复触发鉴权失败日志。
    let visible_apps = crate::settings::get_settings()
        .visible_apps
        .unwrap_or_default();

    let mut script_futures = Vec::new();

    for section in TRAY_SECTIONS.iter() {
        if !visible_apps.is_visible(&section.app_type) {
            continue;
        }

        let app_type_str = section.app_type.as_str();
        let log_name = section.label;

        // 解析 effective current provider；未设置 / 出错都静默跳过，
        // 与 create_tray_menu 的行为保持一致。
        let current_id =
            match crate::settings::get_effective_current_provider(&app_state.db, &section.app_type)
            {
                Ok(Some(id)) => id,
                Ok(None) => continue,
                Err(e) => {
                    log::warn!("[Tray] 读取{log_name}当前供应商失败: {e}");
                    continue;
                }
            };
        // 只需当前 provider —— by-id 查询避免把整个 app 的 provider 列表加载
        // 进内存（每次悬停 × 3 sections 的热路径）。
        let current = match app_state.db.get_provider_by_id(&current_id, app_type_str) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("[Tray] 读取{log_name}当前供应商失败: {e}");
                continue;
            }
        };

        // 与 format_usage_suffix 同一优先级：只有显式启用的用量查询才发请求。
        let is_official_provider = current.category.as_deref() == Some("official");
        if current.has_usage_script_enabled()
            && (!is_official_provider || provider_uses_official_subscription(&current))
        {
            let app_clone = app.clone();
            let state = app.state::<AppState>();
            let copilot_state = app.state::<CopilotAuthState>();
            let xai_state = app.state::<crate::commands::XaiOAuthState>();
            let provider_id = current_id.clone();
            let app_str = app_type_str.to_string();
            script_futures.push(async move {
                if let Err(e) = crate::commands::queryProviderUsage(
                    app_clone,
                    state,
                    copilot_state,
                    xai_state,
                    provider_id.clone(),
                    app_str,
                )
                .await
                {
                    log::debug!("[Tray] 刷新{log_name}供应商 {provider_id} 用量失败: {e}");
                }
            });
        }
    }

    join_all(script_futures).await;
}

#[cfg(test)]
mod tests {
    use super::{
        format_script_summary, format_subscription_summary, tier_model_choices,
        tray_menu_providers, TRAY_ID, TRAY_SECTIONS,
    };
    use crate::app_config::AppType;
    use crate::provider::{Provider, UsageData, UsageResult};
    use crate::services::subscription::{
        CredentialStatus, QuotaTier, SubscriptionQuota, TIER_FIVE_HOUR, TIER_GEMINI_FLASH,
        TIER_GEMINI_FLASH_LITE, TIER_GEMINI_PRO, TIER_MONTHLY, TIER_SEVEN_DAY, TIER_SEVEN_DAY_OPUS,
        TIER_SEVEN_DAY_SONNET, TIER_THIRTY_DAY, TIER_WEEKLY_LIMIT,
    };

    #[test]
    fn tray_id_is_unique_to_app() {
        assert_eq!(TRAY_ID, "cc-switch");
        assert_ne!(TRAY_ID, "main");
    }

    fn provider_map(ids: &[&str]) -> indexmap::IndexMap<String, crate::provider::Provider> {
        ids.iter()
            .enumerate()
            .map(|(idx, id)| {
                let mut p = Provider::with_id(
                    (*id).to_string(),
                    (*id).to_string(),
                    serde_json::json!({}),
                    None,
                );
                // 给定 sort_index 让顺序确定，避免断言依赖 name 兜底排序。
                p.sort_index = Some(idx);
                ((*id).to_string(), p)
            })
            .collect()
    }

    /// P0 回归防线（2026-08-15 反转）：托管档位**必须**出现在托盘子菜单里，
    /// 与自建 provider 混排、顺序不变。
    ///
    /// 配套不变量在 `handle_provider_click`：`is_managed` 的点击必须路由到
    /// `switch_tier_command` 的 relay 编排 —— 列表和路由是一对，动一个必须同时动另一个。
    /// （菜单构建需要 `AppHandle`，单测只能守列表这一半；路由那一半靠代码评审守。）
    #[test]
    fn tray_menu_includes_loongport_managed_providers() {
        let managed = crate::relay::provision::provider_id_for("https://bestapi.store", Some(1), 1);
        let providers = provider_map(&["custom-1", &managed, "codex-official"]);

        let listed = tray_menu_providers(&providers);

        assert_eq!(
            listed.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["custom-1", managed.as_str(), "codex-official"],
            "托管档位必须进托盘菜单（与自建混排），其余项顺序不变"
        );
    }

    /// 真的一无所有才显示「(无供应商)」，不能因为托管档位而误判为空。
    #[test]
    fn tray_menu_is_empty_only_when_there_are_no_providers_at_all() {
        let a = crate::relay::provision::provider_id_for("https://bestapi.store", Some(1), 1);
        let b = crate::relay::provision::provider_id_for("https://bestapi.store", Some(1), 2);
        let providers = provider_map(&[&a, &b]);

        assert!(!tray_menu_providers(&providers).is_empty());

        let empty: indexmap::IndexMap<String, crate::provider::Provider> =
            indexmap::IndexMap::new();
        assert!(tray_menu_providers(&empty).is_empty());
    }

    fn codex_tier_provider(
        id: &str,
        config_toml_model: &str,
        catalog: &[&str],
    ) -> crate::provider::Provider {
        let config = if config_toml_model.is_empty() {
            String::new()
        } else {
            format!("model = \"{config_toml_model}\"\n")
        };
        let catalog_json: Vec<serde_json::Value> = catalog
            .iter()
            .map(|m| serde_json::json!({ "model": m }))
            .collect();
        crate::provider::Provider::with_id(
            id.to_string(),
            format!("站点 · {id}"),
            serde_json::json!({
                "config": config,
                "modelCatalog": { "models": catalog_json },
            }),
            None,
        )
    }

    /// 「模型」子菜单只挂在「托管 Codex 档位 + 目录非空」上，三道闸各自单独验证。
    #[test]
    fn tier_model_choices_requires_managed_codex_tier_with_catalog() {
        let managed = crate::relay::provision::provider_id_for("https://bestapi.store", Some(1), 1);

        // 托管 Codex 档位 + 有目录 → (当前模型, 目录)
        let provider =
            codex_tier_provider(&managed, "gpt-5.6-sol", &["gpt-5.6-sol", "gpt-5.6-nano"]);
        let (current, models) = tier_model_choices(&provider, &AppType::Codex)
            .expect("managed codex tier with catalog should expose models");
        assert_eq!(current.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            models,
            vec!["gpt-5.6-sol".to_string(), "gpt-5.6-nano".to_string()]
        );

        // 非 Codex app（同一个 provider 行挂在 Claude 下）→ 不挂
        assert!(tier_model_choices(&provider, &AppType::Claude).is_none());

        // 自建 provider → 不挂
        let custom = codex_tier_provider("custom-1", "gpt-5.6-sol", &["gpt-5.6-sol"]);
        assert!(tier_model_choices(&custom, &AppType::Codex).is_none());

        // 目录为空 → 不挂
        let no_catalog = codex_tier_provider(&managed, "gpt-5.6-sol", &[]);
        assert!(tier_model_choices(&no_catalog, &AppType::Codex).is_none());
    }

    #[test]
    fn locale_maps_traditional_chinese_variants_to_zh_tw() {
        use super::map_locale_to_tray_language;
        for locale in [
            "zh-TW",
            "zh-HK",
            "zh-MO",
            "zh-Hant",
            "zh-Hant-TW",
            "zh-hant-hk",
        ] {
            assert_eq!(
                map_locale_to_tray_language(locale),
                "zh-TW",
                "expected {locale} -> zh-TW"
            );
        }
    }

    #[test]
    fn locale_maps_simplified_chinese_variants_to_zh() {
        use super::map_locale_to_tray_language;
        for locale in ["zh", "zh-CN", "zh-SG", "zh-Hans", "zh-Hans-CN"] {
            assert_eq!(
                map_locale_to_tray_language(locale),
                "zh",
                "expected {locale} -> zh"
            );
        }
    }

    #[test]
    fn locale_maps_japanese_and_english() {
        use super::map_locale_to_tray_language;
        assert_eq!(map_locale_to_tray_language("ja-JP"), "ja");
        assert_eq!(map_locale_to_tray_language("ja"), "ja");
        assert_eq!(map_locale_to_tray_language("en-US"), "en");
        assert_eq!(map_locale_to_tray_language("en"), "en");
    }

    #[test]
    fn locale_unknown_falls_back_to_zh() {
        use super::map_locale_to_tray_language;
        // 与前端 getInitialLanguage 的默认值保持一致。
        for locale in ["de-DE", "fr", "ko-KR", ""] {
            assert_eq!(
                map_locale_to_tray_language(locale),
                "zh",
                "expected {locale} -> zh (default)"
            );
        }
    }

    #[test]
    fn tray_sections_include_grokbuild_provider_switching() {
        let section = TRAY_SECTIONS
            .iter()
            .find(|section| section.app_type == AppType::GrokBuild)
            .expect("Grok Build tray section should exist");

        assert_eq!(section.event_prefix(), "grokbuild_");
        assert_eq!(section.empty_id(), "grokbuild_empty");
        assert_eq!(section.label, "Grok Build");
    }

    /// 分区事件前缀之间**互不为前缀**：`handle_provider_tray_event` 按数组顺序
    /// `strip_prefix` 分发，若 A 的前缀是 B 的前缀（如假想的 `codex_` 与
    /// `codex_extra_`），B 分区的事件会先被 A 抢走 —— 表现是「点上去没反应」，
    /// 编译器抓不到。扩 app 时这道闸必须跟着绿。
    #[test]
    fn tray_section_prefixes_do_not_shadow_each_other() {
        for (i, earlier) in TRAY_SECTIONS.iter().enumerate() {
            for (j, later) in TRAY_SECTIONS.iter().enumerate() {
                if i == j {
                    continue;
                }
                let earlier_prefix = earlier.event_prefix();
                let later_prefix = later.event_prefix();
                assert!(
                    !later_prefix.starts_with(&earlier_prefix),
                    "{later_prefix} 会被先出现的 {earlier_prefix} 抢走分发"
                );
            }
        }
    }

    /// 托盘分区覆盖主界面全部 app（P2）：每个 AppType 都有对应分区，
    /// 顺序与主界面 `appConfig` 一致，避免菜单顺序和 tab 顺序打架。
    #[test]
    fn tray_sections_cover_all_apps_in_main_ui_order() {
        let section_apps: Vec<&str> = TRAY_SECTIONS
            .iter()
            .map(|section| section.app_type.as_str())
            .collect();
        assert_eq!(
            section_apps,
            vec![
                "claude",
                "claude-desktop",
                "codex",
                "codex-image",
                "gemini",
                "grokbuild",
                "opencode",
                "openclaw",
                "hermes",
                "pi",
            ]
        );
    }

    fn make_quota(tool: &str, success: bool, tiers: Vec<QuotaTier>) -> SubscriptionQuota {
        SubscriptionQuota {
            tool: tool.to_string(),
            credential_status: CredentialStatus::Valid,
            credential_message: None,
            success,
            tiers,
            extra_usage: None,
            error: None,
            queried_at: Some(0),
        }
    }

    fn tier(name: &str, utilization: f64) -> QuotaTier {
        QuotaTier {
            name: name.to_string(),
            utilization,
            resets_at: None,
            used_value_usd: None,
            max_value_usd: None,
        }
    }

    #[test]
    fn claude_summary_uses_h_and_w_labels() {
        let quota = make_quota(
            "claude",
            true,
            vec![tier("five_hour", 9.0), tier("seven_day", 27.0)],
        );
        let s = format_subscription_summary(&quota).expect("should format");
        assert!(s.contains("h9%"), "expected h9% in {s}");
        assert!(s.contains("w27%"), "expected w27% in {s}");
    }

    #[test]
    fn gemini_summary_uses_p_and_f_labels() {
        let quota = make_quota(
            "gemini",
            true,
            vec![tier("gemini_pro", 15.0), tier("gemini_flash", 42.0)],
        );
        let s = format_subscription_summary(&quota).expect("should format");
        assert!(s.contains("p15%"), "expected p15% in {s}");
        assert!(s.contains("f42%"), "expected f42% in {s}");
    }

    #[test]
    fn gemini_summary_includes_all_three_tiers() {
        let quota = make_quota(
            "gemini",
            true,
            vec![
                tier("gemini_pro", 5.0),
                tier("gemini_flash", 42.0),
                tier("gemini_flash_lite", 80.0),
            ],
        );
        let s = format_subscription_summary(&quota).expect("should format");
        assert!(s.contains("p5%"), "expected p5% in {s}");
        assert!(s.contains("f42%"), "expected f42% in {s}");
        assert!(s.contains("l80%"), "expected l80% in {s}");
    }

    #[test]
    fn gemini_summary_lite_only_still_renders() {
        // flash_lite 如果是 API 返回的唯一 tier，仍应显示（避免前端 footer 能看到、
        // 托盘空白的不对称）。
        let quota = make_quota("gemini", true, vec![tier("gemini_flash_lite", 80.0)]);
        let s = format_subscription_summary(&quota).expect("should format");
        assert!(s.contains("l80%"), "expected l80% in {s}");
    }

    #[test]
    fn codex_summary_thirty_day_only_still_renders() {
        // Codex 免费方案的唯一 tier 是 30 天窗口。前端 footer 已能显示（TIER_I18N_KEYS
        // 有 "30_day"），托盘也必须能显示——否则就是这条不变量要防的非对称：footer
        // 能看到、托盘却空白。30_day 归入 "m" 月分组。见 #3651。
        let quota = make_quota("codex", true, vec![tier(TIER_THIRTY_DAY, 85.0)]);
        let s = format_subscription_summary(&quota).expect("should format");
        assert!(s.contains("m85%"), "expected m85% in {s}");
    }

    #[test]
    fn gemini_summary_emoji_reflects_highest_tier_including_lite() {
        // lite 是利用率最高的那条 → emoji 必须是红色，不能被 pro/flash 掩盖。
        let quota = make_quota(
            "gemini",
            true,
            vec![
                tier("gemini_pro", 10.0),
                tier("gemini_flash", 20.0),
                tier("gemini_flash_lite", 95.0),
            ],
        );
        let s = format_subscription_summary(&quota).unwrap();
        assert!(
            s.starts_with("\u{1F534}"),
            "expected red emoji (lite worst) in {s}"
        );
    }

    #[test]
    fn worst_emoji_reflects_highest_utilization() {
        // 🔴 = \u{1F534}; 任一 tier ≥ 90% 时预期显示红色。
        let quota = make_quota(
            "claude",
            true,
            vec![tier("five_hour", 10.0), tier("seven_day", 95.0)],
        );
        let s = format_subscription_summary(&quota).unwrap();
        assert!(s.starts_with("\u{1F534}"), "expected red emoji in {s}");
    }

    #[test]
    fn subscription_summary_week_aliases_use_highest_utilization() {
        let quota = make_quota(
            "claude",
            true,
            vec![
                tier(TIER_FIVE_HOUR, 10.0),
                tier(TIER_SEVEN_DAY_OPUS, 20.0),
                tier(TIER_SEVEN_DAY_SONNET, 95.0),
            ],
        );
        let s = format_subscription_summary(&quota).unwrap();
        assert!(s.contains("w95%"), "expected w95% in {s}");
        assert!(s.starts_with("\u{1F534}"), "expected red emoji in {s}");
    }

    #[test]
    fn failure_quota_returns_none() {
        let quota = make_quota("claude", false, vec![tier("five_hour", 50.0)]);
        assert!(format_subscription_summary(&quota).is_none());
    }

    #[test]
    fn unknown_tiers_return_none() {
        let quota = make_quota("claude", true, vec![tier("one_hour", 80.0)]);
        assert!(format_subscription_summary(&quota).is_none());
    }

    #[test]
    fn gemini_without_any_known_tiers_returns_none() {
        // 完全没有 pro/flash/flash_lite 三种 tier 的退化响应 → None。
        let quota = make_quota("gemini", true, vec![tier("some_future_tier", 80.0)]);
        assert!(format_subscription_summary(&quota).is_none());
    }

    fn usage_data(plan_name: Option<&str>, utilization: f64) -> UsageData {
        UsageData {
            plan_name: plan_name.map(String::from),
            extra: None,
            is_valid: Some(true),
            invalid_message: None,
            total: Some(100.0),
            used: Some(utilization),
            remaining: Some(100.0 - utilization),
            unit: Some("%".to_string()),
        }
    }

    fn usage_result(success: bool, data: Vec<UsageData>) -> UsageResult {
        UsageResult {
            success,
            data: if data.is_empty() { None } else { Some(data) },
            error: None,
        }
    }

    #[test]
    fn script_summary_token_plan_two_tiers() {
        let r = usage_result(
            true,
            vec![
                usage_data(Some(TIER_FIVE_HOUR), 12.0),
                usage_data(Some(TIER_WEEKLY_LIMIT), 80.0),
            ],
        );
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("h12%"), "expected h12% in {s}");
        assert!(s.contains("w80%"), "expected w80% in {s}");
        assert!(s.starts_with("\u{1F7E0}"), "expected orange emoji in {s}");
    }

    #[test]
    fn script_summary_token_plan_worst_drives_emoji() {
        let r = usage_result(
            true,
            vec![
                usage_data(Some(TIER_FIVE_HOUR), 20.0),
                usage_data(Some(TIER_WEEKLY_LIMIT), 95.0),
            ],
        );
        let s = format_script_summary(&r).unwrap();
        assert!(s.starts_with("\u{1F534}"), "expected red emoji in {s}");
    }

    #[test]
    fn script_summary_token_plan_five_hour_only() {
        let r = usage_result(true, vec![usage_data(Some(TIER_FIVE_HOUR), 8.0)]);
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("h8%"), "expected h8% in {s}");
        assert!(
            !s.contains("plan_name"),
            "plan_name should not leak into label: {s}"
        );
    }

    #[test]
    fn script_summary_token_plan_weekly_only() {
        let r = usage_result(true, vec![usage_data(Some(TIER_WEEKLY_LIMIT), 50.0)]);
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("w50%"), "expected w50% in {s}");
    }

    #[test]
    fn script_summary_token_plan_volcengine_three_tiers_with_monthly() {
        // 火山方舟 Agent Plan 回 5h/周/月三档，托盘应包含 m（月）窗口，
        // 不再静默丢弃。
        let r = usage_result(
            true,
            vec![
                usage_data(Some(TIER_FIVE_HOUR), 25.0),
                usage_data(Some(TIER_WEEKLY_LIMIT), 30.0),
                usage_data(Some(TIER_MONTHLY), 42.0),
            ],
        );
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("h25%"), "expected h25% in {s}");
        assert!(s.contains("w30%"), "expected w30% in {s}");
        assert!(s.contains("m42%"), "expected m42% in {s}");
    }

    #[test]
    fn script_summary_token_plan_monthly_only_renders_label_not_raw_name() {
        // 仅月窗口激活时不应回落到原始 "monthly" 机器名，而是走 m 标签。
        let r = usage_result(true, vec![usage_data(Some(TIER_MONTHLY), 60.0)]);
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("m60%"), "expected m60% in {s}");
        assert!(
            !s.contains("monthly"),
            "raw tier name should not leak into label: {s}"
        );
    }

    #[test]
    fn script_summary_official_subscription_claude_uses_h_and_w_labels() {
        let r = usage_result(
            true,
            vec![
                usage_data(Some(TIER_FIVE_HOUR), 12.0),
                usage_data(Some(TIER_SEVEN_DAY), 80.0),
            ],
        );
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("h12%"), "expected h12% in {s}");
        assert!(s.contains("w80%"), "expected w80% in {s}");
        assert!(
            !s.contains(TIER_SEVEN_DAY),
            "tier machine name should not leak into label: {s}"
        );
    }

    #[test]
    fn script_summary_week_aliases_use_highest_utilization() {
        let r = usage_result(
            true,
            vec![
                usage_data(Some(TIER_FIVE_HOUR), 10.0),
                usage_data(Some(TIER_SEVEN_DAY_OPUS), 20.0),
                usage_data(Some(TIER_SEVEN_DAY_SONNET), 95.0),
            ],
        );
        let s = format_script_summary(&r).unwrap();
        assert!(s.contains("w95%"), "expected w95% in {s}");
        assert!(s.starts_with("\u{1F534}"), "expected red emoji in {s}");
    }

    #[test]
    fn script_summary_official_subscription_gemini_uses_short_labels() {
        let r = usage_result(
            true,
            vec![
                usage_data(Some(TIER_GEMINI_PRO), 15.0),
                usage_data(Some(TIER_GEMINI_FLASH), 42.0),
                usage_data(Some(TIER_GEMINI_FLASH_LITE), 80.0),
            ],
        );
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("p15%"), "expected p15% in {s}");
        assert!(s.contains("f42%"), "expected f42% in {s}");
        assert!(s.contains("l80%"), "expected l80% in {s}");
        assert!(
            !s.contains("gemini_"),
            "Gemini tier machine names should not leak into label: {s}"
        );
    }

    #[test]
    fn script_summary_single_bucket_fallback_with_plan_name() {
        let r = usage_result(true, vec![usage_data(Some("Copilot Pro"), 40.0)]);
        let s = format_script_summary(&r).expect("should format");
        assert!(s.contains("Copilot Pro"), "expected plan name in {s}");
        assert!(s.contains("40%"), "expected 40% in {s}");
        assert!(
            !s.contains("h40%"),
            "must not relabel non-token-plan data as h: {s}"
        );
    }

    #[test]
    fn script_summary_single_bucket_fallback_without_plan_name() {
        let r = usage_result(true, vec![usage_data(None, 15.0)]);
        let s = format_script_summary(&r).expect("should format");
        assert_eq!(s, "\u{1F7E2} 15%", "expected emoji + pct only, got {s}");
    }

    #[test]
    fn script_summary_failure_returns_none() {
        let r = usage_result(false, vec![usage_data(Some(TIER_FIVE_HOUR), 12.0)]);
        assert!(format_script_summary(&r).is_none());
    }

    #[test]
    fn script_summary_empty_data_returns_none() {
        let r = usage_result(true, vec![]);
        assert!(format_script_summary(&r).is_none());
    }
}
