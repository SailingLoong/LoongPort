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
    analyses: Option<Vec<crate::paperclip::contract::AnalysisInput>>,
    grouped: Option<bool>,
) -> Result<Value, String> {
    #[cfg(unix)]
    {
        let owner = owner(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            let rows = if analyses.is_none() {
                owner.create_local_task(request_id, input, model)
            } else {
                owner.create_local_task_analyses(request_id, input, model, analyses)
            }?;
            Ok::<_, crate::error::AppError>(if grouped.unwrap_or(false) {
                crate::paperclip::tasks::task_groups(rows)
            } else {
                rows
            })
        })
        .await
        .map_err(|_| "agents.worker_failed".to_string())?
        .map_err(|_| "agents.create_failed".into())
    }
    #[cfg(not(unix))]
    {
        let _ = (app, request_id, input, model, analyses, grouped);
        Err("agents.unsupported_platform".into())
    }
}
#[tauri::command]
pub async fn list_agents_tasks(
    app: tauri::AppHandle,
    grouped: Option<bool>,
) -> Result<Value, String> {
    #[cfg(unix)]
    {
        let owner = owner(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            owner.local_tasks().map(|rows| {
                if grouped.unwrap_or(false) {
                    crate::paperclip::tasks::task_groups(rows)
                } else {
                    rows
                }
            })
        })
        .await
        .map_err(|_| "agents.worker_failed".to_string())?
        .map_err(|_| "agents.read_failed".into())
    }
    #[cfg(not(unix))]
    {
        let _ = (app, grouped);
        Err("agents.unsupported_platform".into())
    }
}
#[tauri::command]
pub async fn cancel_agents_task(
    app: tauri::AppHandle,
    task_id: String,
    grouped: Option<bool>,
) -> Result<Value, String> {
    #[cfg(unix)]
    {
        let owner = owner(&app)?;
        tauri::async_runtime::spawn_blocking(move || {
            owner.cancel_local_task(&task_id).map(|rows| {
                if grouped.unwrap_or(false) {
                    crate::paperclip::tasks::task_groups(rows)
                } else {
                    rows
                }
            })
        })
        .await
        .map_err(|_| "agents.worker_failed".to_string())?
        .map_err(|_| "agents.cancel_failed".into())
    }
    #[cfg(not(unix))]
    {
        let _ = (app, task_id, grouped);
        Err("agents.unsupported_platform".into())
    }
}
