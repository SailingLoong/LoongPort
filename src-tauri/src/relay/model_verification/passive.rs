use std::{
    collections::BTreeSet,
    fmt,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use serde::{
    de::{SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    app_config::AppType,
    relay::model_verification::{
        capability_profiles::CapabilityProfile,
        types::{EvidenceCode, EvidenceFact, EvidenceOutcome, TargetKey, VerificationReport},
        verdict,
    },
};

pub const PASSIVE_AGGREGATE_SCHEMA_VERSION: u8 = 1;
pub(crate) const CLEAN_STREAK_RESOLUTION_COUNT: u8 = 3;

/// Sanitized request capabilities. This intentionally contains no request content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveRequestMeta {
    pub stream_requested: bool,
    pub thinking_requested: bool,
    pub tools_requested: bool,
    pub structured_output_requested: bool,
}

pub const PASSIVE_INGRESS_CAPACITY: usize = 128;

/// Maximum bytes retained for one in-flight server-sent event.
pub const MAX_SSE_EVENT_BYTES: usize = 256 * 1024;
/// Maximum bytes retained while looking for a fixed self-identification phrase.
pub const SELF_ID_TAIL_BYTES: usize = 256;
/// Maximum bytes inspected by the non-streaming reducer.
pub const MAX_RESPONSE_INSPECTION_BYTES: usize = 2 * 1024 * 1024;

/// The only object passed into proxy request handling for passive verification.
/// It owns no database or coordinator state and only accepts sanitized batches.
#[derive(Clone)]
pub struct VerificationIngress {
    sender: mpsc::Sender<EvidenceBatch>,
    enabled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

/// A request-scoped handle carrying only the target and generation barrier.
pub struct IngressTap {
    ingress: VerificationIngress,
    target: TargetKey,
    generation: u64,
}

impl VerificationIngress {
    pub fn channel() -> (Self, mpsc::Receiver<EvidenceBatch>) {
        let (sender, receiver) = mpsc::channel(PASSIVE_INGRESS_CAPACITY);
        (Self::new(sender), receiver)
    }

    pub fn disabled() -> Self {
        let (sender, receiver) = mpsc::channel(PASSIVE_INGRESS_CAPACITY);
        drop(receiver);
        Self {
            sender,
            enabled: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn new(sender: mpsc::Sender<EvidenceBatch>) -> Self {
        Self {
            sender,
            enabled: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        let previous = self.enabled.swap(enabled, Ordering::AcqRel);
        if previous != enabled {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn begin(&self, target: TargetKey) -> Option<IngressTap> {
        if !self.enabled.load(Ordering::Acquire)
            || AppType::from_str(&target.app_type)
                .ok()
                .is_none_or(|app_type| !matches!(app_type, AppType::Codex | AppType::Claude))
        {
            return None;
        }
        Some(IngressTap {
            ingress: self.clone(),
            target,
            generation: self.generation.load(Ordering::Acquire),
        })
    }

    /// Enqueues sanitized evidence without ever backpressuring the real response.
    /// Returns whether the batch was accepted; queue-full and disabled are ordinary drops.
    pub fn try_submit(&self, batch: EvidenceBatch) -> bool {
        if !self.enabled.load(Ordering::Acquire)
            || batch.generation != self.generation.load(Ordering::Acquire)
        {
            return false;
        }
        self.sender.try_send(batch).is_ok()
    }

    pub fn capacity(&self) -> usize {
        self.sender.capacity()
    }
}

impl IngressTap {
    pub fn submit(
        &self,
        completed: bool,
        facts: impl IntoIterator<Item = EvidenceFact>,
        observed_at: i64,
    ) -> bool {
        self.ingress.try_submit(EvidenceBatch::new(
            self.target.clone(),
            self.generation,
            completed,
            facts,
            observed_at,
        ))
    }
}

/// Reduce protocol request capability flags without retaining any request content.
pub fn reduce_request_meta(app_type: &AppType, body: &Value) -> PassiveRequestMeta {
    let stream_requested = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let tools_requested = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
        || body
            .get("tool_choice")
            .is_some_and(|choice| !choice.is_null());

    match app_type {
        AppType::Claude => PassiveRequestMeta {
            stream_requested,
            thinking_requested: body
                .get("thinking")
                .and_then(Value::as_object)
                .and_then(|thinking| thinking.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "enabled"),
            tools_requested,
            structured_output_requested: false,
        },
        AppType::Codex => PassiveRequestMeta {
            stream_requested,
            thinking_requested: false,
            tools_requested,
            structured_output_requested: body
                .get("text")
                .and_then(Value::as_object)
                .and_then(|text| text.get("format"))
                .and_then(Value::as_object)
                .and_then(|format| format.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "text"),
        },
        _ => PassiveRequestMeta::default(),
    }
}

/// A finite, canonical fact set with at most one outcome for each evidence code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FiniteEvidenceFacts(Vec<EvidenceFact>);

impl FiniteEvidenceFacts {
    pub fn new(facts: impl IntoIterator<Item = EvidenceFact>) -> Self {
        let mut outcomes = [None; EvidenceCode::CARDINALITY];
        for fact in facts {
            Self::record(&mut outcomes, fact);
        }
        Self::from_outcomes(outcomes)
    }

    pub fn iter(&self) -> impl Iterator<Item = &EvidenceFact> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn record(
        outcomes: &mut [Option<EvidenceOutcome>; EvidenceCode::CARDINALITY],
        fact: EvidenceFact,
    ) {
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

    fn from_outcomes(outcomes: [Option<EvidenceOutcome>; EvidenceCode::CARDINALITY]) -> Self {
        Self(
            EvidenceCode::ALL
                .into_iter()
                .filter_map(|code| {
                    outcomes[code.index()].map(|outcome| EvidenceFact { code, outcome })
                })
                .collect(),
        )
    }
}

impl<'de> Deserialize<'de> for FiniteEvidenceFacts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FactsVisitor;

        impl<'de> Visitor<'de> for FactsVisitor {
            type Value = FiniteEvidenceFacts;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a sequence of evidence facts")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut outcomes = [None; EvidenceCode::CARDINALITY];
                while let Some(fact) = sequence.next_element()? {
                    FiniteEvidenceFacts::record(&mut outcomes, fact);
                }
                Ok(FiniteEvidenceFacts::from_outcomes(outcomes))
            }
        }

        deserializer.deserialize_seq(FactsVisitor)
    }
}

/// One finite observation produced after a proxied response has completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceBatch {
    pub target: TargetKey,
    pub generation: u64,
    pub completed: bool,
    pub facts: FiniteEvidenceFacts,
    pub observed_at: i64,
}

impl EvidenceBatch {
    pub fn new(
        target: TargetKey,
        generation: u64,
        completed: bool,
        facts: impl IntoIterator<Item = EvidenceFact>,
        observed_at: i64,
    ) -> Self {
        Self {
            target,
            generation,
            completed,
            facts: FiniteEvidenceFacts::new(facts),
            observed_at,
        }
    }
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
    for fact in batch.facts.iter() {
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

/// Merges passive state with the latest active report using the policy owned by `verdict`.
pub fn merge_report(
    active: Option<&VerificationReport>,
    passive: Option<&PassiveAggregate>,
) -> verdict::MergedReport {
    verdict::merge(active, passive)
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
    use crate::app_config::AppType;
    use crate::relay::model_verification::{
        passive::{
            merge_report, reduce_batch, reduce_request_meta, AnomalyFingerprint, EvidenceBatch,
            PassiveAggregate, PassiveRequestMeta, VerificationIngress, PASSIVE_INGRESS_CAPACITY,
        },
        types::{
            EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, TargetKey, Verdict,
            VerificationReport, RULES_VERSION,
        },
    };
    use tokio::sync::mpsc;

    fn batch(completed: bool, facts: Vec<EvidenceFact>) -> EvidenceBatch {
        EvidenceBatch::new(
            TargetKey::new("provider-a", "codex", "gpt-5.6-sol"),
            7,
            completed,
            facts,
            100,
        )
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
    fn evidence_batches_are_bounded_and_failures_win_during_construction_and_deserialization() {
        let facts = EvidenceCode::ALL.into_iter().flat_map(|code| {
            [
                passed(code),
                EvidenceFact {
                    code,
                    outcome: EvidenceOutcome::Skipped,
                },
                failed(code),
            ]
        });
        let batch = batch(true, facts.collect());
        assert_eq!(batch.facts.len(), EvidenceCode::CARDINALITY);
        assert!(batch
            .facts
            .iter()
            .all(|fact| fact.outcome == EvidenceOutcome::Failed));

        let restored: EvidenceBatch = serde_json::from_value(serde_json::json!({
            "target": {"providerId": "provider-a", "appType": "codex", "model": "gpt-5.6-sol"},
            "generation": 7,
            "completed": true,
            "facts": [
                {"code": "modelMatch", "outcome": "passed"},
                {"code": "modelMatch", "outcome": "failed"}
            ],
            "observedAt": 100
        }))
        .unwrap();
        assert_eq!(restored.facts.len(), 1);
        assert_eq!(
            restored.facts.iter().next().unwrap().outcome,
            EvidenceOutcome::Failed
        );
        assert_eq!(
            serde_json::to_value(&restored).unwrap()["facts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn merge_report_delegates_to_the_verdict_policy() {
        let active = VerificationReport {
            target: TargetKey::new("provider-a", "codex", "gpt-5.6-sol"),
            verdict: Verdict::Trusted,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts: vec![],
            rules_version: RULES_VERSION,
            checked_at: 100,
        };
        assert_eq!(merge_report(Some(&active), None).verdict, Verdict::Trusted);
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

    #[test]
    fn request_reduction_discards_prompt_tool_arguments_and_secrets() {
        let body = serde_json::json!({
            "model": "gpt-5.6-sol",
            "stream": true,
            "input": [{"role": "user", "content": "PROMPT_SENTINEL"}],
            "tools": [{"type": "function", "name": "tool", "parameters": {"secret": "ARG_SENTINEL"}}],
            "tool_choice": {"type": "function", "name": "tool"},
            "text": {"format": {"type": "json_schema", "name": "output", "schema": {"secret": "SCHEMA_SENTINEL"}}},
            "api_key": "SECRET_SENTINEL"
        });
        let metadata = reduce_request_meta(&AppType::Codex, &body);
        let serialized = serde_json::to_string(&metadata).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&serialized).unwrap(),
            serde_json::json!({
                "streamRequested": true,
                "thinkingRequested": false,
                "toolsRequested": true,
                "structuredOutputRequested": true,
            })
        );
        for sentinel in [
            "PROMPT_SENTINEL",
            "ARG_SENTINEL",
            "SCHEMA_SENTINEL",
            "SECRET_SENTINEL",
        ] {
            assert!(!serialized.contains(sentinel));
        }
    }

    #[test]
    fn unsupported_protocols_reduce_to_default_metadata() {
        let body = serde_json::json!({
            "stream": true,
            "thinking": {"type": "enabled"},
            "tools": [{"name": "tool"}],
            "text": {"format": {"type": "json_schema"}},
        });
        assert_eq!(
            reduce_request_meta(&AppType::Gemini, &body),
            PassiveRequestMeta::default()
        );
    }

    #[test]
    fn ingress_is_bounded_fail_open_and_generation_scoped() {
        let disabled = VerificationIngress::disabled();
        assert_eq!(disabled.capacity(), PASSIVE_INGRESS_CAPACITY);
        assert!(disabled
            .begin(TargetKey::new("provider", "codex", "model"))
            .is_none());

        let (sender, mut receiver) = mpsc::channel(PASSIVE_INGRESS_CAPACITY);
        let ingress = VerificationIngress::new(sender);
        ingress.set_enabled(true);
        let tap = ingress
            .begin(TargetKey::new("provider", "codex", "model"))
            .expect("enabled ingress should begin a supported tap");
        assert_eq!(ingress.capacity(), PASSIVE_INGRESS_CAPACITY);
        assert!(tap.submit(true, [], 1));
        let _ = receiver
            .try_recv()
            .expect("submitted batch should be readable");

        let batch =
            EvidenceBatch::new(TargetKey::new("provider", "codex", "model"), 0, true, [], 1);
        assert!(
            !ingress.try_submit(batch),
            "stale generation must be dropped"
        );
        ingress.set_enabled(false);
        assert!(!ingress.try_submit(EvidenceBatch::new(
            TargetKey::new("provider", "codex", "model"),
            1,
            true,
            [],
            1,
        )));
    }

    #[test]
    fn ingress_try_submit_drops_when_queue_is_full() {
        let (sender, _receiver) = mpsc::channel(PASSIVE_INGRESS_CAPACITY);
        let ingress = VerificationIngress::new(sender);
        ingress.set_enabled(true);
        for _ in 0..PASSIVE_INGRESS_CAPACITY {
            assert!(ingress.try_submit(EvidenceBatch::new(
                TargetKey::new("provider", "codex", "model"),
                1,
                true,
                [],
                1,
            )));
        }
        assert!(!ingress.try_submit(EvidenceBatch::new(
            TargetKey::new("provider", "codex", "model"),
            1,
            true,
            [],
            1,
        )));
    }
}
