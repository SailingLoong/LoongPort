//! Managed authentication shared by the CLI configuration lifecycle owners.
use crate::{database::Database, error::AppError};
use serde_json::Value;
use toml_edit::{Item, Table};

pub(crate) use crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER as LEGACY_KEY;
pub(crate) const CLI_APPS: &[&str] = &["claude", "codex", "gemini", "grokbuild"];

pub(crate) fn provision(db: &Database) -> Result<(), AppError> {
    for app in CLI_APPS {
        db.provision_managed_client_credential(app)?;
    }
    Ok(())
}

pub(crate) fn key(db: &Database, app: &str) -> Result<String, AppError> {
    db.managed_client_key(app)?
        .ok_or_else(|| AppError::Config("client_access.not_initialized".into()))
}

/// Recognition is not authentication. This also recognizes pre-migration backups.
pub(crate) fn is_projection_key(value: &str) -> bool {
    value == LEGACY_KEY || value.starts_with("lp_client_builtin:managed-")
}

pub(crate) fn owned_key(db: &Database, app: &str, value: &str) -> Result<bool, AppError> {
    Ok(value == LEGACY_KEY || db.owns_managed_client_key(app, value)?)
}

pub(crate) fn codex_table(table: &mut Table, key: &str, native: bool) {
    table.remove("experimental_bearer_token");
    if native {
        let mut headers = Table::new();
        headers.insert(super::CLIENT_KEY_HEADER, toml_edit::value(key));
        table.insert("http_headers", Item::Table(headers));
        table.insert("requires_openai_auth", toml_edit::value(true));
    } else {
        table.insert("experimental_bearer_token", toml_edit::value(key));
    }
}

/// Legacy writer adapter: replace only the owned local authentication fields.
pub(crate) fn project_json(db: &Database, app: &str, config: &Value) -> Result<Value, AppError> {
    let mut out = config.clone();
    if let Some(env) = out.get_mut("env").and_then(Value::as_object_mut) {
        let fields: &[&str] = match app {
            "claude" => &[
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "OPENROUTER_API_KEY",
                "OPENAI_API_KEY",
            ],
            "gemini" => &["GEMINI_API_KEY"],
            _ => &[],
        };
        for field in fields {
            if let Some(value) = env.get_mut(*field) {
                if value.as_str() == Some(LEGACY_KEY) {
                    db.provision_managed_client_credential(app)?;
                    *value = Value::String(key(db, app)?);
                }
            }
        }
    }
    if app == "grokbuild" {
        if let Some(text) = out.get("config").and_then(Value::as_str) {
            let mut doc = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| AppError::Config(e.to_string()))?;
            if let Some(models) = doc.get_mut("model").and_then(Item::as_table_like_mut) {
                for (_, item) in models.iter_mut() {
                    if let Some(table) = item.as_table_like_mut() {
                        if table.get("api_key").and_then(Item::as_str) == Some(LEGACY_KEY) {
                            db.provision_managed_client_credential(app)?;
                            table.insert("api_key", toml_edit::value(key(db, app)?));
                        }
                    }
                }
            }
            out["config"] = Value::String(doc.to_string());
        }
    }
    Ok(out)
}

/// Native Codex auth uses a separate local header on an owned local provider.
pub(crate) fn codex_has_local_credential(text: &str) -> bool {
    text.parse::<toml_edit::DocumentMut>()
        .ok()
        .is_some_and(|doc| {
            doc.get("model_providers")
                .and_then(Item::as_table_like)
                .is_some_and(|providers| {
                    providers.iter().any(|(_, item)| {
                        item.get("http_headers")
                            .and_then(|h| h.get(super::CLIENT_KEY_HEADER))
                            .and_then(Item::as_str)
                            .is_some_and(is_projection_key)
                    })
                })
        })
}

pub(crate) fn clean_codex(db: &Database, text: &str) -> Result<String, AppError> {
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| AppError::Config(e.to_string()))?;
    let mut removed = Vec::new();
    if let Some(providers) = doc
        .get_mut("model_providers")
        .and_then(Item::as_table_like_mut)
    {
        for (id, item) in providers.iter() {
            let bearer = item.get("experimental_bearer_token").and_then(Item::as_str);
            let header = item
                .get("http_headers")
                .and_then(|h| h.get(super::CLIENT_KEY_HEADER))
                .and_then(Item::as_str);
            if bearer
                .or(header)
                .map(|value| owned_key(db, "codex", value))
                .transpose()?
                .unwrap_or(false)
            {
                removed.push(id.to_string());
            }
        }
        for id in &removed {
            providers.remove(id);
        }
    }
    if doc
        .get("model_provider")
        .and_then(Item::as_str)
        .is_some_and(|id| removed.iter().any(|removed| removed == id))
    {
        doc.as_table_mut().remove("model_provider");
    }
    Ok(doc.to_string())
}

pub(crate) fn is_grok_projection(text: &str) -> bool {
    text.parse::<toml_edit::DocumentMut>()
        .ok()
        .is_some_and(|doc| {
            doc.get("model")
                .and_then(Item::as_table_like)
                .is_some_and(|models| {
                    models.iter().any(|(_, item)| {
                        item.get("api_key")
                            .and_then(Item::as_str)
                            .is_some_and(is_projection_key)
                    })
                })
        })
}

pub(crate) fn verify_json(db: &Database, app: &str, config: &Value) -> Result<(), AppError> {
    let expected = key(db, app)?;
    let matches = match app {
        "claude" => config
            .get("env")
            .and_then(Value::as_object)
            .is_some_and(|env| {
                [
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_API_KEY",
                    "OPENROUTER_API_KEY",
                    "OPENAI_API_KEY",
                ]
                .iter()
                .any(|name| env.get(*name).and_then(Value::as_str) == Some(expected.as_str()))
            }),
        "gemini" => {
            config
                .pointer("/env/GEMINI_API_KEY")
                .and_then(Value::as_str)
                == Some(expected.as_str())
        }
        "grokbuild" => {
            config
                .get("config")
                .and_then(Value::as_str)
                .and_then(crate::grok_config::extract_model_config)
                .and_then(|m| m.api_key)
                .as_deref()
                == Some(expected.as_str())
        }
        "codex" => config
            .get("config")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
            .is_some_and(|doc| {
                doc.get("model_provider")
                    .and_then(Item::as_str)
                    .and_then(|id| doc.get("model_providers").and_then(|p| p.get(id)))
                    .is_some_and(|table| {
                        table
                            .get("experimental_bearer_token")
                            .and_then(Item::as_str)
                            == Some(expected.as_str())
                            || table
                                .get("http_headers")
                                .and_then(|h| h.get(super::CLIENT_KEY_HEADER))
                                .and_then(Item::as_str)
                                == Some(expected.as_str())
                    })
            }),
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(AppError::Config("client_access.projection_mismatch".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cli_projection_is_scoped_and_keeps_foreign_configuration() {
        let db = Database::memory().unwrap();
        provision(&db).unwrap();
        let claude = key(&db, "claude").unwrap();
        let gemini = key(&db, "gemini").unwrap();
        assert_ne!(claude, gemini);
        let original =
            json!({"env":{"ANTHROPIC_AUTH_TOKEN":LEGACY_KEY,"UNOWNED":"keep"},"unowned":true});
        let projected = project_json(&db, "claude", &original).unwrap();
        assert_eq!(projected["env"]["ANTHROPIC_AUTH_TOKEN"], claude);
        assert_eq!(projected["env"]["UNOWNED"], "keep");
        assert_eq!(projected["unowned"], true);

        assert!(db
            .authenticate_client(&claude, "claude", Some("model-a"))
            .is_ok());
        assert!(db
            .authenticate_client(&claude, "gemini", Some("model-a"))
            .is_err());
        assert!(db.authenticate_client(LEGACY_KEY, "claude", None).is_err());
        db.revoke_client_credential("builtin:managed-claude:claude")
            .unwrap();
        assert!(owned_key(&db, "claude", &claude).unwrap());
        assert!(!owned_key(&db, "gemini", &claude).unwrap());
        assert!(db.authenticate_client(&claude, "claude", None).is_err());
    }

    #[test]
    fn native_codex_uses_independent_header_and_cleanup_preserves_foreign_table() {
        let db = Database::memory().unwrap();
        provision(&db).unwrap();
        let key = key(&db, "codex").unwrap();
        let mut table = crate::live::project::codex::official_mirror_table(
            Some("http://127.0.0.1:15721/v1"),
            false,
        );
        codex_table(&mut table, &key, true);
        assert!(table.get("experimental_bearer_token").is_none());
        assert_eq!(table["requires_openai_auth"].as_bool(), Some(true));
        assert_eq!(
            table["http_headers"][super::super::CLIENT_KEY_HEADER].as_str(),
            Some(key.as_str())
        );
        let mut doc = "model_provider = 'custom'\n[model_providers.foreign]\nbase_url = 'https://relay.example/v1'\nexperimental_bearer_token = 'foreign-key'\n".parse::<toml_edit::DocumentMut>().unwrap();
        doc["model_providers"]["custom"] = Item::Table(table);
        assert!(codex_has_local_credential(&doc.to_string()));
        let cleaned = clean_codex(&db, &doc.to_string()).unwrap();
        let cleaned = cleaned.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(cleaned.get("model_provider").is_none());
        assert!(cleaned["model_providers"].get("custom").is_none());
        assert_eq!(
            cleaned["model_providers"]["foreign"]["experimental_bearer_token"].as_str(),
            Some("foreign-key")
        );
    }

    #[test]
    fn desktop_adoption_retains_installed_token_and_has_one_authority() {
        let db = Database::memory().unwrap();
        let old = "ccs-0123456789abcdef0123456789abcdef";
        db.set_setting("claude_desktop_gateway_token", old).unwrap();
        crate::claude_desktop_config::migrate_gateway_credential(&db).unwrap();
        assert!(db
            .get_setting("claude_desktop_gateway_token")
            .unwrap()
            .is_none());
        assert_eq!(
            db.builtin_client_key("claude-desktop", "claude-desktop")
                .unwrap(),
            old
        );
        assert!(db
            .authenticate_client(old, "claude-desktop", Some("model-a"))
            .is_ok());
        crate::claude_desktop_config::migrate_gateway_credential(&db).unwrap();
        assert_eq!(db.list_client_credentials().unwrap().len(), 1);
        db.revoke_client_credential("builtin:claude-desktop:claude-desktop")
            .unwrap();
        assert!(crate::claude_desktop_config::migrate_gateway_credential(&db).is_err());
        assert!(db.authenticate_client(old, "claude-desktop", None).is_err());
    }
}
