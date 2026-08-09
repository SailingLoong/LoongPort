use crate::app_config::AppType;

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

fn supports_anthropic_signature_continuation(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized
        .strip_suffix("[1m]")
        .unwrap_or(&normalized)
        .trim();
    ANTHROPIC_SIGNATURE_CONTINUATION_PREFIXES
        .iter()
        .any(|prefix| {
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
}
