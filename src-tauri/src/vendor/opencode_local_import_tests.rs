use super::*;
use rusqlite::{params, Connection};
use serde_json::json;
const KEY: &str = "oc_sk_fixture_SyntheticSecretOnly1234";
fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE credential (id TEXT PRIMARY KEY,integration_id TEXT,label TEXT,value TEXT,active INTEGER,time_updated INTEGER);").unwrap();
    db.execute(
        "INSERT INTO credential VALUES ('cred_test','opencode','Work',?1,1,1)",
        params![json!({"type":"key","key":KEY}).to_string()],
    )
    .unwrap();
    (dir, path, db)
}
#[test]
fn explicit_read_and_save_consents_are_required_and_no_file_is_created() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("absent.db");
    let mut s = Session::default();
    assert_eq!(
        s.preview(&p, false, Instant::now()).err(),
        Some("consent_required")
    );
    assert!(!p.exists());
    let (_d, p, _db) = fixture();
    let now = Instant::now();
    let preview = s.preview(&p, true, now).unwrap();
    assert_eq!(
        s.take(&p, &preview.preview_id, &entry_id("cred_test"), false, now)
            .err(),
        Some("consent_required")
    );
}
#[test]
fn preview_contains_no_secret_and_commit_returns_only_selected_key_once() {
    let (_d, p, db) = fixture();
    let before = std::fs::read(&p).unwrap();
    let mut s = Session::default();
    let now = Instant::now();
    db.execute(
        "INSERT INTO credential VALUES ('cred_other','openai','Other',?1,1,1)",
        params![json!({"type":"key","key":"UNRELATED_SECRET"}).to_string()],
    )
    .unwrap();
    let after_fixture = std::fs::read(&p).unwrap();
    assert_ne!(before, after_fixture);
    let preview = s.preview(&p, true, now).unwrap();
    assert_eq!(preview.entries.len(), 1);
    assert!(preview.entries[0].is_active);
    let wire = serde_json::to_string(&preview).unwrap();
    assert!(!wire.contains(KEY));
    assert!(!wire.contains("UNRELATED_SECRET"));
    let selected = s
        .take(&p, &preview.preview_id, &entry_id("cred_test"), true, now)
        .unwrap();
    assert_eq!(selected.key, KEY);
    assert_eq!(selected.label, "Work");
    assert!(s
        .take(&p, &preview.preview_id, &entry_id("cred_test"), true, now)
        .is_err());
    assert_eq!(std::fs::read(&p).unwrap(), after_fixture);
}
#[test]
fn cancel_expiry_and_replaced_preview_cannot_import() {
    let (_d, p, _db) = fixture();
    let mut s = Session::default();
    let now = Instant::now();
    let a = s.preview(&p, true, now).unwrap();
    s.cancel(&a.preview_id);
    assert!(s
        .take(&p, &a.preview_id, &entry_id("cred_test"), true, now)
        .is_err());
    let a = s.preview(&p, true, now).unwrap();
    assert_eq!(
        s.take(
            &p,
            &a.preview_id,
            &entry_id("cred_test"),
            true,
            now + Duration::from_secs(301)
        )
        .err(),
        Some("preview_expired")
    );
    let a = s.preview(&p, true, now).unwrap();
    let b = s.preview(&p, true, now).unwrap();
    assert!(s
        .take(&p, &a.preview_id, &entry_id("cred_test"), true, now)
        .is_err());
    assert!(s
        .take(&p, &b.preview_id, &entry_id("cred_test"), true, now)
        .is_ok());
}
#[test]
fn account_switch_key_rotation_deletion_and_path_change_fail_closed() {
    for sql in [
        "UPDATE credential SET active=0",
        "UPDATE credential SET value='{}'",
        "DELETE FROM credential",
    ] {
        let (_d, p, db) = fixture();
        let mut s = Session::default();
        let now = Instant::now();
        let a = s.preview(&p, true, now).unwrap();
        db.execute_batch(sql).unwrap();
        assert_eq!(
            s.take(&p, &a.preview_id, &entry_id("cred_test"), true, now)
                .err(),
            Some("source_changed")
        );
    }
    let (_d, p, _db) = fixture();
    let (_d2, p2, _db2) = fixture();
    let mut s = Session::default();
    let now = Instant::now();
    let a = s.preview(&p, true, now).unwrap();
    assert_eq!(
        s.take(&p2, &a.preview_id, &entry_id("cred_test"), true, now)
            .err(),
        Some("source_changed")
    );
}
#[test]
fn oauth_and_unknown_values_are_reported_without_import_or_token_exposure() {
    let (_d, p, db) = fixture();
    db.execute("INSERT INTO credential VALUES ('oauth','opencode','OAuth',?1,0,1)",params![json!({"type":"oauth","access":"OAUTH_ACCESS_SECRET","refresh":"OAUTH_REFRESH_SECRET","expires":1}).to_string()]).unwrap();
    let mut s = Session::default();
    let now = Instant::now();
    let a = s.preview(&p, true, now).unwrap();
    let wire = serde_json::to_string(&a).unwrap();
    assert!(!wire.contains("OAUTH_ACCESS_SECRET"));
    assert!(!wire.contains("OAUTH_REFRESH_SECRET"));
    assert!(a.notices.iter().any(|x| x == "oauth_not_imported"));
    assert!(s
        .take(&p, &a.preview_id, &entry_id("oauth"), true, now)
        .is_err());
}
#[test]
fn nonofficial_server_malformed_or_masked_keys_are_never_eligible() {
    for value in [
        json!({"type":"key","key":KEY,"metadata":{"server":"https://evil.invalid"}}),
        json!({"type":"key","key":"oc_sk_mask_***"}),
        json!({"type":"api","key":KEY}),
        json!({"type":"external"}),
        json!({"type":"key","key":KEY,"metadata":{"orgID":"org_private"}}),
    ] {
        let (_d, p, db) = fixture();
        db.execute("UPDATE credential SET value=?1", params![value.to_string()])
            .unwrap();
        let mut s = Session::default();
        let a = s.preview(&p, true, Instant::now()).unwrap();
        assert!(a.entries.is_empty());
    }
}
#[test]
fn database_lock_missing_file_and_schema_error_do_not_create_or_modify_anything() {
    let (_d, p, db) = fixture();
    let mut s = Session::default();
    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert_eq!(
        s.preview(&p, true, Instant::now()).err(),
        Some("source_locked")
    );
    db.execute_batch("ROLLBACK").unwrap();
    db.execute_batch("DROP TABLE credential").unwrap();
    assert_eq!(
        s.preview(&p, true, Instant::now()).err(),
        Some("unsupported_schema")
    );
    let absent = p.with_file_name("no.db");
    assert_eq!(
        s.preview(&absent, true, Instant::now()).err(),
        Some("source_unavailable")
    );
    assert!(!absent.exists());
}
#[test]
fn labels_cannot_echo_the_key_and_repeated_read_requires_new_ticket() {
    let (_d, p, db) = fixture();
    db.execute(
        "UPDATE credential SET label=?1",
        params![format!("My {KEY}")],
    )
    .unwrap();
    let mut s = Session::default();
    let now = Instant::now();
    let a = s.preview(&p, true, now).unwrap();
    assert!(!serde_json::to_string(&a).unwrap().contains(KEY));
    let b = s.preview(&p, true, now).unwrap();
    assert_ne!(a.preview_id, b.preview_id);
}

#[test]
fn wal_updates_are_read_instead_of_a_stale_immutable_snapshot() {
    let (_d, p, db) = fixture();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    db.execute("UPDATE credential SET label='Latest in WAL'", [])
        .unwrap();
    let mut s = Session::default();
    let now = Instant::now();
    let a = s.preview(&p, true, now).unwrap();
    assert_eq!(a.entries[0].label, "Latest in WAL");
    db.execute("UPDATE credential SET label='Changed in WAL'", [])
        .unwrap();
    assert_eq!(
        s.take(&p, &a.preview_id, &a.entries[0].id, true, now).err(),
        Some("source_changed")
    );
}
#[test]
fn size_and_count_limits_and_view_substitution_fail_closed() {
    let (_d, p, db) = fixture();
    db.execute(
        "UPDATE credential SET value=?1",
        params![json!({"type":"key","key":KEY,"extra":"x".repeat(70000)}).to_string()],
    )
    .unwrap();
    let mut s = Session::default();
    assert!(s
        .preview(&p, true, Instant::now())
        .unwrap()
        .entries
        .is_empty());
    db.execute_batch("ALTER TABLE credential RENAME TO private_store; CREATE VIEW credential AS SELECT * FROM private_store;").unwrap();
    assert_eq!(
        s.preview(&p, true, Instant::now()).err(),
        Some("unsupported_schema")
    );
    db.execute_batch("DROP VIEW credential; ALTER TABLE private_store RENAME TO credential;")
        .unwrap();
    for i in 0..100 {
        db.execute(
            "INSERT INTO credential SELECT ?1,'opencode','extra',value,0,1 FROM credential LIMIT 1",
            params![format!("cred_extra_{i}")],
        )
        .unwrap();
    }
    assert_eq!(
        s.preview(&p, true, Instant::now()).err(),
        Some("too_many_credentials")
    );
}
