//! Synthetic loopback tests of the original transport consumers, not a second protocol owner.
use super::*;
use crate::services::{
    s3_sync,
    sync_protocol::{self, test_remote::TestRemote, PutCondition, DB_COMPAT_VERSION},
    webdav_sync,
};
use crate::settings::{S3SyncSettings, WebDavSyncSettings, WebDavSyncStatus};
use serde_json::{json, Value};

const PLAIN_CANARY: &str = "synthetic-remote-plaintext-upload-canary";
const SECRET_CANARY: &str = "SECRET_CANARY_REMOTE_MANIFEST_BODY_NOT_DIAGNOSTICS";

// Unit tests reuse the existing test logger. The native test-hooks binary does
// not compile that unrelated unit suite, so its capture is mutually exclusive.
#[cfg(test)]
use crate::relay::model_verification::privacy_tests::{captured_logs, init_logger};
#[cfg(not(test))]
mod native_logs {
    use std::sync::{Mutex, Once};
    struct Capture(Mutex<String>);
    impl log::Log for Capture {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.target().starts_with("cc_switch_lib")
                || metadata.target() == "remote_transport_test"
        }
        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                use std::fmt::Write;
                writeln!(
                    self.0.lock().unwrap(),
                    "{} {}",
                    record.target(),
                    record.args()
                )
                .unwrap();
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Capture = Capture(Mutex::new(String::new()));
    static INIT: Once = Once::new();
    pub(super) fn init_logger() {
        INIT.call_once(|| {
            log::set_logger(&LOGGER)
                .expect("isolated native binary owns its synthetic log capture");
            log::set_max_level(log::LevelFilter::Trace);
        });
    }
    pub(super) fn captured_logs() -> String {
        LOGGER.0.lock().unwrap().clone()
    }
}
#[cfg(not(test))]
use native_logs::{captured_logs, init_logger};

fn webdav_settings(remote: &TestRemote) -> WebDavSyncSettings {
    WebDavSyncSettings {
        base_url: remote.endpoint.clone(),
        username: "synthetic-test".into(),
        password: "synthetic-transport-only".into(),
        remote_root: "sync".into(),
        profile: "test".into(),
        ..Default::default()
    }
}
fn s3_settings(remote: &TestRemote) -> S3SyncSettings {
    S3SyncSettings {
        endpoint: remote.endpoint.clone(),
        bucket: "bucket".into(),
        region: "test".into(),
        access_key_id: "synthetic-test".into(),
        secret_access_key: "synthetic-transport-only".into(),
        remote_root: "sync".into(),
        profile: "test".into(),
        ..Default::default()
    }
}
fn manifest_path() -> String {
    // Version 4 changes the descriptor, not the existing v3 transport namespace.
    format!("sync/v3/db-v{DB_COMPAT_VERSION}/test/manifest.json")
}
fn object_path(use_s3: bool, relative: &str) -> String {
    format!("{}{relative}", if use_s3 { "/bucket/" } else { "/" })
}
fn assert_original_encrypted_fixture(fixture: &Fixture) {
    let manifest = serde_json::to_value(&fixture.snapshot.manifest).unwrap();
    assert_eq!(manifest["version"], 3);
    assert!(manifest.get("vault").is_some());
    assert!(manifest.get("storage").is_none());
    assert!(!std::str::from_utf8(&fixture.snapshot.db_sql)
        .unwrap()
        .contains("remote-joint-canary"));
}
async fn upload(use_s3: bool) {
    let fixture = Fixture::new(plain());
    assert_original_encrypted_fixture(&fixture);
    fixture
        .db
        .set_setting("common_config_claude", PLAIN_CANARY)
        .unwrap();
    let original_identity = fixture.db.secrets.read().unwrap().identity();
    let record = std::fs::read(
        fixture
            .db
            .secrets
            .root()
            .join(crate::secrets::protection::FILE),
    )
    .unwrap();
    let remote = TestRemote::new(use_s3).await;
    let status: WebDavSyncStatus = if use_s3 {
        let mut settings = s3_settings(&remote);
        sync_protocol::run_with_sync_lock(s3_sync::upload(&fixture.db, &mut settings))
            .await
            .unwrap();
        settings.status
    } else {
        let mut settings = webdav_settings(&remote);
        sync_protocol::run_with_sync_lock(webdav_sync::upload(&fixture.db, &mut settings))
            .await
            .unwrap();
        settings.status
    };
    let remote = remote.state.lock().unwrap();
    let head_path = object_path(use_s3, &manifest_path());
    let head = &remote.objects.get(&head_path).unwrap().body;
    let manifest: Value = serde_json::from_slice(head).unwrap();
    assert_eq!(manifest["version"], 4);
    assert_eq!(manifest["storage"]["mode"], "plaintext");
    let StorageContext::Plaintext(identity) = &fixture.current else {
        unreachable!()
    };
    assert_eq!(
        manifest["storage"]["identity"],
        serde_json::to_value(identity).unwrap()
    );
    assert!(
        manifest.get("vault").is_none(),
        "plaintext publication must not invent vault metadata"
    );
    let snapshot_id = manifest["snapshotId"].as_str().unwrap();
    let sql_path = object_path(
        use_s3,
        &format!("sync/v3/db-v{DB_COMPAT_VERSION}/test/snapshots/{snapshot_id}/db.sql"),
    );
    let sql = &remote.objects.get(&sql_path).unwrap().body;
    assert!(std::str::from_utf8(sql).unwrap().contains(PLAIN_CANARY));
    assert_eq!(
        manifest["artifacts"]["db.sql"]["sha256"],
        sync_protocol::sha256_hex(sql)
    );
    assert_eq!(
        status.last_remote_manifest_hash.as_deref(),
        Some(sync_protocol::sha256_hex(head).as_str())
    );
    assert!(status.last_remote_etag.is_some());
    assert!(remote
        .requests
        .iter()
        .any(|(method, path)| method == "PUT" && path == &head_path));
    assert!(remote
        .requests
        .iter()
        .any(|(method, path)| method == "PUT" && path == &sql_path));
    assert!(!remote
        .requests
        .iter()
        .any(|(_, path)| path.contains("/v4/")));
    assert_eq!(
        fixture.db.secrets.read().unwrap().identity(),
        original_identity
    );
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
}
async fn preview_privacy(use_s3: bool) {
    init_logger();
    let fixture = Fixture::new(plain());
    assert_original_encrypted_fixture(&fixture);
    let before_tree = tree(fixture._home.path());
    let remote = TestRemote::new(use_s3).await;
    let mut condition = PutCondition::Absent;
    let mut outcomes = Vec::new();
    for field in ["format", "version"] {
        let mut manifest = serde_json::to_value(&fixture.snapshot.manifest).unwrap();
        manifest[field] = json!(SECRET_CANARY);
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let etag = remote
            .publish(&manifest_path(), &bytes, &condition)
            .await
            .unwrap()
            .unwrap();
        condition = PutCondition::Match(etag);
        let start = captured_logs().len();
        log::info!(target: "remote_transport_test", "fetch_remote_info_begin");
        let result = if use_s3 {
            s3_sync::fetch_remote_info(&s3_settings(&remote)).await
        } else {
            webdav_sync::fetch_remote_info(&webdav_settings(&remote)).await
        };
        let (admitted_safely, error_private) = match result {
            Err(error) => (true, !format!("{error} {error:?}").contains(SECRET_CANARY)),
            Ok(Some(value)) => (
                field == "format" && value["compatible"] == false,
                !serde_json::to_string(&value)
                    .unwrap()
                    .contains(SECRET_CANARY),
            ),
            Ok(None) => (false, true),
        };
        let captured = captured_logs();
        let logs = &captured[start..];
        assert!(
            logs.contains("fetch_remote_info_begin"),
            "logger capture must be active"
        );
        outcomes.push((
            field,
            admitted_safely,
            error_private,
            !logs.contains(SECRET_CANARY),
        ));
    }
    // The artifact digest is untrusted manifest text too. Exercise the actual
    // transport fetch/verification path, not only its shared serde/hash helpers.
    let mut manifest = fixture.snapshot.manifest.clone();
    manifest.artifacts.get_mut("db.sql").unwrap().sha256 = SECRET_CANARY.into();
    manifest.snapshot_id =
        sync_protocol::compute_snapshot_id(&manifest.artifacts, manifest.vault_metadata().unwrap());
    remote
        .artifact(
            &format!(
                "sync/v3/db-v{DB_COMPAT_VERSION}/test/snapshots/{}/db.sql",
                manifest.snapshot_id
            ),
            &fixture.snapshot.db_sql,
        )
        .await
        .unwrap();
    remote
        .publish(
            &manifest_path(),
            &serde_json::to_vec(&manifest).unwrap(),
            &condition,
        )
        .await
        .unwrap()
        .unwrap();
    let start = captured_logs().len();
    log::info!(target: "remote_transport_test", "fetch_snapshot_digest_begin");
    let result = if use_s3 {
        s3_sync::fetch_snapshot(&s3_settings(&remote)).await
    } else {
        webdav_sync::fetch_snapshot(&webdav_settings(&remote)).await
    };
    let (rejected, error_private) = match result {
        Err(error) => (
            true,
            !format!("{error} {error:?}").contains(&SECRET_CANARY[..8]),
        ),
        Ok(_) => (false, true),
    };
    let captured = captured_logs();
    let logs = &captured[start..];
    assert!(logs.contains("fetch_snapshot_digest_begin"));
    outcomes.push((
        "artifact_digest",
        rejected,
        error_private,
        !logs.contains(&SECRET_CANARY[..8]),
    ));
    assert_eq!(tree(fixture._home.path()), before_tree);
    let requests = &remote.state.lock().unwrap().requests;
    assert!(
        requests
            .iter()
            .filter(|(method, path)| method == "GET" && path.ends_with("/manifest.json"))
            .count()
            >= 2
    );
    assert!(requests
        .iter()
        .all(|(_, path)| !path.contains(SECRET_CANARY)));
    assert!(outcomes.iter().all(|(_,rejected,error_private,logs_private)| *rejected && *error_private && *logs_private),
        "actual remote admission/privacy outcomes (field,admitted_safely,error_private,logs_private): {outcomes:?}");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_uploads_explicit_plaintext_in_existing_transport_namespace() {
    crate::rt::block_on(upload(false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_uploads_explicit_plaintext_in_existing_transport_namespace() {
    crate::rt::block_on(upload(true));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_preview_rejects_bad_manifests_without_echoing_their_body() {
    crate::rt::block_on(preview_privacy(false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_preview_rejects_bad_manifests_without_echoing_their_body() {
    crate::rt::block_on(preview_privacy(true));
}
fn assert_plain_source(snapshot: &DownloadedSnapshot) {
    let value = serde_json::to_value(&snapshot.manifest).unwrap();
    assert_eq!(value["version"], 4);
    assert_eq!(value["storage"]["mode"], "plaintext");
    assert!(value.get("vault").is_none());
    assert!(std::str::from_utf8(&snapshot.db_sql)
        .unwrap()
        .contains("remote-joint-canary"));
}
async fn seed_snapshot(
    remote: &TestRemote,
    snapshot: &DownloadedSnapshot,
    condition: &PutCondition,
) -> String {
    for (name, bytes) in [
        ("db.sql", &snapshot.db_sql),
        ("skills.zip", &snapshot.skills_zip),
    ] {
        let relative = sync_protocol::snapshot_artifact_path(&snapshot.manifest, name).unwrap();
        remote
            .artifact(
                &format!("sync/v3/db-v{DB_COMPAT_VERSION}/test/{relative}"),
                bytes,
            )
            .await
            .unwrap();
    }
    remote
        .publish(
            &manifest_path(),
            &serde_json::to_vec(&snapshot.manifest).unwrap(),
            condition,
        )
        .await
        .unwrap()
        .unwrap()
}
async fn fetched(remote: &TestRemote, use_s3: bool) -> DownloadedSnapshot {
    if use_s3 {
        s3_sync::fetch_snapshot(&s3_settings(remote)).await.unwrap()
    } else {
        webdav_sync::fetch_snapshot(&webdav_settings(remote))
            .await
            .unwrap()
    }
}
async fn fetch_and_normal_apply(use_s3: bool, encrypted_target: bool) {
    let fixture = Fixture::new_with_source(
        if encrypted_target {
            text(TARGET_TEXT)
        } else {
            plain()
        },
        plain(),
    );
    assert_plain_source(&fixture.snapshot);
    let remote = TestRemote::new(use_s3).await;
    seed_snapshot(&remote, &fixture.snapshot, &PutCondition::Absent).await;
    let _sync = sync_protocol::sync_mutex().lock().await;
    let source = fetched(&remote, use_s3).await;
    assert_plain_source(&source);
    // This is the actual normal-apply body used by public download, with the
    // original NoStore tripwire. We deliberately do not execute the public
    // async wrapper, whose hardcoded SystemKeyStore is outside test authority.
    sync_protocol::apply_snapshot_with_store(
        &fixture.db,
        &source.manifest,
        &source.db_sql,
        &source.skills_zip,
        &NoStore,
    )
    .unwrap();
    fixture.assert_installed();
}
async fn explicit_empty_input() {
    let fixture = Fixture::new_with_source(text(TARGET_TEXT), plain());
    assert_plain_source(&fixture.snapshot);
    let remote = TestRemote::new(true).await;
    seed_snapshot(&remote, &fixture.snapshot, &PutCondition::Absent).await;
    let _sync = sync_protocol::sync_mutex().lock().await;
    let source = fetched(&remote, true).await;
    sync_protocol::restore_from_sync(
        &fixture.db,
        &source,
        &source.manifest.snapshot_id,
        "",
        &NoStore,
    )
    .unwrap();
    fixture.assert_installed();
}
fn rejected_source_id(fixture: &Fixture, source: &DownloadedSnapshot, expected: &str) {
    let before = tree(fixture._home.path());
    let result = sync_protocol::restore_from_sync(&fixture.db, source, expected, "", &NoStore);
    assert!(
        matches!(
            result,
            Err(AppError::Localized {
                key: "sync.conflict",
                ..
            })
        ),
        "wrong/stale source token must reject through original publication conflict"
    );
    assert_eq!(tree(fixture._home.path()), before);
    assert!(fixture.db.secrets.ensure_available().is_ok());
}
async fn wrong_and_stale_source_ids_are_read_only() {
    let source_context = plain();
    let fixture = Fixture::new_with_source(plain(), source_context.clone());
    assert_plain_source(&fixture.snapshot);
    let remote = TestRemote::new(false).await;
    let etag = seed_snapshot(&remote, &fixture.snapshot, &PutCondition::Absent).await;
    let _sync = sync_protocol::sync_mutex().lock().await;
    let previous = fetched(&remote, false).await;
    rejected_source_id(&fixture, &previous, "wrong-source-id");

    // Produce a genuinely newer source through the original database/export
    // owners; do not hand-compute a v4 identity/hash or relabel an old snapshot.
    let source = database(
        &fixture._home.path().join("authenticated-source"),
        &source_context,
    );
    source
        .set_setting("common_config_claude", "remote-joint-canary-newer-source")
        .unwrap();
    crate::settings::unlock_settings_for_test(source.secrets.clone()).unwrap();
    let local = sync_protocol::build_local_snapshot(&source).unwrap();
    crate::settings::unlock_settings_for_test(fixture.db.secrets.clone()).unwrap();
    let next = DownloadedSnapshot {
        manifest: serde_json::from_slice(&local.manifest_bytes).unwrap(),
        manifest_hash: local.manifest_hash,
        etag: None,
        db_sql: local.db_sql,
        skills_zip: local.skills_zip,
        layout: RemoteLayout::Current,
        source_path: "synthetic-newer-plaintext-source".into(),
    };
    assert_ne!(next.manifest.snapshot_id, previous.manifest.snapshot_id);
    seed_snapshot(&remote, &next, &PutCondition::Match(etag)).await;
    let current = fetched(&remote, false).await;
    assert_eq!(current.manifest.snapshot_id, next.manifest.snapshot_id);
    rejected_source_id(&fixture, &current, &previous.manifest.snapshot_id);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_plain_source_normal_apply_keeps_plain_target() {
    crate::rt::block_on(fetch_and_normal_apply(false, false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_plain_source_normal_apply_keeps_encrypted_target() {
    crate::rt::block_on(fetch_and_normal_apply(false, true));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_plain_source_normal_apply_keeps_plain_target() {
    crate::rt::block_on(fetch_and_normal_apply(true, false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_plain_source_normal_apply_keeps_encrypted_target() {
    crate::rt::block_on(fetch_and_normal_apply(true, true));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_explicit_restore_uses_empty_input_without_replacing_target_key() {
    crate::rt::block_on(explicit_empty_input());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_wrong_and_stale_expected_source_ids_preserve_all_local_bytes() {
    crate::rt::block_on(wrong_and_stale_source_ids_are_read_only());
}
async fn upload_with_settings(
    use_s3: bool,
    db: &Database,
    s3: &mut S3SyncSettings,
    webdav: &mut WebDavSyncSettings,
) -> Result<Value, AppError> {
    if use_s3 {
        sync_protocol::run_with_sync_lock(s3_sync::upload(db, s3)).await
    } else {
        sync_protocol::run_with_sync_lock(webdav_sync::upload(db, webdav)).await
    }
}
async fn stale_plain_upload_does_not_publish_head(use_s3: bool) {
    let fixture = Fixture::new(plain());
    let remote = TestRemote::new(use_s3).await;
    let mut s3 = s3_settings(&remote);
    let mut webdav = webdav_settings(&remote);
    upload_with_settings(use_s3, &fixture.db, &mut s3, &mut webdav)
        .await
        .unwrap();
    let mut stale_s3 = s3.clone();
    let mut stale_webdav = webdav.clone();
    let stale_status = if use_s3 {
        serde_json::to_value(&stale_s3.status).unwrap()
    } else {
        serde_json::to_value(&stale_webdav.status).unwrap()
    };
    fixture
        .db
        .set_setting("common_config_claude", "newer-plain-winner-canary")
        .unwrap();
    upload_with_settings(use_s3, &fixture.db, &mut s3, &mut webdav)
        .await
        .unwrap();
    let head_path = object_path(use_s3, &manifest_path());
    let (head_before, revision_before, request_offset) = {
        let store = remote.state.lock().unwrap();
        let head = store.objects.get(&head_path).unwrap();
        (head.body.clone(), head.revision, store.requests.len())
    };
    let before = tree(fixture._home.path());
    let error = upload_with_settings(use_s3, &fixture.db, &mut stale_s3, &mut stale_webdav)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AppError::Localized {
            key: "sync.conflict",
            ..
        }
    ));
    let status_after = if use_s3 {
        serde_json::to_value(&stale_s3.status).unwrap()
    } else {
        serde_json::to_value(&stale_webdav.status).unwrap()
    };
    assert_eq!(status_after, stale_status);
    assert_eq!(tree(fixture._home.path()), before);
    let store = remote.state.lock().unwrap();
    let head = store.objects.get(&head_path).unwrap();
    assert_eq!(head.body, head_before);
    assert_eq!(head.revision, revision_before);
    assert!(
        !store.requests[request_offset..]
            .iter()
            .any(|(method, path)| method == "PUT" && path == &head_path),
        "stale receipt must not attempt remote head publication"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_plain_stale_receipt_does_not_publish_remote_head() {
    crate::rt::block_on(stale_plain_upload_does_not_publish_head(false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_plain_stale_receipt_does_not_publish_remote_head() {
    crate::rt::block_on(stale_plain_upload_does_not_publish_head(true));
}
async fn plain_upload_rejects_ignored_conditions_before_head(use_s3: bool) {
    let fixture = Fixture::new(plain());
    let remote = TestRemote::new(use_s3).await;
    remote.state.lock().unwrap().ignore_conditions = true;
    let mut s3 = s3_settings(&remote);
    let mut webdav = webdav_settings(&remote);
    let status_before = if use_s3 {
        serde_json::to_value(&s3.status).unwrap()
    } else {
        serde_json::to_value(&webdav.status).unwrap()
    };
    let before = tree(fixture._home.path());
    let error = upload_with_settings(use_s3, &fixture.db, &mut s3, &mut webdav)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            AppError::Localized {
                key: "sync.conditions_unsupported",
                ..
            }
        ),
        "actual plaintext upload must reach and reject the unsupported conditional backend"
    );
    let status_after = if use_s3 {
        serde_json::to_value(&s3.status).unwrap()
    } else {
        serde_json::to_value(&webdav.status).unwrap()
    };
    assert_eq!(status_after, status_before);
    assert_eq!(tree(fixture._home.path()), before);
    let store = remote.state.lock().unwrap();
    let head = object_path(use_s3, &manifest_path());
    assert!(
        !store.objects.contains_key(&head),
        "unsupported backend must have no published head"
    );
    assert!(!store
        .requests
        .iter()
        .any(|(method, path)| method == "PUT" && path == &head));
    assert!(store.requests.iter().any(|(method,path)| method == "PUT" && path.ends_with("/db.sql")),
        "negative case must reach actual immutable-artifact/condition probing, not fail during source setup");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn webdav_plain_upload_refuses_ignored_condition_server() {
    crate::rt::block_on(plain_upload_rejects_ignored_conditions_before_head(false));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn s3_plain_upload_refuses_ignored_condition_server() {
    crate::rt::block_on(plain_upload_rejects_ignored_conditions_before_head(true));
}
#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "webdav_upload" => webdav_uploads_explicit_plaintext_in_existing_transport_namespace(),
        "s3_upload" => s3_uploads_explicit_plaintext_in_existing_transport_namespace(),
        "webdav_privacy" => webdav_preview_rejects_bad_manifests_without_echoing_their_body(),
        "s3_privacy" => s3_preview_rejects_bad_manifests_without_echoing_their_body(),
        "webdav_apply_plain" => webdav_plain_source_normal_apply_keeps_plain_target(),
        "webdav_apply_text" => webdav_plain_source_normal_apply_keeps_encrypted_target(),
        "s3_apply_plain" => s3_plain_source_normal_apply_keeps_plain_target(),
        "s3_apply_text" => s3_plain_source_normal_apply_keeps_encrypted_target(),
        "explicit_empty" => {
            plaintext_explicit_restore_uses_empty_input_without_replacing_target_key()
        }
        "wrong_stale_source" => {
            plaintext_wrong_and_stale_expected_source_ids_preserve_all_local_bytes()
        }
        "webdav_stale_receipt" => webdav_plain_stale_receipt_does_not_publish_remote_head(),
        "s3_stale_receipt" => s3_plain_stale_receipt_does_not_publish_remote_head(),
        "webdav_ignored_conditions" => webdav_plain_upload_refuses_ignored_condition_server(),
        "s3_ignored_conditions" => s3_plain_upload_refuses_ignored_condition_server(),
        _ => panic!("unknown remote plaintext transport case"),
    }
}

#[path = "archive_stage_privacy_tests.rs"]
mod archive_privacy;

#[cfg(feature = "test-hooks")]
pub(crate) fn run_archive_case(case: &str) {
    archive_privacy::run_case(case);
}

#[cfg(feature = "gui")]
#[path = "remote_plaintext_auto_tests.rs"]
mod auto_attempts;

#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub(crate) fn run_auto_attempt_case(case: &str) {
    auto_attempts::run_case(case);
}
