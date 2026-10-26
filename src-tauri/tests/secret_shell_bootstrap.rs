//! Shell reads use only temporary fixtures and in-memory keys, never OS custody.
use cc_switch_lib::{
    credential_runtime_availability, get_bootstrap_settings_for_frontend,
    get_settings_for_frontend, reload_settings, unlock_settings, update_settings, AppSettings,
    KeyStore, KeyStoreError, SecretSession,
};
use std::{collections::BTreeMap, sync::Mutex};
use zeroize::Zeroizing;

struct TestHome(Option<std::ffi::OsString>);
impl TestHome {
    fn enter(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", path);
        Self(previous)
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

#[derive(Default)]
struct MemoryKeys(Mutex<BTreeMap<(String, String), Vec<u8>>>);
impl KeyStore for MemoryKeys {
    fn load(&self, vault: &str, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(vault.into(), key.into()))
            .cloned()
            .map(Zeroizing::new))
    }
    fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .unwrap()
            .insert((vault.into(), key.into()), bytes.to_vec());
        Ok(())
    }
    fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
        self.0.lock().unwrap().remove(&(vault.into(), key.into()));
        Ok(())
    }
}

#[test]
fn availability_requires_an_actual_available_session_without_custody_io() {
    assert!(!credential_runtime_availability(None).credential_runtime_available);
    let root = tempfile::tempdir().unwrap();
    let keys = MemoryKeys::default();
    let session = SecretSession::open(root.path(), &keys, None).unwrap();
    let before = std::fs::read(root.path().join("vault.json")).unwrap();
    assert!(credential_runtime_availability(Some(&session)).credential_runtime_available);
    assert_eq!(
        serde_json::to_value(credential_runtime_availability(Some(&session))).unwrap(),
        serde_json::json!({"credentialRuntimeAvailable": true})
    );
    assert_eq!(
        std::fs::read(root.path().join("vault.json")).unwrap(),
        before
    );
    assert!(!root.path().join("loongport.db").exists());
}

// One test owns the process-global settings store and environment override.
#[test]
fn keyless_shell_reads_only_bootstrap_preserves_errors_and_cannot_save() {
    let home = tempfile::tempdir().unwrap();
    let _test_home = TestHome::enter(home.path());
    let root = home.path().join(".loongport");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    let bytes = br#"{"language":"zh","silentStartup":true,"receiveBetaUpdates":true,"webdavSync":"lpenc1:synthetic-unreadable","s3Sync":"lpenc1:synthetic-unreadable","serviceOnboardingCompleted":true}"#;
    std::fs::write(&path, bytes).unwrap();
    reload_settings().unwrap();
    assert!(
        get_settings_for_frontend().is_err(),
        "full settings must never masquerade as bootstrap defaults while unavailable"
    );
    let settings = get_bootstrap_settings_for_frontend()
        .expect("nonsecret shell projection must be readable without a key");
    assert!(settings.silent_startup && settings.receive_beta_updates);
    assert_eq!(settings.language.as_deref(), Some("zh"));
    assert!(settings.webdav_sync.is_none() && settings.s3_sync.is_none());
    assert!(settings.webdav_backup.is_none() && settings.upgrade_completion.is_none());
    assert!(
        !settings.service_onboarding_completed,
        "protected/full settings are not represented as loaded"
    );
    assert!(update_settings(AppSettings::default()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!root.join("vault.json").exists());
    assert!(!root.join("loongport.db").exists());
    std::fs::write(&path, b"{broken synthetic settings").unwrap();
    assert!(reload_settings().is_err());
    assert!(
        get_bootstrap_settings_for_frontend().is_err(),
        "corrupt bootstrap must not look like defaults"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken synthetic settings");
    // A genuinely authenticated settings owner keeps its full original semantics.
    let full = br#"{"silentStartup":false,"serviceOnboardingCompleted":true}"#;
    std::fs::write(&path, full).unwrap();
    reload_settings().unwrap();
    let keys = MemoryKeys::default();
    let session = SecretSession::open(&root, &keys, None).unwrap();
    unlock_settings(session).unwrap();
    let settings = get_settings_for_frontend().unwrap();
    assert!(settings.service_onboarding_completed);
    assert!(!settings.silent_startup);
    assert_eq!(std::fs::read(&path).unwrap(), full);
    std::fs::write(&path, b"{broken authenticated settings").unwrap();
    assert!(reload_settings().is_err());
    assert!(
        get_settings_for_frontend().is_err(),
        "authenticated read failure cannot fallback to bootstrap"
    );
}

#[cfg(feature = "test-hooks")]
#[test]
fn original_settings_owner_rejects_retired_reads_and_preserves_guarded_writes() {
    cc_switch_lib::ProviderService::verify_settings_vault_guard().unwrap();
}

#[test]
fn daily_manual_unlock_commands_are_not_registered() {
    let source = include_str!("../src/lib.rs");
    let registration = source
        .split("tauri::generate_handler![")
        .nth(1)
        .expect("the actual GUI IPC registry")
        .split(
            "];
",
        )
        .next()
        .unwrap();
    for command in [
        "secrets::startup::unlock_secret_vault,",
        "secrets::rewrap::get_secret_protection,",
        "secrets::rewrap::set_secret_password,",
    ] {
        assert!(
            !registration.contains(command),
            "retired daily command remains registered: {command}"
        );
    }
    for command in [
        "secrets::startup::get_runtime_availability,",
        "commands::get_bootstrap_settings,",
        "commands::get_settings,",
        "secrets::startup::restore_startup_vault,",
        "secrets::rewrap::rotate_secret_key,",
    ] {
        assert!(
            registration.contains(command),
            "retained contract missing: {command}"
        );
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn runtime_availability_requires_completed_original_startup_publication() {
    cc_switch_lib::integration_test_support::verify_normal_upgrade_startup("shell-availability");
}
