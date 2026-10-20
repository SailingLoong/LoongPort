//! Account-backed route rows contain model intent and an opaque identity only.
//! Credentials, endpoints and account admission remain in the original adapters.
use crate::{
    app_config::AppType,
    error::AppError,
    provider::{AuthBindingSource, Provider},
};

pub(crate) fn is_managed_inference(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|m| m.auth_binding.as_ref())
        .is_some_and(|b| {
            matches!(b.source, AuthBindingSource::ManagedAccount)
                && matches!(
                    b.auth_provider.as_deref(),
                    Some("workbuddy" | "minimax_code" | "zcode_start_plan" | "trae")
                )
        })
}

pub(crate) fn reject_direct(provider: &Provider) -> Result<(), AppError> {
    if is_managed_inference(provider) {
        return Err(AppError::localized(
            "managedAccount.routingRequired",
            "此账号只能通过本地路由使用，请先在应用页启用本地路由，再选择账号档位。",
            "This account requires local routing. Enable local routing in the application page before selecting it.",
        ));
    }
    Ok(())
}

fn invalid() -> AppError {
    AppError::Config("managedAccount.invalidProjection".into())
}
fn model(value: &str, trae: bool) -> bool {
    if trae {
        // Match the discovered model DTO's safe text contract without rewriting
        // the account's exact model ID. Other providers retain their old gate.
        return !value.trim().is_empty()
            && value.len() <= 256
            && !value.chars().any(char::is_control);
    }
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/@+-".contains(&b))
}

/// Creation accepts only this entry's fixed projection. Original source-bound
/// model updates continue through `validate` and their existing model owner.
pub(crate) fn validate_new_entry(app: &AppType, provider: &Provider) -> Result<(), AppError> {
    validate(app, provider)?;
    let value =
        serde_json::to_value(provider.meta.as_ref().ok_or_else(invalid)?).map_err(|_| invalid())?;
    if value.as_object().ok_or_else(invalid)?.keys().any(|key| {
        !matches!(
            key.as_str(),
            "authBinding" | "apiFormat" | "commonConfigEnabled"
        )
    }) {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn validate(app: &AppType, provider: &Provider) -> Result<(), AppError> {
    let meta = provider.meta.as_ref().ok_or_else(invalid)?;
    let binding = meta.auth_binding.as_ref().ok_or_else(invalid)?;
    let trae = binding.auth_provider.as_deref() == Some("trae");
    let api_format = if matches!(binding.auth_provider.as_deref(), Some("workbuddy" | "trae")) {
        "openai_chat"
    } else {
        "anthropic"
    };
    if !is_managed_inference(provider)
        || binding
            .account_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
        || provider.category.as_deref() != Some("custom")
        || meta.api_format.as_deref() != Some(api_format)
        || meta.common_config_enabled != Some(false)
        || meta.provider_type.is_some()
        || !meta.custom_endpoints.is_empty()
        || meta.usage_script.is_some()
        || meta.local_proxy_request_overrides.is_some()
    {
        return Err(invalid());
    }
    let settings = provider.settings_config.as_object().ok_or_else(invalid)?;
    let valid = match app {
        AppType::Claude => {
            settings.len() == 1
                && settings
                    .get("env")
                    .and_then(|v| v.as_object())
                    .is_some_and(|env| {
                        env.get("ANTHROPIC_MODEL")
                            .and_then(|v| v.as_str())
                            .is_some_and(|value| model(value, trae))
                            && env.iter().all(|(key, value)| {
                                matches!(
                                    key.as_str(),
                                    "ANTHROPIC_MODEL"
                                        | "ANTHROPIC_DEFAULT_SONNET_MODEL"
                                        | "ANTHROPIC_DEFAULT_OPUS_MODEL"
                                        | "ANTHROPIC_DEFAULT_HAIKU_MODEL"
                                ) && value.as_str().is_some_and(|value| model(value, trae))
                            })
                    })
        }
        AppType::Codex => {
            settings.len() == 2
                && settings
                    .get("auth")
                    .and_then(|v| v.as_object())
                    .is_some_and(|auth| auth.is_empty())
                && settings
                    .get("config")
                    .and_then(|v| v.as_str())
                    .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
                    .is_some_and(|doc| {
                        doc.as_table().len() == 1
                            && doc
                                .get("model")
                                .and_then(|v| v.as_str())
                                .is_some_and(|value| model(value, trae))
                    })
        }
        _ => false,
    };
    if !valid {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn edit_error() -> AppError {
    AppError::localized("managedAccount.accountEditorRequired", "此账号档位不支持 API Key 编辑；请在认证中心重新添加所需的账号和模型。", "Use the authentication center to add the desired account and model; this route has no editable API key.")
}
