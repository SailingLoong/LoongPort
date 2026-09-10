//! Optional service onboarding and the existing reward registration command.

use tauri::Emitter;

use crate::events::{RegisterCompletedPayload, ONBOARDING_REGISTER_COMPLETED};
use crate::relay::onboarding;

use super::relay::{import_site, BrowserEntrySource, ImportResult};

/// 打开官方站（BestAPI）注册窗 —— Star 对话框「领取」点击后调用，
/// 所以优惠码必给且显式传入。
///
/// 码走**显式参数**而不是塞进 `promo_codes` 码表：那张表是给所有导入无条件
/// 预填的，而这份码要 gate 在 star 后面 —— 两个 owner、两份数据，别合。
///
/// 窗口生命周期在后台跑（命令不能等 `import_site`：它要到用户注册完 / 关窗 /
/// 超时才返回）。注册成功仍发 [`ONBOARDING_REGISTER_COMPLETED`]，`RelaySection`
/// 的 toast + 档位预配 + 列表刷新原样保留。
#[tauri::command]
pub async fn onboarding_open_register_window(
    app_handle: tauri::AppHandle,
    promo_code: String,
) -> Result<(), String> {
    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let registration_url = format!("{}/register", onboarding::OFFICIAL_SITE_ORIGIN);
        match import_site(
            &handle,
            &registration_url,
            BrowserEntrySource::Onboarding,
            Some(&promo_code),
        )
        .await
        {
            Ok(result) => {
                let ImportResult {
                    relay_id,
                    site_name,
                    ..
                } = result;
                let _ = handle.emit(
                    ONBOARDING_REGISTER_COMPLETED,
                    RegisterCompletedPayload {
                        relay_id,
                        site_name,
                    },
                );
            }
            Err(error) => {
                // 关窗 / 超时走这里（RelayImportError::Incomplete）—— 正常结局，
                // 不打扰用户。真异常（协议冲突等）也只进日志：窗口本身已经把
                // 用户可见的失败呈现过了。
                log::info!("新人引导注册窗未完成：{:?}", error.kind);
            }
        }
    });

    Ok(())
}

/// Only backend-owned onboarding facts cross the command boundary.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceOnboardingStatus {
    should_prompt: bool,
    completed: bool,
    plaza_visible: bool,
}

fn status(settings: &crate::settings::AppSettings) -> ServiceOnboardingStatus {
    ServiceOnboardingStatus {
        should_prompt: !settings.service_onboarding_completed
            && !settings.service_onboarding_dismissed,
        completed: settings.service_onboarding_completed,
        plaza_visible: settings.plaza_visible.unwrap_or(false),
    }
}

#[tauri::command]
pub fn service_onboarding_status() -> ServiceOnboardingStatus {
    status(&crate::settings::get_settings())
}

#[tauri::command]
pub fn service_onboarding_dismiss() -> Result<ServiceOnboardingStatus, String> {
    crate::settings::mutate_settings(|settings| settings.service_onboarding_dismissed = true)
        .map_err(|error| error.to_string())?;
    Ok(service_onboarding_status())
}

fn complete(settings: &mut crate::settings::AppSettings, share_data: bool) {
    if settings.service_onboarding_completed {
        return;
    }
    settings.enable_anonymous_stats = share_data;
    settings.crowd_metrics_enabled = share_data;
    settings.stats_notice_confirmed = Some(true);
    settings.crowd_metrics_notice_confirmed = Some(true);
    settings.service_onboarding_completed = true;
}

#[tauri::command]
pub fn service_onboarding_complete(share_data: bool) -> Result<ServiceOnboardingStatus, String> {
    crate::settings::mutate_settings(|settings| complete(settings, share_data))
        .map_err(|error| error.to_string())?;
    Ok(service_onboarding_status())
}

#[cfg(test)]
mod service_tests {
    use super::*;

    #[test]
    fn dismissal_never_grants_sharing_and_completion_saves_both_choices() {
        let mut settings = crate::settings::AppSettings::default();
        assert!(status(&settings).should_prompt);
        assert!(!status(&settings).plaza_visible);
        settings.service_onboarding_dismissed = true;
        assert!(!status(&settings).should_prompt);
        assert!(!settings.enable_anonymous_stats && !settings.crowd_metrics_enabled);
        for enabled in [true, false] {
            let mut settings = crate::settings::AppSettings::default();
            complete(&mut settings, enabled);
            assert!(status(&settings).completed);
            assert_eq!(settings.enable_anonymous_stats, enabled);
            assert_eq!(settings.crowd_metrics_enabled, enabled);
            assert_eq!(settings.plaza_visible, None);
            complete(&mut settings, !enabled);
            assert_eq!(settings.enable_anonymous_stats, enabled);
            assert_eq!(settings.crowd_metrics_enabled, enabled);
        }
    }

    #[test]
    fn upgrades_keep_preferences_and_do_not_show_new_install_prompt() {
        let mut settings = serde_json::from_str::<crate::settings::AppSettings>(
            r#"{"enableAnonymousStats":false,"crowdMetricsEnabled":true}"#,
        )
        .unwrap();
        complete(&mut settings, true);
        assert!(status(&settings).completed);
        assert!(!status(&settings).should_prompt);
        assert!(!settings.enable_anonymous_stats);
        assert!(settings.crowd_metrics_enabled);
        let value = serde_json::to_value(status(&settings)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"shouldPrompt":false,"completed":true,"plazaVisible":false})
        );
    }
}
