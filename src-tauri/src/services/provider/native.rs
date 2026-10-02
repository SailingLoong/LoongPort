//! Accept native configuration at the boundary where the user enables routing.
//! A native snapshot is a complete configuration, never a credential update to
//! whichever database provider happened to be selected previously.
use crate::{app_config::AppType, database::Database, error::AppError, provider::Provider};
use serde_json::{json, Value};

/// A conservative receipt for the existing native owner. Only file metadata and
/// selected provider identity are hashed: no credentials leave the backend and
/// reading a receipt never imports, repairs, or writes configuration.
fn native_file_revision(
    paths: &[std::path::PathBuf],
    current: Option<&str>,
) -> Result<String, AppError> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    current.hash(&mut hash);
    for path in paths {
        path.hash(&mut hash);
        match std::fs::metadata(path) {
            Ok(metadata) => {
                true.hash(&mut hash);
                metadata.len().hash(&mut hash);
                metadata
                    .modified()
                    .map_err(|error| AppError::io(path, error))?
                    .hash(&mut hash);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    metadata.ino().hash(&mut hash);
                    metadata.ctime().hash(&mut hash);
                    metadata.ctime_nsec().hash(&mut hash);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Windows also reports a missing path when an ancestor is a
                // regular file. Such a path is invalid, not an empty config.
                for ancestor in path.ancestors().skip(1) {
                    match std::fs::metadata(ancestor) {
                        Ok(metadata) if metadata.is_dir() => break,
                        Ok(_) => return Err(AppError::io(path, error)),
                        Err(parent_error)
                            if parent_error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(parent_error) => return Err(AppError::io(ancestor, parent_error)),
                    }
                }
                false.hash(&mut hash);
            }
            Err(error) => return Err(AppError::io(path, error)),
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}

pub(crate) fn service_configuration_revision(
    state: &crate::store::AppState,
    app: &AppType,
) -> Result<String, AppError> {
    let paths = match app {
        AppType::Claude => vec![crate::config::get_claude_settings_path()],
        AppType::Codex => vec![
            crate::codex_config::get_codex_config_path(),
            crate::codex_config::get_codex_auth_path(),
        ],
        AppType::Gemini => vec![
            crate::gemini_config::get_gemini_env_path(),
            crate::gemini_config::get_gemini_settings_path(),
        ],
        AppType::GrokBuild => vec![crate::grok_config::get_grok_config_path()],
        AppType::OpenCode => vec![crate::opencode_config::get_opencode_config_path()],
        AppType::OpenClaw => vec![crate::openclaw_config::get_openclaw_config_path()],
        AppType::Hermes => vec![crate::hermes_config::get_hermes_config_path()],
        AppType::Pi => vec![
            crate::pi_config::get_pi_models_path()?,
            crate::pi_config::get_pi_settings_path()?,
        ],
        AppType::ClaudeDesktop => crate::claude_desktop_config::configuration_revision_paths()?,
        // Image generation has no native file; its owner is the selected DB row.
        AppType::CodexImage => vec![],
    };
    let current = crate::settings::get_effective_current_provider_readonly(&state.db, app)?;
    let native = native_file_revision(&paths, current.as_deref())?;
    let catalog = state.db.configuration_catalog_revision(app.as_str())?;
    Ok(format!("{native}:{catalog}"))
}

fn comparable_settings(settings: &Value) -> Result<Value, AppError> {
    let mut value = settings.clone();
    if let Some(config) = settings.get("config").and_then(Value::as_str) {
        let parsed: toml::Value = toml::from_str(config)
            .map_err(|error| AppError::Config(format!("Invalid Codex configuration: {error}")))?;
        value["config"] =
            serde_json::to_value(parsed).map_err(|error| AppError::Config(error.to_string()))?;
    }
    Ok(value)
}

fn comparable_managed_config(config: &str) -> Result<toml::Value, AppError> {
    let config = crate::codex_config::strip_codex_unified_session_bucket(config)?;
    let mut parsed: toml::Value = toml::from_str(&config)
        .map_err(|error| AppError::Config(format!("Invalid Codex configuration: {error}")))?;
    let generated_catalog = parsed
        .get("model_catalog_json")
        .and_then(toml::Value::as_str)
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .is_some_and(crate::codex_config::is_our_model_catalog_filename);
    if generated_catalog {
        if let Some(table) = parsed.as_table_mut() {
            table.remove("model_catalog_json");
        }
    }
    Ok(parsed)
}

impl super::ProviderService {
    /// Caller holds the Codex switch lock and has verified native files are not
    /// proxy placeholders. Only complete equivalent configurations are reused.
    pub(crate) fn adopt_codex_native_configuration(
        db: &Database,
        native: Value,
    ) -> Result<Provider, AppError> {
        if let Some(current_id) = db.get_current_provider("codex")? {
            if let Some(current) = db.get_provider_by_id(&current_id, "codex")? {
                if let Some(account_id) = current
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.managed_account_id_for("codex_oauth"))
                {
                    let owned_auth = native.get("auth").is_some_and(|auth| {
                        crate::codex_config::codex_auth_matches_recorded_managed_oauth(
                            auth,
                            &account_id,
                        )
                        .unwrap_or(false)
                    });
                    let effective = super::live::build_effective_settings_with_common_config(
                        db,
                        &AppType::Codex,
                        &current,
                    )?;
                    let same_config = native
                        .get("config")
                        .and_then(Value::as_str)
                        .zip(effective.get("config").and_then(Value::as_str))
                        .is_some_and(|(native, expected)| {
                            comparable_managed_config(native).ok()
                                == comparable_managed_config(expected).ok()
                        });
                    if owned_auth && same_config {
                        return Ok(current);
                    }
                }
            }
        }
        let mut candidate = Provider::with_id(
            uuid::Uuid::new_v4().to_string(),
            "Codex native configuration".into(),
            native,
            None,
        );
        let config = candidate
            .settings_config
            .get("config")
            .and_then(Value::as_str);
        let key = crate::codex_config::extract_codex_api_key(
            candidate.settings_config.get("auth"),
            config,
        );
        let official = key.is_none()
            && candidate
                .settings_config
                .get("auth")
                .is_some_and(crate::codex_config::codex_auth_has_login_material);
        candidate.category = Some(if official { "official" } else { "custom" }.into());
        // ChatGPT auth remains native-owned; a third-party snapshot stores only
        // its own effective API credential, never an unrelated native login.
        candidate.settings_config["auth"] = key
            .map(|key| json!({"OPENAI_API_KEY":key}))
            .unwrap_or_else(|| json!({}));
        candidate.meta = Some(crate::provider::ProviderMeta {
            common_config_enabled: Some(false),
            ..Default::default()
        });
        Self::validate_provider_settings(&AppType::Codex, &candidate)?;
        Self::validate_proxy_takeover_target(&AppType::Codex, &candidate)?;
        let snapshot = comparable_settings(&candidate.settings_config)?;
        let profile = crate::proxy::providers::resolve_codex_catalog_tool_profile(&candidate);
        let mut selected = None;
        for existing in db.get_all_providers("codex")?.into_values() {
            if crate::relay::is_managed(&existing.id) || existing.uses_managed_account_auth() {
                continue;
            }
            let Ok(effective) = super::live::build_effective_settings_with_common_config(
                db,
                &AppType::Codex,
                &existing,
            ) else {
                continue;
            };
            if crate::proxy::providers::resolve_codex_catalog_tool_profile(&existing) == profile
                && comparable_settings(&effective).ok().as_ref() == Some(&snapshot)
            {
                selected = Some(existing);
                break;
            }
        }
        let selected = selected.unwrap_or(candidate);
        super::validate_provider_selection(db, &AppType::Codex, &selected.id)?;
        if db.get_provider_by_id(&selected.id, "codex")?.is_none() {
            db.save_provider("codex", &selected)?;
            crate::proxy::application_routing::note_provider_created(db, "codex", &selected.id)?;
        }
        db.set_current_provider("codex", &selected.id)?;
        crate::settings::set_current_provider(&AppType::Codex, Some(&selected.id))?;
        crate::proxy::auto_strategy::set_model_pref(db, "codex", None)?;
        Ok(selected)
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    #[test]
    fn revisions_detect_native_file_and_selection_changes_without_reading_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.json");
        let paths = vec![path.clone()];
        let missing = native_file_revision(&paths, Some("a")).unwrap();
        std::fs::write(&path, b"synthetic configuration").unwrap();
        let first = native_file_revision(&paths, Some("a")).unwrap();
        assert_ne!(missing, first);
        assert_eq!(first, native_file_revision(&paths, Some("a")).unwrap());
        assert_ne!(first, native_file_revision(&paths, Some("b")).unwrap());
        std::fs::write(&path, b"changed synthetic configuration").unwrap();
        assert_ne!(first, native_file_revision(&paths, Some("a")).unwrap());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(missing, native_file_revision(&paths, Some("a")).unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn database_owned_image_receipt_changes_on_configuration_edits_without_writes() {
        let state =
            crate::store::AppState::new(std::sync::Arc::new(Database::memory().unwrap())).unwrap();
        let mut provider = Provider::with_id(
            "fixture".into(),
            "Synthetic image configuration".into(),
            json!({"apiKey":"synthetic-key","model":"synthetic-model"}),
            None,
        );
        state.db.save_provider("codex-image", &provider).unwrap();
        let before_changes = state.db.conn.lock().unwrap().total_changes();
        let first = service_configuration_revision(&state, &AppType::CodexImage).unwrap();
        assert_eq!(
            first,
            service_configuration_revision(&state, &AppType::CodexImage).unwrap()
        );
        assert_eq!(
            before_changes,
            state.db.conn.lock().unwrap().total_changes()
        );
        assert!(!first.contains("synthetic-key"));
        provider.settings_config["model"] = json!("changed-model");
        state.db.save_provider("codex-image", &provider).unwrap();
        assert_ne!(
            first,
            service_configuration_revision(&state, &AppType::CodexImage).unwrap()
        );
    }

    #[test]
    fn missing_native_directories_are_valid_empty_state_and_are_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let paths = [dir
            .path()
            .join("missing")
            .join("nested")
            .join("native.json")];
        assert!(native_file_revision(&paths, None).is_ok());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn inaccessible_revision_is_not_an_empty_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parent");
        std::fs::write(&path, b"not a directory").unwrap();
        assert!(native_file_revision(&[path.join("native.json")], None).is_err());
    }
}
