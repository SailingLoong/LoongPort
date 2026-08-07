use crate::deeplink::{
    import_mcp_from_deeplink, import_prompt_from_deeplink, import_provider_from_deeplink,
    import_skill_from_deeplink, parse_deeplink_url, DeepLinkImportRequest,
};
use crate::events::emit_provider_switched;
use crate::store::AppState;
use tauri::{AppHandle, State};

/// Parse a deep link URL and return the parsed request for frontend confirmation
#[tauri::command]
pub fn parse_deeplink(url: String) -> Result<DeepLinkImportRequest, String> {
    log::info!("Parsing deep link URL: {}", crate::url_for_log(&url));
    parse_deeplink_url(&url).map_err(|e| e.to_string())
}

/// Merge configuration from Base64/URL into a deep link request
/// This is used by the frontend to show the complete configuration in the confirmation dialog
#[tauri::command]
pub fn merge_deeplink_config(
    request: DeepLinkImportRequest,
) -> Result<DeepLinkImportRequest, String> {
    log::info!("Merging config for deep link request: {:?}", request.name);
    crate::deeplink::parse_and_merge_config(&request).map_err(|e| e.to_string())
}

/// Import a provider from a deep link request (legacy, kept for compatibility)
#[tauri::command]
pub fn import_from_deeplink(
    app: AppHandle,
    state: State<AppState>,
    request: DeepLinkImportRequest,
) -> Result<String, String> {
    log::info!(
        "Importing provider from deep link: {:?} for app {:?}",
        request.name,
        request.app
    );

    let target_app = request.app.clone();
    let (provider_id, did_switch_current) =
        import_provider_from_deeplink(&state, request).map_err(|e| e.to_string())?;

    // ⚠️ 切了 current 就必须广播 provider-switched：OperatorSection 靠它 reload，
    // 否则托管档位高亮停留在旧项（deeplink 导入改了 current 却没人通知界面）。
    if did_switch_current {
        if let Some(app_str) = target_app {
            let app_type = app_str
                .parse::<crate::AppType>()
                .map_err(|e| e.to_string())?;
            emit_provider_switched(&app, &app_type, &provider_id);
        }
    }

    log::info!("Successfully imported provider with ID: {provider_id}");

    Ok(provider_id)
}

/// Import resource from a deep link request (unified handler)
#[tauri::command]
pub async fn import_from_deeplink_unified(
    app: AppHandle,
    state: State<'_, AppState>,
    request: DeepLinkImportRequest,
) -> Result<serde_json::Value, String> {
    log::info!("Importing {} resource from deep link", request.resource);

    match request.resource.as_str() {
        "provider" => {
            let target_app = request.app.clone();
            let (provider_id, did_switch_current) =
                import_provider_from_deeplink(&state, request).map_err(|e| e.to_string())?;

            // ⚠️ 同上：切了 current 必须广播，OperatorSection 靠它 reload。
            if did_switch_current {
                if let Some(app_str) = target_app {
                    let app_type = app_str
                        .parse::<crate::AppType>()
                        .map_err(|e| e.to_string())?;
                    emit_provider_switched(&app, &app_type, &provider_id);
                }
            }

            Ok(serde_json::json!({
                "type": "provider",
                "id": provider_id
            }))
        }
        "prompt" => {
            let prompt_id =
                import_prompt_from_deeplink(&state, request).map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "type": "prompt",
                "id": prompt_id
            }))
        }
        "mcp" => {
            let result = import_mcp_from_deeplink(&state, request).map_err(|e| e.to_string())?;
            // Add type field to the result
            Ok(serde_json::json!({
                "type": "mcp",
                "importedCount": result.imported_count,
                "importedIds": result.imported_ids,
                "failed": result.failed
            }))
        }
        "skill" => {
            let skill_key =
                import_skill_from_deeplink(&state, request).map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "type": "skill",
                "key": skill_key
            }))
        }
        _ => Err(format!("Unsupported resource type: {}", request.resource)),
    }
}
