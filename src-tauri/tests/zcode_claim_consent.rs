//! Focused production consent owner tests; no GUI, network or real accounts.
#[allow(dead_code)]
#[path = "../src/zcode_accounts/claim.rs"]
mod claim;
use claim::Record;
#[path = "../src/zcode_accounts/claim_consent.rs"]
mod claim_consent;
use claim_consent::Scope;
use std::collections::BTreeSet;

#[test]
fn stale_grant_cannot_overwrite_newer_revocation() {
    let valid = BTreeSet::from(["a".to_owned()]);
    let mut scope = Scope::default();
    let revision = scope.revision("context");
    scope
        .update("context", &revision, true, vec!["a".into()], &valid)
        .unwrap();
    let stale = scope.revision("context");
    scope
        .update("context", &stale, false, vec![], &valid)
        .unwrap();
    let revoked = serde_json::to_value(&scope).unwrap();
    assert_eq!(
        scope.update("context", &stale, true, vec!["a".into()], &valid),
        Err("changed")
    );
    assert_eq!(serde_json::to_value(&scope).unwrap(), revoked);
}

#[test]
fn stale_participant_edit_and_cross_context_grant_fail_without_changes() {
    let valid = BTreeSet::from(["a".into(), "b".into()]);
    let mut scope = Scope::default();
    let initial = scope.revision("context");
    scope
        .update(
            "context",
            &initial,
            true,
            vec!["a".into(), "b".into()],
            &valid,
        )
        .unwrap();
    let stale = scope.revision("context");
    scope
        .update("context", &stale, true, vec!["b".into()], &valid)
        .unwrap();
    let before = serde_json::to_value(&scope).unwrap();
    for expected in [stale, String::new(), scope.revision("other-context")] {
        assert_eq!(
            scope.update("context", &expected, true, vec!["a".into()], &valid),
            Err("changed")
        );
        assert_eq!(serde_json::to_value(&scope).unwrap(), before);
    }
}

#[test]
fn legacy_records_roundtrip_and_consent_updates_preserve_pending_receipts() {
    let mut scope: Scope = serde_json::from_value(serde_json::json!({
        "dataRoot": "synthetic", "enabled": false, "participants": [], "records": {}
    }))
    .unwrap();
    assert_eq!(scope.revision("context"), "context:0");
    let mut pending = Record::default();
    pending.mark_submitted();
    scope.records.insert("a".into(), pending);
    let valid = BTreeSet::from(["a".into()]);
    for enabled in [true, false, true] {
        let before = scope.revision("context");
        scope
            .update("context", &before, enabled, vec!["a".into()], &valid)
            .unwrap();
        assert_ne!(scope.revision("context"), before);
        assert!(!scope.records["a"].may_submit());
        let raw = serde_json::to_string(&scope).unwrap();
        scope = serde_json::from_str(&raw).unwrap();
        assert!(!scope.records["a"].may_submit());
    }
}

#[test]
fn invalid_participants_and_exhausted_revision_never_mutate_consent() {
    let mut scope = Scope::default();
    let valid = BTreeSet::from(["a".into()]);
    for ids in [
        vec!["missing".into()],
        vec!["a".into(), "a".into()],
        vec!["a".into(); 101],
    ] {
        let before = serde_json::to_value(&scope).unwrap();
        assert!(scope
            .update("context", &scope.revision("context"), true, ids, &valid)
            .is_err());
        assert_eq!(serde_json::to_value(&scope).unwrap(), before);
    }
    let mut raw = serde_json::to_value(&scope).unwrap();
    raw["consentRevision"] = serde_json::json!(u64::MAX);
    scope = serde_json::from_value(raw).unwrap();
    assert!(scope
        .update(
            "context",
            &scope.revision("context"),
            true,
            vec!["a".into()],
            &valid
        )
        .is_err());
    assert!(!scope.enabled);
}
