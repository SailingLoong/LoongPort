use super::checkpoint_tests::Fixture;
use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !current.exists() {
            return;
        }
        for entry in std::fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(root, &path, result);
            } else if metadata.is_file() {
                result.insert(
                    path.strip_prefix(root).unwrap().into(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[cfg_attr(test, test)]
fn upgrade_review_is_read_only_and_secret_free() {
    let f = Fixture::new();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f._temporary.path());
    for _ in 0..2 {
        let view = inspected.upgrade_view(&f.root).unwrap();
        assert_eq!(view.status, "authentication_required");
        assert_eq!(view.source_versions.unwrap().upstream, 17);
        assert_eq!(view.target_versions.upstream, 20);
        assert_eq!(view.target_versions.loongport, 24);
        assert!(view.requires_authentication);
        assert!(!view.can_authenticate && !view.can_check_and_backup && !view.can_start_upgrade);
        assert!(!view.checkpoint_present && view.checkpoint_id.is_none());
        let public = serde_json::to_string(&view).unwrap();
        for secret in [
            "checkpoint-canary",
            f.root.to_str().unwrap(),
            "metadata",
            "revision",
            "ciphertext",
        ] {
            assert!(!public.contains(secret), "public view leaked {secret}");
        }
    }
    assert_eq!(snapshot(f._temporary.path()), before);
}

#[cfg_attr(test, test)]
fn upgrade_review_preserves_future_and_recovery_precedence() {
    let f = Fixture::new();
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    conn.pragma_update(None, "user_version", 20).unwrap();
    drop(conn);
    std::fs::write(f.root.join("vault.json"), b"unreadable-future-vault").unwrap();
    let before = snapshot(f._temporary.path());
    let view = inspect(&f.root, &f.device)
        .unwrap()
        .upgrade_view(&f.root)
        .unwrap();
    assert_eq!(view.status, "newer_binary_required");
    assert!(!view.requires_authentication && !view.can_start_upgrade);
    assert_eq!(snapshot(f._temporary.path()), before);
    let marker = f.root.join(crate::secrets::transition::INTENT);
    std::fs::write(&marker, b"unparsed-pending-operation").unwrap();
    let before = snapshot(f._temporary.path());
    let view = inspect(&f.root, &f.device)
        .unwrap()
        .upgrade_view(&f.root)
        .unwrap();
    assert_eq!(view.status, "recovery_required");
    assert!(view.source_versions.is_none());
    assert!(!view.can_authenticate && !view.can_check_and_backup);
    assert_eq!(snapshot(f._temporary.path()), before);
}

#[cfg_attr(test, test)]
fn upgrade_review_never_treats_checkpoint_presence_as_verified() {
    let f = Fixture::new();
    checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    for corrupt in [false, true] {
        if corrupt {
            std::fs::write(
                f.device.root().join(checkpoint::FILE),
                b"secret-checkpoint-canary",
            )
            .unwrap();
        }
        let before = snapshot(f._temporary.path());
        let view = inspect(&f.root, &f.device)
            .unwrap()
            .upgrade_view(&f.root)
            .unwrap();
        assert_eq!(view.status, "checkpoint_requires_verification");
        assert!(view.checkpoint_present && view.requires_authentication);
        assert!(view.checkpoint_id.is_none());
        assert!(!view.can_check_and_backup && !view.can_start_upgrade);
        assert!(!serde_json::to_string(&view)
            .unwrap()
            .contains("secret-checkpoint-canary"));
        assert_eq!(snapshot(f._temporary.path()), before);
    }
}

#[cfg_attr(test, test)]
fn upgrade_review_refuses_stale_source_without_repair() {
    let f = Fixture::new();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    conn.execute(
        "UPDATE settings SET value='external' WHERE key='synthetic-upgrade'",
        [],
    )
    .unwrap();
    drop(conn);
    let before = snapshot(f._temporary.path());
    assert!(inspected.upgrade_view(&f.root).is_err());
    assert_eq!(snapshot(f._temporary.path()), before);
}

#[cfg_attr(test, test)]
fn upgrade_review_distinguishes_fresh_and_unsupported_sources() {
    let temporary = crate::secrets::testing::tempdir().unwrap();
    let root = temporary.path().join("new-data");
    let device = DeviceStore::at(temporary.path().join("new-device"));
    let before = snapshot(temporary.path());
    let view = inspect(&root, &device)
        .unwrap()
        .upgrade_view(&root)
        .unwrap();
    assert_eq!(view.status, "not_applicable");
    assert!(view.source_versions.is_none());
    assert_eq!(snapshot(temporary.path()), before);
    let f = Fixture::new();
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    conn.pragma_update(None, "user_version", 16).unwrap();
    drop(conn);
    let view = inspect(&f.root, &f.device)
        .unwrap()
        .upgrade_view(&f.root)
        .unwrap();
    assert_eq!(view.status, "unsupported_source");
    assert!(!view.can_check_and_backup);
    let plain_root = temporary.path().join("legacy-plaintext");
    crate::config_file_io::ensure_private_directory(&plain_root).unwrap();
    let conn = rusqlite::Connection::open(plain_root.join(crate::config::DB_FILE_NAME)).unwrap();
    conn.execute_batch("PRAGMA user_version=17; CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER); INSERT INTO loongport_schema_version VALUES(1,23);").unwrap();
    drop(conn);
    let view = inspect(&plain_root, &device)
        .unwrap()
        .upgrade_view(&plain_root)
        .unwrap();
    assert_eq!(view.status, "data_protection_required");
    assert!(!view.can_authenticate && !view.can_check_and_backup);
}

#[cfg(unix)]
#[cfg_attr(test, test)]
fn upgrade_review_rejects_checkpoint_symlink() {
    let f = Fixture::new();
    crate::config_file_io::ensure_private_directory(f.device.root()).unwrap();
    let target = f._temporary.path().join("unowned-target");
    std::fs::write(&target, b"unowned-canary").unwrap();
    std::os::unix::fs::symlink(&target, f.device.root().join(checkpoint::FILE)).unwrap();
    assert!(inspect(&f.root, &f.device).is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"unowned-canary");
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    missing_coordinator_uses_existing_safe_future_error();
    upgrade_review_does_not_treat_missing_checkpoint_source_as_fresh_install();
    super::checkpoint_tests::verify_existing_checkpoint_tests();
    upgrade_review_is_read_only_and_secret_free();
    upgrade_review_preserves_future_and_recovery_precedence();
    upgrade_review_never_treats_checkpoint_presence_as_verified();
    upgrade_review_refuses_stale_source_without_repair();
    upgrade_review_distinguishes_fresh_and_unsupported_sources();
    #[cfg(unix)]
    upgrade_review_rejects_checkpoint_symlink();
    println!("PASS startup upgrade review is passive, secret-free and fail-closed");
    Ok(())
}

#[cfg_attr(test, test)]
fn upgrade_review_does_not_treat_missing_checkpoint_source_as_fresh_install() {
    let f = Fixture::new();
    checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    std::fs::remove_file(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let before = snapshot(f._temporary.path());
    let view = inspect(&f.root, &f.device)
        .unwrap()
        .upgrade_view(&f.root)
        .unwrap();
    assert_eq!(view.status, "checkpoint_requires_verification");
    assert!(view.checkpoint_present && !view.can_check_and_backup);
    assert_eq!(snapshot(f._temporary.path()), before);
    std::fs::remove_file(f.root.join("vault.json")).unwrap();
    let before = snapshot(f._temporary.path());
    assert!(inspect(&f.root, &f.device)
        .unwrap()
        .upgrade_view(&f.root)
        .is_err());
    assert_eq!(snapshot(f._temporary.path()), before);
}

#[cfg_attr(test, test)]
fn missing_coordinator_uses_existing_safe_future_error() {
    assert_eq!(
        startup_upgrade_unavailable(Some("db_version_too_new")),
        "upgrade.future_version"
    );
    assert_eq!(
        startup_upgrade_unavailable(None),
        "secret.startup_unavailable"
    );
    assert_eq!(
        startup_upgrade_unavailable(Some("private-path-or-error")),
        "secret.startup_unavailable"
    );
}
