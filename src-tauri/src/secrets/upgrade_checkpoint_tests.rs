use super::*;
use crate::secrets::{session::SecretSession, testing::MemoryKeyStore};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: std::path::PathBuf,
    device: DeviceStore,
    vault: VaultContext,
}
impl Fixture {
    fn new() -> Self {
        let temporary = crate::secrets::testing::tempdir().unwrap();
        let root = temporary.path().join("data");
        let device = DeviceStore::at(temporary.path().join("device"));
        let session = SecretSession::open(&root, &MemoryKeyStore::default(), None).unwrap();
        let vault = session.read().unwrap().clone();
        let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
        Database::create_tables_on_conn(&conn).unwrap();
        Database::apply_schema_migrations_on_conn(&conn).unwrap();
        database::loongport_schema::apply(&conn).unwrap();
        database::vault::stamp(&conn, &vault).unwrap();
        conn.execute(
            "INSERT INTO settings(key,value) VALUES ('synthetic-upgrade','checkpoint-canary')",
            [],
        )
        .unwrap();
        session.complete_migration().unwrap();
        Self {
            _temporary: temporary,
            root,
            device,
            vault,
        }
    }
}

#[test]
fn checkpoint_is_encrypted_roundtrips_and_stages_without_changing_source() {
    let f = Fixture::new();
    let client = f._temporary.path().join("synthetic-client.json");
    std::fs::write(&client, b"client-canary").unwrap();
    let before = std::fs::read(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let id =
        checkpoint::create(&f.root, &f.device, &f.vault, std::slice::from_ref(&client)).unwrap();
    let bytes = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("checkpoint-canary"));
    assert!(!String::from_utf8_lossy(&bytes).contains("client-canary"));
    let stage = checkpoint::stage(&f.root, &f.device, &f.vault, &id).unwrap();
    assert_eq!(Database::get_user_version(&stage).unwrap(), 20);
    assert_eq!(
        database::loongport_schema::read_stored_version(&stage).unwrap(),
        24
    );
    let value: String = stage
        .query_row(
            "SELECT value FROM settings WHERE key='synthetic-upgrade'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, "checkpoint-canary");
    assert_eq!(
        std::fs::read(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        before
    );
    assert_eq!(std::fs::read(&client).unwrap(), b"client-canary");
    assert!(checkpoint::stage(&f.root, &f.device, &f.vault, &id).is_ok());
}

#[test]
fn checkpoint_refuses_stale_source_client_wrong_operation_or_key() {
    let f = Fixture::new();
    let client = f._temporary.path().join("synthetic-client.json");
    std::fs::write(&client, b"original").unwrap();
    let id =
        checkpoint::create(&f.root, &f.device, &f.vault, std::slice::from_ref(&client)).unwrap();
    assert!(checkpoint::stage(
        &f.root,
        &f.device,
        &f.vault,
        &uuid::Uuid::new_v4().to_string()
    )
    .is_err());
    assert!(
        checkpoint::stage(&f.root, &f.device, &VaultContext::generate().unwrap(), &id).is_err()
    );
    std::fs::write(&client, b"external-change").unwrap();
    assert!(checkpoint::stage(&f.root, &f.device, &f.vault, &id).is_err());
    std::fs::write(&client, b"original").unwrap();
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    conn.execute(
        "UPDATE settings SET value='new' WHERE key='synthetic-upgrade'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(checkpoint::stage(&f.root, &f.device, &f.vault, &id).is_err());
    assert!(f.device.root().join(checkpoint::FILE).exists());
}

#[test]
fn checkpoint_refuses_unexpected_existing_artifact_and_corruption() {
    let f = Fixture::new();
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    assert!(checkpoint::create(&f.root, &f.device, &f.vault, &[]).is_err());
    std::fs::write(f.device.root().join(checkpoint::FILE), b"invalid").unwrap();
    assert!(checkpoint::stage(&f.root, &f.device, &f.vault, &id).is_err());
}

#[cfg(unix)]
#[test]
fn checkpoint_refuses_symlink_client_and_private_output_is_restricted() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let f = Fixture::new();
    let target = f._temporary.path().join("target");
    std::fs::write(&target, b"kept").unwrap();
    let alias = f._temporary.path().join("alias");
    symlink(&target, &alias).unwrap();
    assert!(checkpoint::create(&f.root, &f.device, &f.vault, &[alias]).is_err());
    assert!(!f.device.root().join(checkpoint::FILE).exists());
    checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    assert_eq!(
        std::fs::metadata(f.device.root().join(checkpoint::FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn checkpoint_pauses_sync_even_when_its_contents_are_corrupt() {
    let f = Fixture::new();
    checkpoint::ensure_sync_admitted(&f.device).unwrap();
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    assert!(
        matches!(checkpoint::ensure_sync_admitted(&f.device),Err(AppError::Config(code)) if code=="upgrade.sync_paused")
    );
    checkpoint::stage(&f.root, &f.device, &f.vault, &id).unwrap();
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    std::fs::write(f.device.root().join(checkpoint::FILE), b"corrupt").unwrap();
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}

#[test]
fn checkpoint_staging_builds_missing_tables_before_version_twenty() {
    let f = Fixture::new();
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    conn.execute_batch("DROP TABLE session_log_sync; DROP TABLE mcp_servers")
        .unwrap();
    drop(conn);
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    let stage = checkpoint::stage(&f.root, &f.device, &f.vault, &id).unwrap();
    stage
        .prepare("SELECT last_byte_offset,last_tail_fingerprint FROM session_log_sync")
        .unwrap();
    stage
        .prepare("SELECT enabled_mcode,enabled_pi FROM mcp_servers")
        .unwrap();
}

#[test]
fn checkpoint_atomic_creation_never_overwrites_existing_file() {
    let f = Fixture::new();
    let path = f._temporary.path().join("checkpoint-output");
    crate::config_file_io::write_durable_new(&path, b"existing").unwrap();
    assert!(crate::config_file_io::write_durable_new(&path, b"replace").is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"existing");
}

#[tokio::test]
#[serial_test::serial]
async fn checkpoint_blocks_sync_transports_restore_and_auto_before_side_effects() {
    struct Home(Option<std::ffi::OsString>);
    impl Drop for Home {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }
    let f = Fixture::new();
    let _home = Home(std::env::var_os("CC_SWITCH_TEST_HOME"));
    std::env::set_var("CC_SWITCH_TEST_HOME", f._temporary.path());
    let fixed = DeviceStore::for_device();
    crate::config_file_io::ensure_private_directory(fixed.root()).unwrap();
    std::fs::write(
        fixed.root().join(checkpoint::FILE),
        b"pending-even-if-corrupt",
    )
    .unwrap();
    let db = crate::database::Database::memory().unwrap();
    let mut dav = crate::settings::WebDavSyncSettings::default();
    let mut s3 = crate::settings::S3SyncSettings::default();
    let errors = [
        crate::services::webdav_sync::upload(&db, &mut dav)
            .await
            .map(|_| ())
            .unwrap_err(),
        crate::services::s3_sync::upload(&db, &mut s3)
            .await
            .map(|_| ())
            .unwrap_err(),
        crate::services::webdav_sync::fetch_snapshot(&dav)
            .await
            .map(|_| ())
            .unwrap_err(),
        crate::services::s3_sync::fetch_snapshot(&s3)
            .await
            .map(|_| ())
            .unwrap_err(),
    ];
    for error in errors {
        assert!(matches!(error,AppError::Config(code) if code=="upgrade.sync_paused"));
    }
    let polled = std::cell::Cell::new(false);
    let result = crate::services::sync_protocol::run_with_sync_lock(async {
        polled.set(true);
        Ok(())
    })
    .await;
    assert!(matches!(result,Err(AppError::Config(code)) if code=="upgrade.sync_paused"));
    assert!(!polled.get());
}

#[test]
fn checkpoint_rejects_recapture_after_authenticated_source_changes() {
    for change_vault in [false, true] {
        let f = Fixture::new();
        let result =
            checkpoint::create_with_hook(&f.root, &f.device, &f.vault, &[], &mut |boundary| {
                if boundary == checkpoint::Boundary::Authenticated {
                    if change_vault {
                        std::fs::write(f.root.join("vault.json"), b"unvalidated replacement")
                            .unwrap();
                    } else {
                        crate::config_file_io::ensure_private_directory(f.device.root()).unwrap();
                        std::fs::write(
                            f.device.root().join("codex-login-stash.json"),
                            b"unvalidated replacement",
                        )
                        .unwrap();
                    }
                }
                Ok(())
            });
        assert!(
            result.is_err(),
            "changed authenticated source must be rejected"
        );
        assert!(!f.device.root().join(checkpoint::FILE).exists());
    }
}

#[test]
fn checkpoint_recovers_id_after_post_publication_failure() {
    let f = Fixture::new();
    assert!(
        checkpoint::create_with_hook(&f.root, &f.device, &f.vault, &[], &mut |boundary| {
            if boundary == checkpoint::Boundary::Published {
                return Err(AppError::Config("synthetic readback interruption".into()));
            }
            Ok(())
        })
        .is_err()
    );
    let id = checkpoint::existing_id(&f.root, &f.device, &f.vault).unwrap();
    assert!(checkpoint::stage(&f.root, &f.device, &f.vault, &id).is_ok());
    assert!(
        checkpoint::existing_id(&f.root, &f.device, &VaultContext::generate().unwrap()).is_err()
    );
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}
