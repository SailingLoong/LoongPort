use super::*;
use crate::secrets::storage::{PlaintextIdentity, StorageContext};

fn plaintext() -> StorageContext {
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: "82e2044b-f828-4e57-8d21-e933471c142a".into(),
        generation_id: "46573639-0ca9-4e54-a4b4-6491a9972faa".into(),
        revision: 1,
    })
}

#[test]
fn plaintext_column_codec_preserves_text_without_encrypted_envelope() {
    let context = plaintext();
    let text = " {\"token\":\"example-secret\"}\n";
    let stored = seal_db(
        &context,
        "providers",
        "settings_config",
        &["p", "claude"],
        text,
    )
    .unwrap();
    assert_eq!(stored, text);
    assert_eq!(
        open_db(
            &context,
            "providers",
            "settings_config",
            &["p", "claude"],
            &stored
        )
        .unwrap(),
        text
    );
}

#[test]
fn plaintext_column_codec_rejects_reserved_envelopes_in_both_directions() {
    let context = plaintext();
    for value in ["lpenc", "lpenc:old-value", "lpenc2:future-value"] {
        assert!(seal_db(&context, "settings", "value", &["global_proxy_url"], value).is_err());
        assert!(open_db(&context, "settings", "value", &["global_proxy_url"], value).is_err());
    }
}

#[test]
fn encrypted_column_codec_keeps_authentication_and_identity_binding() {
    let context = StorageContext::from(VaultContext::generate().unwrap());
    let stored = seal_db(
        &context,
        "settings",
        "value",
        &["global_proxy_url"],
        "example-secret",
    )
    .unwrap();
    assert!(stored.starts_with("lpenc"));
    assert_eq!(
        open_db(
            &context,
            "settings",
            "value",
            &["global_proxy_url"],
            &stored
        )
        .unwrap(),
        "example-secret"
    );
    assert!(open_db(
        &context,
        "settings",
        "value",
        &["claude_desktop_gateway_token"],
        &stored
    )
    .is_err());
    assert!(open_db(
        &context,
        "settings",
        "value",
        &["global_proxy_url"],
        "example-secret"
    )
    .is_err());
}

#[test]
fn plaintext_inventory_validation_is_read_only_and_rejects_mixed_protected_values() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT); INSERT INTO settings VALUES ('global_proxy_url','https://proxy.example'),('ordinary_flag','lpenc:not-protected');").unwrap();
    validate_storage_database(&conn, &plaintext()).unwrap();
    conn.execute(
        "UPDATE settings SET value='lpenc:old-value' WHERE key='global_proxy_url'",
        [],
    )
    .unwrap();
    assert!(validate_storage_database(&conn, &plaintext()).is_err());
    assert_eq!(
        conn.query_row(
            "SELECT value FROM settings WHERE key='global_proxy_url'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "lpenc:old-value"
    );
}
