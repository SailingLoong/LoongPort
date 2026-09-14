use crate::services::env_checker::{check_env_conflicts as check_conflicts, EnvConflict};
use crate::services::env_manager::{
    delete_env_vars as delete_vars, restore_from_backup, BackupInfo,
};

/// Check environment variable conflicts for a specific app
#[tauri::command]
pub fn check_env_conflicts(app: String) -> Result<Vec<EnvConflict>, String> {
    check_conflicts(&app)
}

/// Delete environment variables with backup
#[tauri::command]
pub fn delete_env_vars(
    state: tauri::State<crate::store::AppState>,
    conflicts: Vec<EnvConflict>,
) -> Result<BackupInfo, String> {
    delete_vars(state.db.secret_session(), conflicts)
}

/// Restore environment variables from backup file
#[tauri::command]
pub fn restore_env_backup(
    state: tauri::State<crate::store::AppState>,
    backup_path: String,
) -> Result<(), String> {
    restore_from_backup(state.db.secret_session(), backup_path)
}
