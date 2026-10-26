//! Original registered recovery and content transaction, with synthetic plaintext sources.
use super::*;
use serde_json::{json, Value};

fn install_plain(fixture: &Fixture, point: Checkpoint) {
    let result = prepared(&fixture.db, &fixture.snapshot, &mut |actual| {
        if actual == point {
            Err(AppError::Config("synthetic plain interruption".into()))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert!(fixture.db.secrets.read().is_err());
    assert!(pending_record(fixture.db.secrets.root()).unwrap().is_some());
}
fn prepared(
    db: &Database,
    snapshot: &DownloadedSnapshot,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let identity = snapshot.manifest.storage_identity().unwrap();
    let crate::secrets::storage::StorageIdentity::Plaintext(id) = &identity else {
        panic!("plain fixture required")
    };
    let source = StorageContext::Plaintext(id.clone());
    let incoming = {
        let current = db.secrets.read().unwrap();
        db.prepare_sync_storage_snapshot_for_storage(
            std::str::from_utf8(&snapshot.db_sql).unwrap(),
            &identity,
            &source,
            &current,
            &current,
        )
        .unwrap()
    };
    let skills =
        crate::services::webdav_sync::archive::stage_skills_zip(&snapshot.skills_zip).unwrap();
    let _guard = crate::services::skill::skill_state_write_guard();
    content::install_storage_with_hook(
        db,
        incoming,
        &source,
        SkillsReplacement {
            source: skills.source.clone(),
            location: crate::services::skill::SkillStorageLocation::LoongPort,
        },
        hook,
    )
}
fn assert_reopened(
    root: &Path,
    current: &StorageContext,
    metadata: &std::collections::BTreeMap<PathBuf, Vec<u8>>,
) {
    let conn = crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), current)
        .unwrap();
    let db = Database::from_connection(
        conn,
        SecretSession::from_storage_context(root.to_path_buf(), current.clone()),
    );
    assert_eq!(
        db.get_setting("common_config_claude").unwrap().as_deref(),
        Some("remote-joint-canary")
    );
    assert_eq!(
        std::fs::read(root.join("skills/remote-skill/SKILL.md")).unwrap(),
        b"remote skill body"
    );
    assert!(!root.join("skills/old-skill").exists());
    assert!(!root.join(INTENT).exists());
    for (path, bytes) in metadata {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(db.secrets.read().unwrap().identity(), current.identity());
}
fn recover_points(point: Checkpoint) {
    for current in [plain(), text(TARGET_TEXT)] {
        let fixture = Fixture::new_with_source(current, plain());
        install_plain(&fixture, point);
        let app = recovery_app(fixture.db.secrets.root());
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let view = invoke(&window, "get_startup_recovery", json!({})).unwrap();
        assert_eq!(view["status"], "pending");
        assert_eq!(view["canRecover"], true);
        assert_eq!(
            view["inputKind"],
            if fixture.current.is_plaintext() {
                "none"
            } else {
                "current"
            }
        );
        assert_eq!(view["inputRequired"], !fixture.current.is_plaintext());
        let before = tree(fixture._home.path());
        let error = invoke(
            &window,
            "recover_startup_operation",
            json!({"token":view["token"],"password":"not the operation key"}),
        )
        .unwrap_err();
        if fixture.current.is_plaintext() {
            assert_eq!(error["code"], "secret.input_not_required");
        }
        assert_eq!(tree(fixture._home.path()), before);
        let input = if fixture.current.is_plaintext() {
            ""
        } else {
            TARGET_TEXT
        };
        let done = invoke(
            &window,
            "recover_startup_operation",
            json!({"token":view["token"],"password":input}),
        )
        .unwrap();
        assert_eq!(done["status"], "completed");
        assert_eq!(done["restartRequired"], true);
        assert_eq!(done["inputKind"], "none");
        assert_eq!(done["inputRequired"], false);
        assert!(fixture.db.secrets.read().is_err());
        assert_reopened(
            fixture.db.secrets.root(),
            &fixture.current,
            &fixture.metadata,
        );
        let after = tree(fixture._home.path());
        assert!(invoke(
            &window,
            "recover_startup_operation",
            json!({"token":view["token"],"password":input})
        )
        .is_err());
        assert_eq!(tree(fixture._home.path()), after);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_recovery_after_intent() {
    recover_points(Checkpoint::Intent);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_recovery_after_database() {
    recover_points(Checkpoint::Database);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_recovery_after_skills() {
    recover_points(Checkpoint::Metadata);
}

fn attempt_unchanged(fixture: &Fixture, input: &str) -> Value {
    let app = recovery_app(fixture.db.secrets.root());
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let view = invoke(&window, "get_startup_recovery", json!({})).unwrap();
    let before = tree(fixture._home.path());
    assert!(invoke(
        &window,
        "recover_startup_operation",
        json!({"token":view["token"],"password":input})
    )
    .is_err());
    assert_eq!(tree(fixture._home.path()), before);
    view
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_journal_rejects_key_smuggling_and_mixed_descriptor() {
    for change in ["key", "mixed", "future", "identity"] {
        let fixture = Fixture::new_with_source(plain(), plain());
        install_plain(&fixture, Checkpoint::Intent);
        let path = fixture.db.secrets.root().join(INTENT);
        let mut intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(intent["version"], 4);
        assert_eq!(intent["storage"]["mode"], "plaintext");
        assert!(intent.get("authentication").is_none());
        match change {
            "key" => {
                let mut manifest: Value =
                    serde_json::from_str(intent["manifest"].as_str().unwrap()).unwrap();
                assert!(manifest["targetKey"].is_null());
                manifest["targetKey"] = serde_json::to_value(vec![0u8; 32]).unwrap();
                intent["manifest"] = serde_json::to_string(&manifest).unwrap().into();
            }
            "mixed" => {
                intent["authentication"] =
                    serde_json::to_value(text(TARGET_TEXT).encrypted().unwrap().metadata())
                        .unwrap();
            }
            "future" => {
                intent["version"] = 999.into();
            }
            "identity" => {
                intent["storage"]["identity"]["storeId"] = uuid::Uuid::new_v4().to_string().into();
            }
            _ => unreachable!(),
        }
        std::fs::write(&path, serde_json::to_vec(&intent).unwrap()).unwrap();
        let view = attempt_unchanged(&fixture, "");
        assert_eq!(view["canRecover"], false);
        assert_eq!(view["inputKind"], "none");
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_descriptor_cannot_downgrade_an_encrypted_target() {
    let fixture = Fixture::new_with_source(text(TARGET_TEXT), plain());
    install_plain(&fixture, Checkpoint::Intent);
    let root = fixture.db.secrets.root();
    let path = root.join(INTENT);
    let mut intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(intent["storage"]["mode"], "current");
    // Even supply the attacker with the full synthetic manifest: removing AEAD
    // and relabelling its descriptor cannot change the real encrypted receiver.
    let raw = fixture
        .current
        .encrypted()
        .unwrap()
        .open(
            &[
                "local",
                "content-transition-current/v4",
                intent["id"].as_str().unwrap(),
            ],
            intent["manifest"].as_str().unwrap(),
        )
        .unwrap();
    let plain_identity = match plain() {
        StorageContext::Plaintext(id) => id,
        _ => unreachable!(),
    };
    intent["storage"] = json!({"mode":"plaintext", "identity":plain_identity});
    intent["manifest"] = std::str::from_utf8(&raw).unwrap().into();
    std::fs::write(path, serde_json::to_vec(&intent).unwrap()).unwrap();
    let view = attempt_unchanged(&fixture, "");
    assert_eq!(view["canRecover"], false);
    assert!(root.join("vault.json").exists());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_recovery_retains_original_source_cas_and_stage_checks() {
    for change in ["record", "database", "skills", "stage"] {
        let fixture = Fixture::new_with_source(plain(), plain());
        install_plain(&fixture, Checkpoint::Intent);
        let root = fixture.db.secrets.root();
        match change {
            "record" => {
                let path = root.join(crate::secrets::protection::FILE);
                let mut record: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                record["choiceComplete"] = false.into();
                std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
            "database" => {
                let external = Database::from_connection(
                    Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap(),
                    SecretSession::from_storage_context(
                        root.to_path_buf(),
                        fixture.current.clone(),
                    ),
                );
                external
                    .set_setting("late-plain-write", "preserve-new-bytes")
                    .unwrap();
            }
            "skills" => {
                std::fs::write(root.join("skills/old-skill/SKILL.md"), b"new user change").unwrap()
            }
            "stage" => {
                let intent: Value =
                    serde_json::from_slice(&std::fs::read(root.join(INTENT)).unwrap()).unwrap();
                std::fs::write(
                    stage_file(root, intent["id"].as_str().unwrap(), 0).unwrap(),
                    b"tampered plaintext stage",
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        attempt_unchanged(&fixture, "");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn current_descriptor_preview_requires_actual_local_owner() {
    for change in ["foreign", "missing_record"] {
        let fixture = Fixture::new_with_source(text(TARGET_TEXT), plain());
        install_plain(&fixture, Checkpoint::Intent);
        let root = fixture.db.secrets.root();
        if change == "foreign" {
            let path = root.join(INTENT);
            let mut intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            intent["storage"]["metadata"] =
                serde_json::to_value(text("unrelated owner").encrypted().unwrap().metadata())
                    .unwrap();
            std::fs::write(path, serde_json::to_vec(&intent).unwrap()).unwrap();
        } else {
            std::fs::remove_file(root.join(crate::secrets::protection::FILE)).unwrap();
        }
        let view = attempt_unchanged(&fixture, TARGET_TEXT);
        assert_eq!(view["canRecover"], false);
        assert_eq!(view["inputKind"], "none");
        assert_eq!(view["inputRequired"], false);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_cache_only_current_key_recovers_without_a_source_input() {
    let legacy = VaultContext::generate().unwrap();
    let mut fixture = Fixture::new_with_source(legacy.clone().into(), plain());
    let root = fixture.db.secrets.root();
    let record = root.join(crate::secrets::protection::FILE);
    std::fs::remove_file(&record).unwrap();
    fixture.metadata.remove(&record);
    write_metadata(root, &completed_metadata(&legacy, true).unwrap()).unwrap();
    fixture.metadata.insert(
        root.join("vault.json"),
        std::fs::read(root.join("vault.json")).unwrap(),
    );
    install_plain(&fixture, Checkpoint::Intent);
    let store = Arc::new(crate::secrets::testing::MemoryKeyStore::default());
    let app = recovery_app_with_store(root, store.clone());
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let view = invoke(&window, "get_startup_recovery", json!({})).unwrap();
    assert_eq!(view["canRecover"], true);
    assert_eq!(view["inputKind"], "none");
    assert_eq!(view["inputRequired"], false);
    let before = tree(fixture._home.path());
    let error = invoke(
        &window,
        "recover_startup_operation",
        json!({"token":view["token"], "password":""}),
    )
    .unwrap_err();
    assert_eq!(error["code"], "secret.key_missing");
    assert_eq!(tree(fixture._home.path()), before);
    store
        .save(
            &legacy.metadata().vault_id,
            &legacy.metadata().key_id,
            &legacy.export_key(),
        )
        .unwrap();
    let done = invoke(
        &window,
        "recover_startup_operation",
        json!({"token":view["token"], "password":""}),
    )
    .unwrap();
    assert_eq!(done["status"], "completed");
    assert_reopened(root, &fixture.current, &fixture.metadata);
    assert!(!record.exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn real_process_exit_resumes_plain_source_through_original_recovery() {
    if std::env::var_os("LOONGPORT_PLAIN_JOINT_CHILD").is_some() {
        let root = crate::config::get_app_config_dir();
        let record = crate::secrets::protection::read(&root).unwrap().unwrap();
        let local = crate::secrets::session::read_metadata_with_bytes(&root).unwrap();
        let key = local.as_ref().map(|(saved, _)| {
            VaultContext::from_user_text(saved.metadata.clone(), TARGET_TEXT)
                .unwrap()
                .export_key()
        });
        let current = crate::secrets::protection::open_context(
            &record,
            local.as_ref().map(|(saved, _)| &saved.metadata),
            key,
            |_, _| panic!("synthetic child must never access system credentials"),
        )
        .unwrap();
        let conn =
            crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &current)
                .unwrap();
        let db = Database::from_connection(
            conn,
            SecretSession::from_storage_context(root.clone(), current),
        );
        crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
        let input = crate::config::get_home_dir().join("plain-joint-input");
        let snapshot = DownloadedSnapshot {
            manifest: serde_json::from_slice(&std::fs::read(input.join("manifest.json")).unwrap())
                .unwrap(),
            manifest_hash: String::new(),
            etag: None,
            db_sql: std::fs::read(input.join("db.sql")).unwrap(),
            skills_zip: std::fs::read(input.join("skills.zip")).unwrap(),
            layout: RemoteLayout::Current,
            source_path: "synthetic plain source".into(),
        };
        let result = prepared(&db, &snapshot, &mut |point| {
            if point == Checkpoint::Database {
                std::process::exit(79);
            }
            Ok(())
        });
        panic!("synthetic child did not exit at the original database boundary: {result:?}");
    }
    for current in [plain(), text(TARGET_TEXT)] {
        let Fixture {
            _home,
            current,
            db,
            snapshot,
            metadata,
        } = Fixture::new_with_source(current, plain());
        let root = db.secrets.root().to_path_buf();
        let input = _home.path().join("plain-joint-input");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(
            input.join("manifest.json"),
            serde_json::to_vec(&snapshot.manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(input.join("db.sql"), &snapshot.db_sql).unwrap();
        std::fs::write(input.join("skills.zip"), &snapshot.skills_zip).unwrap();
        drop(db);
        let exact = if cfg!(test) {
            "secrets::transition::content_tests::remote_plaintext_recovery::real_process_exit_resumes_plain_source_through_original_recovery"
        } else {
            "remote_plain_recovery_real_exit"
        };
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", exact, "--nocapture"])
            .env("LOONGPORT_PLAIN_JOINT_CHILD", "1")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(79),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result = recover_registered(
            &root,
            if current.is_plaintext() {
                ""
            } else {
                TARGET_TEXT
            },
        )
        .unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(result["restartRequired"], true);
        assert_reopened(&root, &current, &metadata);
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run(case: &str) {
    match case {
        "current_preview" => current_descriptor_preview_requires_actual_local_owner(),
        "legacy_cache" => legacy_cache_only_current_key_recovers_without_a_source_input(),
        "real_exit" => real_process_exit_resumes_plain_source_through_original_recovery(),
        "intent" => plaintext_recovery_after_intent(),
        "database" => plaintext_recovery_after_database(),
        "skills" => plaintext_recovery_after_skills(),
        "descriptor" => plaintext_journal_rejects_key_smuggling_and_mixed_descriptor(),
        "downgrade" => plaintext_descriptor_cannot_downgrade_an_encrypted_target(),
        "drift" => plaintext_recovery_retains_original_source_cas_and_stage_checks(),
        _ => panic!("unknown plaintext recovery case"),
    }
}
