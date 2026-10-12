#![cfg(all(debug_assertions, unix))]

mod support;

use cc_switch_lib::{integration_test_support, AppType};
use std::{fs, os::unix::fs::symlink, path::PathBuf};

struct RestoreDirectory {
    path: PathBuf,
    original: PathBuf,
}

impl Drop for RestoreDirectory {
    fn drop(&mut self) {
        fs::remove_file(&self.path).unwrap();
        fs::rename(&self.original, &self.path).unwrap();
    }
}

fn reject_both() -> bool {
    let session = support::test_secret_session();
    integration_test_support::source17_state(session.clone(), None).is_err()
        && integration_test_support::direct_state(session, None, &[AppType::Codex]).is_err()
}

#[test]
fn fixture_rejects_dangling_database_symlink_without_creating_target() {
    let _lock = support::test_mutex().lock().unwrap();
    support::reset_test_fs();
    let root = support::test_secret_session().root().to_path_buf();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("must-not-create.db");
    let link = root.join("loongport.db");
    symlink(&target, &link).unwrap();
    let rejected = reject_both();
    fs::remove_file(link).unwrap();
    assert!(
        rejected,
        "both constructors must reject existing symlink membership"
    );
    assert!(fs::symlink_metadata(target).is_err());
}

#[test]
fn fixture_rejects_dangling_device_state_symlink() {
    let _lock = support::test_mutex().lock().unwrap();
    support::reset_test_fs();
    let root = support::test_secret_session().root().to_path_buf();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("must-not-create-state");
    let link = root.join("live-state.json");
    symlink(&target, &link).unwrap();
    let rejected = reject_both();
    fs::remove_file(link).unwrap();
    assert!(
        rejected,
        "both constructors must reject dangling device state"
    );
    assert!(fs::symlink_metadata(target).is_err());
    assert!(!root.join("loongport.db").exists());
}

#[test]
fn fixture_rejects_symlinked_app_root() {
    let _lock = support::test_mutex().lock().unwrap();
    support::reset_test_fs();
    let path = support::test_secret_session().root().to_path_buf();
    let original = path.with_extension("fixture-original");
    fs::rename(&path, &original).unwrap();
    symlink(&original, &path).unwrap();
    let restore = RestoreDirectory { path, original };
    let rejected = reject_both();
    assert!(
        rejected,
        "an alias must not authorize writes through the root"
    );
    assert!(!restore.original.join("loongport.db").exists());
}

#[test]
fn fixture_rejects_symlinked_home() {
    let _lock = support::test_mutex().lock().unwrap();
    support::reset_test_fs();
    let path = support::ensure_test_home().to_path_buf();
    let original = path.with_extension("fixture-original");
    fs::rename(&path, &original).unwrap();
    symlink(&original, &path).unwrap();
    let _restore = RestoreDirectory { path, original };
    assert!(reject_both(), "HOME must be physical rather than an alias");
}
