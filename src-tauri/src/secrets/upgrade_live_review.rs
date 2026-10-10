//! Read only the original review owner's bound bytes. Parse errors are per-app
//! facts and never become a negative takeover result or a guessed Direct mode.
use crate::live::patch::LivePatch;
use crate::{app_config::AppType, error::AppError};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) type BoundFiles = BTreeMap<PathBuf, Option<zeroize::Zeroizing<Vec<u8>>>>;

pub(super) struct LiveFacts {
    pub(super) status: &'static str,
    pub(super) marker: Option<bool>,
    pub(super) stored_fields_match: Option<bool>,
    pub(super) native_completion_match: Option<bool>,
    pub(super) catalog_ownership: Option<&'static str>,
    pub(super) catalog_file_present: Option<bool>,
}

pub(super) fn bytes<'a>(files: &'a BoundFiles, path: &Path) -> Result<Option<&'a [u8]>, AppError> {
    files
        .get(path)
        .map(|bytes| bytes.as_ref().map(|bytes| bytes.as_slice()))
        .ok_or_else(|| AppError::Config("upgrade.source_changed".into()))
}
fn object(bytes: Option<&[u8]>) -> Result<Value, ()> {
    let value = bytes
        .map(serde_json::from_slice::<Value>)
        .transpose()
        .map_err(|_| ())?
        .unwrap_or_else(|| json!({}));
    if value.is_null() {
        return Ok(json!({}));
    }
    value.is_object().then_some(value).ok_or(())
}
fn toml(bytes: Option<&[u8]>) -> Result<String, ()> {
    let text = std::str::from_utf8(bytes.unwrap_or_default()).map_err(|_| ())?;
    text.parse::<toml_edit::DocumentMut>().map_err(|_| ())?;
    Ok(text.to_owned())
}

pub(super) fn inspect(
    app: &AppType,
    files: &BoundFiles,
    candidate: Option<&crate::provider::Provider>,
    managed_catalog_present: Option<bool>,
) -> Result<LiveFacts, AppError> {
    inspect_with_grok_retired(app, files, candidate, managed_catalog_present, None)
}

pub(super) fn inspect_with_grok_retired(
    app: &AppType,
    files: &BoundFiles,
    candidate: Option<&crate::provider::Provider>,
    managed_catalog_present: Option<bool>,
    grok_retired: Option<&[String]>,
) -> Result<LiveFacts, AppError> {
    let (present, parsed) = match app {
        AppType::Claude => {
            let data = bytes(files, &crate::config::get_claude_settings_path())?;
            (data.is_some(), object(data).map(|value| (value, true)))
        }
        AppType::Codex => {
            let auth = bytes(files, &crate::codex_config::get_codex_auth_path())?;
            let config = bytes(files, &crate::codex_config::get_codex_config_path())?;
            (
                auth.is_some() || config.is_some(),
                object(auth).and_then(|auth| {
                    toml(config).map(|config| (json!({"auth":auth,"config":config}), true))
                }),
            )
        }
        AppType::Gemini => {
            let path = crate::gemini_config::get_gemini_env_path();
            let env = bytes(files, &path)?;
            let config = bytes(files, &crate::gemini_config::get_gemini_settings_path())?;
            let parsed = (|| {
                let config = object(config)?;
                crate::live::patch::dotenv::DotenvPatch::default()
                    .apply(&path, env)
                    .map_err(|_| ())?;
                let text = std::str::from_utf8(env.unwrap_or_default()).map_err(|_| ())?;
                let entries = crate::live::patch::dotenv::literal_owned_entries(
                    text,
                    crate::live::floor::gemini_floor_env,
                );
                let known = entries
                    .as_ref()
                    .is_some_and(|entries| entries.iter().all(|(_, value)| value.is_some()));
                let env: serde_json::Map<String, Value> = entries
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(key, value)| value.map(|value| (key, Value::String(value))))
                    .collect();
                Ok((json!({"env":env,"config":config}), known))
            })();
            (env.is_some() || config.is_some(), parsed)
        }
        AppType::GrokBuild => {
            let config = bytes(files, &crate::grok_config::get_grok_config_path())?;
            (
                config.is_some(),
                toml(config).map(|config| (json!({"config":config}), true)),
            )
        }
        _ => return Err(AppError::Config("upgrade.unsupported_source".into())),
    };
    if !present {
        return Ok(LiveFacts {
            status: "missing",
            marker: None,
            stored_fields_match: None,
            native_completion_match: None,
            catalog_ownership: None,
            catalog_file_present: None,
        });
    }
    Ok(match parsed {
        Ok((value, owned_values_known)) => {
            let catalog = if *app == AppType::Codex {
                catalog_facts(&value, candidate, managed_catalog_present)
            } else {
                Ok((None, None))
            };
            match catalog {
                Ok((catalog_ownership, catalog_file_present)) => LiveFacts {
                    status: "parsed",
                    stored_fields_match: owned_values_known
                        .then(|| super::projection_review::compare(app, candidate, &value))
                        .flatten(),
                    native_completion_match: owned_values_known
                        .then(|| {
                            if *app == AppType::GrokBuild {
                                return super::projection_review::grok_native_completion_match(
                                    candidate,
                                    &value,
                                    grok_retired?,
                                );
                            }
                            super::projection_review::native_completion_match(
                                app, candidate, &value,
                            )
                        })
                        .flatten(),
                    marker: {
                        let observed =
                            crate::services::ProxyService::live_has_proxy_placeholder_for_app(
                                app, &value,
                            );
                        if observed || owned_values_known {
                            Some(observed)
                        } else {
                            None
                        }
                    },
                    catalog_ownership,
                    catalog_file_present,
                },
                Err(()) => LiveFacts {
                    status: "invalid",
                    marker: None,
                    stored_fields_match: None,
                    native_completion_match: None,
                    catalog_ownership: None,
                    catalog_file_present: None,
                },
            }
        }
        Err(()) => LiveFacts {
            status: "invalid",
            marker: None,
            stored_fields_match: None,
            native_completion_match: None,
            catalog_ownership: None,
            catalog_file_present: None,
        },
    })
}

/// The capture owner supplies path containment evidence. Basenames alone never
/// establish ownership, and this pure comparison never opens external files.
fn catalog_facts(
    config: &Value,
    candidate: Option<&crate::provider::Provider>,
    managed_catalog_present: Option<bool>,
) -> Result<(Option<&'static str>, Option<bool>), ()> {
    let document = config
        .get("config")
        .and_then(Value::as_str)
        .ok_or(())?
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| ())?;
    let Some(pointer) = document.get("model_catalog_json") else {
        return Ok((None, None));
    };
    let pointer = pointer
        .as_str()
        .filter(|pointer| !pointer.trim().is_empty())
        .ok_or(())?;
    if let Some(present) = managed_catalog_present {
        return Ok((Some("managed"), Some(present)));
    }
    let claimed = candidate
        .and_then(|provider| provider.settings_config.get("config"))
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .is_some_and(|doc| {
            doc.get("model_catalog_json")
                .and_then(toml_edit::Item::as_str)
                == Some(pointer)
        });
    Ok((
        Some(if claimed {
            "candidate_external"
        } else {
            "unclaimed_external"
        }),
        None,
    ))
}
