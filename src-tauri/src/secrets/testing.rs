use super::{
    key_store::{KeyStore, KeyStoreError},
    session::SecretSession,
};
use crate::{database::Database, error::AppError};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use zeroize::Zeroizing;

type StoredKeys = HashMap<(String, String), Zeroizing<Vec<u8>>>;

#[derive(Default)]
pub(crate) struct MemoryKeyStore(Mutex<StoredKeys>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, vault: &str, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(vault.into(), key.into()))
            .cloned())
    }
    fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .unwrap()
            .insert((vault.into(), key.into()), Zeroizing::new(bytes.to_vec()));
        Ok(())
    }
    fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
        self.0.lock().unwrap().remove(&(vault.into(), key.into()));
        Ok(())
    }
}

pub(crate) fn initialize_database() -> Result<Database, AppError> {
    let test_home = std::env::var_os("CC_SWITCH_TEST_HOME")
        .expect("disk tests must select an isolated home before initializing the secret session");
    let app_root = crate::config::get_app_config_dir();
    // Tripwire：解析结果必须落在测试 home 内。曾因 Windows 的 v3.10.3 legacy
    // 回退读到真实用户目录，测试密钥把真实数据库做了 vault 迁移——宁可当场
    // 炸掉也不允许测试碰真实数据。
    assert!(
        app_root.starts_with(std::path::Path::new(&test_home)),
        "test app root {} escaped the isolated test home",
        app_root.display()
    );
    static STORE: OnceLock<MemoryKeyStore> = OnceLock::new();
    let session = SecretSession::open(&app_root, STORE.get_or_init(MemoryKeyStore::default), None)?;
    crate::settings::unlock_settings_for_test(session.clone())?;
    Database::init_with_secrets(session)
}

/// Allocate an isolated storage fixture using the physical temporary directory.
/// Storage tests must not inherit system path aliases from the temporary base.
pub(crate) fn tempdir() -> std::io::Result<tempfile::TempDir> {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)
}

/// Select a physical isolated home while holding the test's serial guard.
/// Restore the previous override before deleting the temporary directory.
pub(crate) struct TestHome {
    previous: Option<std::ffi::OsString>,
    settings: crate::settings::TestSettingsScope,
    directory: tempfile::TempDir,
}

impl TestHome {
    pub(crate) fn new() -> std::io::Result<Self> {
        Self::from_directory(tempdir()?)
    }

    pub(crate) fn from_directory(directory: tempfile::TempDir) -> std::io::Result<Self> {
        let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", directory.path());
        let settings = match crate::settings::TestSettingsScope::enter() {
            Ok(settings) => settings,
            Err(error) => {
                match &previous {
                    Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                    None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
                }
                return Err(std::io::Error::other(error.to_string()));
            }
        };
        Ok(Self {
            previous,
            settings,
            directory,
        })
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self.directory.path()
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        self.settings.restore();
    }
}
