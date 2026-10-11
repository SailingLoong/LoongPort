//! Behavioral coverage for the existing settings persistence owner under a held key guard.

use super::*;
use std::sync::mpsc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(3);
const CANARY: &str = "settings-guard-fixture-secret";

struct Fixture {
    _directory: tempfile::TempDir,
    session: Arc<SecretSession>,
    store: Arc<SettingsStore>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let session = SecretSession::ephemeral().unwrap();
        let store = SettingsStore::open_at(directory.path().join("settings.json"), session.clone())
            .unwrap();
        store
            .mutate(|settings| {
                settings.current_provider_codex = Some("before".into());
                settings.webdav_sync = Some(WebDavSyncSettings {
                    enabled: true,
                    base_url: "https://sync.example.invalid/dav".into(),
                    password: CANARY.into(),
                    ..WebDavSyncSettings::default()
                });
            })
            .unwrap();
        Self {
            _directory: directory,
            session,
            store,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        fs::read(&self.store.path).unwrap()
    }

    fn state(&self) -> serde_json::Value {
        serde_json::to_value(self.store.snapshot()).unwrap()
    }
}

fn write_current_with_guard(
    store: &SettingsStore,
    session: &SecretSession,
    vault: &RwLockReadGuard<'_, VaultContext>,
    id: Option<&str>,
) -> Result<(), AppError> {
    store.set_current_provider_with_vault(&AppType::Codex, id, session, vault)
}

#[cfg_attr(test, test)]
fn held_vault_current_write_finishes_before_waiting_writer() {
    let fixture = Fixture::new();
    let (finished_tx, finished_rx) = mpsc::channel();
    let operation = std::thread::spawn(move || {
        let vault = fixture.session.read().unwrap();
        let writer_session = fixture.session.clone();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            waiting_tx.send(()).unwrap();
            let _key_write = writer_session.write().unwrap();
            acquired_tx.send(()).unwrap();
        });
        waiting_rx.recv_timeout(WAIT).unwrap();
        // The held read guard prevents the writer from completing. Give the
        // writer time to queue so an accidental recursive read blocks on Linux.
        assert!(matches!(
            acquired_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        write_current_with_guard(&fixture.store, &fixture.session, &vault, Some("after")).unwrap();
        let bytes = fs::read(&fixture.store.path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(CANARY));
        let decoded = decode_settings_with_vault(&bytes, &vault).unwrap();
        assert_eq!(decoded.current_provider_codex.as_deref(), Some("after"));
        assert_eq!(decoded.webdav_sync.unwrap().password, CANARY);
        assert_eq!(
            fixture.store.snapshot().current_provider_codex.as_deref(),
            Some("after")
        );
        drop(vault);
        acquired_rx.recv_timeout(WAIT).unwrap();
        writer.join().unwrap();
        finished_tx.send(()).unwrap();
    });
    finished_rx
        .recv_timeout(WAIT)
        .expect("settings pointer publication must not reacquire a held vault read lock");
    operation.join().unwrap();
}

#[cfg_attr(test, test)]
fn another_session_with_the_same_key_and_root_cannot_change_settings() {
    let fixture = Fixture::new();
    let before_disk = fixture.bytes();
    let before_state = fixture.state();
    let foreign = SecretSession::from_context(
        fixture.session.root().to_path_buf(),
        fixture.session.read().unwrap().clone(),
    );
    let foreign_vault = foreign.read().unwrap();

    let result = write_current_with_guard(&fixture.store, &foreign, &foreign_vault, Some("wrong"));

    assert!(matches!(result, Err(AppError::Config(code)) if code == "settings.session_mismatch"));
    assert_eq!(fixture.bytes(), before_disk);
    assert_eq!(fixture.state(), before_state);
}

#[cfg_attr(test, test)]
fn blocked_session_is_rejected_even_when_the_guard_was_already_admitted() {
    let fixture = Fixture::new();
    let before_disk = fixture.bytes();
    let before_state = fixture.state();
    let vault = fixture.session.read().unwrap();
    fixture.session.set_blocked(true);

    let result =
        write_current_with_guard(&fixture.store, &fixture.session, &vault, Some("blocked"));

    assert!(matches!(result, Err(AppError::Config(code)) if code == "secret.recovery_required"));
    assert_eq!(fixture.bytes(), before_disk);
    assert_eq!(fixture.state(), before_state);
}

#[cfg_attr(test, test)]
fn failed_publication_preserves_prior_state_and_destination_contents() {
    let fixture = Fixture::new();
    let before_disk = fixture.bytes();
    let before_state = fixture.state();
    let retained = fixture.store.path.with_file_name("retained.json");
    fs::rename(&fixture.store.path, &retained).unwrap();
    fs::create_dir(&fixture.store.path).unwrap();
    let sentinel = fixture.store.path.join("unrelated");
    fs::write(&sentinel, b"preserved").unwrap();
    let vault = fixture.session.read().unwrap();

    assert!(
        write_current_with_guard(&fixture.store, &fixture.session, &vault, Some("failed")).is_err()
    );
    assert_eq!(fixture.state(), before_state);
    assert_eq!(fs::read(&retained).unwrap(), before_disk);
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserved");
    assert_eq!(
        fs::read_dir(fixture.store.path.parent().unwrap())
            .unwrap()
            .count(),
        2
    );

    fs::remove_file(sentinel).unwrap();
    fs::remove_dir(&fixture.store.path).unwrap();
    fs::rename(retained, &fixture.store.path).unwrap();
    write_current_with_guard(&fixture.store, &fixture.session, &vault, Some("retry")).unwrap();
    let decoded = decode_settings_with_vault(&fixture.bytes(), &vault).unwrap();
    assert_eq!(decoded.current_provider_codex.as_deref(), Some("retry"));
    assert_eq!(decoded.webdav_sync.unwrap().password, CANARY);
}

#[cfg_attr(test, test)]
fn retained_settings_failure_cannot_be_reset_by_guarded_mutation() {
    let fixture = Fixture::new();
    let before_state = fixture.state();
    fs::write(&fixture.store.path, b"{broken").unwrap();
    assert!(fixture.store.reload().is_err());
    let vault = fixture.session.read().unwrap();

    let result = write_current_with_guard(&fixture.store, &fixture.session, &vault, None);

    assert!(matches!(result, Err(AppError::Config(code)) if code == "settings.invalid_json"));
    assert_eq!(fixture.bytes(), b"{broken");
    assert_eq!(fixture.state(), before_state);
    assert!(fixture.store.ready_snapshot().is_err());
}

#[cfg_attr(test, test)]
fn failed_readback_keeps_last_verified_memory_and_reports_published_disk_state() {
    for fault in ["missing", "invalid", "different-pointer", "invalid-secret"] {
        let fixture = Fixture::new();
        let before_state = fixture.state();
        let vault = fixture.session.read().unwrap();
        let result = fixture.store.mutate_with_vault(
            &fixture.session,
            &vault,
            |settings| {
                settings.current_provider_codex = Some("target".into());
                settings.current_provider_codex.clone()
            },
            |path, vault, expected| {
                // Model an external change after publication, using actual disk
                // contents and the same verifier as the guarded pointer seam.
                let published = fs::read(path).unwrap();
                let mut decoded = decode_settings_with_vault(&published, vault).unwrap();
                assert_eq!(decoded.current_provider_codex.as_deref(), Some("target"));
                match fault {
                    "missing" => fs::remove_file(path).unwrap(),
                    "invalid" => fs::write(path, b"{broken").unwrap(),
                    "different-pointer" => {
                        decoded.current_provider_codex = Some("external".into());
                        fs::write(path, encode_settings_with_vault(&decoded, vault).unwrap())
                            .unwrap();
                    }
                    "invalid-secret" => {
                        let mut json: serde_json::Value =
                            serde_json::from_slice(&published).unwrap();
                        json["webdavSync"] = serde_json::json!("invalid-ciphertext");
                        fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
                    }
                    _ => unreachable!(),
                }
                verify_current_provider_at(path, vault, &AppType::Codex, expected)
            },
        );

        assert!(result.is_err(), "must reject {fault} readback");
        if fault == "different-pointer" {
            assert!(
                matches!(result, Err(AppError::Config(code)) if code == "settings.current_provider_readback_mismatch")
            );
            let actual = decode_settings_with_vault(&fixture.bytes(), &vault).unwrap();
            assert_eq!(actual.current_provider_codex.as_deref(), Some("external"));
        } else if fault == "missing" {
            assert!(!fixture.store.path.exists());
        } else if fault == "invalid" {
            assert_eq!(fixture.bytes(), b"{broken");
        }
        assert_eq!(
            fixture.state(),
            before_state,
            "must retain cache for {fault}"
        );
    }
}

#[cfg_attr(test, test)]
fn guarded_pointer_mapping_preserves_unrelated_fields_and_pi_has_no_pointer() {
    let fixture = Fixture::new();
    let vault = fixture.session.read().unwrap();
    for (app, field) in [
        (AppType::Claude, Some("currentProviderClaude")),
        (AppType::ClaudeDesktop, Some("currentProviderClaudeDesktop")),
        (AppType::Codex, Some("currentProviderCodex")),
        (AppType::CodexImage, Some("currentProviderCodexImage")),
        (AppType::Gemini, Some("currentProviderGemini")),
        (AppType::GrokBuild, Some("currentProviderGrokbuild")),
        (AppType::OpenCode, Some("currentProviderOpencode")),
        (AppType::OpenClaw, Some("currentProviderOpenclaw")),
        (AppType::Hermes, Some("currentProviderHermes")),
        (AppType::Pi, None),
    ] {
        for id in [Some("chosen"), None] {
            let mut expected = fixture.state();
            if let Some(field) = field {
                if let Some(id) = id {
                    expected[field] = serde_json::json!(id);
                } else {
                    expected.as_object_mut().unwrap().remove(field);
                }
            }
            fixture
                .store
                .set_current_provider_with_vault(&app, id, &fixture.session, &vault)
                .unwrap();
            let persisted = decode_settings_with_vault(&fixture.bytes(), &vault).unwrap();
            assert_eq!(serde_json::to_value(persisted).unwrap(), expected);
            assert_eq!(fixture.state(), expected);
        }
    }
}

#[cfg_attr(test, test)]
fn ordinary_mutation_still_returns_its_result_and_persists_encrypted_settings() {
    let fixture = Fixture::new();
    let previous = fixture
        .store
        .mutate(|settings| {
            settings.language = Some("en".into());
            settings.current_provider_codex.take()
        })
        .unwrap();

    assert_eq!(previous.as_deref(), Some("before"));
    let bytes = fixture.bytes();
    assert!(!String::from_utf8_lossy(&bytes).contains(CANARY));
    let vault = fixture.session.read().unwrap();
    let decoded = decode_settings_with_vault(&bytes, &vault).unwrap();
    assert_eq!(decoded.current_provider_codex, None);
    assert_eq!(decoded.language.as_deref(), Some("en"));
    assert_eq!(decoded.webdav_sync.unwrap().password, CANARY);
    assert_eq!(fixture.store.snapshot().current_provider_codex, None);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.store.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_app_projection_retains_auth_preservation_policy_without_writes() {
    for value in [Some(false), Some(true), None] {
        for native in [false, true] {
            let _home = crate::secrets::testing::TestHome::new().unwrap();
            let session = SecretSession::from_context(
                crate::config::get_app_config_dir(),
                VaultContext::generate().unwrap(),
            );
            let mut document = serde_json::json!({"currentProviderCodex":"synthetic-current"});
            if let Some(value) = value {
                document["preserveCodexOfficialAuthOnSwitch"] = value.into();
            }
            fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
            fs::write(settings_path(), serde_json::to_vec(&document).unwrap()).unwrap();
            if native {
                unlock_settings(session.clone()).unwrap();
            }
            let before = fs::read(settings_path()).unwrap();
            let vault = session.read().unwrap();
            let selected = read_app_settings_with_vault(
                &AppType::Codex,
                &session,
                &vault,
                1024 * 1024,
                native,
            )
            .unwrap();
            assert_eq!(
                selected.preserve_codex_official_auth_on_switch,
                value.unwrap_or(true)
            );
            assert_eq!(
                selected.current_provider_codex.as_deref(),
                Some("synthetic-current")
            );
            assert_eq!(fs::read(settings_path()).unwrap(), before);
            if !native {
                assert!(get_current_provider_ready(&AppType::Codex).is_err());
            }
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_app_projection_rejects_malformed_policy_without_blocking_peers() {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let session = SecretSession::from_context(
        crate::config::get_app_config_dir(),
        VaultContext::generate().unwrap(),
    );
    fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
    for invalid in [
        serde_json::json!("future"),
        serde_json::json!(null),
        serde_json::json!({}),
    ] {
        let document = serde_json::json!({"preserveCodexOfficialAuthOnSwitch":invalid,
            "currentProviderClaude":"synthetic-peer", "currentProviderCodex":"synthetic-codex"});
        fs::write(settings_path(), serde_json::to_vec(&document).unwrap()).unwrap();
        let before = fs::read(settings_path()).unwrap();
        let vault = session.read().unwrap();
        assert!(
            read_upgrade_settings_with_vault(&AppType::Codex, &session, &vault, 1024 * 1024)
                .is_err()
        );
        let selected =
            read_upgrade_settings_with_vault(&AppType::Claude, &session, &vault, 1024 * 1024)
                .unwrap();
        assert_eq!(
            selected.current_provider_claude.as_deref(),
            Some("synthetic-peer")
        );
        assert_eq!(fs::read(settings_path()).unwrap(), before);
        assert!(get_current_provider_ready(&AppType::Claude).is_err());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_codex_projection_rejects_auth_policy_drift_from_ready_owner() {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let session = SecretSession::from_context(
        crate::config::get_app_config_dir(),
        VaultContext::generate().unwrap(),
    );
    fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
    fs::write(
        settings_path(),
        br#"{"preserveCodexOfficialAuthOnSwitch":true}"#,
    )
    .unwrap();
    unlock_settings(session.clone()).unwrap();
    fs::write(
        settings_path(),
        br#"{"preserveCodexOfficialAuthOnSwitch":false}"#,
    )
    .unwrap();
    let before = fs::read(settings_path()).unwrap();
    let vault = session.read().unwrap();
    assert!(
        matches!(read_native_app_settings_with_vault(&AppType::Codex, &session, &vault, 1024 * 1024),
        Err(AppError::Config(code)) if code == "upgrade.source_changed")
    );
    assert!(
        read_native_app_settings_with_vault(&AppType::Claude, &session, &vault, 1024 * 1024)
            .is_ok()
    );
    assert!(get_settings().preserve_codex_official_auth_on_switch);
    assert_eq!(fs::read(settings_path()).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_app_projection_retains_session_history_policy_without_writes() {
    for value in [Some(false), Some(true), None] {
        for native in [false, true] {
            let _home = crate::secrets::testing::TestHome::new().unwrap();
            let session = SecretSession::from_context(
                crate::config::get_app_config_dir(),
                VaultContext::generate().unwrap(),
            );
            let mut document = serde_json::json!({"currentProviderCodex":"synthetic-current"});
            if let Some(value) = value {
                document["unifyCodexSessionHistory"] = value.into();
            }
            fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
            fs::write(settings_path(), serde_json::to_vec(&document).unwrap()).unwrap();
            if native {
                unlock_settings(session.clone()).unwrap();
            }
            let before = fs::read(settings_path()).unwrap();
            let vault = session.read().unwrap();
            let selected = read_app_settings_with_vault(
                &AppType::Codex,
                &session,
                &vault,
                1024 * 1024,
                native,
            )
            .unwrap();
            assert_eq!(selected.unify_codex_session_history, value.unwrap_or(true));
            assert_eq!(
                selected.current_provider_codex.as_deref(),
                Some("synthetic-current")
            );
            assert_eq!(fs::read(settings_path()).unwrap(), before);
            if !native {
                assert!(get_current_provider_ready(&AppType::Codex).is_err());
            }
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_app_projection_rejects_malformed_history_policy_without_blocking_peers() {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let session = SecretSession::from_context(
        crate::config::get_app_config_dir(),
        VaultContext::generate().unwrap(),
    );
    fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
    for invalid in [
        serde_json::json!("future"),
        serde_json::json!(null),
        serde_json::json!({}),
    ] {
        let document = serde_json::json!({"unifyCodexSessionHistory":invalid,
            "currentProviderClaude":"synthetic-peer", "currentProviderCodex":"synthetic-codex"});
        fs::write(settings_path(), serde_json::to_vec(&document).unwrap()).unwrap();
        let before = fs::read(settings_path()).unwrap();
        let vault = session.read().unwrap();
        assert!(
            read_upgrade_settings_with_vault(&AppType::Codex, &session, &vault, 1024 * 1024)
                .is_err()
        );
        let selected =
            read_upgrade_settings_with_vault(&AppType::Claude, &session, &vault, 1024 * 1024)
                .unwrap();
        assert_eq!(
            selected.current_provider_claude.as_deref(),
            Some("synthetic-peer")
        );
        assert_eq!(fs::read(settings_path()).unwrap(), before);
        assert!(get_current_provider_ready(&AppType::Claude).is_err());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_codex_projection_rejects_session_history_policy_drift_from_ready_owner() {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let session = SecretSession::from_context(
        crate::config::get_app_config_dir(),
        VaultContext::generate().unwrap(),
    );
    fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
    fs::write(settings_path(), br#"{"unifyCodexSessionHistory":true}"#).unwrap();
    unlock_settings(session.clone()).unwrap();
    fs::write(settings_path(), br#"{"unifyCodexSessionHistory":false}"#).unwrap();
    let before = fs::read(settings_path()).unwrap();
    let vault = session.read().unwrap();
    assert!(
        matches!(read_native_app_settings_with_vault(&AppType::Codex, &session, &vault, 1024 * 1024),
        Err(AppError::Config(code)) if code == "upgrade.source_changed")
    );
    assert!(
        read_native_app_settings_with_vault(&AppType::Claude, &session, &vault, 1024 * 1024)
            .is_ok()
    );
    assert!(get_settings().unify_codex_session_history);
    assert_eq!(fs::read(settings_path()).unwrap(), before);
}

/// The external headless runner invokes actual production persistence code.
#[cfg(feature = "test-hooks")]
pub(crate) fn run() {
    let cases: [(&str, fn()); 14] = [
        (
            "Codex session history projection",
            codex_app_projection_retains_session_history_policy_without_writes,
        ),
        (
            "Codex malformed session history",
            codex_app_projection_rejects_malformed_history_policy_without_blocking_peers,
        ),
        (
            "Codex session history drift",
            native_codex_projection_rejects_session_history_policy_drift_from_ready_owner,
        ),
        (
            "Codex auth preservation projection",
            codex_app_projection_retains_auth_preservation_policy_without_writes,
        ),
        (
            "Codex malformed policy and peer isolation",
            codex_app_projection_rejects_malformed_policy_without_blocking_peers,
        ),
        (
            "Codex ready auth policy drift",
            native_codex_projection_rejects_auth_policy_drift_from_ready_owner,
        ),
        (
            "held guard with waiting key writer",
            held_vault_current_write_finishes_before_waiting_writer,
        ),
        (
            "foreign session with identical key and root",
            another_session_with_the_same_key_and_root_cannot_change_settings,
        ),
        (
            "blocked session",
            blocked_session_is_rejected_even_when_the_guard_was_already_admitted,
        ),
        (
            "failed publication",
            failed_publication_preserves_prior_state_and_destination_contents,
        ),
        (
            "retained reload failure",
            retained_settings_failure_cannot_be_reset_by_guarded_mutation,
        ),
        (
            "four readback failure states",
            failed_readback_keeps_last_verified_memory_and_reports_published_disk_state,
        ),
        (
            "all pointer mappings and Pi",
            guarded_pointer_mapping_preserves_unrelated_fields_and_pi_has_no_pointer,
        ),
        (
            "ordinary encrypted mutation",
            ordinary_mutation_still_returns_its_result_and_persists_encrypted_settings,
        ),
    ];
    for (name, verify) in cases {
        verify();
        println!("settings guard passed: {name}");
    }
}
