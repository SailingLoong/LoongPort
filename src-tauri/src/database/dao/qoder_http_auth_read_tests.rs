//! Read-only admission must never spend an expiring refresh or change its owner.
use super::*;
use crate::services::provider::qoder_daily::http::{CredentialSource, PatDeclaration};
const NOW: i64 = 1_000_000;
fn fixture(kind: &str) -> (Database, PatDeclaration) {
    let db = Database::memory().unwrap();
    let mut provider = Provider::with_id(
        "cn-read-synthetic".into(),
        "Synthetic".into(),
        if kind == "qoder_cn_device" {
            serde_json::json!({})
        } else {
            serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-pat"}})
        },
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some(kind.into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let provider = db
        .get_provider_by_id(&provider.id, "codex")
        .unwrap()
        .unwrap();
    let source = SourceBinding::from_provider(&provider).unwrap().0;
    let declaration = PatDeclaration {
        provider_id: provider.id,
        credential_generation: source.credential_generation.clone(),
        source: source.source.declaration(),
    };
    (db, declaration)
}
fn reserve(db: &Database, d: &PatDeclaration) -> EstablishLease {
    let PreparedIdentityAction::Establish { receipt, .. } = db
        .qoder_http_prepare_identity_action(
            &IdentityActionRequest {
                declaration: d.clone(),
                action: IdentityAction::Establish,
            },
            NOW,
        )
        .unwrap()
    else {
        panic!("unestablished fixture")
    };
    db.qoder_http_reserve_identity_action(receipt).unwrap()
}
fn establish(db: &Database, d: &PatDeclaration, expires: i64) -> RequestSnapshot {
    let lease = reserve(db, d);
    db.qoder_http_publish_establish(
        &lease,
        Established {
            uid: "synthetic-owner".into(),
            tokens: TokenPair {
                access_token: "synthetic-access".into(),
                refresh_token: "synthetic-refresh".into(),
                expires_at_ms: expires,
            },
        },
        NOW,
    )
    .unwrap()
}
fn raw(db: &Database) -> String {
    db.conn
        .lock()
        .unwrap()
        .query_row("SELECT qoder_http_secret FROM providers", [], |r| r.get(0))
        .unwrap()
}
fn code<T>(result: Result<T, AppError>, wanted: &str) {
    assert!(
        matches!(result,Err(AppError::Config(ref c)) if c==&format!("qoder.http_auth.{wanted}"))
    );
}
#[test]
fn read_snapshot_does_not_refresh_near_expiry_or_write_after_expiry() {
    for kind in ["qoder_cn", "qoder_cn_device"] {
        let (db, d) = fixture(kind);
        establish(&db, &d, NOW + 1);
        let before = raw(&db);
        let read = db
            .qoder_http_read_snapshot(&d, NOW)
            .expect("near-expiry access remains usable for read only");
        assert_eq!(read.uid(), "synthetic-owner");
        assert_eq!(read.access_token(), "synthetic-access");
        assert_eq!(raw(&db), before);
        code(db.qoder_http_read_snapshot(&d, NOW + 1), "snapshot_expired");
        assert_eq!(raw(&db), before);
    }
}
#[test]
fn read_snapshot_rejects_missing_pending_unknown_and_declaration_changes_without_mutation() {
    let (db, mut d) = fixture("qoder_cn");
    code(db.qoder_http_read_snapshot(&d, NOW), "identity_required");
    let lease = reserve(&db, &d);
    let before = raw(&db);
    code(db.qoder_http_read_snapshot(&d, NOW), "attempt_in_progress");
    assert_eq!(raw(&db), before);
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    let before = raw(&db);
    code(
        db.qoder_http_read_snapshot(&d, NOW),
        "outcome_unknown_reestablish_required",
    );
    assert_eq!(raw(&db), before);
    d.source = CredentialSource::InternationalQoderPat;
    code(
        db.qoder_http_read_snapshot(&d, NOW),
        "source_declaration_required",
    );
    assert_eq!(raw(&db), before);
}
#[test]
fn read_snapshot_rechecks_original_owner_source_generation_and_vault_epoch() {
    let (db, mut d) = fixture("qoder_cn");
    let snapshot = establish(&db, &d, NOW + 600_000);
    d.credential_generation = "stale".into();
    code(
        db.qoder_http_read_snapshot(&d, NOW),
        "source_declaration_required",
    );
    db.secrets.set_blocked(true);
    assert!(db.check_qoder_http_snapshot(&snapshot, NOW).is_err());
    db.secrets.set_blocked(false);
    code(db.check_qoder_http_snapshot(&snapshot, NOW), "stale_owner");
    let mut provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    provider.meta.as_mut().unwrap().provider_type = Some("qoder_sdk".into());
    db.save_provider("codex", &provider).unwrap();
    assert!(db.check_qoder_http_snapshot(&snapshot, NOW).is_err());
}

#[test]
fn cn_free_benefit_consent_uses_version_two_without_a_fake_official_device() {
    use crate::services::provider::qoder_daily::automatic;
    for kind in ["qoder_cn", "qoder_cn_device"] {
        let (db, d) = fixture(kind);
        let mut provider = db
            .get_provider_by_id(&d.provider_id, "codex")
            .unwrap()
            .unwrap();
        let state = provider
            .meta
            .as_mut()
            .unwrap()
            .qoder_campaign_state
            .as_mut()
            .unwrap();
        state.automatic = automatic::Policy {
            enabled: true,
            revision: 1,
            policy_version: 2,
            subject: Some("synthetic-owner".into()),
            credential_generation: Some(state.generation.clone()),
        };
        assert!(
            automatic::active(&provider).is_some(),
            "CN v2 consent is source-bound, not an international device receipt"
        );
        provider
            .meta
            .as_mut()
            .unwrap()
            .qoder_campaign_state
            .as_mut()
            .unwrap()
            .automatic
            .policy_version = 1;
        assert!(
            automatic::active(&provider).is_none(),
            "daily v1 consent must never authorize all CN free benefits"
        );
    }
}

#[test]
fn cn_enrollment_binds_actual_owner_and_reauthorization_revokes_at_reservation() {
    use crate::services::provider::qoder_daily::automatic;
    for kind in ["qoder_cn", "qoder_cn_device"] {
        let (db, d) = fixture(kind);
        establish(&db, &d, 9_000_000_000_000);
        let request = automatic::Request {
            provider_id: d.provider_id.clone(),
            credential_generation: d.credential_generation.clone(),
            expected_revision: db
                .get_provider_by_id(&d.provider_id, "codex")
                .unwrap()
                .unwrap()
                .meta
                .unwrap()
                .qoder_campaign_state
                .unwrap()
                .automatic
                .revision,
            policy_version: 2,
            enabled: true,
            source: Some(d.source.clone()),
        };
        let enabled = automatic::set(&db, request);
        assert!(
            enabled.is_ok(),
            "CN enrollment must use its HTTP owner instead of international device validation"
        );
        let enabled = enabled.unwrap();
        assert!(enabled.enabled);
        assert_eq!(enabled.subject.as_deref(), Some("synthetic-owner"));
        let PreparedIdentityAction::Establish { receipt, .. } = db
            .qoder_http_prepare_identity_action(
                &IdentityActionRequest {
                    declaration: d.clone(),
                    action: IdentityAction::Rebind,
                },
                NOW,
            )
            .unwrap()
        else {
            panic!("rebind receipt")
        };
        let new_grant = db.qoder_http_reserve_identity_action(receipt).unwrap();
        let provider = db
            .get_provider_by_id(&d.provider_id, "codex")
            .unwrap()
            .unwrap();
        assert!(
            automatic::active(&provider).is_none(),
            "reserving new authorization revokes old all-free-benefit consent before network"
        );
        db.qoder_http_publish_establish(
            &new_grant,
            Established {
                uid: "synthetic-owner".into(),
                tokens: TokenPair {
                    access_token: "new-grant-access".into(),
                    refresh_token: "new-grant-refresh".into(),
                    expires_at_ms: 9_000_000_000_000,
                },
            },
            NOW,
        )
        .unwrap();
        let provider = db
            .get_provider_by_id(&d.provider_id, "codex")
            .unwrap()
            .unwrap();
        assert!(
            automatic::active(&provider).is_none(),
            "a completed same-UID grant cannot restore old consent"
        );
    }
}

fn cn_policy_enable(db: &Database, d: &PatDeclaration) {
    use crate::services::provider::qoder_daily::automatic;
    let revision = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
        .automatic
        .revision;
    automatic::set(
        db,
        automatic::Request {
            provider_id: d.provider_id.clone(),
            credential_generation: d.credential_generation.clone(),
            expected_revision: revision,
            policy_version: 2,
            enabled: true,
            source: Some(d.source.clone()),
        },
    )
    .unwrap();
}

#[test]
fn cn_policy_cannot_use_the_international_snapshot_free_dispatch() {
    use crate::services::provider::qoder_daily::automatic::{self, Admission, Lease};
    let (db, d) = fixture("qoder_cn");
    establish(&db, &d, 9_000_000_000_000);
    cn_policy_enable(&db, &d);
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let lease = Lease::from_provider(&provider, db.secrets.lifecycle_epoch()).unwrap();
    let admission = Admission { db: &db, lease };
    assert!(
        admission.dispatch(|| Ok(())).is_err(),
        "CN dispatch must require its exact HTTP owner snapshot"
    );
    assert!(automatic::active(&provider).is_some());
}

#[test]
fn cn_admission_refresh_preserves_consent_but_only_the_new_exact_snapshot_can_dispatch() {
    use crate::services::provider::qoder_daily::automatic::{Admission, Lease};
    let (db, d) = fixture("qoder_cn");
    let old = establish(&db, &d, 9_000_000_000_000);
    cn_policy_enable(&db, &d);
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let lease = Lease::from_provider(&provider, db.secrets.lifecycle_epoch()).unwrap();
    let admission = Admission {
        db: &db,
        lease: lease.clone(),
    };
    admission
        .dispatch_cn(&old, NOW, |conn| {
            conn.execute_batch("CREATE TABLE committed_intent(id TEXT)")?;
            Ok(())
        })
        .unwrap();
    let Acquisition::Refresh(refresh) = db
        .qoder_http_acquire(&old.stamp.source, 8_999_999_999_999)
        .unwrap()
    else {
        panic!("refresh")
    };
    assert!(admission.dispatch_cn(&old, NOW, |_| Ok(())).is_err());
    let new = db
        .qoder_http_publish_refresh(
            &refresh,
            TokenPair {
                access_token: "rotated-access".into(),
                refresh_token: "rotated-refresh".into(),
                expires_at_ms: 9_000_000_600_000,
            },
            8_999_999_999_999,
        )
        .unwrap();
    assert!(lease.matches(
        &db.get_provider_by_id(&d.provider_id, "codex")
            .unwrap()
            .unwrap()
    ));
    assert!(admission.dispatch_cn(&old, NOW, |_| Ok(())).is_err());
    admission
        .dispatch_cn(&new, 8_999_999_999_999, |_| Ok(()))
        .unwrap();
}

#[test]
fn cn_manual_admission_needs_no_opt_in_but_cannot_survive_policy_revision_or_epoch_changes() {
    use crate::services::provider::qoder_daily::automatic::{self, Admission, Lease};
    for change in ["opt_out", "lock", "reauthorize", "delete", "source"] {
        let (db, d) = fixture("qoder_cn_device");
        let snapshot = establish(&db, &d, 9_000_000_000_000);
        let provider = db
            .get_provider_by_id(&d.provider_id, "codex")
            .unwrap()
            .unwrap();
        assert!(automatic::active(&provider).is_none());
        let lease = Lease::for_cn_manual(&provider, &snapshot).unwrap();
        let admission = Admission { db: &db, lease };
        admission.dispatch_cn(&snapshot, NOW, |_| Ok(())).unwrap();
        match change {
            "opt_out" => {
                let revision = provider
                    .meta
                    .as_ref()
                    .unwrap()
                    .qoder_campaign_state
                    .as_ref()
                    .unwrap()
                    .automatic
                    .revision;
                automatic::set(
                    &db,
                    automatic::Request {
                        provider_id: d.provider_id.clone(),
                        credential_generation: d.credential_generation.clone(),
                        expected_revision: revision,
                        policy_version: 2,
                        enabled: false,
                        source: None,
                    },
                )
                .unwrap();
            }
            "lock" => {
                db.secrets.set_blocked(true);
                db.secrets.set_blocked(false);
            }
            "reauthorize" => {
                let PreparedIdentityAction::Establish { receipt, .. } = db
                    .qoder_http_prepare_identity_action(
                        &IdentityActionRequest {
                            declaration: d.clone(),
                            action: IdentityAction::Rebind,
                        },
                        NOW,
                    )
                    .unwrap()
                else {
                    panic!("rebind")
                };
                db.qoder_http_reserve_identity_action(receipt).unwrap();
            }
            "delete" => {
                db.conn
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM providers WHERE id=?1", [&d.provider_id])
                    .unwrap();
            }
            _ => {
                let mut changed = provider;
                changed.meta.as_mut().unwrap().provider_type = Some("qoder_cn".into());
                changed.settings_config =
                    serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-replacement"}});
                db.save_provider("codex", &changed).unwrap();
            }
        }
        assert!(
            admission.dispatch_cn(&snapshot, NOW, |_| Ok(())).is_err(),
            "{change}"
        );
    }
}

#[test]
fn cn_duplicate_consent_is_scoped_by_realm_and_blocks_same_realm_duplicates() {
    use crate::services::provider::qoder_daily::automatic::{self, Admission, Lease};
    let (db, d) = fixture("qoder_cn");
    let snapshot = establish(&db, &d, 9_000_000_000_000);
    cn_policy_enable(&db, &d);
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    for (id, kind, version) in [
        ("intl-same-uid", "qoder_sdk", 1),
        ("cn-same-uid", "qoder_cn", 2),
    ] {
        let mut other = Provider::with_id(
            id.into(),
            "Synthetic".into(),
            serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-other"}}),
            None,
        );
        other.meta = Some(crate::provider::ProviderMeta {
            provider_type: Some(kind.into()),
            ..Default::default()
        });
        db.save_provider("codex", &other).unwrap();
        let other = db.get_provider_by_id(id, "codex").unwrap().unwrap();
        let source = SourceBinding::from_provider(&other).unwrap().0;
        let request = automatic::Request {
            provider_id: id.into(),
            credential_generation: source.credential_generation.clone(),
            expected_revision: 0,
            policy_version: version,
            enabled: true,
            source: Some(source.source.declaration()),
        };
        if version == 1 {
            db.save_qoder_automatic(
                &request,
                Some("synthetic-owner".into()),
                db.secrets.lifecycle_epoch(),
            )
            .unwrap();
        } else {
            let declaration = PatDeclaration {
                provider_id: id.into(),
                credential_generation: source.credential_generation,
                source: source.source.declaration(),
            };
            establish(&db, &declaration, 9_000_000_000_000);
            let mut request = request;
            request.expected_revision = db
                .get_provider_by_id(id, "codex")
                .unwrap()
                .unwrap()
                .meta
                .unwrap()
                .qoder_campaign_state
                .unwrap()
                .automatic
                .revision;
            assert!(automatic::set(&db, request).is_err());
        }
    }
    let admission = Admission {
        db: &db,
        lease: Lease::from_provider(&provider, db.secrets.lifecycle_epoch()).unwrap(),
    };
    admission.dispatch_cn(&snapshot, NOW, |_| Ok(())).unwrap();
}

#[test]
fn cn_first_poll_barrier_rechecks_expiry_and_releases_owner_before_the_second_poll() {
    use crate::services::provider::qoder_daily::automatic::{Admission, Lease};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (db, d) = fixture("qoder_cn");
    let snapshot = establish(&db, &d, 9_000_000_000_000);
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let admission = Admission {
        db: &db,
        lease: Lease::for_cn_manual(&provider, &snapshot).unwrap(),
    };
    let polls = AtomicUsize::new(0);
    let out = crate::rt::block_on(admission.send_cn(
        &snapshot,
        || NOW,
        |_| Ok(()),
        || {
            std::future::poll_fn(|cx| {
                if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                    assert!(db.conn.try_lock().is_err());
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                } else {
                    assert!(db.conn.try_lock().is_ok());
                    std::task::Poll::Ready(Ok(reqwest::Response::from(
                        http_response_for_policy_test(),
                    )))
                }
            })
        },
    ));
    assert!(out.is_ok());
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    let created = AtomicUsize::new(0);
    let expired = crate::rt::block_on(admission.send_cn(
        &snapshot,
        || 9_000_000_000_000,
        |_| Ok(()),
        || {
            created.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(reqwest::Response::from(http_response_for_policy_test())))
        },
    ));
    assert!(expired.is_err());
    assert_eq!(created.load(Ordering::SeqCst), 0);
}
fn http_response_for_policy_test() -> http::Response<String> {
    http::Response::builder()
        .status(200)
        .body(String::new())
        .unwrap()
}

#[test]
fn cn_final_poll_checks_expiry_after_waiting_for_original_sqlite_owner() {
    use crate::services::provider::qoder_daily::automatic::{Admission, Lease};
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    let (db, d) = fixture("qoder_cn");
    let snapshot = establish(&db, &d, NOW + 1);
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let admission = Admission {
        db: &db,
        lease: Lease::for_cn_manual(&provider, &snapshot).unwrap(),
    };
    let now = AtomicI64::new(NOW);
    let created = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (held, wait) = std::sync::mpsc::channel();
        let db = &db;
        let now = &now;
        scope.spawn(move || {
            let _guard = db.conn.lock().unwrap();
            held.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
            now.store(NOW + 1, Ordering::SeqCst);
        });
        wait.recv().unwrap();
        let result = crate::rt::block_on(admission.send_cn(
            &snapshot,
            || now.load(Ordering::SeqCst),
            |_| Ok(()),
            || {
                created.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(reqwest::Response::from(http_response_for_policy_test())))
            },
        ));
        assert!(result.is_err());
    });
    assert_eq!(created.load(Ordering::SeqCst), 0);
}

#[test]
fn cn_consent_context_is_default_off_and_enable_requires_exact_source_revision_version() {
    use crate::services::provider::qoder_daily::automatic;
    for kind in ["qoder_cn", "qoder_cn_device"] {
        let (db, d) = fixture(kind);
        let before = automatic::context(&db, &d.provider_id).unwrap();
        assert_eq!(before.policy_version, 2);
        assert!(!before.enabled && !before.can_enable);
        establish(&db, &d, 9_000_000_000_000);
        let ready = automatic::context(&db, &d.provider_id).unwrap();
        assert!(!ready.enabled && ready.can_enable);
        for change in 0..4 {
            let mut request = automatic::Request {
                provider_id: d.provider_id.clone(),
                credential_generation: d.credential_generation.clone(),
                expected_revision: ready.revision,
                policy_version: 2,
                enabled: true,
                source: Some(d.source.clone()),
            };
            match change {
                0 => request.source = None,
                1 => request.policy_version = 1,
                2 => request.expected_revision += 1,
                _ => request.credential_generation.push_str("-stale"),
            };
            assert!(automatic::set(&db, request).is_err());
        }
        assert!(!automatic::context(&db, &d.provider_id).unwrap().enabled);
    }
}
