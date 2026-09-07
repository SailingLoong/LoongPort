//! 档位切换与选模型编排（含确认弹窗判定、ChatGPT 退避）。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::provision;

/// 切换结果，前端据此出话。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchTierResult {
    pub provider_name: String,
    /// ChatGPT 退出前是不是在跑（决定切换后要不要替用户重开）。
    pub chatgpt_was_running: bool,
    /// 有没有重新打开它。
    pub chatgpt_relaunched: bool,
    /// 非致命的问题（如重开失败），如实带给用户。
    ///
    /// 「退不掉 ChatGPT」**不在这里** —— 那种情况整个命令返回 Err、配置不动，见
    /// [`switch_tier_impl`]。
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
// `rename_all` 只转变体名；变体字段要另加 `rename_all_fields`（serde 1.0.185+）。
// 漏掉它时 `target_name` 蛇形下发、前端读 `targetName` 得 undefined，
// 确认弹窗静默打不开 —— 闸在 tests::switch_tier_command_result_wire_contract。
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SwitchTierCommandResult {
    ConfirmationRequired {
        target_name: String,
    },
    Switched {
        #[serde(flatten)]
        result: SwitchTierResult,
    },
}

/// 切换档位：退 ChatGPT → 切换 → 重开。
///
/// `quit_chatgpt` 由前端在用户确认弹窗后传 true。传 false 则只切换（用户自己管重启）。
///
/// `app` 是**必需参数**，不能从 `provider_id` 反推（spec §三）：
/// `provider_id_for(site_origin, group_id)` 不含 platform，而四段 Key 契约恰恰写明
/// 「分组 id 只在平台内唯一，跨平台会撞号」—— 所以同一个 `loongport-<hash>` 可以合法地
/// 存在于两个 app_type 行下（`providers` 主键是 `(id, app_type)`），哈希单向反解不出来。
/// 调用方（前端）本来就知道当前是哪个 tab。
#[tauri::command]
pub async fn relay_switch_tier(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchTierCommandResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_command(&app_handle, &provider_id, app_type, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}

/// Select a supported model from a managed Codex tier and activate that tier.
///
/// The model catalog stored with the provider is the authority for validation;
/// this keeps a stale frontend from writing an arbitrary model into
/// `config.toml`. Updating the provider before the normal switch flow also
/// means ChatGPT is restarted with the selected model already in place.
#[tauri::command]
pub async fn relay_switch_tier_model(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    model: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchTierCommandResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_model_command(&app_handle, &provider_id, app_type, &model, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}

/// [`relay_switch_tier_model`] 的命令层实现，托盘的模型子菜单也走这里 ——
/// 与 [`switch_tier_command`] 同样的「一个编排、多个入口」约定（见它的文档）。
pub(crate) async fn switch_tier_model_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: &str,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    if should_request_switch_confirmation(
        &app_type,
        user_choice,
        chatgpt_app::needs_user_attention(),
    ) {
        let state = app_handle.state::<AppState>();
        let target_name = state
            .db
            .get_provider_by_id(provider_id, app_type.as_str())?
            .map(|provider| format!("{} · {model}", provider.name))
            .unwrap_or_else(|| model.to_string());
        return Ok(SwitchTierCommandResult::ConfirmationRequired { target_name });
    }
    select_tier_model_impl(
        app_handle,
        provider_id,
        app_type,
        model,
        user_choice.unwrap_or(false),
    )
    .await
    .map(|result| SwitchTierCommandResult::Switched { result })
}

pub(crate) fn should_request_switch_confirmation(
    app_type: &AppType,
    user_choice: Option<bool>,
    needs_attention: bool,
) -> bool {
    matches!(app_type, AppType::Codex) && user_choice.is_none() && needs_attention
}

pub(crate) async fn switch_tier_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    let state = app_handle.state::<AppState>();
    let provider = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())?
        .ok_or_else(|| AppError::Config("这个接入配置不存在".to_string()))?;
    if should_request_switch_confirmation(
        &app_type,
        user_choice,
        chatgpt_app::needs_user_attention(),
    ) {
        return Ok(SwitchTierCommandResult::ConfirmationRequired {
            target_name: provider.name,
        });
    }
    switch_tier_impl(
        app_handle,
        provider_id,
        app_type,
        user_choice.unwrap_or(false),
    )
    .await
    .map(|result| SwitchTierCommandResult::Switched { result })
}

async fn select_tier_model_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: &str,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, AppError> {
    // 支持选模型的平台 = 带模型目录的平台（唯一源
    // [`provision::model_catalog_apps`]）：codex（config TOML）/ claude
    // （env.ANTHROPIC_MODEL）/ gemini（env.GEMINI_MODEL）/ grokbuild（config TOML
    // 选中模型表的 model 字段）。目录（modelCatalog）各平台同一份形状。
    if !provision::supports_model_catalog(&app_type) {
        return Err(AppError::Config(
            "模型选择目前只支持 Codex / Claude / Gemini / Grok 档位".to_string(),
        ));
    }
    if !crate::relay::is_managed(provider_id) {
        return Err(AppError::Config(
            "只有 LoongPort 托管的档位才能从模型列表切换".to_string(),
        ));
    }

    let state = app_handle.state::<AppState>();
    let original_settings = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())?
        .ok_or_else(|| AppError::Config("这个档位不存在".to_string()))?
        .settings_config;
    let settings = match &app_type {
        AppType::Codex => select_codex_model(&original_settings, model)?,
        AppType::GrokBuild => {
            let bare = model.trim();
            if bare.is_empty()
                || !models_from_settings(&original_settings)
                    .iter()
                    .any(|candidate| candidate == bare)
            {
                return Err(AppError::Config(format!(
                    "模型 {bare:?} 不在这个档位支持的模型列表中"
                )));
            }
            select_grok_model(&original_settings, bare)?
        }
        // env 形状：成员资格对着目录校验（select_codex_model 内置，这里对齐）
        _ => {
            let bare = model.trim();
            if bare.is_empty()
                || !models_from_settings(&original_settings)
                    .iter()
                    .any(|candidate| candidate == bare)
            {
                return Err(AppError::Config(format!(
                    "模型 {bare:?} 不在这个档位支持的模型列表中"
                )));
            }
            provision::select_env_model(&app_type, &original_settings, bare)?
        }
    };

    state
        .db
        .update_provider_settings_config(app_type.as_str(), provider_id, &settings)?;

    match switch_tier_impl(app_handle, provider_id, app_type.clone(), quit_chatgpt).await {
        Ok(result) => Ok(result),
        Err(error) => {
            // Model selection is a managed preference, not a manual provider
            // edit. If the guarded switch fails, restore the DB value so a
            // later refresh cannot silently apply a model the user never
            // successfully switched to.
            if let Err(rollback_error) = state.db.update_provider_settings_config(
                app_type.as_str(),
                provider_id,
                &original_settings,
            ) {
                return Err(AppError::Config(format!(
                    "{error}；模型配置回滚失败：{rollback_error}"
                )));
            }
            Err(error)
        }
    }
}

/// 这次切换要不要退 ChatGPT。
///
/// 两个条件都得成立：**用户同意了**（`user_agreed`，来自确认弹窗），
/// **且切的是 codex**（`app_type`）。
///
/// ## 为什么 codex 之外不退
///
/// 不是「其它平台不支持」，是**不需要** —— `chatgpt_app` 管的是 ChatGPT 桌面版
/// （bundle id `com.openai.codex`），它只读 `~/.codex`。切 claude/gemini 的档位时
/// 它压根不涉及，去退它是扰民：关掉用户正开着的、与本次切换毫无关系的对话。
///
/// 判据放后端而不是让前端决定：前端传的 `user_agreed` 表达「用户同意了退出」，
/// 而「这个平台要不要退」是后端事实 —— 两件事别混在一个布尔里。
pub(crate) fn should_quit_chatgpt(user_agreed: bool, app_type: &AppType) -> bool {
    user_agreed && matches!(app_type, AppType::Codex)
}

async fn switch_tier_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    quit_chatgpt: bool,
) -> Result<SwitchTierResult, AppError> {
    let quit_chatgpt = should_quit_chatgpt(quit_chatgpt, &app_type);
    // `AppType` 没派生 Copy（上游结构，别为此改它），而下面 `ProviderService::list`
    // 会把它 move 掉 —— 事件那一步要用，先留一份。
    let app_type_for_event = app_type.clone();

    // 编排（退 → 切 → 重开，失败要把 ChatGPT 开回去）走 `chatgpt_app::around`。
    // 那套四分支处理原来内联在这里，抽出去是为了让**上游那条通用 provider 切换**
    // 也走同一份（`switch_provider` 里那处）—— 复制第二遍的必然结局是两份分叉，
    // 而分叉的表现是「从 LoongPort 页切没问题、从 provider 页切就静默用错配置」。
    //
    // `abort_on_unconfirmed_exit = false`：切档位只写 `config.toml`，退不掉也能照常切
    // （配置写进去就生效了），提示用户手动重启即可。这与「切回官方登录」相反 ——
    // 那条要删 `auth.json`，而 ChatGPT 退出时会重写它。
    let switch_once = || {
        let state = app_handle.state::<AppState>();
        ProviderService::switch(&state, app_type.clone(), provider_id)
            .map_err(|e| AppError::Config(format!("切换失败：{e}。配置未改动")))
    };

    let (switched, chatgpt) = if quit_chatgpt {
        chatgpt_app::around(false, switch_once)?
    } else {
        // 不需要碰 ChatGPT（非 codex，或用户选了「只切换」）：直接切，
        // outcome 全默认（没关过 ⇒ 不重开）。
        (switch_once()?, chatgpt_app::AroundOutcome::default())
    };

    let mut warnings = chatgpt.warnings;
    warnings.extend(switched.warnings);

    let provider_name = {
        let state = app_handle.state::<AppState>();
        ProviderService::list(&state, app_type)
            .ok()
            .and_then(|list| list.get(provider_id).map(|p| p.name.clone()))
            .unwrap_or_else(|| provider_id.to_string())
    };

    // 广播「当前供应商变了」—— **镜像方向**：切档位之后 provider 页那份列表
    // （react-query 的 `["providers", app]`）也陈旧了，而它的刷新靠的正是这个事件
    // （`App.tsx` 的 `providersApi.onSwitched`）。
    //
    // 两条切换路径都发，共用 `commands::provider::emit_provider_switched` 那一份实现 ——
    // payload 形状复制第二遍的必然结局是两份分叉（那边的文档写了完整理由）。
    emit_provider_switched(app_handle, &app_type_for_event, provider_id);

    // 托盘也要跟上：这里不刷，用户从主界面切完档位、再看托盘标题还是旧的
    // （前端 relay 那条路不会替我们调 `update_tray_menu`）。放在 `switch_tier_impl`
    // 而不是各命令壳里 —— relay / vendor / 托盘三个入口一次全修，后来者免接。
    crate::tray::refresh_tray_menu(app_handle);

    Ok(SwitchTierResult {
        provider_name,
        chatgpt_was_running: chatgpt.was_running,
        chatgpt_relaunched: chatgpt.relaunched,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约闸：前端 TS 类型手写断言了这份 wire 形状（`src/lib/api/relay.ts` 的
    /// `SwitchTierCommandResult`），serde 的 enum 级 `rename_all` 只转变体名、
    /// 不转变体字段 —— 没有这条闸的话 casing 分叉编译期完全静默
    /// （2026-08-16 线上事故：`target_name` 蛇形下发，确认弹窗永不打开）。
    #[test]
    fn switch_tier_command_result_wire_contract_is_camel_case() {
        let confirmation = serde_json::to_value(SwitchTierCommandResult::ConfirmationRequired {
            target_name: "站点 · 分组".into(),
        })
        .unwrap();
        assert_eq!(confirmation["status"], "confirmationRequired");
        assert!(
            confirmation.get("targetName").is_some(),
            "变体字段必须驼峰下发：{confirmation}"
        );
        assert!(
            confirmation.get("target_name").is_none(),
            "蛇形键意味着前端读到 undefined：{confirmation}"
        );

        let switched = serde_json::to_value(SwitchTierCommandResult::Switched {
            result: SwitchTierResult {
                provider_name: "p".into(),
                chatgpt_was_running: false,
                chatgpt_relaunched: false,
                warnings: vec![],
            },
        })
        .unwrap();
        assert_eq!(switched["status"], "switched");
        assert!(
            switched.get("providerName").is_some(),
            "flatten 的结构体字段同样是驼峰契约：{switched}"
        );
    }

    #[test]
    fn chatgpt_quit_is_codex_only() {
        // 用户同意 + codex ⇒ 退。
        assert!(should_quit_chatgpt(true, &AppType::Codex));
        // 用户同意但切的是别的平台 ⇒ **不退**。ChatGPT 桌面版只读 ~/.codex，
        // 切 claude/gemini 档位去关它纯属扰民（关掉用户正开着的、与本次切换无关的对话）。
        assert!(!should_quit_chatgpt(true, &AppType::Claude));
        assert!(!should_quit_chatgpt(true, &AppType::Gemini));
        // 用户没同意 ⇒ 一律不退，哪怕是 codex。
        assert!(!should_quit_chatgpt(false, &AppType::Codex));
    }

    #[test]
    fn switch_confirmation_is_decided_before_mutating_the_target() {
        assert!(should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Claude,
            None,
            true
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Codex,
            Some(false),
            true
        ));
    }
}
