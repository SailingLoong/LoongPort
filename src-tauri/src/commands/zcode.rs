use crate::zcode_config::{self, ZCodeConfigView, ZCodeProviderInput};

#[tauri::command]
pub async fn get_zcode_config() -> Result<ZCodeConfigView, String> {
    tauri::async_runtime::spawn_blocking(zcode_config::read)
        .await
        .map_err(|_| "ZCode configuration task failed".to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_zcode_provider(input: ZCodeProviderInput) -> Result<ZCodeConfigView, String> {
    tauri::async_runtime::spawn_blocking(move || zcode_config::save(input))
        .await
        .map_err(|_| "ZCode configuration task failed".to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_zcode_provider(
    id: String,
    revision: String,
) -> Result<ZCodeConfigView, String> {
    tauri::async_runtime::spawn_blocking(move || zcode_config::remove(&id, &revision))
        .await
        .map_err(|_| "ZCode configuration task failed".to_string())?
        .map_err(|e| e.to_string())
}
