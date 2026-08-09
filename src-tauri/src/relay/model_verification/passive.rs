use std::{collections::BTreeSet, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{
    app_config::AppType,
    relay::model_verification::{
        capability_profiles::CapabilityProfile,
        types::{EvidenceCode, EvidenceFact, EvidenceOutcome, TargetKey, VerificationReport},
        verdict,
    },
};

pub const PASSIVE_AGGREGATE_SCHEMA_VERSION: u8 = 1;
const CLEAN_STREAK_RESOLUTION_COUNT: u8 = 3;

/// Sanitized request capabilities. This intentionally contains no request content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveRequestMeta {
    pub stream_requested: bool,
    pub thinking_requested: bool,
    pub tools_requested: bool,
    pub structured_output_requested: bool,
}

/// One finite observation produced after a proxied response has completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceBatch {
    pub target: TargetKey,
    pub generation: u64,
    pub completed: bool,
    pub facts: Vec<EvidenceFact>,
    pub observed_at: i64,
}

/// A persisted issue identifier is only an existing evidence code, never response content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnomalyFingerprint(EvidenceCode);

impl AnomalyFingerprint {
    pub const fn from_code(code: EvidenceCode) -> Self {
        Self(code)
    }

    pub const fn code(self) -> EvidenceCode {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeCounters {
    passed: u16,
    failed: u16,
    clean_streak: u8,
}

/// Bounded, versioned state for a result-row's passive evidence.
///
/// No request, response, event, or free-form parser data is retained here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveAggregate {
    schema_version: u8,
    complete_observations: u16,
    counters: [CodeCounters; EvidenceCode::CARDINALITY],
    unresolved_fingerprints: BTreeSet<AnomalyFingerprint>,
    last_observed_at: Option<i64>,
}

impl Default for PassiveAggregate {
    fn default() -> Self {
        Self {
            schema_version: PASSIVE_AGGREGATE_SCHEMA_VERSION,
            complete_observations: 0,
            counters: [CodeCounters::default(); EvidenceCode::CARDINALITY],
            unresolved_fingerprints: BTreeSet::new(),
            last_observed_at: None,
        }
    }
}

impl PassiveAggregate {
    pub fn complete_observations(&self) -> u16 {
        self.complete_observations
    }

    pub fn pass_count(&self, code: EvidenceCode) -> u16 {
        self.counters[code.index()].passed
    }

    pub fn fail_count(&self, code: EvidenceCode) -> u16 {
        self.counters[code.index()].failed
    }

    pub fn clean_streak(&self, code: EvidenceCode) -> u8 {
        self.counters[code.index()].clean_streak
    }

    pub fn unresolved_fingerprints(&self) -> &BTreeSet<AnomalyFingerprint> {
        &self.unresolved_fingerprints
    }

    pub fn last_observed_at(&self) -> Option<i64> {
        self.last_observed_at
    }

    #[cfg(test)]
    fn set_pass_count(&mut self, code: EvidenceCode, count: u16) {
        self.counters[code.index()].passed = count;
    }

    #[cfg(test)]
    fn set_fail_count(&mut self, code: EvidenceCode, count: u16) {
        self.counters[code.index()].failed = count;
    }
}

/// Incorporates one observation without retaining it. Failed facts win within a batch so a
/// contradiction cannot be hidden by a duplicate positive fact.
pub fn reduce_batch(aggregate: &mut PassiveAggregate, batch: &EvidenceBatch) {
    if aggregate.schema_version != PASSIVE_AGGREGATE_SCHEMA_VERSION {
        return;
    }
    let profile = AppType::from_str(&batch.target.app_type)
        .ok()
        .filter(|app_type| matches!(app_type, AppType::Codex | AppType::Claude))
        .map(|app_type| CapabilityProfile::for_target(&app_type, &batch.target.model));
    let Some(profile) = profile else {
        return;
    };
    aggregate.last_observed_at = Some(batch.observed_at);
    if batch.completed {
        aggregate.complete_observations = aggregate.complete_observations.saturating_add(1);
    }

    let mut outcomes = [None; EvidenceCode::CARDINALITY];
    for fact in &batch.facts {
        if !profile.applies(fact.code) {
            continue;
        }
        let outcome = &mut outcomes[fact.code.index()];
        *outcome = match (*outcome, fact.outcome) {
            (Some(EvidenceOutcome::Failed), _) | (_, EvidenceOutcome::Failed) => {
                Some(EvidenceOutcome::Failed)
            }
            (Some(EvidenceOutcome::Passed), _) | (_, EvidenceOutcome::Passed) => {
                Some(EvidenceOutcome::Passed)
            }
            _ => Some(EvidenceOutcome::Skipped),
        };
    }

    for code in EvidenceCode::ALL {
        let counters = &mut aggregate.counters[code.index()];
        match outcomes[code.index()] {
            Some(EvidenceOutcome::Failed) => {
                counters.failed = counters.failed.saturating_add(1);
                counters.clean_streak = 0;
                aggregate
                    .unresolved_fingerprints
                    .insert(AnomalyFingerprint::from_code(code));
            }
            Some(EvidenceOutcome::Passed) => {
                counters.passed = counters.passed.saturating_add(1);
                if batch.completed {
                    counters.clean_streak = counters
                        .clean_streak
                        .saturating_add(1)
                        .min(CLEAN_STREAK_RESOLUTION_COUNT);
                    if counters.clean_streak == CLEAN_STREAK_RESOLUTION_COUNT
                        && !verdict::is_high_confidence_anomaly(code)
                    {
                        aggregate
                            .unresolved_fingerprints
                            .remove(&AnomalyFingerprint::from_code(code));
                    }
                }
            }
            Some(EvidenceOutcome::Skipped) | None => {}
        }
    }
}

/// Resolves only the fingerprints that an applicable active probe has positively checked.
pub fn resolve_with_active(
    aggregate: &mut PassiveAggregate,
    active: &VerificationReport,
) -> BTreeSet<AnomalyFingerprint> {
    let profile = AppType::from_str(&active.target.app_type)
        .ok()
        .filter(|app_type| matches!(app_type, AppType::Codex | AppType::Claude))
        .map(|app_type| CapabilityProfile::for_target(&app_type, &active.target.model));
    let cleared = active
        .facts
        .iter()
        .filter(|fact| fact.outcome == EvidenceOutcome::Passed)
        .filter(|fact| profile.is_some_and(|profile| profile.applies(fact.code)))
        .map(|fact| AnomalyFingerprint::from_code(fact.code))
        .filter(|fingerprint| aggregate.unresolved_fingerprints.remove(fingerprint))
        .collect();
    cleared
}

#[cfg(test)]
mod tests {
    use crate::relay::model_verification::{
        passive::{
            reduce_batch, AnomalyFingerprint, EvidenceBatch, PassiveAggregate, PassiveRequestMeta,
        },
        types::{EvidenceCode, EvidenceFact, EvidenceOutcome, TargetKey},
    };

    fn batch(completed: bool, facts: Vec<EvidenceFact>) -> EvidenceBatch {
        EvidenceBatch {
            target: TargetKey::new("provider-a", "codex", "gpt-5.6-sol"),
            generation: 7,
            completed,
            facts,
            observed_at: 100,
        }
    }

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

    #[test]
    fn reduction_deduplicates_facts_and_caps_all_counters() {
        let mut aggregate = PassiveAggregate::default();
        aggregate.set_pass_count(EvidenceCode::ModelMatch, u16::MAX);
        aggregate.set_fail_count(EvidenceCode::ForeignProtocol, u16::MAX);
        reduce_batch(
            &mut aggregate,
            &batch(
                true,
                vec![
                    passed(EvidenceCode::ModelMatch),
                    failed(EvidenceCode::ModelMatch),
                    failed(EvidenceCode::ForeignProtocol),
                    failed(EvidenceCode::ForeignProtocol),
                ],
            ),
        );

        assert_eq!(aggregate.pass_count(EvidenceCode::ModelMatch), u16::MAX);
        assert_eq!(aggregate.fail_count(EvidenceCode::ModelMatch), 1);
        assert_eq!(
            aggregate.fail_count(EvidenceCode::ForeignProtocol),
            u16::MAX
        );
        assert_eq!(aggregate.complete_observations(), 1);
        assert_eq!(aggregate.unresolved_fingerprints().len(), 2);
    }

    #[test]
    fn aggregate_serialization_is_finite_and_fingerprints_are_code_derived() {
        let mut aggregate = PassiveAggregate::default();
        reduce_batch(
            &mut aggregate,
            &batch(true, vec![failed(EvidenceCode::ForeignProtocol)]),
        );

        assert_eq!(
            AnomalyFingerprint::from_code(EvidenceCode::ForeignProtocol).code(),
            EvidenceCode::ForeignProtocol
        );
        let serialized = serde_json::to_string(&aggregate).unwrap();
        assert!(serialized.contains("foreignProtocol"));
        assert!(!serialized.contains("provider-a"));
        assert!(!serialized.contains("arbitrary"));
    }

    #[test]
    fn request_metadata_serializes_only_capability_booleans() {
        let metadata = PassiveRequestMeta {
            stream_requested: true,
            thinking_requested: false,
            tools_requested: true,
            structured_output_requested: false,
        };

        assert_eq!(
            serde_json::to_value(metadata).unwrap(),
            serde_json::json!({
                "streamRequested": true,
                "thinkingRequested": false,
                "toolsRequested": true,
                "structuredOutputRequested": false,
            })
        );
    }
}
