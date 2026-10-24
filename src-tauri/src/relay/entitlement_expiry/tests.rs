use super::*;
pub(crate) fn evidence(at: i64) -> Evidence {
    Evidence {
        source: "synthetic authoritative bucket".into(),
        trusted: true,
        observed_at_ms: Some(100),
        complete: false,
        applicable_models: Some(vec!["final-model".into()]),
        remaining: Some(42.0),
        unit: Some("synthetic:credits".into()),
        expiry: Expiry::ConsumableEndsAtMs(at),
        loss_events: vec![],
        remaining_precision: AmountPrecision::Exact,
        concurrent_limits: None,
    }
}
fn candidate(id: &str, at: i64) -> Candidate {
    Candidate {
        account_id: id.into(),
        final_model: "final-model".into(),
        bucket_id: "bucket".into(),
        evidence: evidence(at),
    }
}
#[test]
fn partial_account_snapshot_does_not_erase_independently_verified_bucket_hint() {
    let ranked = project_and_order(
        vec![candidate("a", 300), candidate("b", 200)],
        &["final-model".into()],
        Policy::EarliestExpiry,
        100,
        50,
    );
    assert_eq!(
        ranked
            .iter()
            .map(|c| c.candidate.account_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(ranked[0].priority, Priority::KnownExpiryMs(200));
}
#[test]
fn expired_or_exhausted_bucket_never_removes_an_authorized_candidate() {
    let mut a = candidate("a", 99);
    a.evidence.complete = true;
    let mut b = candidate("b", 300);
    b.evidence.complete = true;
    b.evidence.remaining = Some(0.0);
    let ranked = project_and_order(
        vec![a, b],
        &["final-model".into()],
        Policy::EarliestExpiry,
        100,
        50,
    );
    assert_eq!(ranked.len(), 2);
    assert!(ranked
        .iter()
        .all(|c| matches!(c.priority, Priority::Unknown(_))));
}
#[test]
fn stable_ties_unknowns_units_and_explicit_manual_policies() {
    let mut u = candidate("u", 200);
    u.evidence.trusted = false;
    let a = candidate("a", 300);
    let mut b = candidate("b", 300);
    b.evidence.unit = Some("other-platform:tokens".into());
    let mut stale = candidate("stale", 150);
    stale.evidence.observed_at_ms = Some(1);
    let input = vec![u, a, b, stale];
    for policy in [Policy::ManualOrder, Policy::FixedAccount] {
        let out = project_and_order(input.clone(), &["final-model".into()], policy, 100, 50);
        assert_eq!(
            out.iter()
                .map(|c| c.candidate.account_id.as_str())
                .collect::<Vec<_>>(),
            ["u", "a", "b", "stale"]
        );
    }
    let out = project_and_order(
        input,
        &["final-model".into()],
        Policy::EarliestExpiry,
        100,
        50,
    );
    assert_eq!(
        out.iter()
            .map(|c| c.candidate.account_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "u", "stale"]
    );
}
#[test]
fn known_reset_credit_and_subscription_losses_share_earliest_stable_order() {
    let mut reset = candidate("reset", 400);
    reset.evidence.loss_events.push(VerifiedLossEvent {
        at_ms: 200,
        source: LossSource::NonRolloverReset,
    });
    let credit = candidate("credit", 200);
    let mut sub = candidate("sub", 500);
    sub.evidence.loss_events.push(VerifiedLossEvent {
        at_ms: 150,
        source: LossSource::SubscriptionForfeiture,
    });
    let out = project_and_order(
        vec![reset, credit, sub],
        &["final-model".into()],
        Policy::EarliestExpiry,
        100,
        50,
    );
    assert_eq!(
        out.iter()
            .map(|c| c.candidate.account_id.as_str())
            .collect::<Vec<_>>(),
        ["sub", "reset", "credit"]
    );
    assert_eq!(out[1].loss_source, Some(LossSource::NonRolloverReset));
}
#[test]
fn unproven_reset_plan_and_aggregate_dates_do_not_become_loss_events() {
    for expiry in [
        Expiry::PlanEndsAtMs(200),
        Expiry::QuotaResetsAtMs(200),
        Expiry::AggregateNearestMs(200),
    ] {
        let mut c = candidate("a", 200);
        c.evidence.expiry = expiry;
        assert!(matches!(
            priority(&c.evidence, "final-model", 100, 50).unwrap().0,
            Priority::Unknown(_)
        ));
    }
}
#[test]
fn current_zero_parallel_constraint_cancels_hint_but_old_window_does_not_ban_account() {
    let mut c = candidate("a", 500);
    let window = |at: i64, amount: f64, carryover: Carryover| QuotaConstraint {
        window_id: at.to_string(),
        remaining: Some(amount),
        precision: AmountPrecision::Exact,
        reset_at_ms: Some(at),
        carryover,
    };
    c.evidence.concurrent_limits = Some(vec![
        window(100, 0.0, Carryover::Discarded),
        window(300, 0.0, Carryover::Discarded),
    ]);
    assert!(matches!(
        priority(&c.evidence, "final-model", 100, 50).unwrap().0,
        Priority::Unknown(_)
    ));
    c.evidence.concurrent_limits.as_mut().unwrap()[1].remaining = Some(3.0);
    assert_eq!(
        priority(&c.evidence, "final-model", 100, 50).unwrap().0,
        Priority::KnownExpiryMs(300)
    );
    for carryover in [Carryover::Carried, Carryover::Unknown] {
        c.evidence.concurrent_limits = Some(vec![window(200, 3.0, carryover)]);
        assert_eq!(
            priority(&c.evidence, "final-model", 100, 50).unwrap().0,
            Priority::KnownExpiryMs(500)
        );
    }
    c.evidence.loss_events.push(VerifiedLossEvent {
        at_ms: 100,
        source: LossSource::NonRolloverReset,
    });
    assert_eq!(
        priority(&c.evidence, "final-model", 100, 50).unwrap().0,
        Priority::Unknown(UnknownReason::CycleRolledOver)
    );
}
#[test]
fn allowed_final_models_are_exact_and_never_inferred_from_aliases() {
    let a = candidate("a", 300);
    let mut b = candidate("b", 200);
    b.final_model = "second".into();
    b.evidence.applicable_models = Some(vec!["second".into()]);
    let out = project_and_order(
        vec![a.clone(), b.clone()],
        &["final-model".into(), "second".into()],
        Policy::EarliestExpiry,
        100,
        50,
    );
    assert_eq!(out[0].candidate.account_id, "b");
    let out = project_and_order(
        vec![a, b],
        &["final-model".into()],
        Policy::EarliestExpiry,
        100,
        50,
    );
    assert_eq!(out.len(), 1);
    let mut c = candidate("alias", 200);
    c.evidence.applicable_models = Some(vec!["client-alias".into()]);
    assert!(matches!(
        priority(&c.evidence, "final-model", 100, 50).unwrap().0,
        Priority::Unknown(_)
    ));
    c.evidence.applicable_models = Some(vec!["final-model".into()]);
    c.evidence.remaining_precision = AmountPrecision::DisplayRounded;
    c.evidence.remaining = Some(0.0);
    assert_eq!(
        priority(&c.evidence, "final-model", 100, 50).unwrap().0,
        Priority::Unknown(UnknownReason::Balance)
    );
}
