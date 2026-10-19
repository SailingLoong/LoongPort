// Included only by backup.rs::tests; exercise actual entrypoints, not just helpers.
use crate::services::provider::qoder_http_auth as http_auth;
const HTTP_OWNER_ID: &str = "http-owner-backup-fixture";
const HTTP_OWNER_NOW: i64 = 1_000_000;
fn http_owner_fixture(
    db: &Database,
) -> crate::services::provider::qoder_daily::http::PatDeclaration {
    let mut provider = crate::provider::Provider::with_id(
        HTTP_OWNER_ID.into(),
        "Synthetic HTTP owner".into(),
        serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-backup-pat"}}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_sdk".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let generation = db
        .get_provider_by_id(HTTP_OWNER_ID, "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
        .generation;
    crate::services::provider::qoder_daily::http::PatDeclaration {
        provider_id: HTTP_OWNER_ID.into(),
        credential_generation: generation,
        source:
            crate::services::provider::qoder_daily::http::CredentialSource::InternationalQoderPat,
    }
}
fn http_owner_source(db: &Database) -> http_auth::SourceBinding {
    let selected = db
        .get_provider_by_id(HTTP_OWNER_ID, "codex")
        .unwrap()
        .unwrap();
    http_auth::SourceBinding::from_provider(&selected)
        .unwrap()
        .0
}
fn http_owner_pair(tag: &str) -> http_auth::TokenPair {
    http_auth::TokenPair {
        access_token: format!("synthetic-access-{tag}"),
        refresh_token: format!("synthetic-refresh-{tag}"),
        expires_at_ms: HTTP_OWNER_NOW + 1,
    }
}
fn http_owner_ready(
    db: &Database,
    declaration: &crate::services::provider::qoder_daily::http::PatDeclaration,
) {
    let lease = db
        .qoder_http_begin_establish(declaration, http_auth::EstablishIntent::SameIdentity)
        .unwrap();
    db.qoder_http_publish_establish(
        &lease,
        http_auth::Established {
            uid: "synthetic-backup-uid".into(),
            tokens: http_owner_pair("backup"),
        },
        HTTP_OWNER_NOW,
    )
    .unwrap();
}
fn http_owner_raw(conn: &Connection) -> String {
    conn.query_row(
        "SELECT qoder_http_secret FROM providers WHERE id=?1 AND app_type='codex'",
        [HTTP_OWNER_ID],
        |r| r.get(0),
    )
    .unwrap()
}
#[test]
#[serial]
fn http_owner_real_file_restore_retires_every_state_even_ready_spent_after_backup(
) -> Result<(), AppError> {
    for state in ["ready", "pending", "unknown", "establishing"] {
        let _home = TestHomeGuard::new();
        let _settings_scope = crate::settings::TestSettingsScope::enter()?;
        let db = crate::secrets::testing::initialize_database()?;
        let _settings = SettingsGuard::with_backup_retain_count(3);
        let declaration = http_owner_fixture(&db);
        if state == "establishing" {
            db.qoder_http_begin_establish(&declaration, http_auth::EstablishIntent::SameIdentity)?;
        } else {
            http_owner_ready(&db, &declaration);
            if state != "ready" {
                let http_auth::Acquisition::Refresh(lease) =
                    db.qoder_http_acquire(&http_owner_source(&db), HTTP_OWNER_NOW)?
                else {
                    panic!();
                };
                if state == "unknown" {
                    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)?;
                }
            }
        }
        let source = db.backup_database_file()?.expect("file-backed snapshot");
        let source_name = source.file_name().unwrap().to_string_lossy().into_owned();
        let archived =
            Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        assert!(!http_owner_raw(&archived).is_empty());
        drop(archived);
        if state == "ready" {
            let http_auth::Acquisition::Refresh(lease) =
                db.qoder_http_acquire(&http_owner_source(&db), HTTP_OWNER_NOW)?
            else {
                panic!();
            };
            db.qoder_http_publish_refresh(
                &lease,
                http_owner_pair("spent-after-backup"),
                HTTP_OWNER_NOW,
            )?;
        }
        db.restore_from_backup(&source_name)?;
        assert!(
            http_owner_raw(&*db.conn.lock()?).is_empty(),
            "{state} was revived"
        );
        assert!(db
            .qoder_http_acquire(&http_owner_source(&db), HTTP_OWNER_NOW)
            .is_err());
        assert_eq!(
            db.get_provider_by_id(HTTP_OWNER_ID, "codex")?
                .unwrap()
                .settings_config["env"]["OPENAI_API_KEY"],
            "synthetic-backup-pat"
        );
    }
    Ok(())
}
#[test]
#[serial]
fn http_owner_encrypted_sync_join_and_password_backup_restore_retire_derived_credentials(
) -> Result<(), AppError> {
    let home = TestHomeGuard::new();
    let current = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-current-password")
        .unwrap();
    let next = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-next-password")
        .unwrap();
    let local = sync_test_database(&current, &home.path().join("current"));
    let incoming = sync_test_database(&next, &home.path().join("incoming"));
    let local_declaration = http_owner_fixture(&local);
    http_owner_ready(&local, &local_declaration);
    let incoming_declaration = http_owner_fixture(&incoming);
    http_owner_ready(&incoming, &incoming_declaration);
    let (sql, metadata) = incoming.export_sync_snapshot()?;
    for password_restore in [false, true] {
        let staged = if password_restore {
            local.prepare_backup_content(&sql, Some("synthetic-next-password"), &current)?
        } else {
            local.validate_sync_snapshot_for_current(&sql, &metadata, &next)?
        };
        let target = if password_restore { &current } else { &next };
        let joined = Database::prepare_sync_join(staged, &*local.conn.lock()?, &current, target)?;
        assert!(http_owner_raw(&joined).is_empty());
        crate::secrets::inventory::validate_database(&joined, target)?;
    }
    assert!(
        !http_owner_raw(&*local.conn.lock()?).is_empty(),
        "private staging changed live state"
    );
    Ok(())
}
#[test]
fn http_owner_cc_switch_import_callback_cannot_publish_a_forged_private_state(
) -> Result<(), AppError> {
    let db = Database::memory()?;
    let declaration = http_owner_fixture(&db);
    http_owner_ready(&db, &declaration);
    let sql = db.export_sql_string()?;
    // Even valid authenticated caller-created derived state in private staging
    // is retired by the final original import owner before publication.
    db.import_sql_string_from_cc_switch(&sql, &[], |staged| {
        assert!(!http_owner_raw(&*staged.conn.lock()?).is_empty());
        Ok(())
    })?;
    assert!(http_owner_raw(&*db.conn.lock()?).is_empty());
    Ok(())
}
