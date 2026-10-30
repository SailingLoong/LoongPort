//! Synthetic original account writers and schema owners. No remote or user data.
use super::*;
use crate::{database::Database, provider::Provider, secrets::testing::TestHome};
use std::sync::Arc;

fn provider(id: &str) -> Provider {
    Provider::with_id(
        id.into(),
        "Synthetic original".into(),
        crate::relay::provider_config::settings_config_for(
            &crate::app_config::AppType::Codex,
            "synthetic-owner-key",
            "Synthetic original",
            "https://openrouter.ai/api/v1",
            "synthetic/model",
        )
        .unwrap(),
        None,
    )
}
fn revision(db: &Database, id: &str) -> String {
    provider_on(&db.conn.lock().unwrap(), "codex", id).unwrap()
}
fn fresh() -> Database {
    crate::services::native_source_guard::storage_fixture::fresh_plaintext_database()
}
fn proof_save(
    db: &Database,
    provider: &Provider,
    digest: &str,
    proof: &WriteProof,
) -> Result<(), AppError> {
    let storage = db.secrets.read()?;
    db.save_provider_with_vault_credential_write(
        "codex",
        provider,
        digest,
        false,
        db.secrets.as_ref(),
        &storage,
        Some(proof),
    )
}
fn schema(case: &str) {
    let _home = TestHome::new().unwrap();
    let (encrypted, case) = case
        .strip_prefix("encrypted_")
        .map_or((false, case), |case| (true, case));
    let db = if encrypted {
        crate::secrets::testing::initialize_database().unwrap()
    } else {
        fresh()
    };
    let row = provider("schema-original");
    db.save_provider("codex", &row).unwrap();
    let before = revision(&db, &row.id);
    let conn = db.conn.lock().unwrap();
    assert_eq!(
        crate::database::loongport_schema::read_stored_version(&conn).unwrap(),
        crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
    );
    let upstream = Database::get_user_version(&conn).unwrap();
    validate_schema(&conn).unwrap();
    if case == "checkpoint_boundary" {
        let current = crate::database::loongport_schema::read_stored_version(&conn).unwrap();
        assert!(crate::secrets::upgrade::checkpoint::supports_loongport_version(current));
        for version in
            crate::database::loongport_schema::MIN_AUTHENTICATED_SNAPSHOT_VERSION..=current
        {
            assert!(crate::secrets::upgrade::checkpoint::supports_loongport_version(version));
        }
        assert!(!crate::secrets::upgrade::checkpoint::supports_loongport_version(23));
        assert!(!crate::secrets::upgrade::checkpoint::supports_loongport_version(current + 1));
        return;
    }
    if case == "fresh" {
        assert!(valid(&before));
        let dto = serde_json::to_string(&row).unwrap();
        assert!(!dto.contains(&before) && !dto.contains("credential_revision"));
        crate::database::loongport_schema::apply(&conn).unwrap();
        assert_eq!(provider_on(&conn, "codex", &row.id).unwrap(), before);
        assert_eq!(Database::get_user_version(&conn).unwrap(), upstream);
        return;
    }
    if case == "missing_vendor" {
        conn.execute_batch("DROP TABLE loongport_vendor").unwrap();
        let changes = conn.total_changes();
        assert!(crate::database::vault::preflight_structure(&conn).is_err());
        assert!(Database::create_tables_on_conn(&conn).is_err());
        assert!(!Database::table_exists(&conn, "loongport_vendor").unwrap());
        assert_eq!(conn.total_changes(), changes);
        return;
    }
    let settings: String = conn
        .query_row(
            "SELECT settings_config FROM providers WHERE id=?1",
            [&row.id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch("ALTER TABLE providers DROP COLUMN credential_revision; ALTER TABLE loongport_vendor DROP COLUMN api_key_revision;").unwrap();
    if case == "tamper" {
        assert!(crate::database::vault::preflight_structure(&conn).is_err());
        assert!(Database::create_tables_on_conn(&conn).is_err());
        assert_eq!(
            crate::database::loongport_schema::read_stored_version(&conn).unwrap(),
            crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
        );
        return;
    }
    conn.execute(
        "UPDATE loongport_schema_version SET version=30 WHERE id=1",
        [],
    )
    .unwrap();
    crate::database::vault::preflight_structure(&conn).unwrap();
    let changes = conn.total_changes();
    assert!(provider_on(&conn, "codex", &row.id).is_err());
    assert_eq!(
        conn.total_changes(),
        changes,
        "legacy read never silently migrates"
    );
    if matches!(case, "legacy_prepare" | "legacy_missing_vendor") {
        if case == "legacy_missing_vendor" {
            conn.execute_batch("DROP TABLE loongport_vendor").unwrap();
        }
        let context = db.secrets.read().unwrap().clone();
        drop(conn);
        drop(db);
        let path = crate::config::get_app_config_dir().join(crate::config::DB_FILE_NAME);
        let reopened = crate::database::storage::prepare(&path, &context).unwrap();
        assert_eq!(
            crate::database::loongport_schema::read_stored_version(&reopened).unwrap(),
            crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
        );
        assert!(valid(&provider_on(&reopened, "codex", &row.id).unwrap()));
        let after: String = reopened
            .query_row(
                "SELECT settings_config FROM providers WHERE id=?1",
                [&row.id],
                |r| r.get(0),
            )
            .unwrap();
        if encrypted {
            // The original authenticated prepare re-seals inventory values with
            // fresh nonces. Preserve authenticated content, not ciphertext bytes.
            let before_plain = crate::secrets::inventory::open_db(
                &context,
                "providers",
                "settings_config",
                &[&row.id, "codex"],
                &settings,
            )
            .unwrap();
            let after_plain = crate::secrets::inventory::open_db(
                &context,
                "providers",
                "settings_config",
                &[&row.id, "codex"],
                &after,
            )
            .unwrap();
            assert!(after.starts_with("lpenc1."));
            assert!(!after.contains("synthetic-owner-key"));
            assert!(after_plain == before_plain, "authenticated content changed");
            assert!(crate::secrets::inventory::open_db(
                &context,
                "providers",
                "settings_config",
                &["wrong-original-owner", "codex"],
                &after,
            )
            .is_err());
        } else {
            assert!(after == settings, "plaintext content changed");
        }
        assert_eq!(Database::get_user_version(&reopened).unwrap(), upstream);
        return;
    }
    crate::database::loongport_schema::apply(&conn).unwrap();
    let migrated = provider_on(&conn, "codex", &row.id).unwrap();
    assert!(valid(&migrated));
    assert_ne!(migrated, before);
    crate::database::loongport_schema::apply(&conn).unwrap();
    assert_eq!(provider_on(&conn, "codex", &row.id).unwrap(), migrated);
    assert_eq!(Database::get_user_version(&conn).unwrap(), upstream);
    assert_eq!(
        conn.query_row(
            "SELECT settings_config FROM providers WHERE id=?1",
            [&row.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        settings
    );
}
fn original_provider(case: &str) {
    let _home = TestHome::new().unwrap();
    let db = Arc::new(fresh());
    let row = provider("original");
    db.save_provider("codex", &row).unwrap();
    let old = revision(&db, &row.id);
    let digest = Database::provider_update_digest(
        &db.get_provider_by_id(&row.id, "codex").unwrap().unwrap(),
    )
    .unwrap();
    if case == "metadata" {
        let mut label = row.clone();
        label.name = "Renamed".into();
        db.save_provider("codex", &label).unwrap();
        assert_eq!(revision(&db, &row.id), old);
        label.settings_config["config"] = serde_json::json!(label.settings_config["config"]
            .as_str()
            .unwrap()
            .replace("synthetic/model", "different/model"));
        db.save_provider("codex", &label).unwrap();
        assert_eq!(revision(&db, &row.id), old);
        let other = provider("other");
        db.save_provider("codex", &other).unwrap();
        db.save_provider_with_credential_intent("codex", &other, true)
            .unwrap();
        assert_eq!(revision(&db, &row.id), old);
    } else if case == "json_import" {
        let mut config = crate::app_config::MultiAppConfig::default();
        config
            .apps
            .get_mut("codex")
            .unwrap()
            .providers
            .insert(row.id.clone(), row.clone());
        db.migrate_from_json(&config).unwrap();
        let imported = revision(&db, &row.id);
        assert_ne!(imported, old);
        assert!(valid(&imported));
        assert_eq!(
            Database::provider_update_digest(
                &db.get_provider_by_id(&row.id, "codex").unwrap().unwrap()
            )
            .unwrap(),
            digest
        );
        db.migrate_from_json(&config).unwrap();
        assert_ne!(
            revision(&db, &row.id),
            imported,
            "original row replacement is a new incarnation, not stale UI metadata"
        );
    } else if case == "recreate" {
        db.delete_provider("codex", &row.id).unwrap();
        db.save_provider("codex", &row).unwrap();
        assert_ne!(revision(&db, &row.id), old);
    } else if case == "already_current" {
        let storage = db.secrets.read().unwrap();
        db.save_provider_with_vault(
            "codex",
            &row,
            Some(&digest),
            false,
            db.secrets.as_ref(),
            &storage,
        )
        .unwrap();
        assert_eq!(revision(&db, &row.id), old);
    } else if case == "proof" {
        let proof = WriteProof {
            before: old.clone(),
            next: new_revision(),
        };
        proof_save(&db, &row, &digest, &proof).unwrap();
        assert_eq!(revision(&db, &row.id), proof.next);
        proof_save(&db, &row, &digest, &proof).unwrap();
        assert_eq!(
            revision(&db, &row.id),
            proof.next,
            "already-current journal replay cannot issue a third revision"
        );
        let stale = WriteProof {
            before: old,
            next: new_revision(),
        };
        assert!(proof_save(&db, &row, &digest, &stale).is_err());
        assert_eq!(revision(&db, &row.id), proof.next);
    } else if case == "concurrent" {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let db = db.clone();
            let row = row.clone();
            let digest = digest.clone();
            let barrier = barrier.clone();
            let proof = WriteProof {
                before: old.clone(),
                next: new_revision(),
            };
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                (proof_save(&db, &row, &digest, &proof).is_ok(), proof.next)
            }));
        }
        let outcomes = workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|(ok, _)| *ok).count(), 1);
        assert_eq!(
            revision(&db, &row.id),
            outcomes.iter().find(|(ok, _)| *ok).unwrap().1
        );
    } else {
        panic!("unknown provider case");
    }
}
fn vendor(case: &str) {
    let _home = TestHome::new().unwrap();
    let db = fresh();
    let storage = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    let key = "oc_sk_synthetic_1234567890abcdefghijklmnop";
    let id =
        crate::vendor::opencode_personal::save(&conn, &storage, key, "Synthetic personal").unwrap();
    let read = || {
        conn.query_row(
            "SELECT api_key_revision FROM loongport_vendor WHERE id=?1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    let before = read();
    assert!(valid(&before));
    if case == "personal" {
        let same =
            crate::vendor::opencode_personal::save(&conn, &storage, key, "Synthetic personal")
                .unwrap();
        assert_eq!(same, id);
        assert_ne!(read(), before);
    } else if case == "metadata" {
        conn.execute(
            "UPDATE loongport_vendor SET account_label='Renamed' WHERE id=?1",
            [id],
        )
        .unwrap();
        crate::vendor::creds::reorder(&conn, &[id]).unwrap();
        crate::vendor::creds::clear_token(&conn, id).unwrap();
        assert_eq!(read(), before);
    } else if case == "recreate" {
        let account = crate::vendor::opencode_personal::identity(key).unwrap();
        conn.execute("DELETE FROM loongport_vendor WHERE id=?1", [id])
            .unwrap();
        conn.execute(
            "INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(?1,'opencode',?2)",
            rusqlite::params![id, account],
        )
        .unwrap();
        crate::vendor::creds::set_api_key(&conn, &storage, id, key).unwrap();
        assert_ne!(read(), before);
    } else {
        panic!("unknown vendor case");
    }
}

fn legacy_original_provider(case: &str) {
    let _home = TestHome::new().unwrap();
    let db = Arc::new(fresh());
    let mut row = provider("legacy-original");
    db.save_provider("codex", &row).unwrap();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute_batch("ALTER TABLE providers DROP COLUMN credential_revision;")
            .unwrap();
        if case != "latest_missing" {
            conn.execute_batch("ALTER TABLE loongport_vendor DROP COLUMN api_key_revision;")
                .unwrap();
            // LP31 adds only these two original-row columns. Removing them
            // reconstructs the real LP30 shape, not a mislabeled current schema.
            conn.execute(
                "UPDATE loongport_schema_version SET version=30 WHERE id=1",
                [],
            )
            .unwrap();
        }
        if matches!(case, "mixed" | "mixed_case") {
            let name = if case == "mixed_case" {
                "CREDENTIAL_REVISION"
            } else {
                "credential_revision"
            };
            conn.execute_batch(&format!(
                "ALTER TABLE providers ADD COLUMN {name} TEXT NOT NULL DEFAULT '';"
            ))
            .unwrap();
            conn.execute(
                "UPDATE providers SET credential_revision=?1 WHERE id=?2",
                rusqlite::params![new_revision(), row.id],
            )
            .unwrap();
            assert!(
                provider_on(&conn, "codex", &row.id).is_ok(),
                "SQLite column resolution is case-insensitive even in a mixed schema"
            );
        }
    }
    if matches!(case, "latest_missing" | "mixed" | "mixed_case") {
        let before = db.get_provider_by_id(&row.id, "codex").unwrap().unwrap();
        row.name = "Must not publish".into();
        assert!(
            db.save_provider("codex", &row).is_err(),
            "a missing latest column or mixed-version schema is not an ordinary legacy owner"
        );
        assert_eq!(
            db.get_provider_by_id(&row.id, "codex")
                .unwrap()
                .unwrap()
                .name,
            before.name
        );
        return;
    }
    assert_eq!(case, "write");
    row.name = "Legacy label".into();
    db.save_provider("codex", &row)
        .expect("ordinary original legacy save stays available");
    // This is the boolean emitted by the existing key-touched UI, not a new
    // authority to mint shared credentials before the original migration.
    db.save_provider_with_credential_intent("codex", &row, true)
        .expect("ordinary legacy key-write intent retains the original save contract");
    {
        let storage = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            crate::database::loongport_schema::read_stored_version(&conn).unwrap(),
            30
        );
        assert!(provider_on(&conn, "codex", &row.id).is_err());
        assert!(
            crate::services::openrouter_source::read_on(
                &conn,
                &storage,
                &crate::services::openrouter_source::RowRef {
                    origin_app: "codex".into(),
                    provider_id: row.id.clone()
                }
            )
            .is_err(),
            "legacy original save never mints a shared source proof"
        );
        let before = conn.total_changes();
        assert!(provider_on(&conn, "codex", &row.id).is_err());
        assert_eq!(
            conn.total_changes(),
            before,
            "failed proof read never migrates"
        );
        crate::database::loongport_schema::apply(&conn).unwrap();
        assert!(valid(&provider_on(&conn, "codex", &row.id).unwrap()));
        assert!(crate::services::openrouter_source::read_on(
            &conn,
            &storage,
            &crate::services::openrouter_source::RowRef {
                origin_app: "codex".into(),
                provider_id: row.id.clone()
            }
        )
        .unwrap()
        .is_some());
    }
    let fresh_proof = {
        let storage = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        crate::services::openrouter_source::capture_on(
            &conn,
            &storage,
            db.secrets.lifecycle_epoch(),
            &crate::services::openrouter_source::RowRef {
                origin_app: "codex".into(),
                provider_id: row.id.clone(),
            },
        )
        .unwrap()
    };
    let (_, fresh_fence) = crate::services::openrouter_source::acquire_expected(
        db.clone(),
        &crate::app_config::AppType::Codex,
        &fresh_proof,
    )
    .unwrap();
    fresh_fence.check().unwrap();
    let before = revision(&db, &row.id);
    db.save_provider_with_credential_intent("codex", &row, true)
        .unwrap();
    assert_ne!(
        revision(&db, &row.id),
        before,
        "modern same-key reissue still rotates the authoritative row revision"
    );
    assert!(fresh_fence.check().is_err());
    assert!(crate::services::openrouter_source::acquire_expected(
        db,
        &crate::app_config::AppType::Codex,
        &fresh_proof
    )
    .is_err());
}

pub(crate) fn verify(case: &str) {
    if let Some(case) = case.strip_prefix("legacy_") {
        return legacy_original_provider(case);
    }
    if let Some(case) = case.strip_prefix("prior_checkpoint_") {
        return crate::secrets::upgrade::checkpoint::verify_prior_fixture(case);
    }
    if let Some(case) = case.strip_prefix("prior_") {
        return crate::database::loongport_schema::verify_prior_fixture(case);
    }
    if let Some(case) = case.strip_prefix("schema_") {
        return schema(case);
    }
    if let Some(case) = case.strip_prefix("provider_") {
        return original_provider(case);
    }
    if let Some(case) = case.strip_prefix("vendor_") {
        return vendor(case);
    }
    panic!("unknown credential revision case");
}
