use crate::error::AppError;
use crate::zcode_config::{self, ZCodeConfigView, ZCodeProviderInput};

#[derive(serde::Serialize)]
pub struct ZCodeCommandError {
    code: &'static str,
    message: String,
}

impl From<AppError> for ZCodeCommandError {
    fn from(error: AppError) -> Self {
        Self {
            code: if matches!(error, AppError::Conflict(_)) {
                "zcode.configuration_changed"
            } else {
                "zcode.operation_failed"
            },
            message: error.to_string(),
        }
    }
}

fn task_failed() -> ZCodeCommandError {
    ZCodeCommandError {
        code: "zcode.operation_failed",
        message: "ZCode configuration task failed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conflict_code_does_not_depend_on_error_wording() {
        let error = ZCodeCommandError::from(AppError::Conflict("external edit".into()));
        assert_eq!(
            serde_json::to_value(error).unwrap()["code"],
            "zcode.configuration_changed"
        );
        let error = ZCodeCommandError::from(AppError::Config("refresh before saving".into()));
        assert_eq!(
            serde_json::to_value(error).unwrap()["code"],
            "zcode.operation_failed"
        );
    }
}

#[tauri::command]
pub async fn get_zcode_config() -> Result<ZCodeConfigView, ZCodeCommandError> {
    tauri::async_runtime::spawn_blocking(zcode_config::read)
        .await
        .map_err(|_| task_failed())?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn save_zcode_provider(
    input: ZCodeProviderInput,
) -> Result<ZCodeConfigView, ZCodeCommandError> {
    tauri::async_runtime::spawn_blocking(move || zcode_config::save(input))
        .await
        .map_err(|_| task_failed())?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn remove_zcode_provider(
    id: String,
    revision: String,
) -> Result<ZCodeConfigView, ZCodeCommandError> {
    tauri::async_runtime::spawn_blocking(move || zcode_config::remove(&id, &revision))
        .await
        .map_err(|_| task_failed())?
        .map_err(Into::into)
}
