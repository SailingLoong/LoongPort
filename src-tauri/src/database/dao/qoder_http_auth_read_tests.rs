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
