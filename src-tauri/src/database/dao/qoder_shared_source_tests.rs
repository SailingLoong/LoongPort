//! Synthetic original encrypted owner; no HTTP, worker or real account state.
use super::*;
use crate::services::provider::qoder_daily::http::PatDeclaration;
const NOW: i64 = 1_000_000;

fn add(db: &Database, id: &str, kind: &str) -> PatDeclaration {
    let mut provider = Provider::with_id(
        id.into(),
        "Synthetic routing owner".into(),
        if kind == "qoder_cn_device" {
            serde_json::json!({})
        } else {
            serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-original-pat"}})
        },
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some(kind.into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let provider = db.get_provider_by_id(id, "codex").unwrap().unwrap();
    let source = SourceBinding::from_provider(&provider).unwrap().0;
    PatDeclaration {
        provider_id: id.into(),
        credential_generation: source.credential_generation,
        source: source.source.declaration(),
    }
}
fn reserve(db: &Database, declaration: &PatDeclaration) -> EstablishLease {
    let PreparedIdentityAction::Establish { receipt, .. } = db
        .qoder_http_prepare_identity_action(
            &IdentityActionRequest {
                declaration: declaration.clone(),
                action: IdentityAction::Establish,
            },
            NOW,
        )
        .unwrap()
    else {
        panic!("synthetic owner must need establishment")
    };
    db.qoder_http_reserve_identity_action(receipt).unwrap()
}
fn establish(
    db: &Database,
    declaration: &PatDeclaration,
    uid: &str,
    expires: i64,
    refresh: &str,
) -> RequestSnapshot {
    let lease = reserve(db, declaration);
    db.qoder_http_publish_establish(
        &lease,
        Established {
            uid: uid.into(),
            tokens: TokenPair {
                access_token: "synthetic-access".into(),
                refresh_token: refresh.into(),
                expires_at_ms: expires,
            },
        },
        NOW,
    )
    .unwrap()
}
fn fact(db: &Database, id: &str, now: i64) -> Result<Option<RoutingSourceFact>, AppError> {
    let vault = db.secrets.read()?;
    let conn = db.conn.lock()?;
    let provider = Database::get_provider_by_id_on_connection(&conn, &vault, id, "codex")?.unwrap();
    let changes = conn.total_changes();
    let before: String = conn.query_row(
        "SELECT qoder_http_secret FROM providers WHERE id=?1 AND app_type='codex'",
        [id],
        |r| r.get(0),
    )?;
    let result = routing_source_on(&conn, &vault, &provider, now);
    let after: String = conn.query_row(
        "SELECT qoder_http_secret FROM providers WHERE id=?1 AND app_type='codex'",
        [id],
        |r| r.get(0),
    )?;
    assert_eq!(
        before, after,
        "source qualification must not refresh or rewrite the owner"
    );
    assert_eq!(changes, conn.total_changes());
    result
}

#[cfg_attr(test, test)]
fn ready_source_is_opaque_and_readonly_for_every_original_credential_family() {
    let db = Database::memory().unwrap();
    for (kind, platform) in [
        ("qoder_sdk", "qoder_sdk"),
        ("qoder_cn", "qoder_cn"),
        ("qoder_cn_device", "qoder_cn"),
    ] {
        let d = add(&db, kind, kind);
        let snapshot = establish(
            &db,
            &d,
            "synthetic-verified-subject",
            NOW + 600_000,
            "synthetic-refresh",
        );
        let result = fact(&db, kind, NOW)
            .unwrap()
            .expect("authorized original source must be projected");
        assert_eq!(result.platform, platform);
        assert_eq!(result.account_id.len(), 64);
        assert_eq!(result.stable_evidence.len(), 64);
        let visible = format!("{result:?} {}", serde_json::to_string(&result).unwrap());
        for private in [
            "synthetic-verified-subject",
            "synthetic-original-pat",
            "synthetic-access",
            "synthetic-refresh",
            snapshot.machine_id(),
        ] {
            assert!(!visible.contains(private));
        }
    }
}

#[cfg_attr(test, test)]
fn stable_account_identity_uses_verified_subject_and_realm_not_provider_row() {
    let db = Database::memory().unwrap();
    for (id, kind, uid) in [
        ("cn-pat", "qoder_cn", "same-subject"),
        ("cn-device", "qoder_cn_device", "same-subject"),
        ("international", "qoder_sdk", "same-subject"),
        ("other-subject", "qoder_cn", "another-subject"),
    ] {
        let d = add(&db, id, kind);
        establish(&db, &d, uid, NOW + 600_000, "synthetic-refresh");
    }
    let cn = fact(&db, "cn-pat", NOW).unwrap().unwrap();
    let device = fact(&db, "cn-device", NOW).unwrap().unwrap();
    assert_eq!(cn.account_id, device.account_id);
    assert_ne!(
        cn.stable_evidence, device.stable_evidence,
        "same-account projections retain exact owner incarnation evidence"
    );
    assert_ne!(
        cn.account_id,
        fact(&db, "international", NOW).unwrap().unwrap().account_id
    );
    assert_ne!(
        cn.account_id,
        fact(&db, "other-subject", NOW).unwrap().unwrap().account_id
    );
}

#[cfg_attr(test, test)]
fn normal_refresh_preserves_canonical_identity_and_stable_evidence_but_retires_old_lease() {
    let db = Database::memory().unwrap();
    let d = add(&db, "refresh-owner", "qoder_cn");
    let old = establish(&db, &d, "synthetic-subject", NOW + 1, "synthetic-refresh");
    let before = fact(&db, &d.provider_id, NOW).unwrap().unwrap();
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let source = SourceBinding::from_provider(&provider).unwrap().0;
    let Acquisition::Refresh(lease) = db
        .qoder_http_acquire_for_route(
            &source,
            NOW,
            Some(&before),
            Some(db.secrets.lifecycle_epoch()),
        )
        .unwrap()
    else {
        panic!("refresh required")
    };
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_none());
    let new = db
        .qoder_http_publish_refresh(
            &lease,
            TokenPair {
                access_token: "synthetic-rotated-access".into(),
                refresh_token: "synthetic-rotated-refresh".into(),
                expires_at_ms: NOW + 900_000,
            },
            NOW,
        )
        .unwrap();
    assert_eq!(before, fact(&db, &d.provider_id, NOW).unwrap().unwrap());
    assert!(new.stamp.generation > old.stamp.generation);
    assert!(db.check_qoder_http_snapshot_fence(&old).is_err());
    db.check_qoder_http_snapshot_fence(&new).unwrap();
}

#[cfg_attr(test, test)]
fn missing_pending_unknown_and_unrefreshable_sources_are_not_qualified() {
    let db = Database::memory().unwrap();
    let d = add(&db, "unready-owner", "qoder_cn");
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_none());
    let lease = reserve(&db, &d);
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_none());
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_none());
    let d = add(&db, "access-only", "qoder_sdk");
    establish(
        &db,
        &d,
        "synthetic-subject",
        NOW + REFRESH_MARGIN_MS + 1,
        "",
    );
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_some());
    assert!(fact(&db, &d.provider_id, NOW + 1).unwrap().is_none());
    let d = add(&db, "expired-refreshable", "qoder_cn");
    establish(&db, &d, "synthetic-subject", NOW + 1, "synthetic-refresh");
    assert!(
        fact(&db, &d.provider_id, NOW + 2).unwrap().is_some(),
        "original acquire may refresh; source reads must not add an expiry ban"
    );
}

#[cfg_attr(test, test)]
fn corrupt_and_changed_source_records_are_errors_without_writes() {
    let db = Database::memory().unwrap();
    let d = add(&db, "corrupt-owner", "qoder_cn");
    establish(
        &db,
        &d,
        "synthetic-subject",
        NOW + 600_000,
        "synthetic-refresh",
    );
    {
        let vault = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        let mut provider =
            Database::get_provider_by_id_on_connection(&conn, &vault, &d.provider_id, "codex")
                .unwrap()
                .unwrap();
        provider.settings_config["env"]["OPENAI_API_KEY"] = serde_json::json!("different-pat");
        assert!(routing_source_on(&conn, &vault, &provider, NOW).is_err());
        assert!(routing_source_on(&conn, &vault, &provider, -1).is_err());
    }
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE providers SET qoder_http_secret='not-sealed' WHERE id=?1",
            [&d.provider_id],
        )
        .unwrap();
    assert!(fact(&db, &d.provider_id, NOW).is_err());
}

#[cfg_attr(test, test)]
fn owner_incarnation_changes_evidence_without_inventing_a_new_service_account() {
    let db = Database::memory().unwrap();
    let d = add(&db, "recreated-owner", "qoder_cn");
    establish(
        &db,
        &d,
        "synthetic-subject",
        NOW + 600_000,
        "synthetic-refresh",
    );
    let before = fact(&db, &d.provider_id, NOW).unwrap().unwrap();
    // Exercise the original encrypted record's identity incarnation, not a fabricated public DTO.
    {
        let vault = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        let (raw, record) = read_on(&conn, &vault, &d.provider_id).unwrap();
        let mut record = record.unwrap();
        if let Phase::Ready { identity, .. } = &mut record.phase {
            identity.machine_id = uuid::Uuid::new_v4().to_string();
        }
        record.generation = record.next_generation().unwrap();
        publish_on(&conn, &vault, &d.provider_id, &raw, &record).unwrap();
    }
    let after = fact(&db, &d.provider_id, NOW).unwrap().unwrap();
    assert_eq!(before.account_id, after.account_id);
    assert_ne!(before.stable_evidence, after.stable_evidence);
}

#[cfg_attr(test, test)]
fn routed_acquisition_rejects_changed_identity_before_refresh_pending_is_written() {
    for change_uid in [true, false] {
        let db = Database::memory().unwrap();
        let d = add(&db, "route-owner", "qoder_cn");
        establish(&db, &d, "synthetic-subject", NOW + 1, "synthetic-refresh");
        let expected = fact(&db, &d.provider_id, NOW).unwrap().unwrap();
        let provider = db
            .get_provider_by_id(&d.provider_id, "codex")
            .unwrap()
            .unwrap();
        let source = SourceBinding::from_provider(&provider).unwrap().0;
        let before = {
            let vault = db.secrets.read().unwrap();
            let conn = db.conn.lock().unwrap();
            let (raw, record) = read_on(&conn, &vault, &d.provider_id).unwrap();
            let mut record = record.unwrap();
            if let Phase::Ready { identity, .. } = &mut record.phase {
                if change_uid {
                    identity.uid = "synthetic-replaced-subject".into();
                } else {
                    identity.machine_id = uuid::Uuid::new_v4().to_string();
                }
            }
            record.generation = record.next_generation().unwrap();
            publish_on(&conn, &vault, &d.provider_id, &raw, &record).unwrap();
            read_on(&conn, &vault, &d.provider_id).unwrap().0
        };
        let result = db.qoder_http_acquire_for_route(
            &source,
            NOW,
            Some(&expected),
            Some(db.secrets.lifecycle_epoch()),
        );
        assert!(
            matches!(result,Err(AppError::Config(ref message)) if message=="qoder.http_auth.selected_identity_changed")
        );
        let vault = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            before,
            read_on(&conn, &vault, &d.provider_id).unwrap().0,
            "stale route cannot consume refresh into Pending"
        );
    }
}

#[cfg_attr(test, test)]
fn routed_acquisition_rejects_retired_epoch_before_refresh_pending_is_written() {
    let db = Database::memory().unwrap();
    let d = add(&db, "epoch-owner", "qoder_cn");
    establish(&db, &d, "synthetic-subject", NOW + 1, "synthetic-refresh");
    let expected = fact(&db, &d.provider_id, NOW).unwrap().unwrap();
    let epoch = db.secrets.lifecycle_epoch();
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let source = SourceBinding::from_provider(&provider).unwrap().0;
    db.secrets.set_blocked(true);
    db.secrets.set_blocked(false);
    assert_eq!(expected, fact(&db, &d.provider_id, NOW).unwrap().unwrap());
    let before = {
        let vault = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        read_on(&conn, &vault, &d.provider_id).unwrap().0
    };
    let result = db.qoder_http_acquire_for_route(&source, NOW, Some(&expected), Some(epoch));
    assert!(
        matches!(result, Err(AppError::Config(ref message)) if message == "qoder.http_auth.stale_owner")
    );
    {
        let vault = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(before, read_on(&conn, &vault, &d.provider_id).unwrap().0);
    }
    assert!(matches!(
        db.qoder_http_acquire_for_route(
            &source,
            NOW,
            Some(&expected),
            Some(db.secrets.lifecycle_epoch())
        ),
        Ok(Acquisition::Refresh(_))
    ));
}

fn known_identity(db: &Database, id: &str) -> Option<RoutingIdentityFact> {
    let vault = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    let provider = Database::get_provider_by_id_on_connection(&conn, &vault, id, "codex")
        .unwrap()
        .unwrap();
    let changes = conn.total_changes();
    let before = read_on(&conn, &vault, id).unwrap().0;
    let result = routing_identity_on(&conn, &vault, &provider, NOW).unwrap();
    assert_eq!(before, read_on(&conn, &vault, id).unwrap().0);
    assert_eq!(changes, conn.total_changes());
    result
}

#[cfg_attr(test, test)]
fn refresh_pending_preserves_known_capability_without_becoming_send_eligible() {
    let db = Database::memory().unwrap();
    let d = add(&db, "pending-capability", "qoder_cn");
    assert!(known_identity(&db, &d.provider_id).is_none());
    let lease = reserve(&db, &d);
    assert!(known_identity(&db, &d.provider_id).is_none());
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    assert!(known_identity(&db, &d.provider_id).is_none());
    let d = add(&db, "pending-verified-capability", "qoder_cn");
    establish(&db, &d, "synthetic-subject", NOW + 1, "synthetic-refresh");
    let ready = known_identity(&db, &d.provider_id).unwrap();
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let source = SourceBinding::from_provider(&provider).unwrap().0;
    let Acquisition::Refresh(lease) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!("original acquire must reserve refresh");
    };
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_none());
    assert_eq!(
        known_identity(&db, &d.provider_id),
        Some(RoutingIdentityFact {
            fact: ready.fact,
            availability: RoutingSourceAvailability::RefreshPending
        }),
        "verified Pending identity must retain app capability, not send qualification"
    );
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    assert!(known_identity(&db, &d.provider_id).is_none());
}

#[cfg_attr(test, test)]
fn expired_access_only_retains_identity_without_send_admission() {
    let db = Database::memory().unwrap();
    let d = add(&db, "expired-identity", "qoder_sdk");
    establish(&db, &d, "synthetic-subject", NOW + REFRESH_MARGIN_MS, "");
    assert!(fact(&db, &d.provider_id, NOW).unwrap().is_none());
    assert_eq!(
        known_identity(&db, &d.provider_id).unwrap().availability,
        RoutingSourceAvailability::CredentialsExpired
    );
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify(case: &str) {
    match case {
        "expired_identity" => expired_access_only_retains_identity_without_send_admission(),
        "pending_identity" => {
            refresh_pending_preserves_known_capability_without_becoming_send_eligible()
        }
        "ready" => ready_source_is_opaque_and_readonly_for_every_original_credential_family(),
        "identity" => stable_account_identity_uses_verified_subject_and_realm_not_provider_row(),
        "refresh" => {
            normal_refresh_preserves_canonical_identity_and_stable_evidence_but_retires_old_lease()
        }
        "unready" => missing_pending_unknown_and_unrefreshable_sources_are_not_qualified(),
        "corrupt" => corrupt_and_changed_source_records_are_errors_without_writes(),
        "incarnation" => {
            owner_incarnation_changes_evidence_without_inventing_a_new_service_account()
        }
        "route_fence" => {
            routed_acquisition_rejects_changed_identity_before_refresh_pending_is_written()
        }
        "epoch" => routed_acquisition_rejects_retired_epoch_before_refresh_pending_is_written(),
        _ => panic!("unknown synthetic Qoder source case"),
    }
}
