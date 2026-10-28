//! These use the actual encrypted Database/provider writer, not a SQL projection stub.
use crate::database::relay_setup_operations as ledger;
use crate::relay::{creds, setup_contract::*, setup_journal::PreparationAuthority};
use crate::{Database, Provider};
const OP: &str = "f8d02319-67c1-426b-a931-c836de11320e";
fn save_account(db: &Database, token: &str) -> i64 {
    let storage = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    creds::save_authenticated_relay(
        &conn,
        &storage,
        creds::AuthenticatedRelay {
            site: creds::RelaySite {
                site_origin: "https://fixture.example",
                site_name: "Fixture",
                api_base_url: "",
                backend_kind: creds::BackendKind::NewApi,
            },
            account: creds::AccountIdentity {
                id: 7,
                label: "Fixture",
                login_identifier: "fixture",
            },
            auth_token: token,
            refresh_token: None,
            token_expires_at: None,
            session: Default::default(),
        },
    )
    .unwrap()
}
fn fixture() -> (Database, Provider, PreparationAuthority) {
    let db = Database::memory().unwrap();
    let relay_id = save_account(&db, "fixture-session");
    let identity = account_identity_digest("newapi", "https://fixture.example", 7).unwrap();
    let provider_id =
        crate::relay::managed::newapi_provider_id_for("https://fixture.example", 7, "a");
    let mut provider = Provider::with_id(
        provider_id.clone(),
        "Fixture".into(),
        serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"fixture-provider-key","ANTHROPIC_MODEL":"fixture-model"}}),
        Some("https://fixture.example".into()),
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        loongport_account_id: Some(7),
        loongport_group: Some(crate::provider::LoongportGroupIdentity {
            id: "a".into(),
            name: "A".into(),
        }),
        ..Default::default()
    });
    let storage = db.secrets.read().unwrap();
    let authority = PreparationAuthority {
        operation_id: OP.into(),
        storage_binding: storage.identity().store_id().into(),
        account_identity: identity.clone(),
        session_fingerprint: session_fingerprint(&identity, "fixture-session"),
        lifecycle_epoch: db.secrets.lifecycle_epoch(),
    };
    let discovery = RelayDiscovery {
        schema_version: 1,
        relay_id,
        account: RelayAccountIdentity {
            identity,
            site_origin: "https://fixture.example".into(),
            account_label: "Fixture".into(),
        },
        revision: "r1".into(),
        choices: vec![RelayChoice {
            app: "claude".into(),
            group_id: "a".into(),
            group_name: "A".into(),
            provider_id,
            local_configuration: LocalConfiguration::Absent,
            compatibility: ChoiceCompatibility::Unknown,
            key_status: UnknownKeyStatus::Unknown,
        }],
    };
    let receipt = initial_receipt(
        OP,
        &[RelaySelection {
            app: "claude".into(),
            group_id: "a".into(),
        }],
        &discovery,
        1,
    )
    .unwrap();
    let mut conn = db.conn.lock().unwrap();
    ledger::begin_on(&mut conn, &authority.storage_binding, &receipt).unwrap();
    ledger::key_event_on(
        &mut conn,
        &authority.storage_binding,
        OP,
        "a",
        ledger::KeyEvent::Reused("1".into()),
        2,
    )
    .unwrap();
    drop(conn);
    drop(storage);
    (db, provider, authority)
}
#[test]
fn setup_preparation_commits_original_vault_provider_and_receipt_together() {
    let (db, provider, authority) = fixture();
    assert!(db
        .prepare_provider_if_absent("claude", &provider, Some(0.7), Some(&authority))
        .unwrap());
    assert_eq!(
        db.get_provider_by_id(&provider.id, "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        provider.settings_config
    );
    assert_eq!(
        ledger::read_on(&db.conn.lock().unwrap(), OP)
            .unwrap()
            .unwrap()
            .targets[0]
            .configuration,
        ConfigurationState::Prepared
    );
    assert_eq!(
        db.get_setting("application_priority_claude")
            .unwrap()
            .as_deref(),
        Some("[]")
    );
}
#[test]
fn setup_receipt_failure_rolls_back_original_provider_metadata_and_chain() {
    let (db, provider, authority) = fixture();
    db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_prepared BEFORE UPDATE ON relay_setup_operations WHEN json_extract(NEW.receipt_json,'$.targets[0].configuration')='prepared' BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
    let error = db
        .prepare_provider_if_absent("claude", &provider, Some(0.7), Some(&authority))
        .unwrap_err();
    assert!(error.to_string().contains("fixture receipt failure"));
    assert!(db
        .get_provider_by_id(&provider.id, "claude")
        .unwrap()
        .is_none());
    assert!(db
        .get_setting("application_priority_claude")
        .unwrap()
        .is_none());
    let receipt = ledger::read_on(&db.conn.lock().unwrap(), OP)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.targets[0].configuration,
        ConfigurationState::NotStarted
    );
    assert_eq!(receipt.groups[0].state, KeyState::Ready);
}
#[test]
fn setup_preparation_rejects_same_store_restore_lease_before_original_save() {
    let (db, provider, authority) = fixture();
    // The actual restore owner performs this invalidation under its DB lock.
    db.secrets.invalidate_database_leases();
    let error = db
        .prepare_provider_if_absent("claude", &provider, None, Some(&authority))
        .unwrap_err();
    assert!(error.to_string().contains("relaySetup.storageChanged"));
    assert!(db
        .get_provider_by_id(&provider.id, "claude")
        .unwrap()
        .is_none());
    assert!(db
        .get_setting("application_priority_claude")
        .unwrap()
        .is_none());
}
#[test]
fn setup_preparation_rechecks_deleted_or_reauthenticated_account_inside_original_transaction() {
    for delete in [true, false] {
        let (db, provider, authority) = fixture();
        if delete {
            db.conn
                .lock()
                .unwrap()
                .execute("DELETE FROM loongport_relay", [])
                .unwrap();
        } else {
            save_account(&db, "fixture-reauthenticated-session");
        }
        let error = db
            .prepare_provider_if_absent("claude", &provider, None, Some(&authority))
            .unwrap_err();
        assert!(error.to_string().contains("relaySetup.accountChanged"));
        assert!(db
            .get_provider_by_id(&provider.id, "claude")
            .unwrap()
            .is_none());
    }
}
