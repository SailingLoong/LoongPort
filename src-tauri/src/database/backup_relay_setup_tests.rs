// Included inside the original backup owner tests; real import/restore APIs only.
use crate::database::dao::relay_setup_operations as relay_setup_ledger;

fn relay_setup_unknown_fixture(db: &Database, operation: &str) -> Result<(), AppError> {
    use crate::relay::setup_contract::*;
    let discovery = RelayDiscovery {
        schema_version: 1,
        relay_id: 7,
        account: RelayAccountIdentity {
            identity: "fixture/account7".into(),
            site_origin: "https://fixture.example".into(),
            account_label: "Fixture".into(),
        },
        revision: "fixture-revision".into(),
        choices: vec![RelayChoice {
            app: "codex".into(),
            group_id: "stable-group".into(),
            group_name: "Fixture".into(),
            provider_id: "fixture-provider".into(),
            local_configuration: LocalConfiguration::Absent,
            compatibility: ChoiceCompatibility::Unknown,
            key_status: UnknownKeyStatus::Unknown,
        }],
    };
    let receipt = initial_receipt(
        operation,
        &[RelaySelection {
            app: "codex".into(),
            group_id: "stable-group".into(),
        }],
        &discovery,
        1,
    )
    .unwrap();
    let mut conn = db.conn.lock()?;
    relay_setup_ledger::begin_on(&mut conn, "fixture-binding", &receipt)?;
    relay_setup_ledger::key_event_on(
        &mut conn,
        "fixture-binding",
        operation,
        "stable-group",
        relay_setup_ledger::KeyEvent::BeforeCreate,
        2,
    )?;
    relay_setup_ledger::key_event_on(
        &mut conn,
        "fixture-binding",
        operation,
        "stable-group",
        relay_setup_ledger::KeyEvent::OutcomeUnknown,
        3,
    )?;
    Ok(())
}
const RELAY_SETUP_LOCAL_OPERATION: &str = "f8d02319-67c1-426b-a931-c836de11320e";
const RELAY_SETUP_FOREIGN_OPERATION: &str = "95fc0852-e7f3-4d3d-bd83-5e333fa49ec4";

#[test]
fn relay_setup_import_keeps_local_uncertainty_and_rejects_foreign_authority() -> Result<(), AppError>
{
    for has_local in [false, true] {
        let db = Database::memory()?;
        if has_local {
            relay_setup_unknown_fixture(&db, RELAY_SETUP_LOCAL_OPERATION)?;
        }
        let before = relay_setup_ledger::read_rows(&*db.conn.lock()?)?;
        let incoming = same_vault_database(&db)?;
        relay_setup_unknown_fixture(&incoming, RELAY_SETUP_FOREIGN_OPERATION)?;
        let sql = Database::dump_sql(&*incoming.conn.lock()?, &[])?;
        db.import_sql_string(&sql)?;
        assert!(relay_setup_ledger::read_rows(&*db.conn.lock()?)? == before);
        assert!(
            relay_setup_ledger::read_on(&*db.conn.lock()?, RELAY_SETUP_FOREIGN_OPERATION)?
                .is_none()
        );
    }
    Ok(())
}

#[test]
fn relay_setup_import_callback_cannot_erase_retained_evidence() -> Result<(), AppError> {
    let db = Database::memory()?;
    relay_setup_unknown_fixture(&db, RELAY_SETUP_LOCAL_OPERATION)?;
    let sql = Database::dump_sql(&*db.conn.lock()?, &[])?;
    let before = Database::content_digest(&*db.conn.lock()?)?;
    let called = std::cell::Cell::new(false);
    let error = db
        .import_sql_string_from_cc_switch(&sql, &[], |staged| {
            called.set(true);
            staged
                .conn
                .lock()?
                .execute("DELETE FROM relay_setup_operations", [])?;
            Ok(())
        })
        .expect_err("retained uncertainty must not be erased by import preparation");
    assert!(called.get());
    assert!(matches!(error,AppError::Config(code) if code == "relaySetup.localOperationsChanged"));
    assert_eq!(Database::content_digest(&*db.conn.lock()?)?, before);
    Ok(())
}

#[test]
#[serial]
fn relay_setup_backup_restore_does_not_rewind_a_later_attempt() -> Result<(), AppError> {
    let _home = TestHomeGuard::new();
    let _settings_scope = crate::settings::TestSettingsScope::enter()?;
    let db = crate::secrets::testing::initialize_database()?;
    let _settings = SettingsGuard::with_backup_retain_count(1);
    let source = db
        .backup_database_file()?
        .expect("real file-backed snapshot");
    let source_name = source.file_name().unwrap().to_string_lossy().into_owned();
    relay_setup_unknown_fixture(&db, RELAY_SETUP_LOCAL_OPERATION)?;
    let current = relay_setup_ledger::read_rows(&*db.conn.lock()?)?;
    db.restore_from_backup(&source_name)?;
    assert!(relay_setup_ledger::read_rows(&*db.conn.lock()?)? == current);
    let receipt =
        relay_setup_ledger::read_on(&*db.conn.lock()?, RELAY_SETUP_LOCAL_OPERATION)?.unwrap();
    assert_eq!(
        receipt.groups[0].state,
        crate::relay::setup_contract::KeyState::Unknown
    );
    Ok(())
}
