//! Request-scoped free-model policy. The forwarder owns its sticky intent.
use super::error::ProxyError;
use crate::services::model_fetch::{
    is_openrouter_free_model, is_openrouter_source, is_reserved_free_model,
};
use crate::vendor::opencode::external::{classify, ExternalUse};
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
    // Always enforce external-use eligibility on the final outbound body, even
    // when freeOnly=false. Caller already applied body overrides. No mutation,
    // automatic paid fallback, or client/plan identity exception is permitted.
    let external = |model: &str| {
        match classify(base_url, model) {
        ExternalUse::Unrelated | ExternalUse::Allowed | ExternalUse::GoFree => Ok(()),
        ExternalUse::FreeChatRestricted => Err(ProxyError::InvalidRequest(
            "opencode.external_free_chat_unavailable: Free-tier chat requires direct OpenCode use; external model requests are paused. Saved configuration is unchanged.".into(),
        )),
        ExternalUse::Unverified => Err(ProxyError::InvalidRequest(
            "opencode.external_model_unverified: External use for this official product/model pair has not been verified. Saved configuration is unchanged; no paid substitution was made.".into(),
        )),
    }
    };
    external(model(outgoing).as_deref().unwrap_or_default())?;
    if classify(base_url, "") != ExternalUse::Unrelated {
        if let Some(models) = outgoing.get("models") {
            let models = models.as_array().ok_or_else(|| {
                ProxyError::InvalidRequest(
                    "opencode.external_model_unverified: Invalid fallback model list".into(),
                )
            })?;
            for model in models {
                external(model.as_str().unwrap_or_default())?;
            }
        }
    }

    if !*free_only {
        return Ok(());
    }
    let route_free = |model: &str| {
        (is_openrouter_source(base_url) && is_openrouter_free_model(model))
            || classify(base_url, model) == ExternalUse::GoFree
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
    fn opencode_free_chat_external_route_rejects_even_official_variants() {
        let free = json!({"model":"glm-5.3-flash-free"});
        assert!(enforce(
            &mut false,
            &free,
            &free,
            "https://opencode.ai/inference/openai/v1",
            true,
        )
        .is_err());
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

/// Original final-body boundary exercised without linking unrelated unit suites.
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_external_policy(case: &str) {
    use serde_json::json;
    let base = "https://opencode.ai/inference/openai/v1";
    let free = json!({"model":"glm-5.3-flash-free"});
    let paid = json!({"model":"gpt-5.3-codex"});
    match case {
        "policy_free_false" => assert!(enforce(&mut false, &free, &free, base, false).is_err()),
        "policy_free_paid_key_route" => {
            for origin in [
                base,
                "https://opencode.ai/zen/v1",
                "https://opencode.ai/zen/go/v1",
                "https://opencode.ai/inference/go/openai/v1",
            ] {
                assert!(
                    enforce(&mut false, &free, &free, origin, false).is_err(),
                    "known free cannot become external by endpoint/plan"
                );
            }
        }
        "policy_fallback_free" => assert!(enforce(
            &mut false,
            &paid,
            &json!({"model":"gpt-5.3-codex","models":["glm-5.3-flash-free"]}),
            base,
            false
        )
        .is_err()),
        "policy_override_free" => assert!(enforce(&mut false, &paid, &free, base, false).is_err()),
        "policy_unknown" => {
            for body in [
                json!({"model":"synthetic-unclassified-model"}),
                json!({"model":"synthetic-unknown-free"}),
                json!({"model":"big-pickle"}),
                json!({"model":"gpt-5.3-codex","models":["synthetic-unclassified-model"]}),
                json!({"model":"gpt-5.3-codex","models":[{}]}),
                json!({}),
            ] {
                assert!(
                    enforce(&mut false, &body, &body, base, false).is_err(),
                    "unclassified output cannot be called paid"
                );
            }
        }
        "policy_paid_go_preserved" => {
            for (origin, model) in [
                ("https://opencode.ai/zen/v1", "gpt-5.3-codex"),
                (base, "gpt-5.3-codex"),
                ("https://opencode.ai/inference/anthropic", "claude-sonnet-5"),
                ("https://opencode.ai/zen/go/v1", "glm-5.2"),
                (
                    "https://opencode.ai/inference/go/anthropic",
                    "deepseek-v4-pro",
                ),
            ] {
                let body = json!({"model":model});
                enforce(&mut false, &body, &body, origin, false).unwrap();
            }
        }
        "policy_sticky_rejection" => {
            let mut intent = false;
            let alias = json!({"model":"caller-alias"});
            assert!(enforce(&mut intent, &alias, &free, base, false).is_err());
            assert!(
                intent,
                "a local external refusal must not erase original sticky free intent"
            );
            assert!(
                enforce(&mut intent, &alias, &paid, base, false).is_err(),
                "no paid retry after mapped free rejection"
            );
        }
        "policy_go_beam_exact" => {
            let beam = json!({"model":"beam-free"});
            for origin in [
                "https://opencode.ai/inference/go/openai/v1",
                "https://opencode.ai/zen/go/v1",
            ] {
                enforce(&mut false, &beam, &beam, origin, false).unwrap();
            }
            assert!(enforce(&mut false, &beam, &beam, base, false).is_err());
            assert!(enforce(
                &mut false,
                &beam,
                &json!({"model":"beam-free","models":["glm-5.3-flash-free"]}),
                "https://opencode.ai/inference/go/openai/v1",
                false
            )
            .is_err());
        }
        "policy_current_catalog" => {
            for (origin, model) in [
                (base, "gpt-6.1-sol"),
                (base, "claude-fable-5-1"),
                (base, "grok-4.7"),
                (base, "qwen3.8-max"),
                (base, "mistral-large-4"),
                (
                    "https://opencode.ai/inference/go/openai/v1",
                    "kimi-k3.1-pro",
                ),
                (
                    "https://opencode.ai/inference/go/openai/v1",
                    "minimax-m3.1-flash",
                ),
                ("https://opencode.ai/inference/go/anthropic", "qwen3.7-plus"),
                (
                    "https://opencode.ai/inference/go/openai/v1",
                    "muse-spark-1.3-contributor",
                ),
            ] {
                let body = json!({"model":model});
                enforce(&mut false, &body, &body, origin, false).unwrap();
            }
            for (origin, model) in [
                (base, "deepseek-v4-flash"),
                (
                    "https://opencode.ai/inference/go/openai/v1",
                    "gpt-5.3-codex",
                ),
                (base, "muse-spark-1.3-contributor"),
            ] {
                let body = json!({"model":model});
                assert!(
                    enforce(&mut false, &body, &body, origin, false).is_err(),
                    "unconfirmed product/model pair must not be relabeled paid"
                );
            }
        }
        "policy_channel_normalization" => {
            let native_free = json!({"model":"big-pickle"});
            assert!(enforce(
                &mut false,
                &native_free,
                &native_free,
                "https://opencode.ai./inference/openai/v1",
                false
            )
            .is_err());

            for origin in [
                "https://OPENCODE.AI:443/inference/openai/v1/",
                "https://opencode.ai/zen/v1",
            ] {
                enforce(&mut false, &paid, &paid, origin, false).unwrap();
            }
            for origin in [
                "http://opencode.ai/inference/openai/v1",
                "https://opencode.ai:444/inference/openai/v1",
                "https://user:pass@opencode.ai/inference/openai/v1",
                "https://opencode.ai/inference/go-pretend/openai/v1",
                "https://opencode.ai/unknown",
            ] {
                assert!(enforce(&mut false, &paid, &paid, origin, false).is_err());
            }
        }
        "policy_other_platforms_preserved" => {
            let body = json!({"model":"vendor/model:free"});
            enforce(
                &mut false,
                &body,
                &body,
                "https://openrouter.ai/api/v1",
                true,
            )
            .unwrap();
            let ordinary = json!({"model":"synthetic-custom-model"});
            enforce(
                &mut false,
                &ordinary,
                &ordinary,
                "https://ordinary.invalid/v1",
                false,
            )
            .unwrap();
        }
        _ => panic!("unknown external policy fixture"),
    }
}
