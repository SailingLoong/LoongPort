use crate::{
    app_config::AppType,
    relay::model_verification::{
        capability_profiles::CapabilityProfile,
        passive::PassiveAggregate,
        types::{EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, Verdict},
    },
};

/// The only passive failures that can become high-confidence anomalies.
pub(crate) fn is_high_confidence_anomaly(code: EvidenceCode) -> bool {
    matches!(
        code,
        EvidenceCode::ForeignProtocol
            | EvidenceCode::ModelMatch
            | EvidenceCode::ThinkingSignature
            | EvidenceCode::SignatureContinuation
            | EvidenceCode::StreamLifecycle
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergedReport {
    pub verdict: Verdict,
    pub evidence_level: EvidenceLevel,
}

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
        EvidenceCode::ModelMatch | EvidenceCode::ForeignProtocol
    )
}

/// Merges bounded passive state with the latest active report. All precedence and confidence
/// thresholds live here; storage is deliberately policy-free.
pub fn merge(
    active: Option<&crate::relay::model_verification::types::VerificationReport>,
    passive: Option<&PassiveAggregate>,
) -> MergedReport {
    let passive = passive.map(passive_report).unwrap_or(MergedReport {
        verdict: Verdict::Inconclusive,
        evidence_level: EvidenceLevel::Insufficient,
    });
    let active = active.map(|report| MergedReport {
        verdict: report.verdict,
        evidence_level: report.evidence_level,
    });

    match passive.verdict {
        Verdict::Anomaly => passive,
        Verdict::Suspicious => match active {
            Some(active) if active.verdict == Verdict::Anomaly => active,
            _ => passive,
        },
        Verdict::Trusted => match active {
            Some(active) if active.verdict != Verdict::Inconclusive => active,
            _ => passive,
        },
        Verdict::Inconclusive => active.unwrap_or(passive),
    }
}

fn passive_report(aggregate: &PassiveAggregate) -> MergedReport {
    if aggregate
        .unresolved_fingerprints()
        .iter()
        .any(|fingerprint| is_high_confidence_anomaly(fingerprint.code()))
    {
        return MergedReport {
            verdict: Verdict::Anomaly,
            evidence_level: EvidenceLevel::Insufficient,
        };
    }
    if !aggregate.unresolved_fingerprints().is_empty() {
        return MergedReport {
            verdict: Verdict::Suspicious,
            evidence_level: EvidenceLevel::Insufficient,
        };
    }
    if EvidenceCode::ALL
        .iter()
        .any(|code| aggregate.clean_streak(*code) >= 3)
    {
        return MergedReport {
            verdict: Verdict::Trusted,
            evidence_level: if aggregate.clean_streak(EvidenceCode::SignatureContinuation) >= 3 {
                EvidenceLevel::Cryptographic
            } else {
                EvidenceLevel::ProtocolBehavior
            },
        };
    }
    MergedReport {
        verdict: Verdict::Inconclusive,
        evidence_level: EvidenceLevel::Insufficient,
    }
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

    use super::{evaluate, merge};
    use crate::relay::model_verification::{
        passive::{reduce_batch, resolve_with_active, EvidenceBatch, PassiveAggregate},
        types::{TargetKey, VerificationReport, RULES_VERSION},
    };

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

    fn active(facts: Vec<EvidenceFact>, verdict: Verdict) -> VerificationReport {
        VerificationReport {
            target: TargetKey::new("provider-a", "codex", "gpt-5.6-sol"),
            verdict,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts,
            rules_version: RULES_VERSION,
            checked_at: 100,
        }
    }

    fn observation(facts: Vec<EvidenceFact>) -> EvidenceBatch {
        EvidenceBatch {
            target: TargetKey::new("provider-a", "codex", "gpt-5.6-sol"),
            generation: 0,
            completed: true,
            facts,
            observed_at: 100,
        }
    }

    #[test]
    fn inconclusive_passive_data_never_replaces_a_valid_active_report() {
        let aggregate = PassiveAggregate::default();
        assert_eq!(
            merge(
                Some(&active(
                    vec![passed(EvidenceCode::BasicEnvelope)],
                    Verdict::Trusted
                )),
                Some(&aggregate),
            )
            .verdict,
            Verdict::Trusted
        );
    }

    #[test]
    fn suspicious_fingerprint_needs_three_complete_clean_observations_to_resolve() {
        let mut aggregate = PassiveAggregate::default();
        reduce_batch(
            &mut aggregate,
            &observation(vec![failed(EvidenceCode::UsageConsistency)]),
        );
        for _ in 0..2 {
            reduce_batch(
                &mut aggregate,
                &observation(vec![passed(EvidenceCode::UsageConsistency)]),
            );
        }
        assert_eq!(merge(None, Some(&aggregate)).verdict, Verdict::Suspicious);

        reduce_batch(
            &mut aggregate,
            &observation(vec![passed(EvidenceCode::UsageConsistency)]),
        );
        assert_eq!(merge(None, Some(&aggregate)).verdict, Verdict::Trusted);
    }

    #[test]
    fn high_confidence_anomaly_survives_clean_traffic_until_same_code_active_pass() {
        let mut aggregate = PassiveAggregate::default();
        reduce_batch(
            &mut aggregate,
            &observation(vec![failed(EvidenceCode::ForeignProtocol)]),
        );
        for _ in 0..5 {
            reduce_batch(
                &mut aggregate,
                &observation(vec![passed(EvidenceCode::ForeignProtocol)]),
            );
        }
        assert_eq!(merge(None, Some(&aggregate)).verdict, Verdict::Anomaly);

        let active = active(
            vec![passed(EvidenceCode::ForeignProtocol)],
            Verdict::Trusted,
        );
        resolve_with_active(&mut aggregate, &active);
        assert_eq!(
            merge(Some(&active), Some(&aggregate)).verdict,
            Verdict::Trusted
        );
    }

    #[test]
    fn deterministic_contradiction_beats_positive_evidence() {
        let mut aggregate = PassiveAggregate::default();
        reduce_batch(
            &mut aggregate,
            &observation(vec![failed(EvidenceCode::ModelMatch)]),
        );
        assert_eq!(
            merge(
                Some(&active(
                    vec![passed(EvidenceCode::BasicEnvelope)],
                    Verdict::Trusted
                )),
                Some(&aggregate),
            )
            .verdict,
            Verdict::Anomaly
        );
    }

    #[test]
    fn active_pass_only_resolves_its_matching_fingerprint() {
        let mut aggregate = PassiveAggregate::default();
        reduce_batch(
            &mut aggregate,
            &observation(vec![
                failed(EvidenceCode::ForeignProtocol),
                failed(EvidenceCode::UsageConsistency),
            ]),
        );

        let active = active(
            vec![passed(EvidenceCode::UsageConsistency)],
            Verdict::Trusted,
        );
        resolve_with_active(&mut aggregate, &active);

        assert_eq!(
            merge(Some(&active), Some(&aggregate)).verdict,
            Verdict::Anomaly
        );
    }

    #[test]
    fn supported_anthropic_signature_can_raise_passive_evidence_level() {
        let mut aggregate = PassiveAggregate::default();
        let batch = |model: &str| EvidenceBatch {
            target: TargetKey::new("provider-a", "claude", model),
            generation: 0,
            completed: true,
            facts: vec![passed(EvidenceCode::SignatureContinuation)],
            observed_at: 100,
        };
        for _ in 0..3 {
            reduce_batch(&mut aggregate, &batch("claude-sonnet-5"));
        }
        assert_eq!(
            merge(None, Some(&aggregate)).evidence_level,
            EvidenceLevel::Cryptographic
        );

        let mut unknown = PassiveAggregate::default();
        for _ in 0..3 {
            reduce_batch(&mut unknown, &batch("future-model-x"));
        }
        assert_eq!(merge(None, Some(&unknown)).verdict, Verdict::Inconclusive);
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
    fn foreign_self_identification_alone_is_suspicious() {
        assert_eq!(
            evaluate(
                AppType::Codex,
                &codex_profile("gpt-5.6-sol"),
                &[failed(EvidenceCode::ForeignSelfIdentification)],
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
