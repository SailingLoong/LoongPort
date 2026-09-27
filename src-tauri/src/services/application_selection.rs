//! Application tier selection, confirmation and reversible configuration changes.
use crate::{
    app_config::AppType, error::AppError, events::emit_provider_switched, relay::chatgpt_app,
    services::ProviderService, store::AppState,
};
use serde::Serialize;
use tauri::Manager;

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

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TierSelection {
    pub provider_id: String,
    pub model: Option<String>,
}

pub(crate) fn select_with_commit(
    state: &AppState,
    app: &AppType,
    selection: &TierSelection,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<crate::services::SwitchResult, AppError> {
    if !app.supports_local_proxy() {
        if selection.model.is_some() {
            return Err(AppError::Config(
                "Application does not support model selection".into(),
            ));
        }
        let result = ProviderService::switch(state, app.clone(), &selection.provider_id)?;
        commit()?;
        return Ok(result);
    }
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    let provider = state
        .db
        .get_provider_by_id(&selection.provider_id, app.as_str())?
        .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
    crate::services::provider::validate_provider_selection(&state.db, app, &provider.id)?;
    let selected_settings = selection
        .model
        .as_deref()
        .map(|model| crate::relay::model_catalog::select_model(app, &provider, model))
        .transpose()?;
    crate::services::provider::with_provider_config_transaction(state, app, &provider, || {
        if let Some(settings) = selected_settings {
            state
                .db
                .update_provider_settings_config(app.as_str(), &provider.id, &settings)?;
        }
        crate::proxy::auto_strategy::set_model_pref(
            &state.db,
            app.as_str(),
            selection.model.as_deref(),
        )?;
        let result = ProviderService::switch_locked(state, app.clone(), &selection.provider_id)?;
        commit()?;
        Ok(result)
    })
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingOrder {
    pub profile_name: String,
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationRoutingChange {
    pub order: Option<RoutingOrder>,
    pub selection: Option<TierSelection>,
}

pub async fn apply_application_routing(
    app_handle: &tauri::AppHandle,
    app: AppType,
    change: ApplicationRoutingChange,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    if change.order.is_none() && change.selection.is_none() {
        return Err(AppError::Config("No routing change was supplied".into()));
    }
    let state = app_handle.state::<AppState>();
    if let Some(order) = &change.order {
        if !app.supports_local_proxy() {
            return Err(AppError::Config(
                "Application does not support routing order".into(),
            ));
        }
        crate::services::order_profiles::validate_order(
            &state.db,
            app.as_str(),
            &order.profile_name,
            &order.provider_ids,
        )?;
        if let Some(selection) = &change.selection {
            if !order.provider_ids.contains(&selection.provider_id) {
                return Err(AppError::Config(
                    "Selected provider is outside the applied order".into(),
                ));
            }
        }
    }
    let selection = change.selection.as_ref();
    if let Some(selection) = selection {
        let provider = state
            .db
            .get_provider_by_id(&selection.provider_id, app.as_str())?
            .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
        crate::services::provider::validate_provider_selection(&state.db, &app, &provider.id)?;
        if let Some(model) = selection.model.as_deref() {
            crate::relay::model_catalog::select_model(&app, &provider, model)?;
        }
        if should_request_switch_confirmation(
            &app,
            user_choice,
            chatgpt_app::needs_user_attention(),
            crate::services::provider::is_app_taken_over(&state, &app),
        ) {
            return Ok(SwitchTierCommandResult::ConfirmationRequired {
                target_name: match selection.model.as_deref() {
                    Some(model) => format!("{} · {model}", provider.name),
                    None => provider.name,
                },
            });
        }
    }
    let commit_order = || {
        if let Some(order) = &change.order {
            crate::services::order_profiles::apply_order(
                &state.db,
                app.as_str(),
                &order.profile_name,
                &order.provider_ids,
            )?;
        }
        Ok(())
    };
    let result = if let Some(selection) = selection {
        switch_tier_impl(
            app_handle,
            &selection.provider_id,
            app.clone(),
            selection.model.as_deref(),
            user_choice.unwrap_or(false),
            commit_order,
        )
        .await?
    } else {
        let _guard = state.proxy_service.lock_switch_for_app(app.as_str()).await;
        commit_order()?;
        SwitchTierResult {
            provider_name: String::new(),
            chatgpt_was_running: false,
            chatgpt_relaunched: false,
            warnings: Vec::new(),
        }
    };
    Ok(SwitchTierCommandResult::Switched { result })
}

/// The model picker and tray share the same application selection operation.
pub(crate) async fn switch_tier_model_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: &str,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    apply_application_routing(
        app_handle,
        app_type,
        ApplicationRoutingChange {
            order: None,
            selection: Some(TierSelection {
                provider_id: provider_id.into(),
                model: Some(model.into()),
            }),
        },
        user_choice,
    )
    .await
}

pub(crate) fn should_request_switch_confirmation(
    app_type: &AppType,
    user_choice: Option<bool>,
    needs_attention: bool,
    taken_over: bool,
) -> bool {
    // 确认弹窗的唯一职责是授权「退你正开着的 ChatGPT」。代管（takeover）态的
    // 切换是热切换——不写 CLI 配置（见 [`ProviderService::switch`] 的
    // `is_app_taken_over` 分支），退了重开 codex 也不会加载任何新东西，
    // 弹窗问的就是一件不需要做的事 ⇒ 一并跳过。
    !taken_over && matches!(app_type, AppType::Codex) && user_choice.is_none() && needs_attention
}

/// 代管态下即使传了 `quit_chatgpt=true`（确认弹窗时代留下的入参）也不退：
/// 热切换不碰 CLI 配置，退/重开纯属打断。判据与 `ProviderService::switch`
/// 内部同源（[`crate::services::provider::is_app_taken_over`]），别在这层再判一份。
fn wants_chatgpt_quit(quit_chatgpt: bool, app_type: &AppType, taken_over: bool) -> bool {
    !taken_over && should_quit_chatgpt(quit_chatgpt, app_type)
}

pub(crate) async fn switch_tier_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    apply_application_routing(
        app_handle,
        app_type,
        ApplicationRoutingChange {
            order: None,
            selection: Some(TierSelection {
                provider_id: provider_id.into(),
                model: None,
            }),
        },
        user_choice,
    )
    .await
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
    model: Option<&str>,
    quit_chatgpt: bool,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<SwitchTierResult, AppError> {
    let quit_chatgpt = wants_chatgpt_quit(
        quit_chatgpt,
        &app_type,
        crate::services::provider::is_app_taken_over(&app_handle.state::<AppState>(), &app_type),
    );
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
        select_with_commit(
            &state,
            &app_type,
            &TierSelection {
                provider_id: provider_id.to_string(),
                model: model.map(str::to_owned),
            },
            commit,
        )
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
    crate::services::application_overview::record_successful_selection(
        &app_handle.state::<AppState>().db,
        &app_type_for_event,
        provider_id,
    );
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
    #[serial_test::serial]
    fn explicit_model_selection_replaces_old_preference_and_rolls_back_on_commit_failure() {
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                    None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
                }
                let _ = crate::settings::reload_settings();
            }
        }
        let _restore = RestoreHome(previous_home);
        std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
        crate::settings::reload_settings().unwrap();
        let db = std::sync::Arc::new(crate::secrets::testing::initialize_database().unwrap());
        let settings = serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://relay.example", "ANTHROPIC_AUTH_TOKEN":"example-key","ANTHROPIC_MODEL":"model-one","CLAUDE_CODE_SUBAGENT_MODEL":"worker"},"modelCatalog":{"models":[{"model":"model-one"},{"model":"model-two"}]}});
        let provider = crate::provider::Provider::with_id(
            "custom".into(),
            "Custom".into(),
            settings.clone(),
            None,
        );
        db.save_provider("claude", &provider).unwrap();
        db.set_current_provider("claude", "custom").unwrap();
        crate::services::provider::write_standalone_live_snapshot(&AppType::Claude, &provider)
            .unwrap();
        crate::proxy::auto_strategy::set_model_pref(&db, "claude", Some("model-one")).unwrap();
        let state = AppState::new(db.clone()).unwrap();
        let selection = TierSelection {
            provider_id: "custom".into(),
            model: Some("model-two".into()),
        };
        select_with_commit(&state, &AppType::Claude, &selection, || Ok(())).unwrap();
        assert_eq!(
            crate::proxy::application_routing::effective_model(&db, "claude").as_deref(),
            Some("model-two")
        );
        let after = db.get_provider_by_id("custom", "claude").unwrap().unwrap();
        assert_eq!(
            after.settings_config["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
            "worker"
        );
        let live = std::fs::read(crate::config::get_claude_settings_path()).unwrap();
        let error = select_with_commit(
            &state,
            &AppType::Claude,
            &TierSelection {
                model: Some("model-one".into()),
                ..selection
            },
            || Err(AppError::Config("commit rejected".into())),
        )
        .unwrap_err();
        assert!(error.to_string().contains("commit rejected"));
        assert_eq!(
            crate::proxy::application_routing::effective_model(&db, "claude").as_deref(),
            Some("model-two")
        );
        assert_eq!(
            std::fs::read(crate::config::get_claude_settings_path()).unwrap(),
            live
        );
        let mut edited = db.get_provider_by_id("custom", "claude").unwrap().unwrap();
        edited.settings_config["env"]["ANTHROPIC_MODEL"] = serde_json::json!("model-one");
        ProviderService::update(&state, AppType::Claude, None, edited).unwrap();
        assert!(crate::proxy::auto_strategy::get_model_pref(&db, "claude").is_none());
        let live: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::config::get_claude_settings_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "model-one");
        assert_eq!(live["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "worker");
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
            true,
            false
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Claude,
            None,
            true,
            false
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Codex,
            Some(false),
            true,
            false
        ));
    }

    /// 代管（热切换）态：确认弹窗与退/重开一并跳过 —— 热切换不写 CLI 配置，
    /// 退了重开 codex 也不会加载任何新东西，两样都是纯打断。
    /// 回归背景：v6.26.0 前代管态切档/换模型仍弹「退出并切换」并真的退重开。
    #[test]
    fn takeover_hot_switch_skips_confirmation_and_quit() {
        // codex + 用户未选 + ChatGPT 在跑：非代管 ⇒ 弹确认（旧状照旧）。
        assert!(should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true,
            false
        ));
        // 同样条件下代管 ⇒ 不弹。
        assert!(!should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true,
            true
        ));
        // 用户已确认要退（quit=true），代管 ⇒ 也不退。
        assert!(!wants_chatgpt_quit(true, &AppType::Codex, true));
        // 非代管照旧退；非 codex 照旧不退。
        assert!(wants_chatgpt_quit(true, &AppType::Codex, false));
        assert!(!wants_chatgpt_quit(true, &AppType::Claude, false));
        assert!(!wants_chatgpt_quit(false, &AppType::Codex, false));
    }
}
