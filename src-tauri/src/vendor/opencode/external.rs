//! External-use eligibility, not pricing or wire-protocol support.
//! Official endpoint/product tables checked 2026-10-27:
//! https://opencode.ai/v2/docs/console/models/
//! https://opencode.ai/v2/docs/console/go/
//! Unknown is unverified, not an assertion of an official ban. Never rewrite a
//! saved model or infer permission from a key, app name, header, or plan label.

#[derive(Clone, Copy)]
enum Products {
    Console,
    Go,
    Both,
    GoFree,
}

// One positive product/model collection. Original protocol, authentication,
// region and account checks still apply. Go's explicit Beam exception does not
// grant other free chat models access to Go or grant Beam regular Console use.
const MODEL_PRODUCTS: &[(&str, Products)] = &[
    ("beam-free", Products::GoFree),
    ("claude-fable-5", Products::Console),
    ("claude-fable-5-1", Products::Console),
    ("claude-haiku-4-5", Products::Console),
    ("claude-haiku-5-5", Products::Both),
    ("claude-opus-4-5", Products::Console),
    ("claude-opus-4-6", Products::Console),
    ("claude-opus-4-7", Products::Console),
    ("claude-opus-4-8", Products::Console),
    ("claude-opus-5", Products::Console),
    ("claude-opus-5-5", Products::Console),
    ("claude-sonnet-4-5", Products::Console),
    ("claude-sonnet-4-6", Products::Console),
    ("claude-sonnet-5", Products::Console),
    ("claude-sonnet-5-5", Products::Console),
    ("deepseek-v4-pro", Products::Both),
    ("deepseek-v4.1-flash", Products::Both),
    ("gemini-3-flash", Products::Console),
    ("gemini-3.1-pro", Products::Console),
    ("gemini-3.5-flash", Products::Console),
    ("gemini-3.5-flash-lite", Products::Console),
    ("gemini-3.6-flash", Products::Console),
    ("gemini-3.7-flash", Products::Console),
    ("gemini-3.8-flash", Products::Console),
    ("gemini-4-argon", Products::Both),
    ("glm-5", Products::Console),
    ("glm-5.1", Products::Console),
    ("glm-5.2", Products::Both),
    ("glm-5.3", Products::Both),
    ("glm-5.3-flash", Products::Both),
    ("gpt-5", Products::Console),
    ("gpt-5-codex", Products::Console),
    ("gpt-5-nano", Products::Console),
    ("gpt-5.1", Products::Console),
    ("gpt-5.1-codex", Products::Console),
    ("gpt-5.1-codex-max", Products::Console),
    ("gpt-5.1-codex-mini", Products::Console),
    ("gpt-5.2", Products::Console),
    ("gpt-5.2-codex", Products::Console),
    ("gpt-5.3-codex", Products::Console),
    ("gpt-5.3-codex-spark", Products::Console),
    ("gpt-5.4", Products::Console),
    ("gpt-5.4-mini", Products::Console),
    ("gpt-5.4-nano", Products::Console),
    ("gpt-5.4-pro", Products::Console),
    ("gpt-5.5", Products::Console),
    ("gpt-5.5-pro", Products::Console),
    ("gpt-5.6-luna", Products::Both),
    ("gpt-5.6-sol", Products::Console),
    ("gpt-5.6-terra", Products::Console),
    ("gpt-6-astra", Products::Console),
    ("gpt-6-luna", Products::Both),
    ("gpt-6-sol", Products::Console),
    ("gpt-6.1-sol", Products::Console),
    ("grok-4.5", Products::Console),
    ("grok-4.6", Products::Console),
    ("grok-4.7", Products::Both),
    ("grok-build-0.1", Products::Console),
    ("hy3", Products::Go),
    ("hy4-preview", Products::Go),
    ("jev-1.13", Products::Console),
    ("kimi-k2.5", Products::Console),
    ("kimi-k2.6", Products::Console),
    ("kimi-k2.7-code", Products::Both),
    ("kimi-k3", Products::Both),
    ("kimi-k3.1-pro", Products::Go),
    ("longcat-2.0", Products::Go),
    ("mimo-v2.5", Products::Go),
    ("mimo-v2.5-pro", Products::Go),
    ("mimo-v2.6-flash", Products::Go),
    ("mimo-v2.6-pro", Products::Go),
    ("minimax-m2.5", Products::Console),
    ("minimax-m2.7", Products::Console),
    ("minimax-m3", Products::Both),
    ("minimax-m3.1-flash", Products::Go),
    ("mistral-large-4", Products::Console),
    ("muse-spark-1.2", Products::Console),
    ("muse-spark-1.2-contributor", Products::Go),
    ("muse-spark-1.3", Products::Console),
    ("muse-spark-1.3-contributor", Products::Go),
    ("muse-wonder-1.0", Products::Console),
    ("qwen3.5-plus", Products::Console),
    ("qwen3.6-plus", Products::Console),
    ("qwen3.7-plus", Products::Go),
    ("qwen3.8-flash", Products::Both),
    ("qwen3.8-max", Products::Both),
    ("step-5-preview", Products::Go),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalUse {
    Unrelated,
    Allowed,
    GoFree,
    FreeChatRestricted,
    Unverified,
}

// Preserve the original owner's Zen aliases as well as its current inference
// URLs. Exact path boundaries avoid treating arbitrary `/go...` paths as Go.
fn product_path(path: &str) -> Option<bool> {
    let path = path.trim_end_matches('/');
    for (prefix, go, legacy) in [
        ("/zen/go", true, true),
        ("/zen", false, true),
        ("/inference/go", true, false),
        ("/inference", false, false),
    ] {
        let Some(suffix) = path.strip_prefix(prefix) else {
            continue;
        };
        let known = if legacy {
            matches!(
                suffix,
                "" | "/v1" | "/v1/messages" | "/v1/chat/completions" | "/v1/responses"
            )
        } else {
            matches!(
                suffix,
                "" | "/openai/v1"
                    | "/openai/v1/chat/completions"
                    | "/openai/v1/responses"
                    | "/anthropic"
                    | "/anthropic/v1"
                    | "/anthropic/v1/messages"
            )
        };
        if known {
            return Some(go);
        }
    }
    None
}

pub(crate) fn classify(source: &str, model: &str) -> ExternalUse {
    let Ok(url) = url::Url::parse(source) else {
        return ExternalUse::Unrelated;
    };
    if url.host_str().map(|host| host.trim_end_matches('.')) != Some("opencode.ai") {
        return ExternalUse::Unrelated;
    }
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return ExternalUse::Unverified;
    }
    let Some(go) = product_path(url.path()) else {
        return ExternalUse::Unverified;
    };
    if let Some((_, products)) = MODEL_PRODUCTS.iter().find(|(id, _)| *id == model) {
        match (go, products) {
            (true, Products::GoFree) => return ExternalUse::GoFree,
            (true, Products::Go | Products::Both) | (false, Products::Console | Products::Both) => {
                return ExternalUse::Allowed
            }
            _ => {}
        }
    }
    // The native OpenCode Free catalogue is deliberately not external eligibility.
    if super::FREE_CHAT_MODELS.contains(&model) || matches!(model, "big-pickle" | "flint-40b-free")
    {
        ExternalUse::FreeChatRestricted
    } else {
        ExternalUse::Unverified
    }
}
