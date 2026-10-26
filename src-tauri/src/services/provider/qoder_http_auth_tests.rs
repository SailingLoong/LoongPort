//! Synthetic-only tests. No account request, credential fixture, or live transport.
use super::*;
use crate::database::{validate_qoder_http_auth_column, QODER_HTTP_AUTH_COLUMN};
use crate::secrets::inventory;
use rusqlite::Connection;
use std::sync::atomic::{AtomicUsize, Ordering};
const ID: &str = "qoder-http-synthetic";
const NOW: i64 = 1_000_000;

fn fixture_on(db: &Database) -> PatDeclaration {
    let mut provider = Provider::with_id(
        ID.into(),
        "Synthetic HTTP".into(),
        serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-original-pat"}}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_sdk".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    declaration(db)
}
fn fixture() -> (Database, PatDeclaration) {
    let db = Database::memory().unwrap();
    let declaration = fixture_on(&db);
    (db, declaration)
}
fn selected_provider(db: &Database) -> Provider {
    db.get_provider_by_id(ID, "codex").unwrap().unwrap()
}
fn source(db: &Database) -> SourceBinding {
    SourceBinding::from_provider(&selected_provider(db))
        .unwrap()
        .0
}
fn declaration(db: &Database) -> PatDeclaration {
    PatDeclaration {
        provider_id: ID.into(),
        credential_generation: db
            .get_provider_by_id(ID, "codex")
            .unwrap()
            .unwrap()
            .meta
            .unwrap()
            .qoder_campaign_state
            .unwrap()
            .generation,
        source: CredentialSource::InternationalQoderPat,
    }
}
fn pair(tag: &str, expires: i64) -> TokenPair {
    TokenPair {
        access_token: format!("synthetic-access-{tag}"),
        refresh_token: format!("synthetic-refresh-{tag}"),
        expires_at_ms: expires,
    }
}
// Synthetic setup uses the same production preparation/reservation seam.
fn prepare_action_receipt(
    db: &Database,
    declaration: &PatDeclaration,
    action: IdentityAction,
) -> Result<IdentityActionReceipt, AppError> {
    let request = IdentityActionRequest {
        declaration: declaration.clone(),
        action,
    };
    match db.qoder_http_prepare_identity_action(&request, NOW)? {
        PreparedIdentityAction::Establish { receipt, .. } => Ok(receipt),
        _ => Err(denied("test_action_did_not_reserve")),
    }
}
fn reserve_action(
    db: &Database,
    declaration: &PatDeclaration,
    action: IdentityAction,
) -> Result<EstablishLease, AppError> {
    db.qoder_http_reserve_identity_action(prepare_action_receipt(db, declaration, action)?)
}
fn establish_ready(db: &Database, declaration: &PatDeclaration, expires: i64) -> RequestSnapshot {
    let lease = reserve_action(db, declaration, IdentityAction::Establish).unwrap();
    db.qoder_http_publish_establish(
        &lease,
        Established {
            uid: "synthetic-uid".into(),
            tokens: pair("first", expires),
        },
        NOW,
    )
    .unwrap()
}
fn plaintext(db: &Database) -> String {
    let vault = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    let raw: String = conn
        .query_row(
            "SELECT qoder_http_secret FROM providers WHERE id=?1",
            [ID],
            |r| r.get(0),
        )
        .unwrap();
    inventory::open_db(
        &vault,
        "providers",
        QODER_HTTP_AUTH_COLUMN,
        &[ID, "codex"],
        &raw,
    )
    .unwrap()
}
fn has_code<T>(result: Result<T, AppError>, expected: &str) {
    let Err(error) = result else {
        panic!("expected fail-closed result");
    };
    assert!(error
        .to_string()
        .contains(&format!("qoder.http_auth.{expected}")));
}
fn has_non_retryable<T>(result: Result<T, AcquireError>) {
    assert!(matches!(result, Err(AcquireError::NonRetryable)));
}

#[test]
fn acquisition_requires_explicit_international_source() {
    let (db, mut declaration) = fixture();
    has_code(
        db.qoder_http_acquire(&source(&db), NOW),
        "source_declaration_required",
    );
    declaration.credential_generation = "stale-generation".into();
    has_code(
        reserve_action(&db, &declaration, IdentityAction::Establish),
        "source_declaration_required",
    );
    assert!(plaintext(&db).is_empty());
}
#[test]
fn complete_identity_is_published_once_and_public_provider_never_contains_derived_fields() {
    let (db, declaration) = fixture();
    let lease = reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
    has_code(
        db.qoder_http_acquire(&source(&db), NOW),
        "attempt_in_progress",
    );
    assert!(!plaintext(&db).contains("synthetic-access"));
    let snapshot = db
        .qoder_http_publish_establish(
            &lease,
            Established {
                uid: "synthetic-uid".into(),
                tokens: pair("first", NOW + 600_000),
            },
            NOW,
        )
        .unwrap();
    assert_eq!(snapshot.uid(), "synthetic-uid");
    assert_eq!(snapshot.machine_id(), lease.machine_id);
    assert_eq!(snapshot.region(), "international");
    assert_eq!(snapshot.stamp.source.origin, ORIGIN);
    db.check_qoder_http_snapshot(&snapshot, NOW).unwrap();
    let provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    for public in [
        serde_json::to_string(&provider).unwrap(),
        format!("{provider:?}"),
    ] {
        for secret in [
            "synthetic-access",
            "synthetic-refresh",
            "synthetic-uid",
            snapshot.machine_id(),
            "qoderHttpSecret",
        ] {
            assert!(!public.contains(secret));
        }
    }
    let raw: String = db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT qoder_http_secret FROM providers", [], |r| r.get(0))
        .unwrap();
    assert!(raw.starts_with("lpenc"));
    assert!(!raw.contains("synthetic-"));
    has_code(
        db.qoder_http_publish_establish(
            &lease,
            Established {
                uid: "synthetic-uid".into(),
                tokens: pair("duplicate", NOW + 600_000),
            },
            NOW,
        ),
        "stale_owner",
    );
}
#[test]
fn refresh_is_single_flight_and_pending_contains_no_replayable_token() {
    let (db, declaration) = fixture();
    let before = establish_ready(&db, &declaration, NOW + 1);
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source(&db), NOW).unwrap() else {
        panic!("refresh required");
    };
    has_code(
        db.qoder_http_acquire(&source(&db), NOW),
        "attempt_in_progress",
    );
    has_code(db.check_qoder_http_snapshot(&before, NOW), "stale_owner");
    let pending = plaintext(&db);
    assert!(!pending.contains("synthetic-refresh"));
    assert!(!pending.contains("synthetic-access"));
    let after = db
        .qoder_http_publish_refresh(&lease, pair("next", NOW + 600_000), NOW)
        .unwrap();
    assert_eq!(after.uid(), before.uid());
    assert_eq!(after.machine_id(), before.machine_id());
    assert!(after.stamp.generation > before.stamp.generation);
    has_code(
        db.qoder_http_publish_refresh(&lease, pair("late", NOW + 600_000), NOW),
        "stale_owner",
    );
}
#[test]
fn crash_reopen_cannot_replay_pending_and_explicit_recovery_rejects_late_result() {
    let (db, declaration) = fixture();
    let first = establish_ready(&db, &declaration, NOW + 1);
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source(&db), NOW).unwrap() else {
        panic!();
    };
    let mut image = Connection::open_in_memory().unwrap();
    crate::database::vault::copy(&db.conn.lock().unwrap(), &mut image).unwrap();
    let reopened = Database::from_connection(image, db.secrets.clone());
    has_code(
        reopened.qoder_http_acquire(&source(&reopened), NOW),
        "attempt_in_progress",
    );
    let recovery =
        prepare_action_receipt(&reopened, &declaration, IdentityAction::Recover).unwrap();
    let retry = reopened
        .qoder_http_reserve_identity_action(recovery)
        .unwrap();
    has_code(
        reopened.qoder_http_publish_refresh(&lease, pair("late", NOW + 600_000), NOW),
        "stale_owner",
    );
    assert_eq!(retry.machine_id, first.machine_id());
    has_code(
        reopened.qoder_http_publish_establish(
            &retry,
            Established {
                uid: "another-uid".into(),
                tokens: pair("wrong", NOW + 600_000),
            },
            NOW,
        ),
        "identity_rebind_required",
    );
}
#[test]
fn unknown_refresh_requires_explicit_establishment_and_preserves_identity() {
    let (db, declaration) = fixture();
    let before = establish_ready(&db, &declaration, NOW + 1);
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source(&db), NOW).unwrap() else {
        panic!();
    };
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    has_code(
        db.qoder_http_acquire(&source(&db), NOW),
        "outcome_unknown_reestablish_required",
    );
    has_code(
        db.qoder_http_publish_refresh(&lease, pair("late", NOW + 600_000), NOW),
        "stale_owner",
    );
    let retry = reserve_action(&db, &declaration, IdentityAction::Recover).unwrap();
    assert_eq!(retry.expected_uid.as_deref(), Some(before.uid()));
    assert_eq!(retry.machine_id, before.machine_id());
}
#[test]
fn explicit_rebind_requires_current_subject_and_creates_new_cosy_identity() {
    let (db, declaration) = fixture();
    let before = establish_ready(&db, &declaration, NOW + 600_000);
    let mut invalid = prepare_action_receipt(&db, &declaration, IdentityAction::Rebind).unwrap();
    // The public action cannot supply a UID; still fence a corrupted private intent.
    invalid.intent = EstablishIntent::Rebind {
        previous_uid: "wrong".into(),
    };
    has_code(
        db.qoder_http_reserve_identity_action(invalid),
        "rebind_subject_changed",
    );
    let lease = reserve_action(&db, &declaration, IdentityAction::Rebind).unwrap();
    let after = db
        .qoder_http_publish_establish(
            &lease,
            Established {
                uid: "explicit-new-uid".into(),
                tokens: pair("rebind", NOW + 600_000),
            },
            NOW,
        )
        .unwrap();
    assert_ne!(after.machine_id(), before.machine_id());
    assert_eq!(after.uid(), "explicit-new-uid");
}
#[test]
fn source_edit_clears_private_record_in_owner_save_and_rejects_late_exchange() {
    let (db, declaration) = fixture();
    let lease = reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
    let mut provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    provider.settings_config["env"]["OPENAI_API_KEY"] = "synthetic-new-pat".into();
    db.save_provider("codex", &provider).unwrap();
    assert!(plaintext(&db).is_empty());
    has_code(
        db.qoder_http_publish_establish(
            &lease,
            Established {
                uid: "synthetic-uid".into(),
                tokens: pair("late", NOW + 600_000),
            },
            NOW,
        ),
        "stale_owner",
    );
}
#[test]
fn cosmetic_save_preserves_ready_and_delete_recreate_retires_old_lease() {
    let (db, declaration) = fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 600_000);
    let before = plaintext(&db);
    let mut provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    provider.name = "Cosmetic name".into();
    provider.notes = Some("Cosmetic note".into());
    db.save_provider("codex", &provider).unwrap();
    assert_eq!(before, plaintext(&db));
    db.check_qoder_http_snapshot(&snapshot, NOW).unwrap();
    db.delete_provider("codex", ID).unwrap();
    fixture_on(&db);
    has_code(db.check_qoder_http_snapshot(&snapshot, NOW), "stale_owner");
}
#[test]
fn lock_unlock_epoch_never_revives_old_request_or_refresh_lease() {
    let (db, declaration) = fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 1);
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source(&db), NOW).unwrap() else {
        panic!();
    };
    db.secrets.set_blocked(true);
    db.secrets.set_blocked(false);
    has_code(
        db.qoder_http_publish_refresh(&lease, pair("late", NOW + 600_000), NOW),
        "stale_owner",
    );
    has_code(db.check_qoder_http_snapshot(&snapshot, NOW), "stale_owner");
}
#[test]
fn corrupted_reset_future_records_are_not_usable_credentials() {
    assert!(Record::decode("").unwrap().is_none());
    assert!(Record::decode("{}").unwrap().is_none());
    assert!(Record::decode("{\"format\":2}").is_err());
    let (db, declaration) = fixture();
    establish_ready(&db, &declaration, NOW + 600_000);
    let mut json: serde_json::Value = serde_json::from_str(&plaintext(&db)).unwrap();
    json["format"] = 2.into();
    assert!(Record::decode(&json.to_string()).is_err());
    json["format"] = 1.into();
    json["source"]["origin"] = "https://attacker.invalid".into();
    assert!(Record::decode(&json.to_string()).is_err());
}
#[test]
fn inventory_rewrap_reset_export_and_wrong_row_authentication_cover_private_column() {
    let (db, declaration) = fixture();
    establish_ready(&db, &declaration, NOW + 600_000);
    let current = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    let raw: String = conn
        .query_row("SELECT qoder_http_secret FROM providers", [], |r| r.get(0))
        .unwrap();
    assert!(inventory::open_db(
        &current,
        "providers",
        QODER_HTTP_AUTH_COLUMN,
        &["wrong-id", "codex"],
        &raw
    )
    .is_err());
    let mut columns = Vec::new();
    inventory::collect_plaintext_values(
        &conn,
        current.encrypted().unwrap(),
        |table, column, _, _| columns.push((table.to_owned(), column.to_owned())),
    )
    .unwrap();
    assert!(!columns
        .iter()
        .any(|(t, c)| t == "providers" && c == QODER_HTTP_AUTH_COLUMN));
    let target = crate::secrets::VaultContext::generate().unwrap();
    inventory::transform_database(&conn, Some(current.encrypted().unwrap()), &target).unwrap();
    inventory::validate_database(&conn, &target).unwrap();
    assert!(inventory::validate_database(&conn, current.encrypted().unwrap()).is_err());
    inventory::reset_database(&conn, &target).unwrap();
    let raw: String = conn
        .query_row("SELECT qoder_http_secret FROM providers", [], |r| r.get(0))
        .unwrap();
    let reset = inventory::open_db(
        &crate::secrets::storage::StorageContext::Encrypted(target),
        "providers",
        QODER_HTTP_AUTH_COLUMN,
        &[ID, "codex"],
        &raw,
    )
    .unwrap();
    assert!(Record::decode(&reset).unwrap().is_none());
}
#[test]
fn legacy_inventory_keeps_archive_digest_and_v27_missing_column_is_rejected() {
    let (db, _) = fixture();
    let vault = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("ALTER TABLE providers DROP COLUMN qoder_http_secret; UPDATE loongport_schema_version SET version=26;").unwrap();
    let before = Database::content_digest(&conn).unwrap();
    inventory::validate_database(&conn, vault.encrypted().unwrap()).unwrap();
    assert_eq!(before, Database::content_digest(&conn).unwrap());
    conn.execute_batch("UPDATE loongport_schema_version SET version=27;")
        .unwrap();
    assert!(inventory::validate_database(&conn, vault.encrypted().unwrap()).is_err());
    conn.execute_batch("UPDATE loongport_schema_version SET version=26;")
        .unwrap();
    crate::database::loongport_schema::apply(&conn).unwrap();
    validate_qoder_http_auth_column(&conn).unwrap();
    inventory::validate_database(&conn, vault.encrypted().unwrap()).unwrap();
}
#[test]
fn migration_retires_even_experimental_ready_and_keeps_pat() {
    let (db, declaration) = fixture();
    establish_ready(&db, &declaration, NOW + 600_000);
    db.conn
        .lock()
        .unwrap()
        .execute_batch("UPDATE loongport_schema_version SET version=26;")
        .unwrap();
    crate::database::loongport_schema::apply(&db.conn.lock().unwrap()).unwrap();
    assert!(plaintext(&db).is_empty());
    assert_eq!(
        db.get_provider_by_id(ID, "codex")
            .unwrap()
            .unwrap()
            .settings_config["env"]["OPENAI_API_KEY"],
        "synthetic-original-pat"
    );
}
#[test]
fn actual_sql_import_retires_ready_refresh_unknown_and_establishing_states() {
    for state in ["ready", "refresh", "unknown", "establishing"] {
        let (db, declaration) = fixture();
        if state == "establishing" {
            reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
        } else {
            establish_ready(&db, &declaration, NOW + 1);
            if state != "ready" {
                let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source(&db), NOW).unwrap()
                else {
                    panic!();
                };
                if state == "unknown" {
                    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
                        .unwrap();
                }
            }
        }
        let sql = db.export_sql_string().unwrap();
        db.import_sql_string(&sql).unwrap();
        assert!(
            plaintext(&db).is_empty(),
            "{state} survived actual SQL restore"
        );
        has_code(
            db.qoder_http_acquire(&source(&db), NOW),
            "source_declaration_required",
        );
    }
}

struct WaitingTransport {
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    fail: bool,
}
impl AuthTransport for WaitingTransport {
    fn exchange_and_identify<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Established, ()>> + Send + 'a>> {
        Box::pin(async { panic!("no PAT fallback permitted") })
    }
    fn refresh<'a>(
        &'a self,
        token: &'a str,
        _: &'a Identity,
    ) -> Pin<Box<dyn Future<Output = Result<TokenPair, ()>> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(token, "synthetic-refresh-first");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            if self.fail {
                Err(())
            } else {
                Ok(pair("after-await", NOW + 600_000))
            }
        })
    }
}
#[tokio::test]
async fn awaiting_network_holds_no_vault_or_database_lock_and_concurrent_refresh_does_not_submit() {
    let (db, declaration) = fixture();
    establish_ready(&db, &declaration, NOW + 1);
    let transport = WaitingTransport {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: false,
    };
    let selected = selected_provider(&db);
    let first = acquire(&db, &selected, &transport, || NOW);
    let second = async {
        transport.entered.notified().await;
        // These would deadlock if any guard crossed the network await.
        drop(db.secrets.write().unwrap());
        drop(db.conn.lock().unwrap());
        has_code(
            db.qoder_http_acquire(&source(&db), NOW),
            "attempt_in_progress",
        );
        has_non_retryable(acquire(&db, &selected_provider(&db), &transport, || NOW).await);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        transport.release.notify_one();
    };
    let (result, ()) = tokio::join!(first, second);
    assert!(result.is_ok());
}
#[tokio::test]
async fn dropping_submitted_future_does_not_replay_refresh() {
    let (db, declaration) = fixture();
    establish_ready(&db, &declaration, NOW + 1);
    let transport = WaitingTransport {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: false,
    };
    let selected = selected_provider(&db);
    let mut future = Box::pin(acquire(&db, &selected, &transport, || NOW));
    tokio::select! { _=transport.entered.notified()=>{}, _=&mut future=>panic!("response must still be pending") }
    drop(future);
    has_code(
        db.qoder_http_acquire(&source(&db), NOW),
        "attempt_in_progress",
    );
    has_non_retryable(acquire(&db, &selected_provider(&db), &transport, || NOW).await);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(!plaintext(&db).contains("synthetic-refresh"));
}

#[tokio::test]
async fn only_initial_missing_pat_or_declaration_is_retryable_without_transport_calls() {
    let (db, _) = fixture();
    let transport = WaitingTransport {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: false,
    };
    assert!(matches!(
        acquire(&db, &selected_provider(&db), &transport, || NOW).await,
        Err(AcquireError::PreflightMissingSource)
    ));
    let mut provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    provider.settings_config = serde_json::json!({"env":{"OPENAI_API_KEY":""}});
    db.save_provider("codex", &provider).unwrap();
    assert!(matches!(
        acquire(&db, &selected_provider(&db), &transport, || NOW).await,
        Err(AcquireError::PreflightMissingSource)
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

async fn source_removed_after_refresh_submission_remains_non_retryable(fail: bool) {
    let (db, declaration) = fixture();
    establish_ready(&db, &declaration, NOW + 1);
    let transport = WaitingTransport {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail,
    };
    let selected = selected_provider(&db);
    let request = acquire(&db, &selected, &transport, || NOW);
    let edit = async {
        transport.entered.notified().await;
        // The original save retires the pending row while the single refresh
        // is already submitted. Success/publication and failure/mark_unknown
        // will both see source_required, but neither is a preflight failure.
        let mut provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
        provider.settings_config = serde_json::json!({"env":{"OPENAI_API_KEY":""}});
        db.save_provider("codex", &provider).unwrap();
        transport.release.notify_one();
    };
    let (result, ()) = tokio::join!(request, edit);
    has_code(
        SourceBinding::from_provider(&selected_provider(&db)),
        "source_required",
    );
    has_non_retryable(result);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(plaintext(&db).is_empty());
}

#[tokio::test]
async fn late_success_after_pat_clear_cannot_be_reclassified_as_missing_preflight() {
    source_removed_after_refresh_submission_remains_non_retryable(false).await;
}

#[tokio::test]
async fn late_failure_after_pat_clear_cannot_be_reclassified_as_missing_preflight() {
    source_removed_after_refresh_submission_remains_non_retryable(true).await;
}

#[test]
fn acquisition_error_debug_never_exposes_the_private_cause() {
    let error = AcquireError::preflight(AppError::Config("synthetic-private-cause".into()));
    assert_eq!(format!("{error:?}"), "NonRetryable");
}

#[test]
fn natural_expiry_blocks_new_request_but_does_not_revoke_already_submitted_output() {
    let (db, declaration) = fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 1);
    has_code(
        db.check_qoder_http_snapshot(&snapshot, NOW + 2),
        "snapshot_expired",
    );
    db.check_qoder_http_snapshot_fence(&snapshot).unwrap();
}

#[test]
fn original_provider_save_initializes_source_revision_without_any_campaign_or_device() {
    let (db, declaration) = fixture();
    let provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    let state = provider.meta.unwrap().qoder_campaign_state.unwrap();
    assert_eq!(state.generation, declaration.credential_generation);
    assert!(state.pending.is_none());
    assert!(
        crate::database::read_qoder_campaign_operation_rows(&db.conn.lock().unwrap())
            .unwrap()
            .is_empty()
    );
    reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
}

#[test]
fn access_only_exchange_is_usable_but_never_submits_an_empty_refresh() {
    let (db, declaration) = fixture();
    let lease = reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
    let mut tokens = pair("access-only", NOW + 600_000);
    tokens.refresh_token.clear();
    let snapshot = db
        .qoder_http_publish_establish(
            &lease,
            Established {
                uid: "synthetic-uid".into(),
                tokens,
            },
            NOW,
        )
        .unwrap();
    db.check_qoder_http_snapshot(&snapshot, NOW).unwrap();
    assert!(matches!(
        db.qoder_http_acquire(&source(&db), NOW).unwrap(),
        Acquisition::Ready(_)
    ));
    has_code(
        db.qoder_http_acquire(&source(&db), NOW + 400_000),
        "reestablish_required",
    );
    assert!(plaintext(&db).contains("ready"));
}

#[test]
fn delayed_recovery_confirmation_cannot_replace_a_new_attempt_on_the_same_source() {
    let (db, declaration) = fixture();
    let first = reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
    let stale_receipt = prepare_action_receipt(&db, &declaration, IdentityAction::Recover).unwrap();
    db.qoder_http_mark_unknown(&first.stamp, &first.attempt_id)
        .unwrap();
    let second = reserve_action(&db, &declaration, IdentityAction::Recover).unwrap();
    has_code(
        db.qoder_http_reserve_identity_action(stale_receipt),
        "stale_owner",
    );
    db.qoder_http_publish_establish(
        &second,
        Established {
            uid: "synthetic-uid".into(),
            tokens: pair("new-attempt", NOW + 600_000),
        },
        NOW,
    )
    .unwrap();
}
#[test]
fn recovery_confirmation_is_invalid_after_original_vault_epoch_changes() {
    let (db, declaration) = fixture();
    reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
    let receipt = prepare_action_receipt(&db, &declaration, IdentityAction::Recover).unwrap();
    db.secrets.set_blocked(true);
    db.secrets.set_blocked(false);
    has_code(
        db.qoder_http_reserve_identity_action(receipt),
        "stale_owner",
    );
    has_code(
        db.qoder_http_acquire(&source(&db), NOW),
        "attempt_in_progress",
    );
}

#[test]
fn selected_owner_fence_does_not_decrypt_an_unrelated_corrupt_provider() {
    let (db, declaration) = fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 600_000);
    db.save_provider(
        "codex",
        &Provider::with_id(
            "unrelated".into(),
            "Unrelated".into(),
            serde_json::json!({}),
            None,
        ),
    )
    .unwrap();
    db.conn.lock().unwrap().execute("UPDATE providers SET settings_config='invalid-unrelated-ciphertext' WHERE id='unrelated'",[]).unwrap();
    db.check_qoder_http_snapshot(&snapshot, NOW).unwrap();
    assert!(matches!(
        db.qoder_http_acquire(&source(&db), NOW).unwrap(),
        Acquisition::Ready(_)
    ));
    let vault = db.secrets.read().unwrap();
    assert!(
        inventory::validate_database(&db.conn.lock().unwrap(), vault.encrypted().unwrap()).is_err()
    );
}
#[test]
fn recovery_receipt_cannot_be_consumed_twice() {
    let (db, declaration) = fixture();
    reserve_action(&db, &declaration, IdentityAction::Establish).unwrap();
    let receipt = prepare_action_receipt(&db, &declaration, IdentityAction::Recover).unwrap();
    // Both receipts capture the exact same pending generation/attempt. The
    // owned first receipt cannot be reused in Rust; an equivalent second one
    // must also fail the original transaction CAS after the first consumes it.
    let duplicate = prepare_action_receipt(&db, &declaration, IdentityAction::Recover).unwrap();
    db.qoder_http_reserve_identity_action(receipt).unwrap();
    has_code(
        db.qoder_http_reserve_identity_action(duplicate),
        "stale_owner",
    );
}

#[test]
fn selected_provider_source_must_match_snapshot_after_await() {
    let (db, initial_declaration) = fixture();
    let selected = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    let before = establish_ready(&db, &initial_declaration, NOW + 600_000);
    check_selected_provider(&selected, &before).unwrap();
    let mut next = selected.clone();
    next.settings_config["env"]["OPENAI_API_KEY"] = "synthetic-new-pat".into();
    db.save_provider("codex", &next).unwrap();
    let after = establish_ready(&db, &declaration(&db), NOW + 600_000);
    has_code(
        check_selected_provider(&selected, &after),
        "selected_source_changed",
    );
}

#[tokio::test]
async fn stale_selected_provider_cannot_spend_the_new_sources_near_expiry_refresh() {
    let (db, _) = fixture();
    let selected = selected_provider(&db);
    let mut next = selected.clone();
    next.settings_config["env"]["OPENAI_API_KEY"] = "synthetic-new-pat".into();
    db.save_provider("codex", &next).unwrap();
    establish_ready(&db, &declaration(&db), NOW + 1);
    let before = plaintext(&db);
    let transport = WaitingTransport {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: false,
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        acquire(&db, &selected, &transport, || NOW),
    )
    .await
    .expect("stale selection must fail before the waiting transport");
    has_code(
        db.qoder_http_acquire(&SourceBinding::from_provider(&selected).unwrap().0, NOW),
        "selected_source_changed",
    );
    has_non_retryable(result);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    assert_eq!(plaintext(&db), before);
    assert!(matches!(
        Record::decode(&before).unwrap().unwrap().phase,
        Phase::Ready { .. }
    ));
}

#[test]
fn source_digest_remains_stable_lowercase_hex_with_sha2_011() {
    let (db, _) = fixture();
    let provider = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    let (source, _) = SourceBinding::from_provider(&provider).unwrap();
    assert_eq!(
        source.credential_digest,
        "0ae30de816667fc04ebbe678b2420287e177cb80d8f119ca5a51eb81d758eb8d"
    );
}

#[test]
fn cn_provider_is_a_distinct_original_source_and_cross_region_changes_retire_snapshot() {
    let (db, international) = fixture();
    let old = establish_ready(&db, &international, NOW + 900_000);
    let mut cn = selected_provider(&db);
    cn.meta.as_mut().unwrap().provider_type = Some("qoder_cn".into());
    db.save_provider("codex", &cn).unwrap();
    let cn = selected_provider(&db);
    let source = SourceBinding::from_provider(&cn);
    assert!(source.is_ok(), "CN must be admitted as its own source");
    let (source, _) = source.unwrap();
    assert_eq!(source.origin, "https://openapi.qoder.com.cn");
    assert!(source.admit(&international).is_err());
    assert!(db.check_qoder_http_snapshot(&old, NOW).is_err());
}

struct CnWaitingTransport(WaitingTransport, SourceKind);
impl AuthTransport for CnWaitingTransport {
    fn source_kind(&self) -> SourceKind {
        self.1.clone()
    }
    fn exchange_and_identify<'a>(
        &'a self,
        pat: &'a str,
        machine: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Established, ()>> + Send + 'a>> {
        self.0.exchange_and_identify(pat, machine)
    }
    fn refresh<'a>(
        &'a self,
        token: &'a str,
        identity: &'a Identity,
    ) -> Pin<Box<dyn Future<Output = Result<TokenPair, ()>> + Send + 'a>> {
        self.0.refresh(token, identity)
    }
}
#[tokio::test]
async fn cn_refresh_reuses_one_owner_lock_and_never_submits_to_international_transport() {
    let (db, _) = fixture();
    let mut provider = selected_provider(&db);
    provider.meta.as_mut().unwrap().provider_type = Some("qoder_cn".into());
    db.save_provider("codex", &provider).unwrap();
    let mut declaration = declaration(&db);
    declaration.source = CredentialSource::ChinaQoderPat;
    let snapshot = establish_ready(&db, &declaration, NOW + 1);
    assert_eq!(snapshot.region(), "cn");
    let wrong = WaitingTransport {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: false,
    };
    has_non_retryable(acquire(&db, &selected_provider(&db), &wrong, || NOW).await);
    assert_eq!(wrong.calls.load(Ordering::SeqCst), 0);
    let transport = CnWaitingTransport(wrong, SourceKind::ChinaQoderPat);
    let selected = selected_provider(&db);
    let first = acquire(&db, &selected, &transport, || NOW);
    let second = async {
        transport.0.entered.notified().await;
        has_non_retryable(acquire(&db, &selected_provider(&db), &transport, || NOW).await);
        assert_eq!(transport.0.calls.load(Ordering::SeqCst), 1);
        transport.0.release.notify_one();
    };
    let (result, ()) = tokio::join!(first, second);
    let refreshed = result.unwrap();
    assert_eq!(refreshed.region(), "cn");
    assert!(db.check_qoder_http_snapshot(&refreshed, NOW).is_ok());
}

#[test]
fn same_pat_and_uid_in_two_realms_keep_distinct_original_account_owners() {
    let (db, international) = fixture();
    let international_snapshot = establish_ready(&db, &international, NOW + 900_000);
    let mut cn = selected_provider(&db);
    cn.id = "separate-cn-account".into();
    cn.meta.as_mut().unwrap().provider_type = Some("qoder_cn".into());
    // Deliberately identical synthetic PAT and returned UID in both realms.
    db.save_provider("codex", &cn).unwrap();
    let cn = db.get_provider_by_id(&cn.id, "codex").unwrap().unwrap();
    let (cn_source, _) = SourceBinding::from_provider(&cn).unwrap();
    let mut cn_declaration = PatDeclaration {
        provider_id: cn.id.clone(),
        credential_generation: cn_source.credential_generation.clone(),
        source: CredentialSource::InternationalQoderPat,
    };
    assert!(reserve_action(&db, &cn_declaration, IdentityAction::Establish).is_err());
    cn_declaration.source = CredentialSource::ChinaQoderPat;
    let cn_snapshot = establish_ready(&db, &cn_declaration, NOW + 900_000);
    assert_eq!(cn_snapshot.uid(), international_snapshot.uid());
    assert_ne!(
        cn_snapshot.machine_id(),
        international_snapshot.machine_id()
    );
    assert_eq!(cn_snapshot.region(), "cn");
    assert_eq!(international_snapshot.region(), "international");
    assert_eq!(cn_source.origin, "https://openapi.qoder.com.cn");
    assert!(db.check_qoder_http_snapshot(&cn_snapshot, NOW).is_ok());
    assert!(db
        .check_qoder_http_snapshot(&international_snapshot, NOW)
        .is_ok());
    assert!(check_selected_provider(&cn, &international_snapshot).is_err());
    assert!(check_selected_provider(&selected_provider(&db), &cn_snapshot).is_err());
    let mut forged = cn_source;
    forged.origin = ORIGIN.into();
    assert!(!forged.validate());
}

#[test]
fn saved_legacy_international_source_never_defaults_to_cn() {
    let (db, declaration) = fixture();
    let binding = source(&db);
    let encoded = serde_json::to_value(&binding).unwrap();
    assert_eq!(encoded["source"], "InternationalQoderPat");
    let old: SourceBinding = serde_json::from_value(encoded.clone()).unwrap();
    assert!(old.source == SourceKind::InternationalQoderPat);
    assert_eq!(old.origin, ORIGIN);
    assert!(old.admit(&declaration).is_ok());
    let mut missing = encoded;
    missing.as_object_mut().unwrap().remove("source");
    assert!(serde_json::from_value::<SourceBinding>(missing).is_err());
}

#[test]
fn cn_device_binding_admits_no_api_key_without_pat() {
    let (db, _) = fixture();
    let mut provider = selected_provider(&db);
    provider.meta.as_mut().unwrap().provider_type = Some("qoder_cn_device".into());
    provider.settings_config = serde_json::json!({});
    let result = SourceBinding::from_provider(&provider);
    assert!(
        result.is_ok(),
        "independent CN device source must not require a PAT"
    );
    let (binding, pat) = result.unwrap();
    assert!(pat.is_empty());
    assert_eq!(binding.origin, "https://openapi.qoder.com.cn");
    assert_eq!(
        serde_json::to_value(binding.source.declaration()).unwrap(),
        "chinaQoderDevice"
    );
    provider.settings_config =
        serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-not-a-device-token"}});
    assert!(SourceBinding::from_provider(&provider).is_err());
}

#[test]
fn cn_device_context_preserves_status_without_exposing_pat_actions() {
    let (db, _) = fixture();
    let mut provider = selected_provider(&db);
    provider.meta.as_mut().unwrap().provider_type = Some("qoder_cn_device".into());
    provider.settings_config = serde_json::json!({});
    let source = SourceBinding::from_provider(&provider).unwrap().0;
    let missing = IdentityContext::from_record(&source, None, NOW);
    assert_eq!(missing.status, IdentityStatus::Unestablished);
    assert_eq!(
        serde_json::to_value(&missing).unwrap()["allowedDeviceActions"],
        serde_json::json!(["login"])
    );
    assert!(missing.allowed_actions.is_empty());
    for phase in [
        Phase::Ready {
            identity: Identity {
                uid: "synthetic-cn".into(),
                machine_id: uuid::Uuid::new_v4().to_string(),
            },
            tokens: pair("cn-device", NOW + 900_000),
        },
        Phase::Unknown {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            identity: None,
            machine_id: uuid::Uuid::new_v4().to_string(),
        },
    ] {
        let expected = if matches!(phase, Phase::Ready { .. }) {
            IdentityStatus::Ready
        } else {
            IdentityStatus::Unknown
        };
        let record = Record::new(source.clone(), 1, phase);
        let context = IdentityContext::from_record(&source, Some(&record), NOW);
        assert_eq!(context.status, expected);
        assert_eq!(context.source, Some(CredentialSource::ChinaQoderDevice));
        assert!(context.allowed_actions.is_empty());
    }
}

#[test]
fn cn_device_snapshot_projects_its_exact_credential_family() {
    let (db, _) = fixture();
    let mut provider = selected_provider(&db);
    for (provider_type, kind, region) in [
        ("qoder_sdk", "international_qoder_pat", "international"),
        ("qoder_cn", "china_qoder_pat", "cn"),
        ("qoder_cn_device", "china_qoder_device", "cn"),
    ] {
        provider.meta.as_mut().unwrap().provider_type = Some(provider_type.into());
        if provider_type == "qoder_cn_device" {
            provider.settings_config = serde_json::json!({});
        }
        let source = SourceBinding::from_provider(&provider).unwrap().0;
        let snapshot = RequestSnapshot::new(
            Stamp {
                source,
                generation: 1,
                vault_epoch: 0,
            },
            Identity {
                uid: "synthetic-cn".into(),
                machine_id: uuid::Uuid::new_v4().to_string(),
            },
            &pair("device", NOW + 900000),
        );
        assert_eq!(snapshot.credential_kind(), kind);
        assert_eq!(snapshot.region(), region);
    }
}

#[test]
fn cn_device_first_save_initializes_generation_and_login_without_any_pat() {
    let db = Database::memory().unwrap();
    let mut provider = Provider::with_id(
        ID.into(),
        "Synthetic independent CN".into(),
        serde_json::json!({}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_cn_device".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let stored = selected_provider(&db);
    assert!(
        stored.meta.as_ref().unwrap().qoder_campaign_state.is_some(),
        "device first save must initialize its original source generation"
    );
    let (source, pat) = SourceBinding::from_provider(&stored).unwrap();
    assert!(!source.credential_generation.is_empty());
    assert!(pat.is_empty());
    let context = identity_context(&db, ID, NOW);
    assert_eq!(context.status, IdentityStatus::Unestablished);
    assert_eq!(
        context.allowed_device_actions,
        vec![DeviceIdentityAction::Login]
    );
    assert!(context.allowed_actions.is_empty());
    assert!(plaintext(&db).is_empty());
}

fn device_fixture() -> (Database, PatDeclaration) {
    let db = Database::memory().unwrap();
    let mut provider = Provider::with_id(
        ID.into(),
        "Synthetic independent CN".into(),
        serde_json::json!({}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_cn_device".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let mut declaration = declaration(&db);
    declaration.source = CredentialSource::ChinaQoderDevice;
    (db, declaration)
}

#[test]
fn device_auth_rejection_retires_only_its_current_tokens_and_keeps_relogin_identity() {
    let (db, declaration) = device_fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 900000);
    db.qoder_http_reject_device_snapshot(&snapshot).unwrap();
    assert!(db.check_qoder_http_snapshot(&snapshot, NOW).is_err());
    let raw = plaintext(&db);
    assert!(!raw.contains("synthetic-access") && !raw.contains("synthetic-refresh"));
    let record = Record::decode(&raw).unwrap().unwrap();
    let Phase::Unknown {
        identity: Some(identity),
        machine_id,
        ..
    } = record.phase
    else {
        panic!("rejected device owner must require a fresh grant");
    };
    assert_eq!(identity.uid, snapshot.uid());
    assert_eq!(machine_id, snapshot.machine_id());
    let context = identity_context(&db, ID, NOW);
    assert_eq!(context.status, IdentityStatus::Unknown);
    assert_eq!(
        context.allowed_device_actions,
        vec![DeviceIdentityAction::Login, DeviceIdentityAction::Rebind]
    );
    let lease = reserve_action(&db, &declaration, IdentityAction::Recover).unwrap();
    assert_eq!(lease.expected_uid.as_deref(), Some(snapshot.uid()));
    assert_eq!(lease.machine_id, snapshot.machine_id());
    assert!(lease.pat.is_empty());
}

#[test]
fn stale_device_rejection_cannot_revoke_a_new_owner_or_an_inflight_refresh() {
    let (db, declaration) = device_fixture();
    let old = establish_ready(&db, &declaration, NOW + 900000);
    let lease = reserve_action(&db, &declaration, IdentityAction::Rebind).unwrap();
    let new = db
        .qoder_http_publish_establish(
            &lease,
            Established {
                uid: "synthetic-new-uid".into(),
                tokens: pair("new", NOW + 1),
            },
            NOW,
        )
        .unwrap();
    assert!(db.qoder_http_reject_device_snapshot(&old).is_err());
    assert!(db.check_qoder_http_snapshot(&new, NOW).is_ok());
    let Acquisition::Refresh(refresh) = db.qoder_http_acquire(&source(&db), NOW).unwrap() else {
        panic!("short-lived grant must reserve refresh");
    };
    assert!(db.qoder_http_reject_device_snapshot(&new).is_err());
    let newer = db
        .qoder_http_publish_refresh(&refresh, pair("refreshed", NOW + 900000), NOW)
        .unwrap();
    assert!(db.check_qoder_http_snapshot(&newer, NOW).is_ok());
}

#[test]
fn device_rejection_cannot_retire_a_pat_owner() {
    let (db, declaration) = fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 900000);
    assert!(db.qoder_http_reject_device_snapshot(&snapshot).is_err());
    assert!(db.check_qoder_http_snapshot(&snapshot, NOW).is_ok());
}

#[tokio::test]
async fn native_device_refresh_is_single_consumption_even_when_outcome_unknown() {
    let (db, declaration) = device_fixture();
    let snapshot = establish_ready(&db, &declaration, NOW + 1);
    let transport = CnWaitingTransport(
        WaitingTransport {
            calls: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            fail: true,
        },
        SourceKind::ChinaQoderPat,
    );
    has_non_retryable(acquire(&db, &selected_provider(&db), &transport, || NOW).await);
    assert_eq!(transport.0.calls.load(Ordering::SeqCst), 0);
    let transport = CnWaitingTransport(transport.0, SourceKind::ChinaQoderDevice);
    let selected = selected_provider(&db);
    let first = acquire(&db, &selected, &transport, || NOW);
    let second = async {
        transport.0.entered.notified().await;
        has_non_retryable(acquire(&db, &selected_provider(&db), &transport, || NOW).await);
        transport.0.release.notify_one();
    };
    let (result, ()) = tokio::join!(first, second);
    has_non_retryable(result);
    has_non_retryable(acquire(&db, &selected, &transport, || NOW).await);
    assert_eq!(transport.0.calls.load(Ordering::SeqCst), 1);
    let raw = plaintext(&db);
    assert!(!raw.contains("synthetic-refresh"));
    let record = Record::decode(&raw).unwrap().unwrap();
    let Phase::Unknown {
        identity: Some(identity),
        ..
    } = record.phase
    else {
        panic!("ambiguous refresh cannot be replayed");
    };
    assert_eq!(identity.machine_id, snapshot.machine_id());
}
