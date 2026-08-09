use crate::{app_config::AppType, proxy::model_mapper::strip_one_m_suffix_for_upstream};

use super::types::EvidenceCode;

/// The verification checks that a concrete target is expected to support.
///
/// A model that is not explicitly recognized receives only the protocol checks shared by every
/// target. This prevents a newly introduced model from being penalized for optional behavior we
/// have not established for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityProfile {
    pub supports_structured_output: bool,
    pub supports_thinking_signature: bool,
    pub supports_signature_continuation: bool,
}

impl CapabilityProfile {
    pub fn for_target(app_type: &AppType, model: &str) -> Self {
        if matches!(app_type, AppType::Codex) && supports_codex_structured_output(model) {
            return Self {
                supports_structured_output: true,
                supports_thinking_signature: false,
                supports_signature_continuation: false,
            };
        }

        if matches!(app_type, AppType::Claude) && supports_anthropic_signature_continuation(model) {
            return Self {
                supports_structured_output: false,
                supports_thinking_signature: true,
                supports_signature_continuation: true,
            };
        }

        Self {
            supports_structured_output: false,
            supports_thinking_signature: false,
            supports_signature_continuation: false,
        }
    }

    pub fn applies(&self, code: EvidenceCode) -> bool {
        match code {
            EvidenceCode::StructuredOutput => self.supports_structured_output,
            EvidenceCode::ThinkingSignature => self.supports_thinking_signature,
            EvidenceCode::SignatureContinuation => self.supports_signature_continuation,
            EvidenceCode::BasicEnvelope
            | EvidenceCode::ModelMatch
            | EvidenceCode::StreamLifecycle
            | EvidenceCode::UsageConsistency
            | EvidenceCode::ToolCallShape
            | EvidenceCode::ForeignProtocol
            | EvidenceCode::ForeignSelfIdentification => true,
        }
    }
}

/// These are the current managed Claude route families in `relay::provision`.
///
/// Matching permits the repository's `[1M]` marker and compact release-date suffixes, but excludes
/// a broad `claude-` match so an unsupported future model cannot opt into signature probes merely
/// by its brand.
const ANTHROPIC_SIGNATURE_CONTINUATION_PREFIXES: &[&str] =
    &["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"];

/// Current managed Codex routes documented to support Responses structured outputs.
///
/// This intentionally does not match a broad `gpt-` family: future routes must remain on the
/// protocol core until their capability is independently established.
const CODEX_STRUCTURED_OUTPUT_PREFIXES: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.4",
    "gpt-5.4-2026-03-05",
];

fn supports_codex_structured_output(model: &str) -> bool {
    let upstream_model = strip_one_m_suffix_for_upstream(model).trim();
    CODEX_STRUCTURED_OUTPUT_PREFIXES.contains(&upstream_model)
}

fn supports_anthropic_signature_continuation(model: &str) -> bool {
    supports_known_model_with_display_suffix(model, ANTHROPIC_SIGNATURE_CONTINUATION_PREFIXES)
}

fn supports_known_model_with_display_suffix(model: &str, prefixes: &[&str]) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized
        .strip_suffix("[1m]")
        .unwrap_or(&normalized)
        .trim();
    prefixes.iter().any(|prefix| {
        normalized == *prefix
            || normalized
                .strip_prefix(prefix)
                .is_some_and(is_compact_release_date_suffix)
    })
}

fn is_compact_release_date_suffix(suffix: &str) -> bool {
    suffix.len() == 9
        && suffix.starts_with('-')
        && suffix[1..].bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use crate::app_config::AppType;

    use super::CapabilityProfile;

    #[test]
    fn unknown_models_keep_only_core_checks() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");

        assert!(!profile.supports_thinking_signature);
        assert!(!profile.supports_structured_output);
    }

    #[test]
    fn current_claude_routes_enable_signature_continuation() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "claude-sonnet-5[1M]");

        assert!(profile.supports_thinking_signature);
        assert!(profile.supports_signature_continuation);
    }

    #[test]
    fn only_known_claude_ids_and_display_suffixes_enable_signature_checks() {
        assert!(
            CapabilityProfile::for_target(&AppType::Claude, "claude-haiku-4-5-20251001")
                .supports_signature_continuation
        );
        assert!(
            !CapabilityProfile::for_target(&AppType::Claude, "claude-sonnet-5-future")
                .supports_signature_continuation
        );
    }

    #[test]
    fn current_codex_routes_enable_structured_output_only_for_known_ids() {
        for model in [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna[1m]",
            "gpt-5.4",
            "gpt-5.4-2026-03-05[1M]",
        ] {
            assert!(
                CapabilityProfile::for_target(&AppType::Codex, model).supports_structured_output
            );
        }
        assert!(
            !CapabilityProfile::for_target(&AppType::Codex, "gpt-5.7-future")
                .supports_structured_output
        );
        assert!(
            !CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-terra-20260809")
                .supports_structured_output
        );
    }
}
