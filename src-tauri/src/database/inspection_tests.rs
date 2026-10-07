use super::*;
use crate::database::{loongport_schema, vault, Database};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                result.insert(path.strip_prefix(root).unwrap().to_owned(), Vec::new());
                walk(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(path, path, &mut result);
    result
}

fn fixture(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        PRAGMA user_version=17;
        CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY, version INTEGER);
        INSERT INTO loongport_schema_version VALUES(1,24);
        CREATE TABLE inspection_rows(value TEXT);
        INSERT INTO inspection_rows VALUES('checkpointed');",
    )
    .unwrap();
    vault::stamp(&conn, &crate::secrets::VaultContext::generate().unwrap()).unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    conn
}

#[test]
fn inspection_closed_wal_creates_no_source_sidecars() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("fixture.db");
    drop(fixture(&path));
    let before = tree(dir.path());
    vault::preflight(&path).unwrap();
    assert!(
        tree(dir.path()) == before,
        "passive startup must preserve all original bytes and membership"
    );
}

#[test]
fn inspection_version_wrappers_create_no_source_sidecars() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("fixture.db");
    drop(fixture(&path));
    let before = tree(dir.path());
    assert_eq!(
        Database::stored_user_version_exceeds_supported(&path).unwrap(),
        None
    );
    assert_eq!(
        loongport_schema::stored_version_exceeds_supported(&path).unwrap(),
        None
    );
    assert!(
        tree(dir.path()) == before,
        "version probes must not open the original with SQLite"
    );
}

#[test]
fn inspection_includes_committed_wal_rows() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("fixture.db");
    let writer = fixture(&path);
    writer
        .execute_batch(
            "INSERT INTO inspection_rows VALUES('committed-in-wal');
        CREATE TABLE only_in_wal(value INTEGER); INSERT INTO only_in_wal VALUES(42);",
        )
        .unwrap();
    let before = tree(dir.path());
    let inspected = capture(&path).unwrap().unwrap();
    assert_eq!(
        inspected
            .image
            .query_row("SELECT count(*) FROM inspection_rows", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        inspected
            .image
            .query_row("SELECT value FROM only_in_wal", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        42
    );
    assert_eq!(Database::get_user_version(&inspected.image).unwrap(), 17);
    assert_eq!(
        loongport_schema::read_stored_version(&inspected.image).unwrap(),
        24
    );
    verify_unchanged(&path, &inspected.revision).unwrap();
    assert!(tree(dir.path()) == before);
}

#[test]
fn inspection_refuses_changes_between_copy_observation_and_validation() {
    for phase in [
        CapturePhase::Copied,
        CapturePhase::Observed,
        CapturePhase::Validated,
    ] {
        for mutation in [
            "wal-append",
            "wal-truncate",
            "replace",
            "add-journal",
            "remove-shm",
        ] {
            let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
            let path = dir.path().join("fixture.db");
            let writer = fixture(&path);
            writer
                .execute_batch("INSERT INTO inspection_rows VALUES('wal')")
                .unwrap();
            let mut writer = Some(writer);
            let result = capture_with_hook(&path, &mut |point| {
                if point == phase {
                    match mutation {
                        "wal-append" => writer
                            .as_ref()
                            .unwrap()
                            .execute_batch("INSERT INTO inspection_rows VALUES('later')")
                            .unwrap(),
                        "wal-truncate" => writer
                            .as_ref()
                            .unwrap()
                            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                            .unwrap(),
                        "replace" => {
                            // SQLite's Windows handle prevents replacement while open.
                            drop(writer.take());
                            let replacement = dir.path().join("replacement");
                            std::fs::copy(&path, &replacement).unwrap();
                            std::fs::rename(replacement, &path).unwrap();
                        }
                        "add-journal" => {
                            std::fs::write(sidecar(&path, "-journal"), b"changed").unwrap()
                        }
                        "remove-shm" => {
                            drop(writer.take());
                            // Closing the last writer may already remove the SHM file.
                            if sidecar(&path, "-shm").exists() {
                                std::fs::remove_file(sidecar(&path, "-shm")).unwrap();
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(())
            });
            assert!(
                matches!(result, Err(AppError::Config(code)) if code == "upgrade.source_changed"),
                "mutation {mutation} must invalidate the capture"
            );
        }
    }
}

#[test]
fn inspection_missing_database_is_not_fresh_when_a_sidecar_survives() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = dir.path().join("fixture.db");
        std::fs::write(sidecar(&path, suffix), []).unwrap();
        let before = tree(dir.path());
        assert!(
            matches!(capture(&path), Err(AppError::Config(code)) if code == "upgrade.source_recovery_required")
        );
        assert!(tree(dir.path()) == before);
    }
}

#[test]
fn inspection_malformed_or_empty_file_never_becomes_fresh() {
    for bytes in [b"not a database".as_slice(), b""] {
        let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = dir.path().join("fixture.db");
        std::fs::write(&path, bytes).unwrap();
        let before = tree(dir.path());
        assert!(
            matches!(capture(&path), Err(AppError::Config(code)) if code == "upgrade.invalid_database")
        );
        assert!(tree(dir.path()) == before);
    }
}

#[cfg(unix)]
#[test]
fn inspection_rejects_symlinks_ancestor_swap_and_read_permission_failure() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let tree = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let root = tree.path().join("source");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("fixture.db");
    drop(fixture(&path));
    let link = root.join("alias.db");
    symlink(&path, &link).unwrap();
    assert!(capture(&link).is_err());
    std::fs::remove_file(link).unwrap();
    let moved = tree.path().join("moved");
    let result = capture_with_hook(&path, &mut |phase| {
        if phase == CapturePhase::Copied {
            std::fs::rename(&root, &moved).unwrap();
            symlink(&moved, &root).unwrap();
        }
        Ok(())
    });
    assert!(matches!(result, Err(AppError::Config(code)) if code == "upgrade.source_changed"));
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(moved, &root).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
    let denied = capture(&path);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(denied.is_err());
}

#[test]
fn inspection_hot_journal_is_blocked_without_recovery() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("fixture.db");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "database::inspection::tests::inspection_crash_journal_fixture_child",
            "--nocapture",
        ])
        .env("LOONGPORT_INSPECTION_CRASH_FIXTURE", &path)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(std::fs::metadata(sidecar(&path, "-journal")).unwrap().len() > 512);
    let before = tree(dir.path());
    assert!(
        matches!(capture(&path), Err(AppError::Config(code)) if code == "upgrade.source_recovery_required")
    );
    assert!(tree(dir.path()) == before);
}

#[test]
#[ignore = "subprocess fixture invoked by inspection_hot_journal_is_blocked_without_recovery"]
fn inspection_crash_journal_fixture_child() {
    let Some(path) = std::env::var_os("LOONGPORT_INSPECTION_CRASH_FIXTURE") else {
        return;
    };
    let conn = Connection::open(PathBuf::from(path)).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA cache_size=1;
        CREATE TABLE records(value BLOB); INSERT INTO records VALUES(zeroblob(65536));
        BEGIN IMMEDIATE; UPDATE records SET value=randomblob(65536);",
    )
    .unwrap();
    // Simulate termination without dropping SQLite (and hence without rollback).
    std::process::exit(0);
}

#[test]
fn inspection_refuses_scratch_inside_the_source_before_allocating() {
    let tree = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let root = tree.path().join("source");
    let requested = root.join("temporary");
    std::fs::create_dir_all(&requested).unwrap();
    let before = self::tree(tree.path());
    assert!(inspection_temp_base(&root, &tree.path().join("device"), &requested).is_err());
    assert!(self::tree(tree.path()) == before);
}

#[cfg(unix)]
#[test]
fn inspection_refuses_a_source_alias_before_allocating_scratch() {
    let tree = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let root = tree.path().join("source");
    let requested = root.join("temporary");
    std::fs::create_dir_all(&requested).unwrap();
    let alias = tree.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(inspection_temp_base(&alias, &tree.path().join("device"), &requested).is_err());
    assert!(std::fs::read_dir(&requested).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn inspection_device_alias_only_denies_scratch_membership() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let source = dir.path().join("source");
    let actual = dir.path().join("device");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&actual).unwrap();
    let alias = dir.path().join("alias");
    symlink(&actual, &alias).unwrap();
    assert!(inspection_temp_base(&source, &alias, &std::env::temp_dir()).is_ok());
    assert!(inspection_temp_base(&source, &alias, &actual).is_err());
}
