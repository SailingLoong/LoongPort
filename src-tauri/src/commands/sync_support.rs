use serde_json::{json, Value};

use crate::error::AppError;
use crate::services::{model_pricing, PromptService, ProviderService};
use crate::settings;
use crate::store::AppState;

pub(crate) fn run_post_import_sync(app_state: &AppState) -> Result<(), AppError> {
    let mut failures = Vec::new();
    if let Err(error) = ProviderService::sync_current_to_live(app_state) {
        failures.push(format!("live configuration: {error}"));
    }
    finish_post_import_sync(app_state, failures)
}

/// Explicit configuration imports have already applied providers under the
/// application locks. Continue with shared auxiliary projections exactly once.
pub(crate) fn run_post_import_sync_after_providers(app_state: &AppState) -> Result<(), AppError> {
    let mut failures = Vec::new();
    if let Err(error) = ProviderService::sync_non_provider_live(app_state) {
        failures.push(format!("live configuration: {error}"));
    }
    finish_post_import_sync(app_state, failures)
}

fn finish_post_import_sync(
    app_state: &AppState,
    mut failures: Vec<String>,
) -> Result<(), AppError> {
    if let Err(error) = PromptService::sync_all_to_live(app_state) {
        failures.push(format!("prompts: {error}"));
    }
    if let Err(error) = model_pricing::sync_local_model_pricing(&app_state.db) {
        failures.push(format!("model pricing: {error}"));
    }
    if let Err(error) = settings::reload_settings() {
        failures.push(format!("settings cache: {error}"));
    }

    match app_state.db.get_log_config() {
        Ok(log_config) => log::set_max_level(log_config.to_level_filter()),
        Err(error) => {
            log::set_max_level(log::LevelFilter::Info);
            failures.push(format!("runtime log level: {error}"));
        }
    }
    app_state.usage_cache.invalidate_all();

    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "部分导入后同步失败: {}",
            failures.join("; ")
        )))
    }
}

fn post_sync_warning<E: std::fmt::Display>(err: E) -> String {
    AppError::localized(
        "sync.post_operation_sync_failed",
        format!("后置同步状态失败: {err}"),
        format!("Post-operation synchronization failed: {err}"),
    )
    .to_string()
}

pub(crate) fn post_sync_warning_from_result(
    result: Result<Result<(), AppError>, String>,
) -> Option<String> {
    match result {
        Ok(Ok(())) => None,
        Ok(Err(err)) => Some(post_sync_warning(err)),
        Err(err) => Some(post_sync_warning(err)),
    }
}

pub(crate) fn attach_warning(mut value: Value, warning: Option<String>) -> Value {
    if let Some(message) = warning {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("warning".to_string(), Value::String(message));
        }
    }
    value
}

pub(crate) fn success_payload_with_warning(backup_id: String, warning: Option<String>) -> Value {
    attach_warning(
        json!({
            "success": true,
            "message": "SQL imported successfully",
            "backupId": backup_id
        }),
        warning,
    )
}

/// Caller keeps the shared sync mutex through the transition and live projection.
pub(crate) async fn restore_downloaded_snapshot(
    app_state: AppState,
    snapshot: crate::services::sync_protocol::DownloadedSnapshot,
    password: zeroize::Zeroizing<String>,
    expected_snapshot_id: String,
) -> Result<(crate::services::sync_protocol::DownloadedSnapshot, Value), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::services::sync_protocol::restore_from_sync(
            &app_state.db,
            &snapshot,
            &expected_snapshot_id,
            &password,
            &crate::secrets::key_store::SystemKeyStore,
        )
        .map_err(|error| match error {
            AppError::Localized { key, .. } => key.to_owned(),
            AppError::Config(code) if code.starts_with("sync.") || code.starts_with("secret.") => {
                code
            }
            other => other.to_string(),
        })?;
        let warning = run_post_import_sync(&app_state)
            .err()
            .map(post_sync_warning);
        let result = attach_warning(json!({"status":"restored"}), warning);
        Ok((snapshot, result))
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::{attach_warning, post_sync_warning_from_result};
    use serde_json::json;

    #[test]
    fn post_sync_warning_from_result_returns_none_on_success() {
        let warning = post_sync_warning_from_result(Ok(Ok(())));
        assert!(warning.is_none());
    }

    #[test]
    fn post_sync_warning_from_result_returns_some_on_sync_error() {
        let warning =
            post_sync_warning_from_result(Ok(Err(crate::error::AppError::Config("boom".into()))));
        assert!(warning.is_some());
    }

    #[tokio::test]
    async fn post_sync_warning_from_result_returns_some_on_join_error() {
        let handle = tokio::spawn(async move {
            panic!("forced join error");
        });
        let join_err = handle.await.expect_err("task should panic");
        let warning = post_sync_warning_from_result(Err(join_err.to_string()));
        assert!(warning.is_some());
    }

    #[test]
    fn attach_warning_adds_warning_without_dropping_existing_fields() {
        let payload = json!({ "status": "downloaded" });
        let updated = attach_warning(payload, Some("post sync warning".to_string()));
        assert_eq!(
            updated.get("status").and_then(|v| v.as_str()),
            Some("downloaded")
        );
        assert_eq!(
            updated.get("warning").and_then(|v| v.as_str()),
            Some("post sync warning")
        );
    }
}
