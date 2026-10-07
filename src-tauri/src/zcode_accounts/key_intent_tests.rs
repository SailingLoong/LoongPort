use super::*;

fn scope(account: &str, project: &str) -> KeyScope {
    KeyScope::new(OAuthFamily::BigModel, account, "synthetic-org", project).unwrap()
}
fn fresh(ledger: &mut KeyIntentLedger, scope: KeyScope) -> FreshIntent {
    match ledger.reserve(scope).unwrap() {
        Reservation::Fresh(grant) => grant,
        Reservation::Existing(_) => panic!("unexpected old intent"),
    }
}

#[test]
fn persisted_intent_requires_readback_after_restart_not_another_creation_grant() {
    let vault = VaultContext::generate().unwrap();
    let mut ledger = KeyIntentLedger::default();
    let scope = scope("synthetic-user-a", "synthetic-project");
    let _grant = fresh(&mut ledger, scope.clone());
    let bytes = ledger.seal(&vault).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-user-a"));
    let mut reopened = KeyIntentLedger::open(&bytes, &vault).unwrap();
    let Reservation::Existing(record) = reopened.reserve(scope).unwrap() else {
        panic!("restart must not grant another POST")
    };
    assert_eq!(record.state, IntentState::Pending);
    assert!(KeyIntentLedger::open(&bytes, &VaultContext::generate().unwrap()).is_err());
}

#[test]
fn no_post_cleanup_is_exact_and_cannot_remove_created_or_other_intent() {
    let mut ledger = KeyIntentLedger::default();
    let a = scope("synthetic-user-a", "project-a");
    let b = scope("synthetic-user-b", "project-b");
    let grant_a = fresh(&mut ledger, a.clone());
    let grant_b = fresh(&mut ledger, b.clone());
    ledger.mark_created(&grant_a).unwrap();
    assert_eq!(ledger.clear_unsubmitted(&grant_a), Err(IntentError::Stale));
    ledger.clear_unsubmitted(&grant_b).unwrap();
    assert_eq!(ledger.get(&a).unwrap().state, IntentState::Created);
    assert!(ledger.get(&b).is_none());
    let old_record = ledger.get(&a).unwrap().clone();
    ledger.clear_resolved(&old_record).unwrap();
    let _new_grant = fresh(&mut ledger, a.clone());
    assert_eq!(ledger.clear_resolved(&old_record), Err(IntentError::Stale));
    assert!(ledger.get(&a).is_some());
}

#[test]
fn duplicate_or_unknown_fields_are_rejected_after_authentication() {
    let vault = VaultContext::generate().unwrap();
    let mut ledger = KeyIntentLedger::default();
    let _ = fresh(&mut ledger, scope("synthetic-user", "synthetic-project"));
    let mut value = serde_json::to_value(&ledger).unwrap();
    let duplicate = value["entries"][0].clone();
    value["entries"].as_array_mut().unwrap().push(duplicate);
    let file = OwnedFile::registered(KEY_INTENT_FILE).unwrap();
    let sealed = file
        .encode(&vault, &serde_json::to_vec(&value).unwrap())
        .unwrap();
    assert!(KeyIntentLedger::open(&sealed, &vault).is_err());
    value["entries"].as_array_mut().unwrap().pop();
    value["unexpected"] = serde_json::json!(true);
    let sealed = file
        .encode(&vault, &serde_json::to_vec(&value).unwrap())
        .unwrap();
    assert!(KeyIntentLedger::open(&sealed, &vault).is_err());
}

#[test]
fn intent_file_blocks_vault_change_and_reset_without_reading_secrets() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(KEY_INTENT_FILE),
        b"synthetic-interrupted-intent",
    )
    .unwrap();
    assert!(crate::secrets::owned_file::ensure_no_pending_zcode_transaction(root.path()).is_err());
    assert!(crate::secrets::owned_file::ensure_zcode_reset_allowed(root.path()).is_err());
    assert!(crate::secrets::owned_file::is_zcode_reset_archive_member(
        &format!("data/{KEY_INTENT_FILE}")
    ));
    assert!(!OwnedFile::registered(KEY_INTENT_FILE)
        .unwrap()
        .allows_legacy_plaintext());
}

#[test]
fn cache_identity_aliases_share_the_same_remote_project_intent() {
    let mut ledger = KeyIntentLedger::default();
    let original = scope("synthetic-cache-a", "same-personal-project");
    let alias = scope("synthetic-cache-b", "same-personal-project");
    let _ = fresh(&mut ledger, original);
    assert!(
        matches!(
            ledger.reserve(alias.clone()).unwrap(),
            Reservation::Existing(_)
        ),
        "a cache slot cannot grant a second POST to the same remote project"
    );
    assert!(ledger.get(&alias).is_some());
}
