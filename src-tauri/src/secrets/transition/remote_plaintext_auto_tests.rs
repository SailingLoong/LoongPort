//! Executes one original automatic attempt, not the global worker/debounce scheduler.
use super::{
    manifest_path, object_path, plain, s3_settings, tree, webdav_settings, Fixture, TestRemote,
};
use crate::{
    database::Database,
    error::AppError,
    secrets::storage::StorageContext,
    services::{s3_auto_sync, sync_protocol, webdav_auto_sync},
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{test::MockRuntime, AppHandle, Listener};

const CANARY: &str = "synthetic-automatic-plaintext-attempt-canary";
async fn attempt(
    use_s3: bool,
    db: &Database,
    app: &AppHandle<MockRuntime>,
) -> Result<(), AppError> {
    if use_s3 {
        s3_auto_sync::run_auto_sync_upload_for_test(db, app).await
    } else {
        webdav_auto_sync::run_auto_sync_upload_for_test(db, app).await
    }
}
fn save_settings(remote: &TestRemote, use_s3: bool, enabled: bool, auto_sync: bool) {
    if use_s3 {
        let mut settings = s3_settings(remote);
        settings.enabled = enabled;
        settings.auto_sync = auto_sync;
        crate::settings::set_s3_sync_settings(Some(settings)).unwrap();
    } else {
        let mut settings = webdav_settings(remote);
        settings.enabled = enabled;
        settings.auto_sync = auto_sync;
        crate::settings::set_webdav_sync_settings(Some(settings)).unwrap();
    }
}
fn status(use_s3: bool) -> crate::settings::WebDavSyncStatus {
    if use_s3 {
        crate::settings::get_s3_sync_settings()
            .unwrap()
            .unwrap()
            .status
    } else {
        crate::settings::get_webdav_sync_settings()
            .unwrap()
            .unwrap()
            .status
    }
}
async fn automatic_attempt(use_s3: bool) {
    let fixture = Fixture::new(plain());
    fixture
        .db
        .set_setting("common_config_claude", CANARY)
        .unwrap();
    let identity = fixture.db.secrets.read().unwrap().identity();
    let record = std::fs::read(
        fixture
            .db
            .secrets
            .root()
            .join(crate::secrets::protection::FILE),
    )
    .unwrap();
    let remote = TestRemote::new(use_s3).await;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let received = events.clone();
    let name = if use_s3 {
        crate::events::S3_SYNC_STATUS_UPDATED
    } else {
        crate::events::WEBDAV_SYNC_STATUS_UPDATED
    };
    let listener = app.listen(name, move |event| {
        received
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });

    for (enabled, auto_sync) in [(false, true), (true, false)] {
        save_settings(&remote, use_s3, enabled, auto_sync);
        let before = tree(fixture._home.path());
        let before_status = serde_json::to_value(status(use_s3)).unwrap();
        attempt(use_s3, &fixture.db, app.handle()).await.unwrap();
        assert!(
            remote.state.lock().unwrap().requests.is_empty(),
            "disabled automatic attempt must not issue HTTP"
        );
        assert!(
            events.lock().unwrap().is_empty(),
            "disabled attempt must not emit a success/error upload event"
        );
        assert_eq!(serde_json::to_value(status(use_s3)).unwrap(), before_status);
        assert_eq!(tree(fixture._home.path()), before);
    }
    save_settings(&remote, use_s3, true, true);
    attempt(use_s3, &fixture.db, app.handle()).await.unwrap();
    let state = remote.state.lock().unwrap();
    let head_path = object_path(use_s3, &manifest_path());
    let head = state.objects.get(&head_path).unwrap();
    let manifest: Value = serde_json::from_slice(&head.body).unwrap();
    assert_eq!(manifest["version"], 4);
    assert_eq!(manifest["storage"]["mode"], "plaintext");
    assert!(manifest.get("vault").is_none());
    let StorageContext::Plaintext(plain_identity) = &fixture.current else {
        unreachable!()
    };
    assert_eq!(
        manifest["storage"]["identity"],
        serde_json::to_value(plain_identity).unwrap()
    );
    let id = manifest["snapshotId"].as_str().unwrap();
    let sql_path = object_path(
        use_s3,
        &format!(
            "sync/v3/db-v{}/test/snapshots/{id}/db.sql",
            sync_protocol::DB_COMPAT_VERSION
        ),
    );
    let sql = &state.objects.get(&sql_path).unwrap().body;
    assert!(std::str::from_utf8(sql).unwrap().contains(CANARY));
    let status = status(use_s3);
    assert_eq!(
        status.last_remote_manifest_hash.as_deref(),
        Some(sync_protocol::sha256_hex(&head.body).as_str())
    );
    assert!(status.last_remote_etag.is_some());
    assert!(status.last_error.is_none());
    assert!(state
        .requests
        .iter()
        .any(|(method, path)| method == "PUT" && path == &head_path));
    let emitted = events.lock().unwrap();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0]["source"], "auto");
    assert_eq!(emitted[0]["status"], "success");
    assert!(emitted[0].get("error").is_none());
    assert_eq!(fixture.db.secrets.read().unwrap().identity(), identity);
    assert_eq!(
        std::fs::read(
            fixture
                .db
                .secrets
                .root()
                .join(crate::secrets::protection::FILE)
        )
        .unwrap(),
        record
    );
    assert!(!fixture.db.secrets.root().join("vault.json").exists());
    app.unlisten(listener);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_original_auto_attempt_honors_gates_and_uploads_plaintext() {
    crate::rt::block_on(automatic_attempt(false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_original_auto_attempt_honors_gates_and_uploads_plaintext() {
    crate::rt::block_on(automatic_attempt(true));
}
#[cfg(feature = "test-hooks")]
pub(super) fn run_case(case: &str) {
    match case {
        "webdav" => webdav_original_auto_attempt_honors_gates_and_uploads_plaintext(),
        "s3" => s3_original_auto_attempt_honors_gates_and_uploads_plaintext(),
        _ => panic!("unknown automatic attempt case"),
    }
}
