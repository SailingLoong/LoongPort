//! Actual SQL source preparation / original local installation; synthetic only.
use super::*;
use crate::database::{storage, storage_generation_tests as fixture, storage_identity};
use crate::secrets::{
    inventory, session::SecretSession, storage::PlaintextIdentity, testing::TestHome,
    transition::fence_test_support::tree,
};
const TEXT: &str = "  synthetic source 用户 key\n  ";
const LOCAL_SECRET: &str = "synthetic-local-preserved-config";

fn plain() -> StorageContext {
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    })
}
fn text() -> StorageContext {
    StorageContext::from(
        VaultContext::create_generation_with_user_text(uuid::Uuid::new_v4().to_string(), 1, TEXT)
            .unwrap(),
    )
}
fn db(home: &TestHome, context: &StorageContext, version: i32) -> Database {
    let connection = fixture::source(context);
    if version == 20 {
        Database::apply_upstream4_migrations_on_conn(&connection).unwrap();
    }
    let root = home.path().join(uuid::Uuid::new_v4().to_string());
    crate::config::ensure_private_directory(&root).unwrap();
    let mut persisted = Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
    crate::database::vault::copy(&connection, &mut persisted).unwrap();
    Database::from_connection(
        persisted,
        SecretSession::from_storage_context(root, context.clone()),
    )
}
fn sql(db: &Database) -> String {
    Database::dump_sql(&db.snapshot_to_memory().unwrap(), &[]).unwrap()
}
fn code<T>(result: Result<T, AppError>, expected: &str) {
    match result {
        Err(AppError::Config(actual)) => assert_eq!(actual, expected),
        Err(error) => panic!("expected {expected}, got {error}"),
        Ok(_) => panic!("expected {expected}, unexpectedly succeeded"),
    }
}
fn assert_prepared(staged: &ValidatedSyncSnapshot, destination: &StorageContext, version: i32) {
    storage::check_identity(&staged.connection, destination).unwrap();
    inventory::validate_storage_database(&staged.connection, destination).unwrap();
    fixture::assert_values(&staged.connection, destination);
    assert_eq!(
        Database::get_user_version(&staged.connection).unwrap(),
        version
    );
    if destination.is_plaintext() {
        assert!(crate::database::vault::stored_metadata(&staged.connection)
            .unwrap()
            .is_none());
        assert!(storage_identity::stored(&staged.connection)
            .unwrap()
            .is_some());
    } else {
        assert!(storage_identity::stored(&staged.connection)
            .unwrap()
            .is_none());
    }
}
fn prepare_local(source: &StorageContext, current: &StorageContext, input: Option<&str>) {
    let home = TestHome::new().unwrap();
    for version in [17, 20] {
        let incoming = db(&home, source, version);
        let local = db(&home, current, version);
        let before_local = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
        let before_source = Database::content_digest(&incoming.conn.lock().unwrap()).unwrap();
        let before_tree = tree(home.path());
        let staged = local
            .prepare_backup_content_for_storage(&sql(&incoming), input, current, current)
            .unwrap();
        assert_prepared(&staged, current, version);
        fixture::assert_unchanged(&local.conn.lock().unwrap(), &before_local);
        fixture::assert_unchanged(&incoming.conn.lock().unwrap(), &before_source);
        assert_eq!(tree(home.path()), before_tree);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn local_text_source_prepares_for_explicit_plaintext() {
    prepare_local(&text(), &plain(), Some(TEXT));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn local_plaintext_source_prepares_for_user_text() {
    prepare_local(&plain(), &text(), None);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn local_text_source_prepares_for_another_user_text() {
    prepare_local(&text(), &text(), Some(TEXT));
}
fn remote(current: StorageContext) {
    let home = TestHome::new().unwrap();
    let source = text();
    let vault = source.encrypted().unwrap();
    let destination = if current.is_plaintext() {
        fixture::plaintext_successor(&current)
    } else {
        StorageContext::from(
            VaultContext::create_generation_with_user_text(
                current.identity().store_id().to_owned(),
                current.identity().revision() + 1,
                TEXT,
            )
            .unwrap(),
        )
    };
    for version in [17, 20] {
        let incoming = db(&home, &source, version);
        let local = db(&home, &current, version);
        let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
        let staged = local
            .prepare_sync_snapshot_for_storage(
                &sql(&incoming),
                vault.metadata(),
                vault,
                &current,
                &destination,
            )
            .unwrap();
        assert_prepared(&staged, &destination, version);
        fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn remote_authenticated_text_accepts_plaintext_receiver() {
    remote(plain());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn remote_authenticated_text_accepts_user_text_receiver() {
    remote(text());
}

// Valid provider JSON is required by the original restore-specific Qoder owner.
fn runtime_rows(db: &Database, context: &StorageContext, marker: &str) {
    let value = inventory::seal_db(
        context,
        "providers",
        "settings_config",
        &["generation-provider", "codex"],
        &serde_json::json!({"env":{"OPENAI_API_KEY":marker}}).to_string(),
    )
    .unwrap();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE providers SET settings_config=?1,qoder_http_secret=''",
            [value],
        )
        .unwrap();
    db.set_setting("source-marker", marker).unwrap();
}
fn local_facts(db: &Database, context: &StorageContext, version: i32) {
    let conn = db.conn.lock().unwrap();
    conn.execute("INSERT INTO session_log_sync(file_path,last_modified,last_line_offset,last_synced_at) VALUES('synthetic-session',11,22,33)", []).unwrap();
    if version == 20 {
        conn.execute("UPDATE session_log_sync SET last_byte_offset=1234,last_tail_fingerprint='synthetic-tail'", []).unwrap();
    }
    conn.execute("INSERT INTO session_usage_dedup(data_source,request_id,semantic_id,has_entry_id) VALUES('synthetic','request-1','semantic-1',1)", []).unwrap();
    conn.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result) VALUES('synthetic-attempt','synthetic-subject','international','synthetic-campaign','synthetic-key',1,2,'synthetic-generation','unknown')", []).unwrap();
    let raw = inventory::seal_db(
        context,
        "proxy_live_backup",
        "original_config",
        &["claude"],
        LOCAL_SECRET,
    )
    .unwrap();
    conn.execute("INSERT INTO proxy_live_backup(app_type,original_config,backed_up_at) VALUES('claude',?1,1)", [raw]).unwrap();
}
fn assert_local_facts(conn: &Connection, context: &StorageContext, version: i32) {
    assert_eq!(
        conn.query_row(
            "SELECT last_line_offset FROM session_log_sync WHERE file_path='synthetic-session'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        22
    );
    if version == 20 {
        assert_eq!(
            conn.query_row(
                "SELECT last_byte_offset FROM session_log_sync WHERE file_path='synthetic-session'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1234
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM session_usage_dedup WHERE request_id='request-1'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT attempt_id FROM qoder_campaign_operations",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "synthetic-attempt"
    );
    let raw: String = conn
        .query_row(
            "SELECT original_config FROM proxy_live_backup WHERE app_type='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        inventory::open_db(
            context,
            "proxy_live_backup",
            "original_config",
            &["claude"],
            &raw
        )
        .unwrap(),
        LOCAL_SECRET
    );
    let blob: Vec<u8> = conn
        .query_row(
            "SELECT value FROM future_unknown_data WHERE id=7",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blob, vec![0, 255, 18]);
    assert_eq!(Database::get_user_version(conn).unwrap(), version);
    storage::check_identity(conn, context).unwrap();
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_join_preserves_original_local_rows_and_unknown_schema() {
    let home = TestHome::new().unwrap();
    let encrypted = text();
    let plaintext = plain();
    for (current, next) in [(&plaintext, &encrypted), (&encrypted, &plaintext)] {
        for version in [17, 20] {
            let local = db(&home, current, version);
            let source = db(&home, &encrypted, version);
            runtime_rows(&local, current, "local-before");
            runtime_rows(&source, &encrypted, "incoming-value");
            local_facts(&local, current, version);
            source.conn.lock().unwrap().execute_batch("CREATE TRIGGER synthetic_provider_audit AFTER UPDATE ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
            let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
            let staged = local
                .prepare_backup_content_for_storage(&sql(&source), Some(TEXT), current, next)
                .unwrap();
            let joined = Database::prepare_sync_join_for_storage(
                staged,
                &local.conn.lock().unwrap(),
                current,
                next,
            )
            .unwrap();
            assert_local_facts(&joined, next, version);
            assert_eq!(
                joined
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='synthetic_provider_audit'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(
                joined
                    .query_row(
                        "SELECT value FROM settings WHERE key='source-marker'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "incoming-value"
            );
            fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn local_source_rejects_wrong_exact_text_without_writes() {
    let home = TestHome::new().unwrap();
    let source = text();
    let current = plain();
    let incoming = db(&home, &source, 17);
    let local = db(&home, &current, 17);
    let before = tree(home.path());
    let digest = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    for wrong in [TEXT.trim(), "wrong synthetic text"] {
        code(
            local.prepare_backup_content_for_storage(
                &sql(&incoming),
                Some(wrong),
                &current,
                &current,
            ),
            "secret.key_rejected",
        );
    }
    code(
        local.prepare_backup_content_for_storage(&sql(&incoming), None, &current, &current),
        "backup.password_required",
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &digest);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn remote_source_requires_manifest_and_authenticated_key_match() {
    let home = TestHome::new().unwrap();
    let source = text();
    let other = text();
    let current = plain();
    let incoming = db(&home, &source, 17);
    let local = db(&home, &current, 17);
    let source_vault = source.encrypted().unwrap();
    let other_vault = other.encrypted().unwrap();
    code(
        local.prepare_sync_snapshot_for_storage(
            &sql(&incoming),
            source_vault.metadata(),
            other_vault,
            &current,
            &current,
        ),
        "sync.vault_adoption_required",
    );
    let mut expected = source_vault.metadata().clone();
    expected.revision += 1;
    code(
        local.prepare_sync_snapshot_for_storage(
            &sql(&incoming),
            &expected,
            source_vault,
            &current,
            &current,
        ),
        "sync.vault_revision_newer",
    );
    code(
        local.prepare_sync_snapshot_for_storage(
            &sql(&incoming),
            other_vault.metadata(),
            other_vault,
            &current,
            &current,
        ),
        "sync.source_identity_mismatch",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_source_rejects_missing_mixed_and_corrupt_identity() {
    let home = TestHome::new().unwrap();
    let source = plain();
    let current = plain();
    let local = db(&home, &current, 17);
    let missing = db(&home, &source, 17);
    missing
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE loongport_plaintext_storage")
        .unwrap();
    code(
        local.prepare_backup_content_for_storage(&sql(&missing), None, &current, &current),
        "secret.metadata_missing",
    );
    let mixed = db(&home, &source, 17);
    let vault = VaultContext::generate().unwrap();
    mixed
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO loongport_vault(id,metadata) VALUES(1,?1)",
            [serde_json::to_string(vault.metadata()).unwrap()],
        )
        .unwrap();
    code(
        local.prepare_backup_content_for_storage(&sql(&mixed), Some(TEXT), &current, &current),
        "protection.encrypted_data_present",
    );
    let corrupt = db(&home, &source, 17);
    corrupt
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE providers SET qoder_http_secret='lpenc:damaged'", [])
        .unwrap();
    code(
        local.prepare_backup_content_for_storage(&sql(&corrupt), None, &current, &current),
        "secret.unsupported_format",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_source_requires_actual_current_identity_and_schema() {
    let home = TestHome::new().unwrap();
    let source = plain();
    let current = plain();
    let wrong = plain();
    let local = db(&home, &current, 17);
    let incoming = db(&home, &source, 17);
    code(
        local.prepare_backup_content_for_storage(&sql(&incoming), None, &wrong, &current),
        "protection.database_identity_mismatch",
    );
    incoming
        .conn
        .lock()
        .unwrap()
        .pragma_update(None, "user_version", 21)
        .unwrap();
    code(
        local.prepare_backup_content_for_storage(&sql(&incoming), None, &current, &current),
        "secret.database_version_too_new",
    );
    let newer = db(&home, &source, 20);
    code(
        local.prepare_backup_content_for_storage(&sql(&newer), None, &current, &current),
        "sync.database_version_incompatible",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_legacy_sql_uses_original_admission_for_both_modes() {
    let home = TestHome::new().unwrap();
    let legacy = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&legacy).unwrap();
    Database::apply_upgrade_source_migrations_on_conn(&legacy).unwrap();
    legacy
        .execute(
            "INSERT INTO settings(key,value) VALUES('global_proxy_url','synthetic-legacy-secret')",
            [],
        )
        .unwrap();
    let sql = Database::dump_sql(&legacy, &[]).unwrap();
    for current in [plain(), text()] {
        let local = db(&home, &current, 17);
        let staged = local
            .prepare_backup_content_for_storage(&sql, None, &current, &current)
            .unwrap();
        storage::check_identity(&staged.connection, &current).unwrap();
        let raw: String = staged
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='global_proxy_url'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            inventory::open_db(&current, "settings", "value", &["global_proxy_url"], &raw).unwrap(),
            "synthetic-legacy-secret"
        );
    }
}
fn durable_runtime(db: &Database, context: &StorageContext) {
    use crate::secrets::{
        protection::{ProtectionRecord, ProtectionStorage},
        session,
    };
    let root = db.secret_session().root();
    crate::config::ensure_private_directory(root).unwrap();
    let record = match context {
        StorageContext::Encrypted(vault) => {
            session::write_metadata(root, &session::completed_metadata(vault, false).unwrap())
                .unwrap();
            ProtectionRecord::encrypted(vault, None).unwrap()
        }
        StorageContext::Plaintext(identity) => ProtectionRecord {
            format_version: 1,
            choice_complete: true,
            storage: ProtectionStorage::Plaintext {
                identity: identity.clone(),
            },
            commitment: None,
            cleanup: None,
        },
    };
    record.matches_context(context).unwrap();
    record.verify_commitment(context).unwrap();
    crate::config::atomic_write_private(
        &root.join("protection.json"),
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}
fn install(current: StorageContext, audit: bool) {
    let home = TestHome::new().unwrap();
    let source = plain();
    let local = db(&home, &current, 20);
    let incoming = db(&home, &source, 20);
    runtime_rows(&local, &current, "local-before");
    runtime_rows(&incoming, &source, "incoming-value");
    local_facts(&local, &current, 20);
    durable_runtime(&local, &current);
    if audit {
        incoming.conn.lock().unwrap().execute_batch("CREATE TRIGGER synthetic_install_audit AFTER UPDATE ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
    }
    let protection_before =
        fs::read(local.secret_session().root().join("protection.json")).unwrap();
    let metadata_before = fs::read(local.secret_session().root().join("vault.json")).ok();
    let identity = local.secret_session().read().unwrap().identity();
    let staged = local
        .prepare_backup_content_for_storage(&sql(&incoming), None, &current, &current)
        .unwrap();
    let id = local.install_backup_content_for_storage(staged).unwrap();
    assert!(!id.is_empty(), "original safety copy ID must be returned");
    assert!(local
        .secret_session()
        .root()
        .join("backups")
        .join(format!("{id}.db"))
        .is_file());
    assert_eq!(local.secret_session().read().unwrap().identity(), identity);
    assert_local_facts(&local.conn.lock().unwrap(), &current, 20);
    assert_eq!(
        fs::read(local.secret_session().root().join("protection.json")).unwrap(),
        protection_before
    );
    assert_eq!(
        fs::read(local.secret_session().root().join("vault.json")).ok(),
        metadata_before
    );
    assert_eq!(
        local.get_setting("source-marker").unwrap().as_deref(),
        Some("incoming-value")
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_installer_restores_plaintext_without_changing_mode() {
    install(plain(), false);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_installer_restores_user_text_without_changing_key() {
    install(text(), false);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn typed_install_rejects_another_destination_before_safety_copy() {
    let home = TestHome::new().unwrap();
    let vault = VaultContext::generate()
        .unwrap()
        .with_password("synthetic original wrapper")
        .unwrap();
    let source = StorageContext::from(vault.clone());
    let other = plain();
    let incoming = db(&home, &source, 17);
    let receiver = db(&home, &other, 17);
    durable_runtime(&receiver, &other);
    let staged =
        Database::validate_sync_snapshot(&sql(&incoming), vault.metadata(), &vault).unwrap();
    let before = tree(home.path());
    let digest = Database::content_digest(&receiver.conn.lock().unwrap()).unwrap();
    code(
        receiver.install_backup_content_for_storage(staged),
        "sync.source_identity_mismatch",
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&receiver.conn.lock().unwrap(), &digest);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn typed_install_rejects_corrupted_staging_before_safety_copy() {
    let home = TestHome::new().unwrap();
    let vault = VaultContext::generate()
        .unwrap()
        .with_password("synthetic original wrapper")
        .unwrap();
    let context = StorageContext::from(vault.clone());
    let receiver = db(&home, &context, 17);
    durable_runtime(&receiver, &context);
    let incoming = db(&home, &context, 17);
    let staged =
        Database::validate_sync_snapshot(&sql(&incoming), vault.metadata(), &vault).unwrap();
    let mut foreign = vault.metadata().clone();
    foreign.revision += 1;
    staged
        .connection
        .execute(
            "UPDATE loongport_vault SET metadata=?1",
            [serde_json::to_string(&foreign).unwrap()],
        )
        .unwrap();
    let before = tree(home.path());
    let digest = Database::content_digest(&receiver.conn.lock().unwrap()).unwrap();
    code(
        receiver.install_backup_content_for_storage(staged),
        "secret.identity_mismatch",
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&receiver.conn.lock().unwrap(), &digest);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_installer_keeps_unknown_rows_despite_source_audit_trigger() {
    install(plain(), true);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn typed_preparation_cannot_revive_after_session_block_unblock() {
    let home = TestHome::new().unwrap();
    let current = plain();
    let local = db(&home, &current, 17);
    let source = db(&home, &plain(), 17);
    durable_runtime(&local, &current);
    runtime_rows(&local, &current, "local-before");
    let source_context = source.secret_session().read().unwrap().clone();
    runtime_rows(&source, &source_context, "incoming-value");
    let staged = local
        .prepare_backup_content_for_storage(&sql(&source), None, &current, &current)
        .unwrap();
    local.secret_session().set_blocked(true);
    local.secret_session().set_blocked(false);
    let before = tree(home.path());
    let digest = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    code(
        local.install_backup_content_for_storage(staged),
        "sync.source_identity_mismatch",
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &digest);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn typed_preparation_is_bound_to_original_receiver_session() {
    let home = TestHome::new().unwrap();
    let current = plain();
    let local = db(&home, &current, 17);
    let another = db(&home, &current, 17);
    let source_context = plain();
    let source = db(&home, &source_context, 17);
    for receiver in [&local, &another] {
        durable_runtime(receiver, &current);
        runtime_rows(receiver, &current, "local-before");
    }
    runtime_rows(&source, &source_context, "incoming-value");
    let staged = local
        .prepare_backup_content_for_storage(&sql(&source), None, &current, &current)
        .unwrap();
    let before = tree(home.path());
    let digest = Database::content_digest(&another.conn.lock().unwrap()).unwrap();
    code(
        another.install_backup_content_for_storage(staged),
        "sync.source_identity_mismatch",
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&another.conn.lock().unwrap(), &digest);
}

// Receiver-bound joint preparation must be checked before the existing journal
// owner mutates any live file. These fixtures hold the actual original guards.
fn receiver_prepared(
    home: &TestHome,
    current: &StorageContext,
) -> (Database, ValidatedSyncSnapshot) {
    let local = db(home, current, 17);
    let source_context = plain();
    let source = db(home, &source_context, 17);
    durable_runtime(&local, current);
    runtime_rows(&local, current, "local-before");
    runtime_rows(&source, &source_context, "incoming-value");
    local_facts(&local, current, 17);
    let prepared = {
        let pinned = local.secret_session().read().unwrap();
        local
            .prepare_backup_content_for_storage(&sql(&source), None, &pinned, &pinned)
            .unwrap()
    };
    (local, prepared)
}
fn reject_receiver_join(
    home: &TestHome,
    local: &Database,
    incoming: ValidatedSyncSnapshot,
    expected: &str,
) {
    use rusqlite::config::DbConfig;
    let current = local.secret_session().write().unwrap();
    let conn = local.conn.lock().unwrap();
    let before = conn.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let before_tree = tree(home.path());
    let flags = conn
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap();
    code(
        local.prepare_sync_join_for_receiver(incoming, &conn, &current),
        expected,
    );
    assert_eq!(
        &*conn.serialize(rusqlite::MAIN_DB).unwrap(),
        before.as_slice()
    );
    assert_eq!(
        conn.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
            .unwrap(),
        flags
    );
    assert_eq!(tree(home.path()), before_tree);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn receiver_join_returns_private_image_under_original_guards() {
    use rusqlite::config::DbConfig;
    let home = TestHome::new().unwrap();
    let source_context = text();
    let source_vault = source_context.encrypted().unwrap();
    for current in [plain(), text()] {
        for version in [17, 20] {
            let local = db(&home, &current, version);
            let source = db(&home, &source_context, version);
            durable_runtime(&local, &current);
            runtime_rows(&local, &current, "local-before");
            runtime_rows(&source, &source_context, "incoming-value");
            local_facts(&local, &current, version);
            source.conn.lock().unwrap().execute_batch("CREATE TRIGGER synthetic_receiver_audit AFTER UPDATE ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
            let staged = {
                let pinned = local.secret_session().read().unwrap();
                local
                    .prepare_sync_snapshot_for_storage(
                        &sql(&source),
                        source_vault.metadata(),
                        source_vault,
                        &pinned,
                        &pinned,
                    )
                    .unwrap()
            };
            let pinned = local.secret_session().write().unwrap();
            let conn = local.conn.lock().unwrap();
            let before = conn.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
            let before_tree = tree(home.path());
            let flags = conn
                .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
                .unwrap();
            let joined = local
                .prepare_sync_join_for_receiver(staged, &conn, &pinned)
                .unwrap();
            assert_local_facts(&joined, &current, version);
            assert_eq!(
                joined
                    .query_row(
                        "SELECT value FROM settings WHERE key='source-marker'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "incoming-value"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT value FROM settings WHERE key='source-marker'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "local-before"
            );
            assert_eq!(
                joined
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='synthetic_receiver_audit'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(
                &*conn.serialize(rusqlite::MAIN_DB).unwrap(),
                before.as_slice()
            );
            assert_eq!(
                conn.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
                    .unwrap(),
                flags
            );
            assert_eq!(tree(home.path()), before_tree);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn receiver_join_rejects_foreign_session_without_writes() {
    let home = TestHome::new().unwrap();
    let current = plain();
    let (_local, staged) = receiver_prepared(&home, &current);
    let another = db(&home, &current, 17);
    durable_runtime(&another, &current);
    runtime_rows(&another, &current, "another-before");
    reject_receiver_join(&home, &another, staged, "sync.source_identity_mismatch");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn receiver_join_rejects_expired_epoch_without_writes() {
    let home = TestHome::new().unwrap();
    let (local, staged) = receiver_prepared(&home, &plain());
    local.secret_session().set_blocked(true);
    local.secret_session().set_blocked(false);
    reject_receiver_join(&home, &local, staged, "sync.source_identity_mismatch");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn receiver_join_rejects_changed_scope_and_unbound_proof_without_writes() {
    let home = TestHome::new().unwrap();
    for change in ["destination", "live-identity", "live-schema", "unbound"] {
        let current = plain();
        let (local, mut staged) = receiver_prepared(&home, &current);
        let expected = match change {
            "destination" => {
                staged.destination = plain().identity();
                "sync.source_identity_mismatch"
            }
            "live-identity" => {
                let StorageContext::Plaintext(other) = plain() else {
                    unreachable!()
                };
                local
                    .conn
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE loongport_plaintext_storage SET identity=?1",
                        [serde_json::to_string(&other).unwrap()],
                    )
                    .unwrap();
                "protection.database_identity_mismatch"
            }
            "live-schema" => {
                local
                    .conn
                    .lock()
                    .unwrap()
                    .execute_batch("PRAGMA user_version=21")
                    .unwrap();
                "sync.database_version_incompatible"
            }
            "unbound" => {
                staged.receiver = None;
                "sync.source_identity_mismatch"
            }
            _ => unreachable!(),
        };
        reject_receiver_join(&home, &local, staged, expected);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn receiver_join_reauthenticates_staged_secret_before_private_writes() {
    let home = TestHome::new().unwrap();
    let current = text();
    let (local, staged) = receiver_prepared(&home, &current);
    staged
        .connection
        .execute("UPDATE providers SET settings_config='lpenc1.!invalid'", [])
        .unwrap();
    reject_receiver_join(&home, &local, staged, "secret.invalid_envelope");
}

#[cfg(feature = "test-hooks")]
pub(super) fn verify(case: &str) {
    match case {
        "receiver_join_returns_private_image_under_original_guards" => {
            receiver_join_returns_private_image_under_original_guards()
        }
        "receiver_join_rejects_foreign_session_without_writes" => {
            receiver_join_rejects_foreign_session_without_writes()
        }
        "receiver_join_rejects_expired_epoch_without_writes" => {
            receiver_join_rejects_expired_epoch_without_writes()
        }
        "receiver_join_rejects_changed_scope_and_unbound_proof_without_writes" => {
            receiver_join_rejects_changed_scope_and_unbound_proof_without_writes()
        }
        "receiver_join_reauthenticates_staged_secret_before_private_writes" => {
            receiver_join_reauthenticates_staged_secret_before_private_writes()
        }

        "typed_preparation_cannot_revive_after_session_block_unblock" => {
            typed_preparation_cannot_revive_after_session_block_unblock()
        }
        "typed_preparation_is_bound_to_original_receiver_session" => {
            typed_preparation_is_bound_to_original_receiver_session()
        }
        "original_installer_keeps_unknown_rows_despite_source_audit_trigger" => {
            original_installer_keeps_unknown_rows_despite_source_audit_trigger()
        }
        "typed_install_rejects_corrupted_staging_before_safety_copy" => {
            typed_install_rejects_corrupted_staging_before_safety_copy()
        }
        "local_text_source_prepares_for_explicit_plaintext" => {
            local_text_source_prepares_for_explicit_plaintext()
        }
        "local_plaintext_source_prepares_for_user_text" => {
            local_plaintext_source_prepares_for_user_text()
        }
        "local_text_source_prepares_for_another_user_text" => {
            local_text_source_prepares_for_another_user_text()
        }
        "remote_authenticated_text_accepts_plaintext_receiver" => {
            remote_authenticated_text_accepts_plaintext_receiver()
        }
        "remote_authenticated_text_accepts_user_text_receiver" => {
            remote_authenticated_text_accepts_user_text_receiver()
        }
        "storage_join_preserves_original_local_rows_and_unknown_schema" => {
            storage_join_preserves_original_local_rows_and_unknown_schema()
        }
        "local_source_rejects_wrong_exact_text_without_writes" => {
            local_source_rejects_wrong_exact_text_without_writes()
        }
        "remote_source_requires_manifest_and_authenticated_key_match" => {
            remote_source_requires_manifest_and_authenticated_key_match()
        }
        "storage_source_rejects_missing_mixed_and_corrupt_identity" => {
            storage_source_rejects_missing_mixed_and_corrupt_identity()
        }
        "storage_source_requires_actual_current_identity_and_schema" => {
            storage_source_requires_actual_current_identity_and_schema()
        }
        "explicit_legacy_sql_uses_original_admission_for_both_modes" => {
            explicit_legacy_sql_uses_original_admission_for_both_modes()
        }
        "original_installer_restores_plaintext_without_changing_mode" => {
            original_installer_restores_plaintext_without_changing_mode()
        }
        "original_installer_restores_user_text_without_changing_key" => {
            original_installer_restores_user_text_without_changing_key()
        }
        "typed_install_rejects_another_destination_before_safety_copy" => {
            typed_install_rejects_another_destination_before_safety_copy()
        }
        _ => panic!("unknown storage source case"),
    }
}
