//! Pure revision and path primitives from cc-switch v4.0.2.
//! The remaining engine joins this same module after encrypted persistence and
//! recovery integration; no backup/publication/replan entrypoints are active here.

use sha2::{Digest, Sha256};

use crate::secrets::owned_file::{DeviceFile, DEVICE_BACKUP_DIR, DEVICE_STATE_FILE};
use std::path::PathBuf;

/// Path-only upstream device-store seam. It never resolves the configured/synced
/// data root, creates directories, opens a vault, or performs a recovery action.
/// Persistence must additionally validate the resolved root and its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceStore {
    root: PathBuf,
}

impl DeviceStore {
    pub(crate) fn for_device() -> Self {
        Self::at(crate::config::get_home_dir().join(crate::config::APP_DIR_NAME))
    }

    pub(crate) fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub(crate) fn state_path(&self) -> PathBuf {
        self.root.join(DEVICE_STATE_FILE)
    }

    pub(crate) fn first_write_backup_dir(&self) -> PathBuf {
        self.root.join(DEVICE_BACKUP_DIR)
    }

    pub(crate) fn path_for(&self, file: &DeviceFile) -> PathBuf {
        self.root.join(file.relative_path())
    }
}

/// 文件内容的 hash；文件不存在是 `None`。
pub fn digest(bytes: Option<&[u8]>) -> Option<String> {
    bytes.map(sha256_hex)
}

/// 十六进制的 SHA-256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod device_path_tests {
    use super::*;
    use crate::secrets::owned_file::DeviceFile;

    #[test]
    #[serial_test::serial]
    fn device_store_uses_fixed_home_instead_of_configured_database_root() {
        let store = DeviceStore::for_device();
        assert_eq!(
            store.state_path(),
            crate::config::get_home_dir()
                .join(crate::config::APP_DIR_NAME)
                .join("live-state.json")
        );
        assert_eq!(
            store.state_path().parent().unwrap().file_name().unwrap(),
            ".loongport"
        );
    }

    #[test]
    fn typed_path_construction_does_not_create_any_storage() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("not-created");
        let store = DeviceStore::at(&root);
        let state = DeviceFile::registered("live-state.json").unwrap();
        assert_eq!(store.path_for(&state), root.join("live-state.json"));
        assert_eq!(store.state_path(), root.join("live-state.json"));
        assert_eq!(
            store.first_write_backup_dir(),
            root.join("backups/live-first-write")
        );
        assert!(!root.exists());
        assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 0);
    }
}
