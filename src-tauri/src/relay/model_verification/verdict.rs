use crate::{
    app_config::AppType,
    relay::model_verification::{
        capability_profiles::CapabilityProfile,
        types::{EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, Verdict},
    },
};

/// Reduces finite verification evidence to its single user-facing verdict.
///
/// Protocol probes emit facts only; their meaning and precedence live here so adding a probe
/// cannot create a competing verdict policy.
pub fn evaluate(
    app_type: AppType,
    profile: &CapabilityProfile,
    facts: &[EvidenceFact],
) -> (Verdict, EvidenceLevel) {
    if facts
        .iter()
        .any(|fact| fact.outcome == EvidenceOutcome::Failed && is_critical_contradiction(fact.code))
    {
        return (Verdict::Anomaly, EvidenceLevel::Insufficient);
    }

    let applicable_facts = facts.iter().filter(|fact| profile.applies(fact.code));
    if applicable_facts
        .clone()
        .any(|fact| fact.outcome == EvidenceOutcome::Failed)
    {
        return (Verdict::Suspicious, EvidenceLevel::Insufficient);
    }

    if !applicable_facts
        .clone()
        .any(|fact| fact.outcome == EvidenceOutcome::Passed)
    {
        return (Verdict::Inconclusive, EvidenceLevel::Insufficient);
    }

    let evidence_level = if matches!(app_type, AppType::Claude)
        && profile.supports_signature_continuation
        && facts.iter().any(|fact| {
            fact.code == EvidenceCode::SignatureContinuation
                && fact.outcome == EvidenceOutcome::Passed
        }) {
        EvidenceLevel::Cryptographic
    } else {
        EvidenceLevel::ProtocolBehavior
    };
    (Verdict::Trusted, evidence_level)
}

fn is_critical_contradiction(code: EvidenceCode) -> bool {
    matches!(
        code,
        EvidenceCode::ModelMatch
            | EvidenceCode::ForeignProtocol
            | EvidenceCode::ForeignSelfIdentification
    )
}

#[cfg(test)]
mod tests {
    use crate::{
        app_config::AppType,
        relay::model_verification::{
            capability_profiles::CapabilityProfile,
            types::{EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, Verdict},
        },
    };

    use super::evaluate;

    fn passed(code: EvidenceCode) -> EvidenceFact {
        EvidenceFact {
            code,
            outcome: EvidenceOutcome::Passed,
        }
    }

    fn failed(code: EvidenceCode) -> EvidenceFact {
        EvidenceFact {
            code,
            outcome: EvidenceOutcome::Failed,
        }
    }

    fn codex_profile(model: &str) -> CapabilityProfile {
        CapabilityProfile::for_target(&AppType::Codex, model)
    }

    #[test]
    fn foreign_protocol_caps_the_result_at_anomaly() {
        let facts = vec![
            passed(EvidenceCode::BasicEnvelope),
            failed(EvidenceCode::ForeignProtocol),
        ];

        assert_eq!(
            evaluate(AppType::Codex, &codex_profile("gpt-5.6-sol"), &facts).0,
            Verdict::Anomaly
        );
    }

    #[test]
    fn model_mismatch_is_an_anomaly() {
        assert_eq!(
            evaluate(
                AppType::Codex,
                &codex_profile("gpt-5.6-sol"),
                &[failed(EvidenceCode::ModelMatch)],
            )
            .0,
            Verdict::Anomaly
        );
    }

    #[test]
    fn malformed_stream_is_suspicious() {
        assert_eq!(
            evaluate(
                AppType::Codex,
                &codex_profile("gpt-5.6-sol"),
                &[failed(EvidenceCode::StreamLifecycle)],
            )
            .0,
            Verdict::Suspicious
        );
    }

    #[test]
    fn unknown_model_skips_optional_checks_without_false_anomaly() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");
        assert!(!profile.supports_thinking_signature);

        assert_eq!(
            evaluate(
                AppType::Claude,
                &profile,
                &[
                    passed(EvidenceCode::BasicEnvelope),
                    passed(EvidenceCode::ModelMatch),
                    failed(EvidenceCode::ThinkingSignature),
                ],
            ),
            (Verdict::Trusted, EvidenceLevel::ProtocolBehavior)
        );
    }

    #[test]
    fn passed_signature_continuation_is_cryptographic_for_supported_anthropic_models() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "claude-sonnet-5");

        assert_eq!(
            evaluate(
                AppType::Claude,
                &profile,
                &[
                    passed(EvidenceCode::BasicEnvelope),
                    passed(EvidenceCode::SignatureContinuation),
                ],
            ),
            (Verdict::Trusted, EvidenceLevel::Cryptographic)
        );
    }

    #[test]
    fn codex_never_reports_cryptographic_evidence() {
        assert_eq!(
            evaluate(
                AppType::Codex,
                &codex_profile("gpt-5.6-sol"),
                &[
                    passed(EvidenceCode::BasicEnvelope),
                    passed(EvidenceCode::SignatureContinuation),
                ],
            ),
            (Verdict::Trusted, EvidenceLevel::ProtocolBehavior)
        );
    }

    #[test]
    fn no_applicable_pass_is_inconclusive_for_both_active_protocols() {
        for app_type in [AppType::Codex, AppType::Claude] {
            assert_eq!(
                evaluate(
                    app_type.clone(),
                    &CapabilityProfile::for_target(&app_type, "future-model-x"),
                    &[passed(EvidenceCode::ThinkingSignature)],
                ),
                (Verdict::Inconclusive, EvidenceLevel::Insufficient)
            );
        }
    }
}
