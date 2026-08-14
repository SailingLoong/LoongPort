use serde::Serialize;
use std::time::Duration;
use tauri_plugin_updater::UpdaterExt;

use crate::error::AppError;

const APP_UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

fn app_update_check_timeout() -> Duration {
    APP_UPDATE_CHECK_TIMEOUT
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub available_version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AppUpdateCheckResult {
    UpToDate,
    Available { info: AppUpdateInfo },
}

impl AppUpdateCheckResult {
    pub fn available(
        current_version: String,
        available_version: String,
        notes: Option<String>,
        pub_date: Option<String>,
    ) -> Self {
        Self::Available {
            info: AppUpdateInfo {
                current_version,
                available_version,
                notes,
                pub_date,
            },
        }
    }
}

pub async fn check(app: &tauri::AppHandle) -> Result<AppUpdateCheckResult, AppError> {
    let updater = app
        .updater_builder()
        .timeout(app_update_check_timeout())
        .build()
        .map_err(|error| AppError::Message(format!("初始化更新器失败: {error}")))?;
    let update = updater
        .check()
        .await
        .map_err(|error| AppError::Message(format!("检查更新失败: {error}")))?;

    let Some(update) = update else {
        return Ok(AppUpdateCheckResult::UpToDate);
    };

    // The updater validates `pub_date` as RFC 3339 before constructing `Update`.
    // Preserve that original manifest string instead of using OffsetDateTime's
    // human-readable Display representation, which is not RFC 3339.
    let pub_date = update
        .date
        .and_then(|_| update.raw_json.get("pub_date")?.as_str().map(str::to_owned));

    Ok(AppUpdateCheckResult::available(
        update.current_version,
        update.version,
        update.body,
        pub_date,
    ))
}

#[cfg(test)]
mod tests {
    use super::{app_update_check_timeout, AppUpdateCheckResult};
    use std::time::Duration;

    #[test]
    fn update_checks_use_the_backend_request_timeout_policy() {
        assert_eq!(app_update_check_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn available_update_serializes_for_the_frontend() {
        let result = AppUpdateCheckResult::available(
            "3.24.0".into(),
            "3.25.0".into(),
            Some("notes".into()),
            Some("2026-08-14T00:00:00Z".into()),
        );

        let value = serde_json::to_value(result).expect("serialize available update");

        assert_eq!(value["status"], "available");
        assert_eq!(value["info"]["currentVersion"], "3.24.0");
        assert_eq!(value["info"]["availableVersion"], "3.25.0");
        assert_eq!(value["info"]["notes"], "notes");
        assert_eq!(value["info"]["pubDate"], "2026-08-14T00:00:00Z");
    }

    #[test]
    fn absent_update_metadata_serializes_as_null() {
        let result = AppUpdateCheckResult::available("3.24.0".into(), "3.25.0".into(), None, None);

        let value = serde_json::to_value(result).expect("serialize absent update metadata");

        assert_eq!(value["info"]["notes"], serde_json::Value::Null);
        assert_eq!(value["info"]["pubDate"], serde_json::Value::Null);
    }

    #[test]
    fn up_to_date_serializes_as_a_tagged_result() {
        let value = serde_json::to_value(AppUpdateCheckResult::UpToDate)
            .expect("serialize up-to-date result");

        assert_eq!(value, serde_json::json!({ "status": "upToDate" }));
    }
}
