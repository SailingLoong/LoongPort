//! Installation identity lifecycle through the existing unlocked settings owner.

use super::*;
use crate::secrets::testing::TestHome;

fn fixture() -> (TestHome, Arc<SecretSession>) {
    let home = TestHome::new().unwrap();
    let session = SecretSession::from_context(
        crate::config::get_app_config_dir(),
        VaultContext::generate().unwrap(),
    );
    fs::create_dir_all(settings_path().parent().unwrap()).unwrap();
    unlock_settings(session.clone()).unwrap();
    (home, session)
}

fn persisted(session: &SecretSession) -> AppSettings {
    decode_settings_with_vault(
        &fs::read(settings_path()).unwrap(),
        &session.read().unwrap(),
    )
    .unwrap()
}

#[test]
#[serial_test::serial]
fn frontend_snapshot_cannot_replace_or_clear_an_existing_installation_id() {
    let (_home, session) = fixture();
    mutate_settings(|settings| settings.stats_install_id = Some("existing-installation".into()))
        .unwrap();

    for incoming in [None, Some("frontend-replacement".into())] {
        let mut update = get_settings_for_frontend().unwrap();
        update.stats_install_id = incoming;
        update.enable_anonymous_stats = false;
        update_settings(update).unwrap();
        assert_eq!(
            get_settings().stats_install_id.as_deref(),
            Some("existing-installation")
        );
        assert_eq!(
            persisted(&session).stats_install_id.as_deref(),
            Some("existing-installation")
        );
        assert!(!persisted(&session).enable_anonymous_stats);
    }
}

#[test]
#[serial_test::serial]
fn frontend_snapshot_cannot_create_an_installation_id() {
    let (_home, session) = fixture();
    let mut update = get_settings_for_frontend().unwrap();
    update.stats_install_id = Some("frontend-created".into());
    update.enable_anonymous_stats = false;
    update_settings(update).unwrap();
    assert_eq!(get_settings().stats_install_id, None);
    assert_eq!(persisted(&session).stats_install_id, None);
}

#[test]
#[serial_test::serial]
fn stale_frontend_snapshot_retains_the_id_generated_after_it_was_read() {
    let (_home, session) = fixture();
    let mut stale = get_settings_for_frontend().unwrap();
    assert_eq!(stale.stats_install_id, None);
    mutate_settings(|settings| settings.stats_install_id = Some("later-installation".into()))
        .unwrap();
    stale.language = Some("ja".into());
    update_settings(stale).unwrap();
    assert_eq!(
        get_settings().stats_install_id.as_deref(),
        Some("later-installation")
    );
    assert_eq!(
        persisted(&session).stats_install_id.as_deref(),
        Some("later-installation")
    );
    assert_eq!(persisted(&session).language.as_deref(), Some("ja"));
}

#[test]
#[serial_test::serial]
fn turning_statistics_off_and_on_preserves_the_existing_installation() {
    let (_home, session) = fixture();
    mutate_settings(|settings| settings.stats_install_id = Some("same-installation".into()))
        .unwrap();
    for enabled in [false, true] {
        let mut update = get_settings_for_frontend().unwrap();
        update.enable_anonymous_stats = enabled;
        // Older clients still send None when turning statistics off.
        update.stats_install_id = None;
        update_settings(update).unwrap();
        assert_eq!(
            persisted(&session).stats_install_id.as_deref(),
            Some("same-installation")
        );
        assert_eq!(persisted(&session).enable_anonymous_stats, enabled);
    }
}

#[test]
#[serial_test::serial]
fn explicit_feedback_identity_generation_keeps_statistics_and_metrics_disabled() {
    let (_home, session) = fixture();
    let initial = get_settings_for_frontend().unwrap();
    assert!(!initial.enable_anonymous_stats);
    assert!(!initial.crowd_metrics_enabled);
    let id = ensure_stats_install_id().unwrap();
    let uuid = uuid::Uuid::parse_str(&id).unwrap();
    assert_eq!(uuid.get_version_num(), 4);
    assert_eq!(uuid.to_string(), id);
    assert_eq!(
        get_settings().stats_install_id.as_deref(),
        Some(id.as_str())
    );
    let saved = persisted(&session);
    assert_eq!(saved.stats_install_id.as_deref(), Some(id.as_str()));
    assert!(!saved.enable_anonymous_stats);
    assert!(!saved.crowd_metrics_enabled);
    assert_eq!(saved.stats_notice_confirmed, initial.stats_notice_confirmed);
    unlock_settings_for_test(session).unwrap();
    assert_eq!(ensure_stats_install_id().unwrap(), id);
    assert!(!get_settings().enable_anonymous_stats);
}

#[test]
#[serial_test::serial]
fn an_existing_installation_id_is_reused_without_rewriting_the_settings_file() {
    let (_home, session) = fixture();
    mutate_settings(|settings| settings.stats_install_id = Some("legacy-installation".into()))
        .unwrap();
    let before = fs::read(settings_path()).unwrap();
    assert_eq!(ensure_stats_install_id().unwrap(), "legacy-installation");
    assert_eq!(fs::read(settings_path()).unwrap(), before);
    let reopened = SettingsStore::open_at(settings_path(), session).unwrap();
    assert_eq!(
        reopened.snapshot().stats_install_id.as_deref(),
        Some("legacy-installation")
    );
}

#[test]
#[serial_test::serial]
fn concurrent_requests_share_one_persisted_installation_identity() {
    let (_home, session) = fixture();
    let barrier = Arc::new(std::sync::Barrier::new(12));
    let workers: Vec<_> = (0..12)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                ensure_stats_install_id().unwrap()
            })
        })
        .collect();
    let ids: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(persisted(&session).stats_install_id.as_ref(), Some(&ids[0]));
    assert!(!persisted(&session).enable_anonymous_stats);
}

#[test]
#[serial_test::serial]
fn a_failed_identity_save_never_publishes_an_id_and_can_be_retried() {
    let (_home, session) = fixture();
    mutate_settings(|settings| settings.language = Some("ja".into())).unwrap();
    let before = fs::read(settings_path()).unwrap();
    let retained = settings_path().with_file_name("retained.json");
    fs::rename(settings_path(), &retained).unwrap();
    fs::create_dir(settings_path()).unwrap();
    let sentinel = settings_path().join("unrelated");
    fs::write(&sentinel, b"preserved").unwrap();

    assert!(ensure_stats_install_id().is_err());
    assert_eq!(get_settings().stats_install_id, None);
    assert!(!get_settings().enable_anonymous_stats);
    assert_eq!(fs::read(&retained).unwrap(), before);
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserved");

    fs::remove_file(sentinel).unwrap();
    fs::remove_dir(settings_path()).unwrap();
    fs::rename(retained, settings_path()).unwrap();
    let id = ensure_stats_install_id().unwrap();
    assert_eq!(
        persisted(&session).stats_install_id.as_deref(),
        Some(id.as_str())
    );
    assert_eq!(persisted(&session).language.as_deref(), Some("ja"));
}

#[test]
#[serial_test::serial]
fn a_locked_settings_owner_cannot_generate_an_installation_identity() {
    let _home = TestHome::new().unwrap();
    assert!(
        matches!(ensure_stats_install_id(), Err(AppError::Config(code)) if code == "secret.locked")
    );
    assert!(!settings_path().exists());
}

#[test]
#[serial_test::serial]
fn a_blocked_secret_session_cannot_generate_an_installation_identity() {
    let (_home, session) = fixture();
    session.set_blocked(true);
    assert!(
        matches!(ensure_stats_install_id(), Err(AppError::Config(code)) if code == "secret.recovery_required")
    );
    assert_eq!(get_settings().stats_install_id, None);
    assert!(!settings_path().exists());
}

#[test]
#[cfg(unix)]
#[serial_test::serial]
fn a_failed_atomic_write_keeps_the_generated_id_out_of_memory_and_disk() {
    use std::os::unix::fs::PermissionsExt;

    let (_home, session) = fixture();
    mutate_settings(|settings| settings.language = Some("ja".into())).unwrap();
    let path = settings_path();
    let before = fs::read(&path).unwrap();
    let parent = path.parent().unwrap();
    let permissions = fs::metadata(parent).unwrap().permissions();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
    let result = ensure_stats_install_id();
    fs::set_permissions(parent, permissions).unwrap();

    assert!(
        result.is_err(),
        "the synthetic parent must reject the atomic write"
    );
    assert_eq!(get_settings().stats_install_id, None);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
    let id = ensure_stats_install_id().unwrap();
    assert_eq!(
        persisted(&session).stats_install_id.as_deref(),
        Some(id.as_str())
    );
}
