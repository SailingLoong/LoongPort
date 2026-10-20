//! In-process LoongPort commands. No Paperclip server or socket is required.
use serde_json::Value;
#[cfg(unix)]
fn owner(app: &tauri::AppHandle) -> Result<crate::paperclip::Owner, String> {
    use tauri::Manager;
    let coordinator = app
        .try_state::<crate::secrets::startup::StartupCoordinator>()
        .ok_or("agents.not_ready")?;
    let state = app
        .try_state::<crate::store::AppState>()
        .ok_or("agents.not_ready")?;
    coordinator
        .local_agents_owner(&state)
        .map_err(|_| "agents.not_ready".into())
}
#[tauri::command]
pub async fn create_agents_task(
    app: tauri::AppHandle,
    request_id: String,
    input: String,
    model: String,
) -> Result<Value, String> {
    #[cfg(unix)]
    {
        let owner = owner(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            owner.create_local_task(request_id, input, model)
        })
        .await
        .map_err(|_| "agents.worker_failed".to_string())?
        .map_err(|_| "agents.create_failed".into())
    }
    #[cfg(not(unix))]
    {
        let _ = (app, request_id, input, model);
        Err("agents.unsupported_platform".into())
    }
}
#[tauri::command]
pub async fn list_agents_tasks(app: tauri::AppHandle) -> Result<Value, String> {
    #[cfg(unix)]
    {
        let owner = owner(&app)?;
        tauri::async_runtime::spawn_blocking(move || owner.local_tasks())
            .await
            .map_err(|_| "agents.worker_failed".to_string())?
            .map_err(|_| "agents.read_failed".into())
    }
    #[cfg(not(unix))]
    {
        let _ = app;
        Err("agents.unsupported_platform".into())
    }
}
#[tauri::command]
pub async fn cancel_agents_task(app: tauri::AppHandle, task_id: String) -> Result<Value, String> {
    #[cfg(unix)]
    {
        let owner = owner(&app)?;
        tauri::async_runtime::spawn_blocking(move || owner.cancel_local_task(&task_id))
            .await
            .map_err(|_| "agents.worker_failed".to_string())?
            .map_err(|_| "agents.cancel_failed".into())
    }
    #[cfg(not(unix))]
    {
        let _ = (app, task_id);
        Err("agents.unsupported_platform".into())
    }
}
