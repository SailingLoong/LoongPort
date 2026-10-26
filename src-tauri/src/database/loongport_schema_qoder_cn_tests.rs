//! Regression fixtures for the independently published client-access v28 and
//! Qoder CN v29 owners. Earlier migration steps are outside this fixture.
//! Client secrets are sealed by the actual vault owner using synthetic plaintext.
use super::*;
use crate::database::dao::qoder_campaign_operations as ledger;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const LEGACY_LEDGER: &str =
    include_str!("../secrets/test_fixtures/qoder_campaign_operations_v27.sql");

/// Build only prerequisite owners touched by apply's v27+ branch/validation.
/// In particular, never stamp a current ten-column ledger as historical v27/28.
fn historical(version: i32) -> Connection {
    assert!(matches!(version, 27 | 28));
    let conn = Connection::open_in_memory().unwrap();
    crate::paperclip::bindings::ensure_schema(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE providers(id TEXT PRIMARY KEY, qoder_http_secret TEXT NOT NULL DEFAULT '');
        CREATE TABLE proxy_request_logs(request_id TEXT PRIMARY KEY);
        PRAGMA user_version=20;",
    )
    .unwrap();
    conn.execute_batch(LEGACY_LEDGER).unwrap();
    conn.execute("INSERT INTO qoder_campaign_operations VALUES('legacy-attempt','synthetic-subject','international','legacy-campaign','legacy-key',100,200,'legacy-generation','unknown')", []).unwrap();
    if version == 28 {
        crate::proxy::client_access::create_schema(&conn).unwrap();
        add_client_canaries(&conn);
    }
    ensure_version_table(&conn).unwrap();
    set_version(&conn, version).unwrap();
    conn
}

fn add_client_canaries(conn: &Connection) {
    let vault = crate::secrets::VaultContext::generate().unwrap();
    let sealed = crate::secrets::inventory::seal_encrypted_db(
        &vault,
        "loongport_client_credentials",
        "secret",
        &["fixture-credential"],
        "synthetic-client-secret-not-a-real-account",
    )
    .unwrap();
    assert!(sealed.starts_with("lpenc1."));
    conn.execute("INSERT INTO loongport_client_credentials(id,client_id,route_app,allowed_models,secret,secret_digest,revoked) VALUES('fixture-credential','synthetic-client','codex','[\"synthetic-model\"]',?1,'synthetic-digest',1)", [sealed]).unwrap();
    conn.execute("INSERT INTO proxy_request_logs(request_id,client_id,client_credential_id) VALUES('fixture-request','synthetic-client','fixture-credential')", []).unwrap();
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn schema_image(conn: &Connection) -> Vec<(String, String, String)> {
    conn.prepare("SELECT type,name,coalesce(sql,'') FROM sqlite_master ORDER BY type,name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn raw_intl(conn: &Connection) -> Vec<rusqlite::types::Value> {
    conn.query_row("SELECT attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result FROM qoder_campaign_operations WHERE attempt_id='legacy-attempt'", [], |r| (0..9).map(|i|r.get(i)).collect()).unwrap()
}

fn client_image(conn: &Connection) -> (Vec<rusqlite::types::Value>, Vec<rusqlite::types::Value>) {
    let credential = conn.query_row("SELECT id,client_id,route_app,allowed_models,secret,secret_digest,revoked FROM loongport_client_credentials WHERE id='fixture-credential'", [], |r| (0..7).map(|i|r.get(i)).collect()).unwrap();
    let log = conn.query_row("SELECT request_id,client_id,client_credential_id FROM proxy_request_logs WHERE request_id='fixture-request'", [], |r| (0..3).map(|i|r.get(i)).collect()).unwrap();
    (credential, log)
}

fn assert_final(conn: &Connection, receipt: &[rusqlite::types::Value]) {
    assert_eq!(read_stored_version(conn).unwrap(), 29);
    assert_eq!(columns(conn, "qoder_campaign_operations").len(), 10);
    assert_eq!(raw_intl(conn), receipt);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        20
    );
    crate::proxy::client_access::validate_schema(conn).unwrap();
    assert!(columns(conn, "proxy_request_logs").contains(&"client_id".into()));
    assert!(columns(conn, "proxy_request_logs").contains(&"client_credential_id".into()));
    assert!(!table_exists(conn, "qoder_campaign_operations_v27").unwrap());
    assert!(!table_exists(conn, "qoder_campaign_operations_v28").unwrap());
    ledger::read_rows(conn).unwrap();
}

#[test]
fn real_v27_builds_client_owner_then_cn_v29_without_losing_intl_unknown() {
    let conn = historical(27);
    assert_eq!(columns(&conn, "qoder_campaign_operations").len(), 9);
    assert!(!table_exists(&conn, "loongport_client_credentials").unwrap());
    assert_eq!(columns(&conn, "proxy_request_logs"), ["request_id"]);
    let before = raw_intl(&conn);
    apply(&conn).expect("real v27 must traverse the independently published v28 then CN v29");
    assert_final(&conn, &before);
    add_client_canaries(&conn);
    let client = client_image(&conn);
    apply(&conn).unwrap();
    assert_final(&conn, &before);
    assert_eq!(client_image(&conn), client);
}

#[test]
fn main_v28_migrates_to_v29_preserving_client_ciphertext_revocation_and_log_owner() {
    let conn = historical(28);
    assert_eq!(columns(&conn, "qoder_campaign_operations").len(), 9);
    let receipt = raw_intl(&conn);
    let client = client_image(&conn);
    apply(&conn).expect("published main v28 ledger must be admitted and upgraded to v29");
    assert_final(&conn, &receipt);
    assert_eq!(client_image(&conn), client);
    apply(&conn).unwrap();
    assert_final(&conn, &receipt);
    assert_eq!(client_image(&conn), client);
}

#[test]
fn current_v29_keeps_ten_column_cn_receipt_and_all_existing_owners() {
    let conn = historical(28);
    conn.execute_batch("DROP TABLE qoder_campaign_operations")
        .unwrap();
    ledger::create_schema(&conn).unwrap();
    conn.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,credential_generation,result,provider_id) VALUES('cn-attempt','synthetic-cn-subject','china','cn-campaign','cn-generation','unknown','synthetic-provider')",[]).unwrap();
    set_version(&conn, 29).unwrap();
    let ledger = ledger::read_rows(&conn).unwrap();
    let client = client_image(&conn);
    let schema = schema_image(&conn);
    apply(&conn).expect("already-current v29 must validate without reconstructing any owner");
    apply(&conn).unwrap();
    assert_eq!(read_stored_version(&conn).unwrap(), 29);
    assert_eq!(columns(&conn, "qoder_campaign_operations").len(), 10);
    assert_eq!(ledger::read_rows(&conn).unwrap(), ledger);
    assert_eq!(client_image(&conn), client);
    assert_eq!(schema_image(&conn), schema);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        20
    );
}

#[test]
fn v28_cn_failures_roll_back_receipt_schema_and_version_without_touching_clients() {
    for point in ["create", "copy", "drop", "stamp"] {
        let conn = historical(28);
        let before_schema = schema_image(&conn);
        let receipt = raw_intl(&conn);
        let client = client_image(&conn);
        let denied = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&denied);
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            let reject = match context.action {
                AuthAction::CreateTable { table_name } => {
                    point == "create" && table_name == "qoder_campaign_operations"
                }
                AuthAction::Insert { table_name } => {
                    (point == "copy" && table_name == "qoder_campaign_operations")
                        || (point == "stamp" && table_name == VERSION_TABLE)
                }
                AuthAction::DropTable { table_name } => {
                    point == "drop" && table_name.starts_with("qoder_campaign_operations_")
                }
                _ => false,
            };
            if reject {
                observed.store(true, Ordering::SeqCst);
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
        let result = apply(&conn);
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        assert!(
            denied.load(Ordering::SeqCst),
            "fault injection {point} must reach the real migration action"
        );
        assert!(result.is_err(), "fault injection {point} must abort");
        assert_eq!(read_stored_version(&conn).unwrap(), 28, "{point}");
        assert_eq!(schema_image(&conn), before_schema, "{point}");
        assert_eq!(raw_intl(&conn), receipt, "{point}");
        assert_eq!(client_image(&conn), client, "{point}");
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            20
        );
        assert!(
            conn.is_autocommit(),
            "failed migration leaked a savepoint at {point}"
        );
        apply(&conn).expect("retry after removing the injected failure must succeed");
        assert_final(&conn, &receipt);
        assert_eq!(client_image(&conn), client);
    }
}

#[test]
fn experimental_cn_v28_missing_client_owner_is_rejected_without_self_repair() {
    let conn = historical(27);
    conn.execute_batch("DROP TABLE qoder_campaign_operations")
        .unwrap();
    ledger::create_schema(&conn).unwrap();
    conn.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,credential_generation,result,provider_id) VALUES('experimental-attempt','synthetic-cn-subject','china','cn-campaign','cn-generation','unknown','synthetic-provider')",[]).unwrap();
    set_version(&conn, 28).unwrap();
    let schema = schema_image(&conn);
    let receipt = ledger::read_rows(&conn).unwrap();
    assert!(
        apply(&conn).is_err(),
        "a v28 stamp cannot authorize reconstructing its missing client owner"
    );
    assert_eq!(read_stored_version(&conn).unwrap(), 28);
    assert_eq!(schema_image(&conn), schema);
    assert_eq!(ledger::read_rows(&conn).unwrap(), receipt);
    assert!(!table_exists(&conn, "loongport_client_credentials").unwrap());
    assert_eq!(columns(&conn, "proxy_request_logs"), ["request_id"]);
}

#[test]
fn real_v27_cn_failure_keeps_completed_v28_client_owner_and_unknown_receipt() {
    let conn = historical(27);
    let receipt = raw_intl(&conn);
    let denied = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&denied);
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::CreateTable {
                table_name: "qoder_campaign_operations"
            }
        ) {
            observed.store(true, Ordering::SeqCst);
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }))
    .unwrap();
    let result = apply(&conn);
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert!(
        denied.load(Ordering::SeqCst),
        "CN fault must occur after the real v28 client migration"
    );
    assert!(result.is_err());
    assert_eq!(read_stored_version(&conn).unwrap(), 28);
    crate::proxy::client_access::validate_schema(&conn).unwrap();
    assert!(columns(&conn, "proxy_request_logs").contains(&"client_id".into()));
    assert!(columns(&conn, "proxy_request_logs").contains(&"client_credential_id".into()));
    assert_eq!(columns(&conn, "qoder_campaign_operations").len(), 9);
    assert_eq!(raw_intl(&conn), receipt);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        20
    );
    assert!(conn.is_autocommit());
    apply(&conn).expect("the legitimate intermediate v28 must remain migratable");
    assert_final(&conn, &receipt);
}
