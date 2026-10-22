//! Commands accept credential references only. Decrypted caller keys never cross IPC.
use crate::{
    clients::{
        configuration::{Configuration, Connection, Preview},
        registry::{self, ClientDescriptor},
    },
    live::engine::DeviceStore,
    store::AppState,
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    descriptor: ClientDescriptor,
    connection: Option<Connection>,
    recovery_required: bool,
    status: &'static str,
}
fn owner() -> Configuration {
    Configuration::new(DeviceStore::for_device())
}
fn message(error: crate::error::AppError) -> String {
    match error {
        crate::error::AppError::Config(code) if code.starts_with("clients.") => code,
        _ => "clients.operation_failed".into(),
    }
}

#[tauri::command]
pub fn list_client_connections(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ClientStatus>, String> {
    let vault = state.db.secrets.read().map_err(message)?;
    registry::CLIENTS
        .iter()
        .map(|entry| {
            let (connection, recovery_required, status) = owner()
                .connection(&vault, entry.descriptor.id, &entry.local_path())
                .map_err(message)?;
            Ok(ClientStatus {
                descriptor: entry.descriptor.clone(),
                connection,
                recovery_required,
                status,
            })
        })
        .collect()
}

#[tauri::command]
pub fn list_client_credentials(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::proxy::client_access::CredentialRef>, String> {
    state.db.list_client_credentials().map_err(message)
}

#[tauri::command]
pub fn create_client_credential(
    state: tauri::State<'_, AppState>,
    client_id: String,
    route_app: String,
    models: Vec<String>,
) -> Result<crate::proxy::client_access::CredentialRef, String> {
    let entry = registry::get(&client_id).map_err(message)?;
    if !entry.descriptor.route_apps.contains(&route_app.as_str())
        || models.is_empty()
        || models.iter().any(|id| id.trim().is_empty())
    {
        return Err("clients.invalid_request".into());
    }
    state
        .db
        .issue_client_credential(&client_id, &route_app, &models)
        .map_err(message)
}

async fn project(state: &AppState, request: &Connection) -> Result<serde_json::Value, String> {
    let entry = registry::get(&request.client_id).map_err(message)?;
    if !entry
        .descriptor
        .route_apps
        .contains(&request.route_app.as_str())
        || request.models.is_empty()
    {
        return Err("clients.invalid_request".into());
    }
    let credential = state
        .db
        .list_client_credentials()
        .map_err(message)?
        .into_iter()
        .find(|credential| credential.id == request.credential_ref)
        .ok_or("clients.invalid_credential")?;
    if request
        .models
        .iter()
        .any(|model| !credential.allowed_models.contains(model))
    {
        return Err("clients.invalid_credential".into());
    }
    let secret = zeroize::Zeroizing::new(
        state
            .db
            .resolve_client_credential(
                &request.credential_ref,
                &request.client_id,
                &request.route_app,
            )
            .map_err(message)?,
    );
    if !state.db.client_access_ready().map_err(message)? {
        return Err("clients.access_not_ready".into());
    }
    let status = state.proxy_service.get_status().await?;
    if !status.running {
        return Err("clients.proxy_not_running".into());
    }
    // Always use loopback for this device's clients, including wildcard listeners.
    let address: std::net::IpAddr = status
        .address
        .parse()
        .map_err(|_| "clients.invalid_endpoint")?;
    if !address.is_loopback() && !address.is_unspecified() {
        return Err("clients.invalid_endpoint".into());
    }
    let host = if address.is_ipv6() {
        "[::1]"
    } else {
        "127.0.0.1"
    };
    let endpoint = format!("http://{host}:{}/v1/chat/completions", status.port);
    Ok(entry.adapter.desired(&endpoint, &secret, &request.models))
}

#[tauri::command]
pub async fn preview_client_connection(
    state: tauri::State<'_, AppState>,
    request: Connection,
) -> Result<Preview, String> {
    let entries = project(&state, &request).await?;
    let path = registry::get(&request.client_id)
        .map_err(message)?
        .local_path();
    let vault = state.db.secrets.read().map_err(message)?;
    owner()
        .preview(&vault, &path, &request, &entries)
        .map_err(message)
}

#[tauri::command]
pub async fn apply_client_connection(
    state: tauri::State<'_, AppState>,
    request: Connection,
    token: String,
) -> Result<(), String> {
    let entries = project(&state, &request).await?;
    let path = registry::get(&request.client_id)
        .map_err(message)?
        .local_path();
    let vault = state.db.secrets.read().map_err(message)?;
    owner()
        .apply(&vault, &path, &request, &entries, &token)
        .map_err(message)
}

#[tauri::command]
pub fn disconnect_client(
    state: tauri::State<'_, AppState>,
    client_id: String,
) -> Result<(), String> {
    let path = registry::get(&client_id).map_err(message)?.local_path();
    let vault = state.db.secrets.read().map_err(message)?;
    owner()
        .disconnect(&vault, &path, &client_id)
        .map_err(message)
}

#[tauri::command]
pub fn recover_client_connection(
    state: tauri::State<'_, AppState>,
    client_id: String,
) -> Result<(), String> {
    let path = registry::get(&client_id).map_err(message)?.local_path();
    let vault = state.db.secrets.read().map_err(message)?;
    owner().recover(&vault, &path, &client_id).map_err(message)
}

#[tauri::command]
pub fn revoke_client_credential(
    state: tauri::State<'_, AppState>,
    client_id: String,
    credential_ref: String,
) -> Result<(), String> {
    registry::get(&client_id).map_err(message)?;
    let credential = state
        .db
        .list_client_credentials()
        .map_err(message)?
        .into_iter()
        .find(|reference| reference.id == credential_ref && reference.client_id == client_id)
        .ok_or("clients.invalid_credential")?;
    state
        .db
        .revoke_client_credential(&credential.id)
        .map_err(message)
}
