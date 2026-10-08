//! Compare the stored row's original projector with bound live input. These are
//! read-only facts, not an effective-model selection or a final application plan.
use crate::live::{floor, project};
use crate::{app_config::AppType, provider::Provider};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn compare(app: &AppType, candidate: Option<&Provider>, live: &Value) -> Option<bool> {
    let candidate = candidate?;
    compare_inner(app, candidate, live).ok().flatten()
}
fn compare_inner(app: &AppType, candidate: &Provider, live: &Value) -> Result<Option<bool>, ()> {
    match app {
        AppType::Claude => {
            for value in [&candidate.settings_config, live] {
                if !value.is_object() || value.get("env").is_some_and(|env| !env.is_object()) {
                    return Err(());
                }
            }
            Ok(Some(
                project::claude::ClaudeProjection::of(&candidate.settings_config)
                    == project::claude::ClaudeProjection::of(live),
            ))
        }
        AppType::Gemini => {
            let expected =
                crate::services::provider::gemini_direct::projection(candidate).map_err(|_| ())?;
            let env = live.get("env").and_then(Value::as_object).ok_or(())?;
            let actual: BTreeMap<_, _> = env
                .iter()
                .filter(|(key, _)| floor::gemini_floor_env(key))
                .map(|(key, value)| Ok((key.as_str(), value.as_str().ok_or(())?)))
                .collect::<Result<_, ()>>()?;
            let expected_env: BTreeMap<_, _> = expected
                .env
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let selected_type = live
                .pointer("/config/security/auth/selectedType")
                .and_then(Value::as_str);
            let model = live
                .pointer("/config/model/name")
                .filter(|value| !value.is_null());
            Ok(Some(
                actual == expected_env
                    && selected_type == expected.selected_type
                    && model == expected.model_name.as_ref(),
            ))
        }
        AppType::GrokBuild => {
            let expected =
                crate::services::provider::grok_direct::projection(candidate).map_err(|_| ())?;
            let actual = live
                .get("config")
                .and_then(Value::as_str)
                .ok_or(())?
                .parse::<toml::Table>()
                .map_err(|_| ())?;
            let models = actual
                .get("models")
                .map(|value| value.as_table().ok_or(()))
                .transpose()?;
            let selected = models
                .and_then(|table| table.get("default"))
                .map(|value| value.as_str().ok_or(()))
                .transpose()?;
            match expected.table {
                Some((name, table)) => {
                    let expected = table.to_string().parse::<toml::Table>().map_err(|_| ())?;
                    let current = actual
                        .get("model")
                        .and_then(toml::Value::as_table)
                        .and_then(|models| models.get(&name))
                        .and_then(toml::Value::as_table);
                    Ok(Some(
                        selected == Some(name.as_str()) && current == Some(&expected),
                    ))
                }
                None => Ok(Some(selected.is_none())),
            }
        }
        AppType::Codex => codex_declared_fields(candidate, live),
        _ => Err(()),
    }
}

fn value_at<'a>(
    doc: &'a toml_edit::DocumentMut,
    path: &[&str],
) -> Result<Option<&'a toml_edit::Value>, ()> {
    let mut table: &dyn toml_edit::TableLike = doc.as_table();
    for (index, key) in path.iter().enumerate() {
        let Some(item) = table.get(key) else {
            return Ok(None);
        };
        if index == path.len() - 1 {
            return item.as_value().map(Some).ok_or(());
        }
        table = item.as_table_like().ok_or(())?;
    }
    Err(())
}
// Review reports semantic field differences. Keep the writer's textual
// same_value/remove-if rules unchanged, including its layout decisions.
fn semantic(value: &toml_edit::Value) -> Result<toml::Value, ()> {
    let text = format!("value = {}", crate::live::patch::toml::value_text(value));
    text.parse::<toml::Table>()
        .map_err(|_| ())?
        .remove("value")
        .ok_or(())
}
fn equal(
    expected: Option<&toml_edit::Value>,
    actual: Option<&toml_edit::Value>,
) -> Result<bool, ()> {
    match (expected, actual) {
        (None, None) => Ok(true),
        (Some(expected), Some(actual)) => Ok(semantic(expected)? == semantic(actual)?),
        _ => Ok(false),
    }
}

fn codex_declared_fields(candidate: &Provider, live: &Value) -> Result<Option<bool>, ()> {
    let projection = crate::services::provider::codex_direct::project(candidate).map_err(|_| ())?;
    let doc = live
        .get("config")
        .and_then(Value::as_str)
        .ok_or(())?
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| ())?;
    let exclusive = crate::services::provider::codex_direct::exclusive_of(candidate, &projection);
    for key in floor::CODEX_FLOOR_TOP
        .iter()
        .chain(floor::CODEX_EXCLUSIVE_TOP)
    {
        // Route/auth policy and external catalog preservation require the later
        // explicit application plan. Do not pretend their default means a match.
        if ["model_provider", "openai_base_url", "model_catalog_json"].contains(key) {
            continue;
        }
        let expected = projection
            .top
            .iter()
            .chain(&exclusive)
            .find(|(name, _)| name == key)
            .map(|(_, value)| value);
        if !equal(expected, value_at(&doc, &[*key])?)? {
            return Ok(Some(false));
        }
    }
    for path in floor::CODEX_FLOOR_NESTED {
        let expected = projection
            .nested
            .iter()
            .find(|(name, _)| name.iter().map(String::as_str).eq(path.iter().copied()))
            .map(|(_, value)| value);
        if !equal(expected, value_at(&doc, path)?)? {
            return Ok(Some(false));
        }
    }
    // No known declared-field conflict is not a complete route/auth/catalog
    // proof. Null remains unresolved, never a successful full Codex plan.
    Ok(None)
}
