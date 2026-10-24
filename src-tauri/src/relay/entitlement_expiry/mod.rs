//! Pure, opt-in account × final model × entitlement bucket projection.
//! Positive ranking hints only: no I/O, alias resolution, capability discovery or accounting.
//! Caller supplies only candidates already admitted by capability, permission,
//! account status and binding checks. All timestamps are Unix milliseconds.

// Retained integration vocabulary includes other platforms and manual callers.
#![allow(dead_code)]

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub account_id: String,
    pub final_model: String,
    pub bucket_id: String,
    pub evidence: Evidence,
}

/// Source metadata must describe the specific bucket, not an account aggregate.
/// `complete` records snapshot coverage; it is NOT required for a positive bucket hint.
/// Adapters must not infer completeness from a numeric total or nearest expiry.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub source: String,
    pub trusted: bool,
    pub observed_at_ms: Option<i64>,
    pub complete: bool,
    /// Exact final model IDs with verified entitlement ownership; None = unknown.
    pub applicable_models: Option<Vec<String>>,
    /// Native units; only finite positive consumption headroom is actionable.
    pub remaining: Option<f64>,
    /// Namespaced platform unit; never converted, added or used as a sort key.
    pub unit: Option<String>,
    pub expiry: Expiry,
    /// Extra loss events whose forfeiture semantics and ownership are proven.
    pub loss_events: Vec<VerifiedLossEvent>,
    pub remaining_precision: AmountPrecision,
    /// Observed concurrent quota limits for this account × model; None = unknown.
    /// They are AND constraints, never independently selectable buckets.
    pub concurrent_limits: Option<Vec<QuotaConstraint>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expiry {
    ConsumableEndsAtMs(i64),
    PlanEndsAtMs(i64),
    QuotaResetsAtMs(i64),
    AggregateNearestMs(i64),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossSource {
    CreditExpiry,
    NonRolloverReset,
    SubscriptionForfeiture,
}
/// Caller-attested evidence, not a cryptographic or type-level owner proof.
/// These public fields do not verify anything by themselves. The existing owner
/// must establish this bucket's ownership and forfeiture semantics before the
/// caller constructs an event; the kernel only validates the supplied facts.
/// Unknown owner observations must not be promoted into this type to get a rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedLossEvent {
    pub at_ms: i64,
    pub source: LossSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmountPrecision {
    Exact,
    DisplayRounded,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carryover {
    Discarded,
    Carried,
    Unknown,
}
#[derive(Clone, Debug, PartialEq)]
pub struct QuotaConstraint {
    pub window_id: String,
    pub remaining: Option<f64>,
    pub precision: AmountPrecision,
    pub reset_at_ms: Option<i64>,
    pub carryover: Carryover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    EarliestExpiry,
    FixedAccount,
    ManualOrder,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    Untrusted,
    Observation,
    Stale,
    Inapplicable,
    Exhausted,
    Expired,
    Applicability,
    Balance,
    Unit,
    Expiry,
    CycleRolledOver,
    ConcurrentLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    KnownExpiryMs(i64),
    Unknown(UnknownReason),
}
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedCandidate {
    pub candidate: Candidate,
    pub priority: Priority,
    pub loss_source: Option<LossSource>,
}

/// Unknown or unusable buckets lose only their ranking contribution. This function
/// never excludes an authorized account on the strength of one bucket. Admission
/// and whole-account exhaustion conclusions remain with the existing owners.
/// Stable key ordering preserves ties and the original order of unknowns.
pub fn project_and_order(
    candidates: Vec<Candidate>,
    allowed_final_models: &[String],
    policy: Policy,
    now_ms: i64,
    max_age_ms: i64,
) -> Vec<ProjectedCandidate> {
    let mut projected: Vec<_> = candidates
        .into_iter()
        .filter(|c| !c.final_model.is_empty() && allowed_final_models.contains(&c.final_model))
        .filter_map(|candidate| {
            priority(
                &candidate.evidence,
                &candidate.final_model,
                now_ms,
                max_age_ms,
            )
            .map(|(priority, loss_source)| ProjectedCandidate {
                candidate,
                priority,
                loss_source,
            })
        })
        .collect();
    if policy == Policy::EarliestExpiry {
        // Stable sort: equal expiry and all unknowns retain the caller's order.
        projected.sort_by_key(|c| match c.priority {
            Priority::KnownExpiryMs(expiry) => (0, expiry),
            Priority::Unknown(_) => (1, 0),
        });
    }
    projected
}

fn priority(
    e: &Evidence,
    model: &str,
    now: i64,
    max_age: i64,
) -> Option<(Priority, Option<LossSource>)> {
    let unknown = |reason| Some((Priority::Unknown(reason), None));
    if !e.trusted || e.source.trim().is_empty() {
        return unknown(UnknownReason::Untrusted);
    }
    let Some(observed) = e.observed_at_ms else {
        return unknown(UnknownReason::Observation);
    };
    if now < 0 || max_age < 0 || observed < 0 || observed > now {
        return unknown(UnknownReason::Observation);
    }
    if now - observed > max_age {
        return unknown(UnknownReason::Stale);
    }

    // Independent exclusion facts remain valid even when another quota window
    // has rolled over. Do not let that old window revive an incompatible bucket
    // or bypass a still-current concurrent limit.
    if e.applicable_models
        .as_ref()
        .is_some_and(|models| !models.iter().any(|m| m == model))
    {
        return unknown(UnknownReason::Inapplicable);
    }
    if e.concurrent_limits.iter().flatten().any(|window| {
        window.precision == AmountPrecision::Exact
            && window.remaining == Some(0.0)
            && window.reset_at_ms.is_some_and(|at| at > now)
    }) {
        return unknown(UnknownReason::ConcurrentLimits);
    }

    let mut losses = e.loss_events.clone();
    if let Expiry::ConsumableEndsAtMs(at_ms) = e.expiry {
        losses.insert(
            0,
            VerifiedLossEvent {
                at_ms,
                source: LossSource::CreditExpiry,
            },
        );
    }
    // Actual forfeiture ends this bucket; it says nothing about sibling buckets.
    if losses.iter().any(|event| {
        event.source != LossSource::NonRolloverReset && event.at_ms > 0 && event.at_ms <= now
    }) {
        return unknown(UnknownReason::Expired);
    }
    // A reset ends the old snapshot, not the account. After independent current
    // exclusions, invalidate old zeros so a refresh can restore their headroom.
    if losses.iter().any(|event| {
        event.source == LossSource::NonRolloverReset && event.at_ms > 0 && event.at_ms <= now
    }) || matches!(e.expiry, Expiry::QuotaResetsAtMs(at) if at > 0 && at <= now)
    {
        return unknown(UnknownReason::CycleRolledOver);
    }
    if e.remaining_precision == AmountPrecision::Exact && e.remaining == Some(0.0) {
        return unknown(UnknownReason::Exhausted);
    }
    if e.applicable_models.is_none() {
        return unknown(UnknownReason::Applicability);
    }
    if e.remaining_precision != AmountPrecision::Exact
        || !e.remaining.is_some_and(|v| v.is_finite() && v > 0.0)
    {
        return unknown(UnknownReason::Balance);
    }
    for window in e.concurrent_limits.iter().flatten() {
        if window.window_id.is_empty()
            || window.precision != AmountPrecision::Exact
            || !window.remaining.is_some_and(|v| v.is_finite() && v > 0.0)
            || window.reset_at_ms.is_some_and(|at| at <= now)
        {
            // Unknown/old constraints cannot negate independent positive facts.
            // A known, current exact zero was already handled above.
            continue;
        }
        if window.carryover == Carryover::Discarded {
            if let Some(at_ms) = window.reset_at_ms {
                losses.push(VerifiedLossEvent {
                    at_ms,
                    source: LossSource::NonRolloverReset,
                });
            }
        }
    }
    if e.unit.as_ref().is_none_or(|u| u.trim().is_empty()) {
        return unknown(UnknownReason::Unit);
    }
    if losses.iter().any(|event| event.at_ms <= 0) {
        return unknown(UnknownReason::Expiry);
    }
    // Same rule as tier_windows: choose the earliest proven loss, never compute
    // a window anchor here. Sources and units are preserved; amounts are not added.
    match losses
        .into_iter()
        .filter(|event| event.at_ms > now)
        .min_by_key(|event| event.at_ms)
    {
        Some(event) => Some((Priority::KnownExpiryMs(event.at_ms), Some(event.source))),
        None => unknown(UnknownReason::Expiry),
    }
}

#[cfg(test)]
mod tests;
