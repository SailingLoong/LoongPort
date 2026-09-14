use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cc_switch_lib::{
    reload_settings, unlock_settings, update_settings, AppSettings, AppState, Database, KeyStore,
    KeyStoreError, MultiAppConfig, SecretSession,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct TestKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for TestKeyStore {
    fn load(
        &self,
        vault_id: &str,
        key_id: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .expect("test key store poisoned")
            .get(&format!("{vault_id}/{key_id}"))
            .cloned()
            .map(Zeroizing::new))
    }

    fn save(&self, vault_id: &str, key_id: &str, key: &[u8]) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .expect("test key store poisoned")
            .insert(format!("{vault_id}/{key_id}"), key.to_vec());
        Ok(())
    }

    fn remove(&self, vault_id: &str, key_id: &str) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .expect("test key store poisoned")
            .remove(&format!("{vault_id}/{key_id}"));
        Ok(())
    }
}

/// 为测试设置隔离的 HOME 目录，避免污染真实用户数据。
pub fn ensure_test_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("cc-switch-test-home-{}", std::process::id()));
        if base.exists() {
            let _ = std::fs::remove_dir_all(&base);
        }
        std::fs::create_dir_all(&base).expect("create test home");
        // Windows 上 `dirs::home_dir()` 不受 HOME/USERPROFILE 影响（走 Known Folder API），
        // 用 CC_SWITCH_TEST_HOME 显式覆盖，以确保测试不会污染真实用户目录。
        std::env::set_var("CC_SWITCH_TEST_HOME", &base);
        std::env::set_var("HOME", &base);
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", &base);
        base
    })
    .as_path()
}

/// 清理测试目录中生成的配置文件与缓存。
pub fn reset_test_fs() {
    let home = ensure_test_home();
    for sub in [
        ".claude",
        ".codex",
        ".gemini",
        ".grok",
        ".config",
        ".openclaw",
        "profiles",
    ] {
        let path = home.join(sub);
        if path.exists() {
            if let Err(err) = std::fs::remove_dir_all(&path) {
                eprintln!("failed to clean {}: {}", path.display(), err);
            }
        }
    }
    let app_root = home.join(cc_switch_lib::APP_DIR_NAME);
    if let Ok(entries) = std::fs::read_dir(&app_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|name| name.to_str()) == Some("vault.json") {
                continue;
            }
            let result = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(error) = result {
                eprintln!("failed to clean {}: {error}", path.display());
            }
        }
    }
    let claude_json = home.join(".claude.json");
    if claude_json.exists() {
        let _ = std::fs::remove_file(&claude_json);
    }

    ensure_test_settings();
    reload_settings().expect("reload missing isolated settings as defaults");
    update_settings(AppSettings::default()).expect("reset encrypted test settings");
}

pub fn test_secret_session() -> Arc<SecretSession> {
    static KEY_STORE: OnceLock<TestKeyStore> = OnceLock::new();
    static SESSION: OnceLock<Arc<SecretSession>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let root = ensure_test_home().join(cc_switch_lib::APP_DIR_NAME);
            // export_sql / 同步快照要求 vault 带口令包裹材料（便携可恢复），
            // 共享 session 固定用测试口令打开，避免测试间因执行顺序漂移。
            SecretSession::open(
                &root,
                KEY_STORE.get_or_init(TestKeyStore::default),
                Some("integration-test-portable"),
            )
            .expect("open isolated test secret session")
        })
        .clone()
}

fn ensure_test_settings() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        unlock_settings(test_secret_session()).expect("install isolated encrypted settings")
    });
}

#[allow(dead_code)]
pub fn enable_codex_official_auth_preservation() {
    ensure_test_settings();
    update_settings(AppSettings {
        preserve_codex_official_auth_on_switch: true,
        ..Default::default()
    })
    .expect("enable Codex official auth preservation");
}

/// 全局互斥锁，避免多测试并发写入相同的 HOME 目录。
pub fn test_mutex() -> &'static Mutex<()> {
    static MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

/// 创建测试用的 AppState，包含一个空的数据库
#[allow(dead_code)]
pub fn create_test_state() -> Result<AppState, Box<dyn std::error::Error>> {
    ensure_test_settings();
    let db = Arc::new(Database::init_with_secrets(test_secret_session())?);
    Ok(AppState::new(db).unwrap())
}

/// 创建测试用的 AppState，并从 MultiAppConfig 迁移数据
#[allow(dead_code)]
pub fn create_test_state_with_config(
    config: &MultiAppConfig,
) -> Result<AppState, Box<dyn std::error::Error>> {
    ensure_test_settings();
    let db = Arc::new(Database::init_with_secrets(test_secret_session())?);
    db.migrate_from_json(config)?;
    Ok(AppState::new(db).unwrap())
}
