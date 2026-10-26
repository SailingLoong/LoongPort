//! Real file-deletion orders at the original authenticated cleanup checkpoint.
//! No arbitrary truncation/corruption is substituted for remove_dir_all behavior.
use super::*;
use crate::secrets::{testing::TestHome, transition::fence_test_support as support};

const PASSWORD: &str = "synthetic committed cleanup recovery password";
struct Fixture {
    root: PathBuf,
    previous: PathBuf,
    archive: PathBuf,
    archive_bytes: Vec<u8>,
    active_database: Vec<u8>,
    home: TestHome,
}
impl Fixture {
    fn committed() -> Self {
        let original = support::Fixture::new();
        let root = original.root().to_owned();
        let support::Fixture { db, store: _, home } = original;
        drop(db);
        // Make actual SQLite WAL/SHM files persist after the fixture connection
        // closes. No live SQLite handle crosses the reset's directory rename.
        let conn = Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT OR REPLACE INTO settings(key,value) VALUES('theme','dark');").unwrap();
        let mut persist: std::ffi::c_int = 1;
        // SQLite writes only one c_int; connection and pointer outlive this call.
        let result = unsafe {
            rusqlite::ffi::sqlite3_file_control(
                conn.handle(),
                c"main".as_ptr(),
                rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
                std::ptr::addr_of_mut!(persist).cast(),
            )
        };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
        conn.close().unwrap();
        for suffix in ["-wal", "-shm"] {
            assert!(root
                .join(format!("{}{suffix}", crate::config::DB_FILE_NAME))
                .is_file());
        }
        let approval = preview(&root).unwrap();
        let result = reset_with_hook(&root, &approval.fingerprint, PASSWORD, &mut |at| {
            if at == Checkpoint::CleanupCommitted {
                Err(AppError::Config("test.committed_cleanup_checkpoint".into()))
            } else {
                Ok(())
            }
        });
        assert!(
            matches!(result,Err(AppError::Config(code)) if code=="test.committed_cleanup_checkpoint")
        );
        let bytes = pending_record(&root).unwrap().unwrap();
        let intent: Intent = serde_json::from_slice(&bytes).unwrap();
        let next = VaultContext::from_password(intent.metadata.clone(), PASSWORD).unwrap();
        let plain = next
            .open(&["local", "reset-intent", &intent.id], &intent.body)
            .unwrap();
        let manifest: Manifest = serde_json::from_slice(&plain).unwrap();
        assert!(manifest.phase == ResetPhase::CleanupCommitted);
        let (stage, previous, archive) = locations(&root, &intent.id).unwrap();
        assert!(!stage.exists());
        assert!(previous.is_dir());
        for suffix in ["-wal", "-shm"] {
            assert!(
                previous
                    .join(format!("{}{suffix}", crate::config::DB_FILE_NAME))
                    .is_file(),
                "original reset did not retain the actual sidecar fixture"
            );
        }
        let archive_bytes = fs_read(&archive);
        let active_database = fs_read(&root.join(crate::config::DB_FILE_NAME));
        Self {
            root,
            previous,
            archive,
            archive_bytes,
            active_database,
            home,
        }
    }
    fn remove(&self, relative: &str) {
        let path = self.previous.join(relative);
        assert!(
            path.is_file(),
            "cleanup deletion precondition missing: {relative}"
        );
        std::fs::remove_file(path).unwrap();
    }
    fn assert_recovered(&self) {
        recover(&self.root, Some(PASSWORD)).unwrap();
        assert!(!pending(&self.root).unwrap());
        assert!(!self.previous.exists());
        assert_eq!(fs_read(&self.archive), self.archive_bytes);
        assert_eq!(
            fs_read(&self.root.join(crate::config::DB_FILE_NAME)),
            self.active_database
        );
        let local = session::read_metadata(&self.root).unwrap();
        let next = VaultContext::from_password(local.metadata, PASSWORD).unwrap();
        let captured =
            crate::database::inspection::capture(&self.root.join(crate::config::DB_FILE_NAME))
                .unwrap()
                .unwrap();
        vault::check_identity(&captured.image, &next).unwrap();
        inventory::validate_database(&captured.image, &next).unwrap();
        assert_eq!(
            captured
                .image
                .query_row(
                    "SELECT count(*) FROM settings WHERE key='global_proxy_url'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
fn fs_read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn committed_cleanup_recovers_after_database_removed_before_sidecars() {
    let f = Fixture::committed();
    f.remove(crate::config::DB_FILE_NAME);
    assert!(f
        .previous
        .join(format!("{}-wal", crate::config::DB_FILE_NAME))
        .exists());
    f.assert_recovered();
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn committed_cleanup_recovers_after_each_sidecar_is_removed_first() {
    for first in ["-wal", "-shm"] {
        let f = Fixture::committed();
        f.remove(&format!("{}{first}", crate::config::DB_FILE_NAME));
        assert!(f.previous.join(crate::config::DB_FILE_NAME).exists());
        f.assert_recovered();
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn committed_cleanup_recovers_after_metadata_and_sidecars_are_removed() {
    let f = Fixture::committed();
    f.remove("vault.json");
    for suffix in ["-wal", "-shm"] {
        f.remove(&format!("{}{suffix}", crate::config::DB_FILE_NAME));
    }
    f.assert_recovered();
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn committed_cleanup_still_rejects_protected_previous_after_partial_deletion() {
    for scope in support::SCOPES {
        let f = Fixture::committed();
        f.remove(crate::config::DB_FILE_NAME);
        let context = VaultContext::generate().unwrap();
        support::write_record(&f.previous, scope, &context);
        let before = support::tree(f.home.path());
        support::assert_rejected(recover(&f.root, Some(PASSWORD)), scope);
        assert_eq!(support::tree(f.home.path()), before);
    }
    let f = Fixture::committed();
    f.remove(crate::config::DB_FILE_NAME);
    let context = VaultContext::create_generation_with_user_text(
        uuid::Uuid::new_v4().to_string(),
        1,
        "synthetic cleanup exact text",
    )
    .unwrap();
    session::write_metadata(
        &f.previous,
        &session::completed_metadata(&context, false).unwrap(),
    )
    .unwrap();
    let before = support::tree(f.home.path());
    support::assert_rejected(recover(&f.root, Some(PASSWORD)), support::Scope::Encrypted);
    assert_eq!(support::tree(f.home.path()), before);
}

pub(super) fn verify(case: &str) {
    match case {
        "committed_cleanup_recovers_after_database_removed_before_sidecars" => {
            committed_cleanup_recovers_after_database_removed_before_sidecars()
        }
        "committed_cleanup_recovers_after_each_sidecar_is_removed_first" => {
            committed_cleanup_recovers_after_each_sidecar_is_removed_first()
        }
        "committed_cleanup_recovers_after_metadata_and_sidecars_are_removed" => {
            committed_cleanup_recovers_after_metadata_and_sidecars_are_removed()
        }
        "committed_cleanup_still_rejects_protected_previous_after_partial_deletion" => {
            committed_cleanup_still_rejects_protected_previous_after_partial_deletion()
        }
        _ => panic!("unknown legacy reset cleanup test case"),
    }
}
