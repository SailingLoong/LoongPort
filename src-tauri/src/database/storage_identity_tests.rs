use super::*;
fn identity() -> PlaintextIdentity {
    PlaintextIdentity {
        format_version: 1,
        store_id: "82e2044b-f828-4e57-8d21-e933471c142a".into(),
        generation_id: "46573639-0ca9-4e54-a4b4-6491a9972faa".into(),
        revision: 1,
    }
}
#[test]
fn database_identity_is_explicit_and_not_missing_row_inferred() {
    let conn = Connection::open_in_memory().unwrap();
    assert_eq!(stored(&conn).unwrap(), None);
    assert!(check(&conn, &identity()).is_err());
    stamp(&conn, &identity()).unwrap();
    assert_eq!(stored(&conn).unwrap(), Some(identity()));
    check(&conn, &identity()).unwrap();
    conn.execute("DELETE FROM loongport_plaintext_storage", [])
        .unwrap();
    assert!(stored(&conn).is_err());
    assert!(stamp(&conn, &identity()).is_err());
}
#[test]
fn database_identity_rejects_revision_conflict_and_malformed_data() {
    let conn = Connection::open_in_memory().unwrap();
    stamp(&conn, &identity()).unwrap();
    let mut other = identity();
    other.revision = 2;
    assert!(check(&conn, &other).is_err());
    assert!(stamp(&conn, &other).is_err());
    assert_eq!(stored(&conn).unwrap(), Some(identity()));
    conn.execute("UPDATE loongport_plaintext_storage SET identity='{}'", [])
        .unwrap();
    assert!(stored(&conn).is_err());
}
#[test]
fn encrypted_metadata_is_never_replaced_by_plaintext_identity() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE loongport_vault(id INTEGER PRIMARY KEY,metadata TEXT NOT NULL); INSERT INTO loongport_vault VALUES(1,'old-encrypted-metadata');").unwrap();
    assert!(stamp(&conn, &identity()).is_err());
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='loongport_plaintext_storage'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        conn.query_row("SELECT metadata FROM loongport_vault", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "old-encrypted-metadata"
    );
}
