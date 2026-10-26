//! Pool proofs must retire through the original same-storage journal and recovery.
use super::*;
use crate::services::native_source_guard::storage_fixture::seed_restore_proofs;

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
        assert_eq!(identity, current.identity());
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

pub(super) async fn verify(case: &str) {
    let point = match case {
        "intent" => Checkpoint::Intent,
        "database" => Checkpoint::Database,
        _ => panic!("unknown pool restore consumer case: {case}"),
    };
    let fixture = Fixture::new_with_source(plain(), plain());
    let proofs = seed_restore_proofs(fixture.db.clone()).await;
    proofs.assert_same_credentials(&fixture.db);
    let root = fixture.db.secrets.root().to_path_buf();
    let skill = root.join("skills/pool-restore-canary/SKILL.md");
    {
        let _skills = crate::services::skill::skill_state_write_guard();
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, b"snapshot skill body").unwrap();
    }
    fixture
        .db
        .set_setting("common_config_claude", "pool-restore-snapshot")
        .unwrap();
    // Capture the real seeded owner, not a separately issued copy of its credentials.
    let local = sync_protocol::build_local_snapshot(&fixture.db).unwrap();
    let snapshot = DownloadedSnapshot {
        manifest: serde_json::from_slice(&local.manifest_bytes).unwrap(),
        manifest_hash: local.manifest_hash,
        etag: None,
        db_sql: local.db_sql,
        skills_zip: local.skills_zip,
        layout: RemoteLayout::Current,
        source_path: "synthetic-same-owner-source".into(),
    };
    assert_eq!(
        snapshot.manifest.storage_identity().unwrap(),
        fixture.current.identity()
    );
    fixture
        .db
        .set_setting("common_config_claude", "pool-restore-current")
        .unwrap();
    {
        let _skills = crate::services::skill::skill_state_write_guard();
        std::fs::write(&skill, b"current skill body").unwrap();
    }
    let epoch = fixture.db.secrets.lifecycle_epoch();
    let mut interrupted = false;
    let error = prepared(&fixture.db, &snapshot, &mut |actual| {
        if actual == point {
            interrupted = true;
            Err(AppError::Config(
                "synthetic pool restore interruption".into(),
            ))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(interrupted, "must reach the original journal checkpoint");
    assert!(error
        .to_string()
        .contains("synthetic pool restore interruption"));
    assert!(fixture.db.secrets.lifecycle_epoch() > epoch);
    assert!(fixture.db.secrets.read().is_err());
    assert!(pending_record(&root).unwrap().is_some());
    let interrupted_marker: String = {
        // Observe the published image without reopening the retired runtime owner.
        let observed = rusqlite::Connection::open_with_flags(
            root.join(crate::config::DB_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        observed
            .query_row(
                "SELECT value FROM settings WHERE key='common_config_claude'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(
        interrupted_marker,
        if point == Checkpoint::Intent {
            "pool-restore-current"
        } else {
            "pool-restore-snapshot"
        }
    );
    assert_eq!(std::fs::read(&skill).unwrap(), b"current skill body");
    proofs.assert_retired().await;

    let recovery_root = root.clone();
    tokio::task::spawn_blocking(move || {
        let app = recovery_app(&recovery_root);
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let status = invoke(&window, "get_startup_recovery", serde_json::json!({})).unwrap();
        assert_eq!(status["status"], "pending");
        assert_eq!(status["canRecover"], true);
        assert_eq!(status["inputKind"], "none");
        assert_eq!(status["inputRequired"], false);
        let done = invoke(
            &window,
            "recover_startup_operation",
            serde_json::json!({"token":status["token"], "password":""}),
        )
        .unwrap();
        assert_eq!(done["status"], "completed");
        assert_eq!(done["restartRequired"], true);
    })
    .await
    .unwrap();

    // Recovery settles durable storage; it must not reactivate the original session.
    assert!(fixture.db.secrets.read().is_err());
    proofs.assert_retired().await;
    let connection = crate::database::storage::prepare(
        &root.join(crate::config::DB_FILE_NAME),
        &fixture.current,
    )
    .unwrap();
    let fresh = Arc::new(Database::from_connection(
        connection,
        SecretSession::from_storage_context(root.clone(), fixture.current.clone()),
    ));
    assert!(!Arc::ptr_eq(&fresh.secrets, &fixture.db.secrets));
    assert_eq!(
        fresh.secrets.read().unwrap().identity(),
        fixture.current.identity()
    );
    assert_eq!(
        fresh
            .get_setting("common_config_claude")
            .unwrap()
            .as_deref(),
        Some("pool-restore-snapshot")
    );
    assert_eq!(std::fs::read(&skill).unwrap(), b"snapshot skill body");
    for (path, bytes) in &fixture.metadata {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    assert!(!root.join(INTENT).exists());
    proofs.assert_same_credentials(&fresh);
    proofs.assert_fresh(fresh).await;
    assert!(fixture.db.secrets.read().is_err());
}
