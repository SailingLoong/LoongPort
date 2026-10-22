//! Synthetic original-owner tests. Rust execution is NOTRUN in the local task.
use super::*;
use crate::provider::ProviderMeta;
use std::sync::atomic::{AtomicUsize, Ordering};

const ID: &str = "identity-action-fixture";
const NOW: i64 = 1_000_000;
fn fixture() -> (Database, IdentityActionRequest) {
    let db = Database::memory().unwrap();
    let mut provider = Provider::with_id(
        ID.into(),
        "Synthetic Provider".into(),
        serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-original-pat"}}),
        None,
    );
    provider.meta = Some(ProviderMeta {
        provider_type: Some("qoder_sdk".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let generation = db
        .get_provider_by_id(ID, "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
        .generation;
    (
        db,
        IdentityActionRequest {
            declaration: PatDeclaration {
                provider_id: ID.into(),
                credential_generation: generation,
                source: CredentialSource::InternationalQoderPat,
            },
            action: IdentityAction::Establish,
        },
    )
}
fn tokens(refresh: bool, expires: i64) -> TokenPair {
    TokenPair {
        access_token: "synthetic-access".into(),
        refresh_token: if refresh {
            "synthetic-refresh".into()
        } else {
            String::new()
        },
        expires_at_ms: expires,
    }
}
fn reserve_initial(db: &Database, request: &IdentityActionRequest) -> EstablishLease {
    let PreparedIdentityAction::Establish {
        receipt,
        confirmation,
    } = db.qoder_http_prepare_identity_action(request, NOW).unwrap()
    else {
        panic!("initial setup must reserve")
    };
    assert!(confirmation.is_none());
    db.qoder_http_reserve_identity_action(receipt).unwrap()
}
fn ready(
    db: &Database,
    request: &IdentityActionRequest,
    refresh: bool,
    expires: i64,
) -> RequestSnapshot {
    let lease = reserve_initial(db, request);
    db.qoder_http_publish_establish(
        &lease,
        Established {
            uid: "synthetic-uid".into(),
            tokens: tokens(refresh, expires),
        },
        NOW,
    )
    .unwrap()
}
fn unknown(db: &Database, request: &IdentityActionRequest) {
    ready(db, request, true, NOW + 1);
    let selected = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
    let source = SourceBinding::from_provider(&selected).unwrap().0;
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!()
    };
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
}
struct Transport {
    exchanges: AtomicUsize,
    uid: &'static str,
    fail: bool,
}
impl Transport {
    fn new() -> Self {
        Self {
            exchanges: AtomicUsize::new(0),
            uid: "synthetic-uid",
            fail: false,
        }
    }
}
impl AuthTransport for Transport {
    fn exchange_and_identify<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Established, ()>> + Send + 'a>> {
        Box::pin(async move {
            self.exchanges.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(())
            } else {
                Ok(Established {
                    uid: self.uid.into(),
                    tokens: tokens(true, NOW + 600_000),
                })
            }
        })
    }
    fn refresh<'a>(
        &'a self,
        _: &'a str,
        _: &'a Identity,
    ) -> Pin<Box<dyn Future<Output = Result<TokenPair, ()>> + Send + 'a>> {
        Box::pin(async { panic!("identity action must never refresh") })
    }
}
fn completed(result: IdentityActionResult) -> IdentityContext {
    match result {
        IdentityActionResult::Completed { context } => context,
        _ => panic!("expected completed"),
    }
}
#[test]
fn strict_action_wire_rejects_unknown_fields_without_echoing_values() {
    let valid = serde_json::json!({"declaration":{"providerId":ID,"credentialGeneration":"generation","source":"internationalQoderPat"},"action":"establish"});
    assert!(serde_json::from_value::<IdentityActionRequest>(valid.clone()).is_ok());
    for key in [
        "origin",
        "accessToken",
        "refreshToken",
        "pat",
        "uid",
        "receipt",
        "confirmed",
        "providerId",
        "appType",
    ] {
        let mut input = valid.clone();
        input[key] = "synthetic-secret-sentinel".into();
        let error = serde_json::from_value::<IdentityActionRequest>(input)
            .err()
            .unwrap();
        assert!(!error.to_string().contains("synthetic-secret-sentinel"));
    }
    for path in ["source", "providerId", "credentialGeneration"] {
        let mut input = valid.clone();
        input["declaration"][path] = serde_json::json!(["synthetic-secret-sentinel"]);
        assert!(serde_json::from_value::<IdentityActionRequest>(input).is_err());
    }
    let mut input = valid;
    input["declaration"]["source"] = "cnQoderPat".into();
    assert!(serde_json::from_value::<IdentityActionRequest>(input).is_err());
    for declaration in [
        serde_json::json!({"providerId":ID,"credentialGeneration":"generation"}),
        serde_json::json!({"providerId":ID,"credentialGeneration":"generation","source":"internationalQoderPat","pat":"synthetic-secret-sentinel"}),
    ] {
        let error = serde_json::from_value::<IdentityActionRequest>(
            serde_json::json!({"declaration":declaration,"action":"establish"}),
        )
        .err()
        .unwrap();
        assert!(!error.to_string().contains("synthetic-secret-sentinel"));
    }
}
#[test]
fn context_projection_has_only_six_safe_fields_and_never_derived_identity() {
    let (db, request) = fixture();
    ready(&db, &request, true, NOW + 600_000);
    let value = serde_json::to_value(identity_context_admitted(&db, ID, NOW).unwrap()).unwrap();
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 6);
    for key in [
        "providerId",
        "source",
        "credentialGeneration",
        "status",
        "expiresAtMs",
        "allowedActions",
    ] {
        assert!(object.contains_key(key));
    }
    let public = value.to_string();
    for secret in [
        "synthetic-original-pat",
        "synthetic-access",
        "synthetic-refresh",
        "synthetic-uid",
        "machineId",
        "receipt",
        "attemptId",
    ] {
        assert!(!public.contains(secret));
    }
}
#[tokio::test]
async fn first_setup_is_one_action_without_a_second_confirmation() {
    let (db, request) = fixture();
    let transport = Transport::new();
    let context = completed(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| async { panic!("ordinary setup cannot ask twice") },
            || NOW,
        )
        .await,
    );
    assert_eq!(context.status, IdentityStatus::Ready);
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn ordinary_ready_and_refreshable_ready_are_noops() {
    for expires in [NOW + 600_000, NOW + 1] {
        let (db, request) = fixture();
        let before = ready(&db, &request, true, expires);
        let transport = Transport::new();
        completed(
            perform_identity_action(
                &db,
                request,
                &transport,
                |_| async { panic!("Ready cannot ask") },
                || NOW,
            )
            .await,
        );
        assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
        db.check_qoder_http_snapshot_fence(&before).unwrap();
    }
}
#[tokio::test]
async fn access_only_expiry_reestablishes_without_extra_approval() {
    let (db, request) = fixture();
    ready(&db, &request, false, NOW + 1);
    let transport = Transport::new();
    completed(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| async { panic!("ordinary renew cannot ask twice") },
            || NOW,
        )
        .await,
    );
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn unknown_requires_recover_action_and_never_silently_escalates_establish() {
    let (db, request) = fixture();
    unknown(&db, &request);
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| async { panic!("wrong action cannot ask") },
            || NOW
        )
        .await,
        IdentityActionResult::Changed { .. }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unknown_recovery_confirms_once_and_preserves_known_identity_and_machine() {
    let (db, mut request) = fixture();
    let before = ready(&db, &request, true, NOW + 1);
    let source =
        SourceBinding::from_provider(&db.get_provider_by_id(ID, "codex").unwrap().unwrap())
            .unwrap()
            .0;
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!()
    };
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    let confirms = AtomicUsize::new(0);
    completed(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| {
                confirms.fetch_add(1, Ordering::SeqCst);
                std::future::ready(true)
            },
            || NOW,
        )
        .await,
    );
    assert_eq!(confirms.load(Ordering::SeqCst), 1);
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
    let Acquisition::Ready(after) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!()
    };
    assert_eq!(after.uid(), before.uid());
    assert_eq!(after.machine_id(), before.machine_id());
}
#[tokio::test]
async fn cancelled_or_closed_recovery_never_mutates_or_submits() {
    for action in [IdentityAction::Recover, IdentityAction::Rebind] {
        let (db, mut request) = fixture();
        unknown(&db, &request);
        request.action = action;
        let transport = Transport::new();
        let before = identity_context_admitted(&db, ID, NOW).unwrap();
        assert!(matches!(
            perform_identity_action(&db, request, &transport, |_| async { false }, || NOW).await,
            IdentityActionResult::Cancelled
        ));
        assert_eq!(
            identity_context_admitted(&db, ID, NOW).unwrap().status,
            before.status
        );
        assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn pending_ordinary_action_never_takes_over() {
    let (db, request) = fixture();
    reserve_initial(&db, &request);
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(&db, request, &transport, |_| async { panic!() }, || NOW).await,
        IdentityActionResult::Changed { .. }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn pending_recovery_directly_reserves_and_late_old_response_is_rejected() {
    let (db, mut request) = fixture();
    let old = reserve_initial(&db, &request);
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    completed(perform_identity_action(&db, request, &transport, |_| async { true }, || NOW).await);
    assert!(db
        .qoder_http_publish_establish(
            &old,
            Established {
                uid: "synthetic-uid".into(),
                tokens: tokens(true, NOW + 600_000)
            },
            NOW
        )
        .is_err());
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn late_confirmation_cannot_replace_new_ready() {
    let (db, mut request) = fixture();
    let old = reserve_initial(&db, &request);
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    let result = perform_identity_action(
        &db,
        request,
        &transport,
        |_| {
            db.qoder_http_publish_establish(
                &old,
                Established {
                    uid: "synthetic-uid".into(),
                    tokens: tokens(true, NOW + 600_000),
                },
                NOW,
            )
            .unwrap();
            std::future::ready(true)
        },
        || NOW,
    )
    .await;
    assert!(matches!(
        result,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::OwnerChanged
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn source_edit_during_confirmation_blocks_before_exchange() {
    let (db, mut request) = fixture();
    unknown(&db, &request);
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    let result = perform_identity_action(
        &db,
        request,
        &transport,
        |_| {
            let mut p = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
            p.settings_config["env"]["OPENAI_API_KEY"] = "synthetic-replacement".into();
            db.save_provider("codex", &p).unwrap();
            std::future::ready(true)
        },
        || NOW,
    )
    .await;
    assert!(matches!(
        result,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::SourceChanged
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn explicit_rebind_confirms_once_and_changes_machine() {
    let (db, mut request) = fixture();
    let before = ready(&db, &request, true, NOW + 600_000);
    request.action = IdentityAction::Rebind;
    let mut transport = Transport::new();
    transport.uid = "synthetic-new-uid";
    completed(perform_identity_action(&db, request, &transport, |_| async { true }, || NOW).await);
    let source =
        SourceBinding::from_provider(&db.get_provider_by_id(ID, "codex").unwrap().unwrap())
            .unwrap()
            .0;
    let Acquisition::Ready(after) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!()
    };
    assert_ne!(after.machine_id(), before.machine_id());
    assert_eq!(after.uid(), "synthetic-new-uid");
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn mismatched_uid_never_silently_rebinds() {
    let (db, mut request) = fixture();
    unknown(&db, &request);
    request.action = IdentityAction::Recover;
    let mut transport = Transport::new();
    transport.uid = "synthetic-new-uid";
    assert!(matches!(
        perform_identity_action(&db, request, &transport, |_| async { true }, || NOW).await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::IdentityRebindRequired
        }
    ));
    assert_eq!(
        identity_context_admitted(&db, ID, NOW).unwrap().status,
        IdentityStatus::Unknown
    );
}
#[tokio::test]
async fn public_entry_rejects_missing_provider_before_authentication_or_confirmation() {
    let (db, mut request) = fixture();
    request.declaration.provider_id = "missing-provider".into();
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(&db, request, &transport, |_| async { panic!() }, || NOW).await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::SourceChanged
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
    assert_eq!(
        identity_context(&db, "missing-provider", NOW).status,
        IdentityStatus::Unavailable
    );
}

#[test]
fn fresh_context_is_unestablished_without_authentication_or_daily_device() {
    let (db, _) = fixture();
    let context = identity_context(&db, ID, NOW);
    assert_eq!(context.status, IdentityStatus::Unestablished);
    assert_eq!(context.allowed_actions, vec![IdentityAction::Establish]);
    assert!(context.expires_at_ms.is_none());
    assert!(
        crate::database::read_qoder_campaign_operation_rows(&db.conn.lock().unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn two_prepared_absent_receipts_cannot_both_reserve() {
    let (db, request) = fixture();
    let PreparedIdentityAction::Establish { receipt: first, .. } = db
        .qoder_http_prepare_identity_action(&request, NOW)
        .unwrap()
    else {
        panic!()
    };
    let PreparedIdentityAction::Establish {
        receipt: second, ..
    } = db
        .qoder_http_prepare_identity_action(&request, NOW)
        .unwrap()
    else {
        panic!()
    };
    db.qoder_http_reserve_identity_action(first).unwrap();
    assert!(db.qoder_http_reserve_identity_action(second).is_err());
}

#[test]
fn late_recovery_receipt_cannot_replace_new_pending() {
    let (db, mut request) = fixture();
    unknown(&db, &request);
    request.action = IdentityAction::Recover;
    let PreparedIdentityAction::Establish { receipt: first, .. } = db
        .qoder_http_prepare_identity_action(&request, NOW)
        .unwrap()
    else {
        panic!()
    };
    let PreparedIdentityAction::Establish {
        receipt: second, ..
    } = db
        .qoder_http_prepare_identity_action(&request, NOW)
        .unwrap()
    else {
        panic!()
    };
    db.qoder_http_reserve_identity_action(first).unwrap();
    assert!(db.qoder_http_reserve_identity_action(second).is_err());
    assert_eq!(
        identity_context_admitted(&db, ID, NOW).unwrap().status,
        IdentityStatus::Pending
    );
}

#[tokio::test]
async fn deletion_during_approval_has_zero_authentication_calls() {
    let (db, mut request) = fixture();
    unknown(&db, &request);
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| {
                db.delete_provider("codex", ID).unwrap();
                std::future::ready(true)
            },
            || NOW
        )
        .await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::SourceChanged
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn vault_epoch_change_during_approval_has_zero_authentication_calls() {
    let (db, mut request) = fixture();
    unknown(&db, &request);
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| {
                db.secrets.set_blocked(true);
                db.secrets.set_blocked(false);
                std::future::ready(true)
            },
            || NOW
        )
        .await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::OwnerChanged
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_source_generation_is_rejected_without_confirmation_or_authentication() {
    let (db, mut request) = fixture();
    request.declaration.credential_generation = "stale-generation".into();
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(&db, request, &transport, |_| async { panic!() }, || NOW).await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::SourceChanged
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn transport_failure_is_unknown_with_no_retry_and_no_pat_fallback() {
    let (db, request) = fixture();
    let mut transport = Transport::new();
    transport.fail = true;
    assert!(matches!(
        perform_identity_action(&db, request, &transport, |_| async { panic!() }, || NOW).await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::OutcomeUnknown
        }
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
    assert_eq!(
        identity_context_admitted(&db, ID, NOW).unwrap().status,
        IdentityStatus::Unknown
    );
}

#[test]
fn safe_failure_serialization_is_fixed_and_does_not_echo_private_error() {
    let result = IdentityActionResult::error(&AppError::Config("synthetic-secret-sentinel".into()));
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::json!({"status":"failed","code":"unavailable"})
    );
    assert_eq!(
        serde_json::to_value(IdentityActionResult::Cancelled).unwrap(),
        serde_json::json!({"status":"cancelled"})
    );
}

struct WaitingExchange {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: AtomicUsize,
}
impl AuthTransport for WaitingExchange {
    fn exchange_and_identify<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Established, ()>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(Established {
                uid: "synthetic-uid".into(),
                tokens: tokens(true, NOW + 600_000),
            })
        })
    }
    fn refresh<'a>(
        &'a self,
        _: &'a str,
        _: &'a Identity,
    ) -> Pin<Box<dyn Future<Output = Result<TokenPair, ()>> + Send + 'a>> {
        Box::pin(async { panic!() })
    }
}
#[tokio::test]
async fn submitted_action_holds_no_locks_and_rejects_late_source_replacement() {
    let (db, request) = fixture();
    let transport = WaitingExchange {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
    };
    let action = perform_identity_action(&db, request, &transport, |_| async { panic!() }, || NOW);
    let edit = async {
        transport.entered.notified().await;
        drop(db.secrets.write().unwrap());
        drop(db.conn.lock().unwrap());
        let mut p = db.get_provider_by_id(ID, "codex").unwrap().unwrap();
        p.settings_config["env"]["OPENAI_API_KEY"] = "synthetic-new-source".into();
        db.save_provider("codex", &p).unwrap();
        transport.release.notify_one();
    };
    let (result, ()) = tokio::join!(action, edit);
    assert!(matches!(result, IdentityActionResult::Failed { .. }));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        identity_context_admitted(&db, ID, NOW).unwrap().status,
        IdentityStatus::Unestablished
    );
}
#[tokio::test]
async fn lost_submitted_action_keeps_pending_and_does_not_replay() {
    let (db, request) = fixture();
    let transport = WaitingExchange {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
    };
    let mut future = Box::pin(perform_identity_action(
        &db,
        request,
        &transport,
        |_| async { panic!() },
        || NOW,
    ));
    tokio::select! {_=transport.entered.notified()=>{}, _=&mut future=>panic!("must remain pending")}
    drop(future);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        identity_context_admitted(&db, ID, NOW).unwrap().status,
        IdentityStatus::Pending
    );
}

#[tokio::test]
async fn restored_ready_requires_fresh_explicit_setup_without_second_approval() {
    let (db, request) = fixture();
    ready(&db, &request, true, NOW + 600_000);
    let sql = db.export_sql_string().unwrap();
    db.import_sql_string(&sql).unwrap();
    assert_eq!(
        identity_context_admitted(&db, ID, NOW).unwrap().status,
        IdentityStatus::Unestablished
    );
    let transport = Transport::new();
    completed(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| async { panic!("explicit setup is sufficient") },
            || NOW,
        )
        .await,
    );
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exceptional_confirmation_holds_neither_database_nor_vault_guard() {
    let (db, mut request) = fixture();
    unknown(&db, &request);
    request.action = IdentityAction::Recover;
    let transport = Transport::new();
    assert!(matches!(
        perform_identity_action(
            &db,
            request,
            &transport,
            |_| {
                drop(db.secrets.write().unwrap());
                drop(db.conn.lock().unwrap());
                std::future::ready(false)
            },
            || NOW
        )
        .await,
        IdentityActionResult::Cancelled
    ));
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}
