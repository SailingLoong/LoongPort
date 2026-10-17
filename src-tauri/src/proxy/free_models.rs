//! Request-scoped free-model policy. The forwarder owns its sticky intent.
use super::error::ProxyError;
use crate::services::model_fetch::{
    is_opencode_free_source, is_openrouter_free_model, is_openrouter_source, is_reserved_free_model,
};
use serde_json::Value;

pub(super) fn configured(settings: &Value) -> bool {
    settings.get("freeOnly").and_then(Value::as_bool) == Some(true)
}

pub(super) fn enforce(
    free_only: &mut bool,
    requested: &Value,
    outgoing: &Value,
    base_url: &str,
    configured: bool,
) -> Result<(), ProxyError> {
    let model = |body: &Value| body.get("model").and_then(Value::as_str).map(str::to_owned);
    *free_only |= configured
        || model(requested)
            .as_deref()
            .is_some_and(is_reserved_free_model)
        || model(outgoing)
            .as_deref()
            .is_some_and(is_reserved_free_model);
    if !*free_only {
        return Ok(());
    }
    let route_free = |model: &str| {
        (is_openrouter_source(base_url) && is_openrouter_free_model(model))
            || (is_opencode_free_source(base_url)
                && crate::vendor::opencode::FREE_CHAT_MODELS.contains(&model))
    };
    let fallbacks_free = outgoing.get("models").is_none_or(|models| {
        models.as_array().is_some_and(|models| {
            models
                .iter()
                .all(|model| model.as_str().is_some_and(route_free))
        })
    });
    // Optional server-side plugins can have separate fees. Unknown pricing is
    // excluded from a guaranteed-free request instead of silently spending.
    let no_plugins = outgoing
        .get("plugins")
        .is_none_or(|plugins| plugins.as_array().is_some_and(Vec::is_empty));
    if !model(outgoing).as_deref().is_some_and(route_free) || !fallbacks_free || !no_plugins {
        return Err(ProxyError::InvalidRequest(
            "Free-only route unavailable".into(),
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn opencode_free_shared_route_accepts_only_official_documented_free_variants() {
        let free = json!({"model":"glm-5.3-flash-free"});
        enforce(
            &mut false,
            &free,
            &free,
            "https://opencode.ai/inference/openai/v1",
            true,
        )
        .unwrap();
        for (base, outgoing) in [
            (
                "https://opencode.ai/inference/openai/v1",
                json!({"model":"glm-5.3-flash"}),
            ),
            ("https://opencode.ai/zen/go/v1", free.clone()),
            (
                "https://opencode.ai.evil.invalid/inference/openai/v1",
                free.clone(),
            ),
            ("http://opencode.ai/inference/openai/v1", free.clone()),
            (
                "https://opencode.ai/inference/openai/v1",
                json!({"model":"unknown-free"}),
            ),
            (
                "https://opencode.ai/inference/openai/v1",
                json!({"model":"glm-5.3-flash-free","models":["glm-5.3-flash"]}),
            ),
        ] {
            assert!(enforce(&mut false, &free, &outgoing, base, true).is_err());
        }
    }

    #[test]
    fn free_intent_survives_provider_failure_and_rejects_a_paid_fallback() {
        let mut intent = false;
        enforce(
            &mut intent,
            &json!({"model":"alias"}),
            &json!({"model":"vendor/model:free"}),
            "https://openrouter.ai/api/v1",
            false,
        )
        .unwrap();
        assert!(intent);
        assert!(enforce(
            &mut intent,
            &json!({"model":"alias"}),
            &json!({"model":"paid/model"}),
            "https://openrouter.ai/api/v1",
            false
        )
        .is_err());
        assert!(enforce(
            &mut intent,
            &json!({"model":"alias"}),
            &json!({"model":"vendor/model:free"}),
            "https://other.invalid/api",
            false
        )
        .is_err());
    }
    #[test]
    fn free_requests_reject_mapped_paid_models_and_paid_model_arrays() {
        for outgoing in [
            json!({"model":"paid/model"}),
            json!({"model":"openrouter/free","models":["vendor/free:free","paid/model"]}),
            json!({"model":"openrouter/free","plugins":[{"id":"web"}]}),
            json!({}),
        ] {
            assert!(
                enforce(
                    &mut false,
                    &json!({"model":"openrouter/free"}),
                    &outgoing,
                    "https://openrouter.ai/api/v1",
                    false
                )
                .is_err(),
                "{outgoing}"
            );
        }
        enforce(
            &mut false,
            &json!({"model":"openrouter/free"}),
            &json!({"model":"openrouter/free","models":["vendor/model:free"]}),
            "https://openrouter.ai/api/v1",
            false,
        )
        .unwrap();
    }
    #[test]
    fn configured_free_only_fails_closed_and_unrelated_paid_routes_are_preserved() {
        assert!(enforce(
            &mut false,
            &json!({"model":"alias"}),
            &json!({"model":"paid/model"}),
            "https://openrouter.ai/api/v1",
            true
        )
        .is_err());
        enforce(
            &mut false,
            &json!({"model":"paid/model"}),
            &json!({"model":"paid/model"}),
            "https://ordinary.invalid/api",
            false,
        )
        .unwrap();
        for host in [
            "https://openrouter.ai.evil.invalid/api",
            "https://evil.invalid/openrouter.ai/api",
            "http://openrouter.ai/api",
        ] {
            assert!(enforce(
                &mut false,
                &json!({"model":"openrouter/free"}),
                &json!({"model":"openrouter/free"}),
                host,
                false
            )
            .is_err());
        }
    }
}
