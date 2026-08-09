use crate::{relay::model_verification::target, AppState};

#[tauri::command]
pub async fn list_verification_models(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    app_type: String,
) -> Result<Vec<String>, String> {
    target::list_models(&state.db, &provider_id, &app_type)
        .await
        .map_err(|error| error.to_string())
}
