use super::super::core::OAuthFamily;
use super::super::native::tests::replace;
use super::super::native::tests::{native_document, TEST_CONTEXT, TEST_SECRET};
use super::*;

fn native() -> NativeCipher {
    NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap()
}

fn fixture(
    vault: &VaultContext,
    name: &str,
    phase: TransactionPhase,
) -> (JournalEvidence, CredentialDocument, CredentialDocument) {
    let native = native();
    let current = native_document(OAuthFamily::Zai, "source", "fresh");
    let target = native
        .inspect(&native_document(OAuthFamily::Zai, name, "saved"))
        .unwrap();
    let mut checkpoint = SwitchCheckpoint::prepare(&current, &target, &native).unwrap();
    checkpoint
        .bind(TransactionBinding {
            operation: uuid::Uuid::new_v4().to_string(),
            native_root: [1, 2],
            vault_root: [3, 4],
            source_revision: [5; 32],
            source_profile_revision: [6; 32],
        })
        .unwrap();
    let after = checkpoint.apply(&current).unwrap();
    checkpoint.set_phase(phase).unwrap();
    (
        JournalEvidence::open_journal(&checkpoint.seal(vault, &native).unwrap(), vault).unwrap(),
        current,
        after,
    )
}

fn evidence(vault: &VaultContext, name: &str, phase: TransactionPhase) -> JournalEvidence {
    fixture(vault, name, phase).0
}

#[test]
fn recovery_archives_exact_authenticated_bytes_without_native_key() {
    let vault = VaultContext::generate().unwrap();
    let original = evidence(&vault, "target", TransactionPhase::Prepared);
    let raw = original.raw_payload().to_vec();
    let id = original.id().to_owned();
    let binding = original.binding().unwrap().clone();
    assert_eq!(original.context(), TEST_CONTEXT);
    assert_eq!(original.phase(), TransactionPhase::Prepared);
    assert!(original
        .native_checkpoint(&NativeCipher::new(TEST_CONTEXT, "wrong").unwrap())
        .is_err());
    let mut ledger = RecoveryLedger::default();
    assert_eq!(ledger.archive(original).unwrap(), id);
    assert!(ledger.needs_confirmation());
    let encoded = ledger.seal(&vault).unwrap();
    assert!(!encoded.contains(TEST_CONTEXT));
    let restored = RecoveryLedger::open(&encoded, &vault).unwrap();
    assert!(restored.needs_confirmation());
    let archived = restored.archived().next().unwrap();
    assert_eq!(archived.evidence().raw_payload(), raw);
    assert_eq!(archived.evidence().id(), id);
    assert!(archived.evidence().binding() == Some(&binding));
    assert_eq!(archived.disposition(), DispositionKind::NativeUnconfirmed);
    assert!(archived.evidence().native_checkpoint(&native()).is_ok());
}

#[test]
fn recovery_keeps_latest_and_two_archives_without_replacing_different_evidence() {
    let vault = VaultContext::generate().unwrap();
    let completed = evidence(&vault, "completed", TransactionPhase::Committed);
    let completed_id = completed.id().to_owned();
    let mut ledger = RecoveryLedger::default();
    ledger.record_completion(completed).unwrap();
    let first = evidence(&vault, "first", TransactionPhase::Captured);
    let first_id = first.id().to_owned();
    let first_raw = first.raw_payload().to_vec();
    ledger.archive(first).unwrap();
    let second = evidence(&vault, "second", TransactionPhase::CredentialsPublished);
    let second_id = second.id().to_owned();
    ledger.archive(second).unwrap();
    assert_eq!(ledger.archived().count(), 2);
    assert_eq!(
        ledger.latest_completed().unwrap().evidence().id(),
        completed_id
    );
    assert!(matches!(
        ledger.archive(evidence(&vault, "third", TransactionPhase::Prepared)),
        Err(RecoveryError::ArchiveFull)
    ));
    let duplicate = vault
        .seal(&["file", JOURNAL_FILE, "content"], &first_raw)
        .unwrap();
    ledger
        .archive(JournalEvidence::open_journal(&duplicate, &vault).unwrap())
        .unwrap();
    assert_eq!(ledger.archived().count(), 2);
    assert_eq!(
        ledger
            .archived()
            .map(|r| r.evidence().id())
            .collect::<Vec<_>>(),
        [first_id, second_id]
    );
}

#[test]
fn recovery_rejects_wrong_aad_key_schema_phase_and_duplicate_entries() {
    let vault = VaultContext::generate().unwrap();
    let item = evidence(&vault, "target", TransactionPhase::Committed);
    let journal = vault
        .seal(&["file", JOURNAL_FILE, "content"], item.raw_payload())
        .unwrap();
    assert!(RecoveryLedger::open(&journal, &vault).is_err());
    assert!(JournalEvidence::open_journal(&journal, &VaultContext::generate().unwrap()).is_err());
    let old_layout = vault
        .seal(&["file", RECOVERY_FILE, "content"], item.raw_payload())
        .unwrap();
    assert!(RecoveryLedger::open(&old_layout, &vault).is_err());
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("phase", serde_json::json!("future")),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut payload: serde_json::Value = serde_json::from_slice(item.raw_payload()).unwrap();
        payload[field] = value;
        let encoded = vault
            .seal(
                &["file", JOURNAL_FILE, "content"],
                &serde_json::to_vec(&payload).unwrap(),
            )
            .unwrap();
        assert!(JournalEvidence::open_journal(&encoded, &vault).is_err());
    }
    let mut ledger = RecoveryLedger::default();
    ledger.archive(item).unwrap();
    let encoded = ledger.seal(&vault).unwrap();
    assert!(JournalEvidence::open_journal(&encoded, &vault).is_err());
    assert!(RecoveryLedger::open(&encoded, &VaultContext::generate().unwrap()).is_err());
    let raw = vault
        .open(&["file", RECOVERY_FILE, "content"], &encoded)
        .unwrap();
    let mut payload: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let duplicate = payload["archived"][0].clone();
    payload["archived"].as_array_mut().unwrap().push(duplicate);
    let encoded = vault
        .seal(
            &["file", RECOVERY_FILE, "content"],
            &serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        RecoveryLedger::open(&encoded, &vault),
        Err(RecoveryError::DuplicateRecord)
    ));
}

#[test]
fn recovery_confirmation_requires_an_entire_image_and_preserves_original_evidence() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let (item, before, after) = fixture(&vault, "target", TransactionPhase::Captured);
    let raw = item.raw_payload().to_vec();
    let id = item.id().to_owned();
    let key = native.inspect(&after).unwrap().identity().credential_keys()[1].clone();
    let mixed = replace(&after, &key, before.get(&key).unwrap().into());
    assert!(RecoveryConfirmation::from_image(&item, &mixed, &native).is_err());
    let confirmation = RecoveryConfirmation::from_image(&item, &before, &native).unwrap();
    let mut ledger = RecoveryLedger::default();
    ledger.archive(item).unwrap();
    ledger.confirm(&id, confirmation).unwrap();
    assert!(!ledger.needs_confirmation());
    assert_eq!(
        ledger.archived().next().unwrap().disposition(),
        DispositionKind::FullBefore
    );
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        raw
    );
    let ledger = RecoveryLedger::open(&ledger.seal(&vault).unwrap(), &vault).unwrap();
    let item = ledger.archived().next().unwrap().evidence();
    let confirmation = RecoveryConfirmation::from_image(item, &after, &native).unwrap();
    let mut ledger = ledger;
    ledger.confirm(&id, confirmation).unwrap();
    assert_eq!(
        ledger.archived().next().unwrap().disposition(),
        DispositionKind::FullAfter
    );
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        raw
    );
}

#[test]
fn recovery_confirmation_is_bound_to_selected_evidence_and_fresh_persisted_capture() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let (item, before, _) = fixture(&vault, "target", TransactionPhase::Prepared);
    let (other, _, _) = fixture(&vault, "other", TransactionPhase::Prepared);
    let id = item.id().to_owned();
    let other_id = other.id().to_owned();
    let raw = item.raw_payload().to_vec();
    let confirmation = RecoveryConfirmation::from_image(&item, &before, &native).unwrap();
    let fresh = native_document(OAuthFamily::Zai, "official-c", "relogged-in");
    let persisted = native.inspect(&fresh).unwrap();
    let old = native.inspect(&before).unwrap();
    assert!(RecoveryConfirmation::after_persisted_capture(
        &item, &fresh, &old, &native, [9; 32], [8; 32]
    )
    .is_err());
    assert!(RecoveryConfirmation::after_persisted_capture(
        &item,
        &fresh,
        &persisted,
        &NativeCipher::new(TEST_CONTEXT, "wrong-key").unwrap(),
        [9; 32],
        [8; 32]
    )
    .is_err());
    let capture = RecoveryConfirmation::after_persisted_capture(
        &item, &fresh, &persisted, &native, [9; 32], [8; 32],
    )
    .unwrap();
    let mut ledger = RecoveryLedger::default();
    ledger.archive(item).unwrap();
    ledger.archive(other).unwrap();
    assert!(matches!(
        ledger.confirm(&other_id, confirmation),
        Err(RecoveryError::ConfirmationMismatch)
    ));
    assert!(ledger.needs_confirmation());
    ledger.confirm(&id, capture).unwrap();
    let reopened = RecoveryLedger::open(&ledger.seal(&vault).unwrap(), &vault).unwrap();
    let first = reopened.archived().next().unwrap();
    assert_eq!(first.disposition(), DispositionKind::ExplicitCapture);
    assert_eq!(first.evidence().raw_payload(), raw);
    match &first.disposition {
        Disposition::ExplicitCapture {
            profile_id,
            snapshot_revision,
            profile_revision,
            native_revision,
        } => {
            assert_eq!(profile_id, &persisted.identity().opaque_id());
            assert_eq!(
                snapshot_revision,
                &<[u8; 32]>::from(Sha256::digest(
                    persisted.scoped_document().to_bytes().unwrap()
                ))
            );
            assert_eq!(profile_revision, &[9; 32]);
            assert_eq!(native_revision, &[8; 32]);
        }
        _ => panic!("fresh capture proof was not retained"),
    }
    assert!(reopened.needs_confirmation());
}

#[test]
fn recovery_reappearing_journal_revokes_old_confirmation_without_duplicating_payload() {
    let vault = VaultContext::generate().unwrap();
    for latest in [false, true] {
        let (item, before, _) = fixture(&vault, "target", TransactionPhase::Committed);
        let raw = item.raw_payload().to_vec();
        let id = item.id().to_owned();
        let mut ledger = RecoveryLedger::default();
        if latest {
            ledger.record_completion(item).unwrap();
        } else {
            let confirmation = RecoveryConfirmation::from_image(&item, &before, &native()).unwrap();
            ledger.archive(item).unwrap();
            ledger.confirm(&id, confirmation).unwrap();
        }
        assert!(!ledger.needs_confirmation());
        let encoded = vault
            .seal(&["file", JOURNAL_FILE, "content"], &raw)
            .unwrap();
        ledger
            .archive(JournalEvidence::open_journal(&encoded, &vault).unwrap())
            .unwrap();
        assert!(ledger.needs_confirmation());
        assert_eq!(ledger.archived().count(), usize::from(!latest));
        assert!(matches!(
            ledger.record_completion(evidence(&vault, "new", TransactionPhase::Committed)),
            Err(RecoveryError::Unconfirmed)
        ));
        let record = if latest {
            ledger.latest_completed().unwrap()
        } else {
            ledger.archived().next().unwrap()
        };
        assert_eq!(record.evidence().raw_payload(), raw);
    }
}

#[test]
fn recovery_deletes_only_selected_confirmed_record_and_completion_preserves_archives() {
    let vault = VaultContext::generate().unwrap();
    let (item, before, _) = fixture(&vault, "archive", TransactionPhase::Prepared);
    let id = item.id().to_owned();
    let raw = item.raw_payload().to_vec();
    let confirmation = RecoveryConfirmation::from_image(&item, &before, &native()).unwrap();
    let mut ledger = RecoveryLedger::default();
    ledger.archive(item).unwrap();
    assert!(matches!(
        ledger.delete_confirmed(&id),
        Err(RecoveryError::Unconfirmed)
    ));
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        raw
    );
    ledger.confirm(&id, confirmation).unwrap();
    ledger
        .record_completion(evidence(
            &vault,
            "completed-first",
            TransactionPhase::Committed,
        ))
        .unwrap();
    let next = evidence(&vault, "completed-next", TransactionPhase::Committed);
    let latest_id = next.id().to_owned();
    ledger.record_completion(next).unwrap();
    assert_eq!(
        ledger.latest_completed().unwrap().evidence().id(),
        latest_id
    );
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        raw
    );
    assert!(matches!(
        ledger.delete_confirmed("missing"),
        Err(RecoveryError::NotFound)
    ));
    ledger.delete_confirmed(&latest_id).unwrap();
    assert!(ledger.latest_completed().is_none());
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        raw
    );
    ledger.delete_confirmed(&id).unwrap();
    assert!(ledger.is_empty());
    assert!(!ledger.needs_confirmation());
}

fn reauthenticate(vault: &VaultContext, raw: &[u8]) -> JournalEvidence {
    let encoded = vault.seal(&["file", JOURNAL_FILE, "content"], raw).unwrap();
    JournalEvidence::open_journal(&encoded, vault).unwrap()
}

#[test]
fn recovery_capacity_reserves_real_later_phases_and_confirmation_metadata_without_mutation() {
    use super::super::checkpoint::MAX_PAYLOAD_BYTES;
    let vault = VaultContext::generate().unwrap();
    let completed = evidence(&vault, "previous", TransactionPhase::Committed);
    let candidate = evidence(&vault, "candidate", TransactionPhase::Prepared);
    let mut ledger = RecoveryLedger::default();
    ledger.record_completion(completed.clone()).unwrap();
    ledger.ensure_switch_capacity(&candidate, &vault).unwrap();
    assert_eq!(
        ledger.latest_completed().unwrap().evidence().raw_payload(),
        completed.raw_payload()
    );
    assert_eq!(ledger.archived().count(), 0);
    let mut direct_archive = ledger.clone();
    direct_archive.archive(candidate.clone()).unwrap();
    let plain_size = serde_json::to_vec(&direct_archive.payload()).unwrap().len();
    let padding = MAX_PAYLOAD_BYTES - plain_size;
    let mut padded_raw = completed.raw_payload().to_vec();
    padded_raw.extend(std::iter::repeat_n(b' ', padding));
    let completed = reauthenticate(&vault, &padded_raw);
    let mut full = RecoveryLedger::default();
    full.record_completion(completed).unwrap();
    let mut exactly_fits = full.clone();
    exactly_fits.archived.push(RecoveryRecord {
        evidence: candidate.clone(),
        disposition: Disposition::NativeUnconfirmed {},
    });
    assert_eq!(
        serde_json::to_vec(&exactly_fits.payload()).unwrap().len(),
        MAX_PAYLOAD_BYTES
    );
    // PREPARED alone fits. Archive admission must reserve room to confirm it too.
    assert!(matches!(
        full.archive(candidate.clone()),
        Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
    ));
    // A later phase and its largest confirmation proof must also fit.
    assert!(matches!(
        full.ensure_switch_capacity(&candidate, &vault),
        Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
    ));
    assert!(full.latest_completed().unwrap().evidence().raw_payload() == padded_raw);
    assert_eq!(full.archived().count(), 0);
}

#[test]
fn recovery_capacity_refuses_full_slots_unconfirmed_state_and_unbound_candidate() {
    let vault = VaultContext::generate().unwrap();
    let candidate = evidence(&vault, "candidate", TransactionPhase::Prepared);
    let mut ledger = RecoveryLedger::default();
    for name in ["first", "second"] {
        let (item, before, _) = fixture(&vault, name, TransactionPhase::Captured);
        let id = item.id().to_owned();
        let confirmation = RecoveryConfirmation::from_image(&item, &before, &native()).unwrap();
        ledger.archive(item).unwrap();
        assert!(matches!(
            ledger.ensure_switch_capacity(&candidate, &vault),
            Err(RecoveryError::Unconfirmed)
        ));
        ledger.confirm(&id, confirmation).unwrap();
    }
    assert!(matches!(
        ledger.ensure_switch_capacity(&candidate, &vault),
        Err(RecoveryError::ArchiveFull)
    ));
    assert_eq!(ledger.archived().count(), 2);
    let mut payload: serde_json::Value = serde_json::from_slice(candidate.raw_payload()).unwrap();
    payload["binding"] = serde_json::Value::Null;
    let unbound = reauthenticate(&vault, &serde_json::to_vec(&payload).unwrap());
    assert!(matches!(
        RecoveryLedger::default().ensure_switch_capacity(&unbound, &vault),
        Err(RecoveryError::Checkpoint(CheckpointError::InvalidPhase))
    ));
}

#[test]
fn recovery_total_plaintext_limit_is_atomic_and_distinct_from_each_journal_limit() {
    use super::super::checkpoint::{MAX_ENVELOPE_BYTES, MAX_PAYLOAD_BYTES};
    let vault = VaultContext::generate().unwrap();
    let mut ledger = RecoveryLedger::default();
    let mut original = Vec::new();
    for name in ["first", "second", "third"] {
        let item = evidence(&vault, name, TransactionPhase::Committed);
        let mut padded = item.raw_payload().to_vec();
        padded.resize(3 * 1024 * 1024, b' ');
        let item = reauthenticate(&vault, &padded);
        if name == "first" {
            ledger.record_completion(item).unwrap();
        } else if name == "second" {
            ledger.archive(item).unwrap();
            original = ledger
                .archived()
                .next()
                .unwrap()
                .evidence()
                .raw_payload()
                .to_vec();
        } else {
            assert!(matches!(
                ledger.archive(item),
                Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
            ));
        }
    }
    assert_eq!(ledger.archived().count(), 1);
    assert!(ledger.archived().next().unwrap().evidence().raw_payload() == original);
    assert!(RecoveryLedger::open(&ledger.seal(&vault).unwrap(), &vault).is_ok());
    let oversized = "x".repeat(MAX_ENVELOPE_BYTES + 1);
    assert!(matches!(
        RecoveryLedger::open(&oversized, &vault),
        Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
    ));
    assert!(matches!(
        JournalEvidence::open_journal(&oversized, &vault),
        Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
    ));
    let oversized_plain = vec![b' '; MAX_PAYLOAD_BYTES + 1];
    let encoded = vault
        .seal(&["file", RECOVERY_FILE, "content"], &oversized_plain)
        .unwrap();
    assert!(matches!(
        RecoveryLedger::open(&encoded, &vault),
        Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
    ));
}

#[test]
fn recovery_strict_parsing_rejects_duplicate_fields_unknown_dispositions_and_invalid_proofs() {
    let vault = VaultContext::generate().unwrap();
    let item = evidence(&vault, "target", TransactionPhase::Prepared);
    let raw = std::str::from_utf8(item.raw_payload()).unwrap();
    for changed in [
        raw.replacen("{", "{\"version\":1,", 1),
        raw.replacen(
            "\"source\":{",
            "\"source\":{\"oauth:active_provider\":\"duplicated\",",
            1,
        ),
        raw.replacen(
            "\"before\":{",
            "\"before\":{\"oauth:active_provider\":null,",
            1,
        ),
    ] {
        let encoded = vault
            .seal(&["file", JOURNAL_FILE, "content"], changed.as_bytes())
            .unwrap();
        assert!(JournalEvidence::open_journal(&encoded, &vault).is_err());
    }
    let mut ledger = RecoveryLedger::default();
    ledger.archive(item).unwrap();
    let original = serde_json::to_value(ledger.payload()).unwrap();
    for (case, disposition) in [
        ("unknown kind", serde_json::json!({"kind":"future"})),
        (
            "extra field",
            serde_json::json!({"kind":"native-unconfirmed", "confirmed":true}),
        ),
        (
            "invalid opaque ID",
            serde_json::json!({"kind":"explicit-capture", "profile_id":"not-an-opaque-id", "snapshot_revision":vec![0;32], "profile_revision":vec![0;32], "native_revision":vec![0;32]}),
        ),
    ] {
        let mut payload = original.clone();
        payload["archived"][0]["disposition"] = disposition;
        let encoded = vault
            .seal(
                &["file", RECOVERY_FILE, "content"],
                &serde_json::to_vec(&payload).unwrap(),
            )
            .unwrap();
        assert!(
            RecoveryLedger::open(&encoded, &vault).is_err(),
            "accepted {case}"
        );
    }
    for field in ["version", "archived", "latest_completed"] {
        let raw = serde_json::to_string(&original).unwrap();
        let extra = format!("{{\"{field}\":{},", original[field]);
        let changed = raw.replacen('{', &extra, 1);
        let encoded = vault
            .seal(&["file", RECOVERY_FILE, "content"], changed.as_bytes())
            .unwrap();
        assert!(RecoveryLedger::open(&encoded, &vault).is_err());
    }
}

#[test]
fn recovery_preserves_supported_native_invalid_evidence_and_does_not_dedup_by_operation_uuid() {
    let vault = VaultContext::generate().unwrap();
    let first = evidence(&vault, "target", TransactionPhase::Captured);
    let mut payload: serde_json::Value = serde_json::from_slice(first.raw_payload()).unwrap();
    payload["target"]["oauth:zai:access_token"] = "unreadable-native-value".into();
    let altered_raw = serde_json::to_vec_pretty(&payload).unwrap();
    let second = reauthenticate(&vault, &altered_raw);
    assert!(first.binding() == second.binding());
    assert_ne!(first.id(), second.id());
    assert!(second.native_checkpoint(&native()).is_err());
    let mut ledger = RecoveryLedger::default();
    ledger.archive(first).unwrap();
    ledger.archive(second).unwrap();
    let reopened = RecoveryLedger::open(&ledger.seal(&vault).unwrap(), &vault).unwrap();
    assert_eq!(reopened.archived().count(), 2);
    assert!(reopened.archived().last().unwrap().evidence().raw_payload() == altered_raw);
}

#[test]
fn recovery_phase_growth_reservation_is_required_even_when_prepared_and_confirmation_fit() {
    use super::super::checkpoint::MAX_PAYLOAD_BYTES;
    let vault = VaultContext::generate().unwrap();
    let completed = evidence(&vault, "previous", TransactionPhase::Committed);
    let candidate = evidence(&vault, "candidate", TransactionPhase::Prepared);
    let mut ledger = RecoveryLedger::default();
    ledger.record_completion(completed.clone()).unwrap();
    ledger.archive(candidate.clone()).unwrap();
    let max_prepared = serde_json::to_vec(&ledger.with_largest_confirmations().payload())
        .unwrap()
        .len();
    let mut raw = completed.raw_payload().to_vec();
    raw.resize(raw.len() + MAX_PAYLOAD_BYTES - max_prepared, b' ');
    let mut near_limit = RecoveryLedger::default();
    near_limit
        .record_completion(reauthenticate(&vault, &raw))
        .unwrap();
    let mut prepared_fits = near_limit.clone();
    prepared_fits.archive(candidate.clone()).unwrap();
    assert_eq!(
        serde_json::to_vec(&prepared_fits.with_largest_confirmations().payload())
            .unwrap()
            .len(),
        MAX_PAYLOAD_BYTES
    );
    assert!(matches!(
        near_limit.ensure_switch_capacity(&candidate, &vault),
        Err(RecoveryError::Checkpoint(CheckpointError::ResourceLimit))
    ));
    assert_eq!(near_limit.archived().count(), 0);
}

#[test]
fn recovery_dedup_requires_exact_bytes_even_if_an_evidence_digest_collides() {
    let vault = VaultContext::generate().unwrap();
    let original = evidence(&vault, "original", TransactionPhase::Committed);
    let raw = original.raw_payload().to_vec();
    let id = original.id().to_owned();
    let mut collision = evidence(&vault, "different", TransactionPhase::Committed);
    // Simulate a digest collision without changing either authenticated payload.
    collision.id = id;
    let mut ledger = RecoveryLedger::default();
    ledger.record_completion(original).unwrap();
    assert!(matches!(
        ledger.archive(collision.clone()),
        Err(RecoveryError::DuplicateRecord)
    ));
    assert!(matches!(
        ledger.record_completion(collision),
        Err(RecoveryError::DuplicateRecord)
    ));
    assert!(!ledger.needs_confirmation());
    assert_eq!(
        ledger.latest_completed().unwrap().evidence().raw_payload(),
        raw
    );
}

#[test]
fn recovery_requires_all_layout_fields_and_rejects_duplicate_records_across_slots() {
    let vault = VaultContext::generate().unwrap();
    let completed = evidence(&vault, "completed", TransactionPhase::Committed);
    let mut ledger = RecoveryLedger::default();
    ledger.record_completion(completed).unwrap();
    let original = serde_json::to_value(ledger.payload()).unwrap();
    for field in ["version", "latest_completed", "archived"] {
        let mut payload = original.clone();
        payload.as_object_mut().unwrap().remove(field);
        let encoded = vault
            .seal(
                &["file", RECOVERY_FILE, "content"],
                &serde_json::to_vec(&payload).unwrap(),
            )
            .unwrap();
        assert!(
            RecoveryLedger::open(&encoded, &vault).is_err(),
            "missing {field} was accepted"
        );
    }
    let mut duplicate = original;
    duplicate["archived"] = serde_json::json!([duplicate["latest_completed"].clone()]);
    let encoded = vault
        .seal(
            &["file", RECOVERY_FILE, "content"],
            &serde_json::to_vec(&duplicate).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        RecoveryLedger::open(&encoded, &vault),
        Err(RecoveryError::DuplicateRecord)
    ));
}
