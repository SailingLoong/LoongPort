//! Actual Codex service entry, synthetic files/account bundles only.
use super::*;
use crate::live::engine::{read_current, DeviceStore};
use crate::mode::{
    operation::failpoint,
    state::{self, Mode},
};
use crate::secrets::testing::TestHome;
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    state: AppState,
    config: std::path::PathBuf,
    auth: std::path::PathBuf,
    _home: TestHome,
}
impl Fixture {
    fn new() -> Self {
        Self::with_schema(true)
    }
    fn with_schema(modern: bool) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(crate::secrets::testing::initialize_database().unwrap());
        if modern {
            let conn = db.conn.lock().unwrap();
            crate::database::Database::apply_upstream4_migrations_on_conn(&conn).unwrap();
        }
        for id in ["a", "b"] {
            let provider = Provider::with_id(
                id.into(),
                format!("synthetic-{id}"),
                json!({
                    "auth": {"OPENAI_API_KEY": format!("synthetic-key-{id}")},
                    "config": format!("model = \"model-{id}\"\nmodel_provider = \"synthetic\"\n[model_providers.synthetic]\nname = \"Synthetic\"\nbase_url = \"https://{id}.example.invalid/v1\"\nwire_api = \"responses\"\n")
                }),
                None,
            );
            db.save_provider("codex", &provider).unwrap();
        }
        db.set_current_provider("codex", "a").unwrap();
        crate::settings::set_current_provider(&AppType::Codex, Some("a")).unwrap();
        let state = AppState::new(db).unwrap();
        let config = crate::codex_config::get_codex_config_path();
        let auth = crate::codex_config::get_codex_auth_path();
        assert!(config.starts_with(home.path()));
        assert!(auth.starts_with(home.path()));
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            b"# handwritten\nmodel = \"model-a\"\n[unowned]\nkeep = \"exact\" # untouched\n",
        )
        .unwrap();
        std::fs::write(&auth, br#"{"OPENAI_API_KEY":"synthetic-native"}"#).unwrap();
        {
            let vault = state.db.secret_session().read().unwrap();
            state::update(&DeviceStore::for_device(), &vault, |live| {
                live.apps.entry("codex".into()).or_default().mode = Some(Mode::Direct);
                Ok(())
            })
            .unwrap();
        }
        Self {
            _home: home,
            state,
            config,
            auth,
        }
    }
    fn pending(&self) -> Option<state::Pending> {
        state::pending(
            &DeviceStore::for_device(),
            &self.state.db.secret_session().read().unwrap(),
            "codex",
        )
        .unwrap()
    }
    fn assert_current(&self, id: &str) {
        assert_eq!(
            self.state
                .db
                .get_current_provider("codex")
                .unwrap()
                .as_deref(),
            Some(id)
        );
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Codex).as_deref(),
            Some(id)
        );
    }
}
struct Fault;
impl Fault {
    fn at(point: &str) -> Self {
        failpoint::crash_at(Some(point));
        Self
    }
}
impl Drop for Fault {
    fn drop(&mut self) {
        failpoint::crash_at(None);
        failpoint::on_before_publish(None);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_service_intent_precedes_all_client_publication() {
    let fixture = Fixture::new();
    let config = read_current(&fixture.config).unwrap();
    let auth = read_current(&fixture.auth).unwrap();
    let _fault = Fault::at("pending");
    let result = ProviderService::switch(&fixture.state, AppType::Codex, "b");
    assert!(result.is_err());
    assert!(
        fixture.pending().is_some(),
        "real Codex switch must use the shared intent owner: {result:?}"
    );
    assert_eq!(read_current(&fixture.config).unwrap(), config);
    assert_eq!(read_current(&fixture.auth).unwrap(), auth);
    fixture.assert_current("a");
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    super::codex_login::verify_original_tests();
    println!("PASS 10 original upstream Codex login state-machine cases");
    codex_service_intent_precedes_all_client_publication();
    println!("PASS Codex service intent precedes all client publication");
    codex_service_preserves_native_auth_and_unowned_text();
    println!("PASS Codex native auth and unowned byte preservation");
    codex_native_login_roundtrip_honors_preserve_off_and_auth_placement();
    println!("PASS Codex native roundtrip and auth placement");
    codex_faults_recover_original_intent_without_legacy_snapshot_rollback();
    println!("PASS Codex staged intent/publication/target recovery");
    codex_malformed_inputs_never_publish_a_partial_bundle();
    println!("PASS Codex five-file malformed-input admission");
    codex_external_changes_and_missing_staging_retain_barrier();
    println!("PASS Codex changed/missing-stage verification barrier");
    codex_unclaimed_external_catalog_is_preserved_by_default();
    println!("PASS Codex default external catalog preservation");
    managed_token_refresh_during_prepare_does_not_publish_live_before_intent();
    println!("PASS Codex token refresh prepares without live publication");
    managed_generation_advanced_after_prepare_is_not_overwritten();
    println!("PASS Codex newer manager generation blocks stale prepared bundle");
    managed_recovery_refuses_a_newer_manager_generation_before_more_effects();
    println!("PASS Codex recovery retains barrier for newer manager generation");
    verify_generation_proof()?;
    explicit_catalog_takeover_uses_bound_revision_and_retains_previous_pointer();
    println!("PASS Codex explicit catalog takeover retains original pointer evidence");
    catalog_restore_changes_only_the_pointer_under_a_fresh_revision();
    println!("PASS Codex catalog restore uses fresh revision and only changes pointer");
    catalog_takeover_faults_keep_original_pointer_evidence_through_recovery();
    println!("PASS Codex catalog takeover evidence survives publication/target recovery");
    managed_cross_account_publication_and_real_target_failures_recover();
    println!("PASS Codex cross-account/marker publication and real target failure recovery");
    ambiguous_cli_generations_and_missing_accounts_never_write();
    println!("PASS Codex equal/undated generation and missing-account barriers");
    schema17_and_codex_image_keep_their_existing_paths();
    println!("PASS Codex source17 and independent image entry boundaries");
    admission_failures_precede_even_credential_preparation();
    println!("PASS Codex unknown/checkpoint/settings/legacy gates precede credential work");
    catalog_choices_reject_wrong_revision_and_restore_faults_recover();
    println!("PASS Codex stale catalog choices and restore fault recovery");
    verify_review_cases()?;
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_service_preserves_native_auth_and_unowned_text() {
    let fixture = Fixture::new();
    let auth = read_current(&fixture.auth).unwrap();
    ProviderService::switch(&fixture.state, AppType::Codex, "b").unwrap();
    fixture.assert_current("b");
    assert!(fixture.pending().is_none());
    assert_eq!(read_current(&fixture.auth).unwrap(), auth);
    let text = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(text.contains("# handwritten\n"));
    assert!(text.contains("[unowned]\nkeep = \"exact\" # untouched\n"));
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    assert_eq!(doc["model"].as_str(), Some("model-b"));
    assert_eq!(
        doc["model_providers"]["custom"]["experimental_bearer_token"].as_str(),
        Some("synthetic-key-b")
    );
    assert!(doc["model_providers"]["custom"]
        .as_table()
        .unwrap()
        .get("requires_openai_auth")
        .is_none());
    let stash =
        crate::secrets::owned_file::DeviceFile::registered("codex-login-stash.json").unwrap();
    let store = DeviceStore::for_device();
    let ciphertext = std::fs::read(store.path_for(&stash)).unwrap();
    assert!(stash
        .decode(
            &fixture.state.db.secret_session().read().unwrap(),
            &ciphertext
        )
        .is_ok());
    // A second application replaces an existing positioned route in place.
    ProviderService::switch(&fixture.state, AppType::Codex, "a").unwrap();
    fixture.assert_current("a");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_native_login_roundtrip_honors_preserve_off_and_auth_placement() {
    let fixture = Fixture::new();
    let native = read_current(&fixture.auth).unwrap();
    let mut settings = crate::settings::get_settings();
    settings.preserve_codex_official_auth_on_switch = false;
    crate::settings::update_settings(settings).unwrap();
    let mut provider = Provider::with_id(
        "native".into(),
        "Native".into(),
        json!({"auth":{},"config":"model = \"gpt-5.5\"\n"}),
        None,
    );
    provider.category = Some("official".into());
    fixture.state.db.save_provider("codex", &provider).unwrap();
    ProviderService::switch(&fixture.state, AppType::Codex, "b").unwrap();
    let auth: Value = serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
    assert_eq!(auth["OPENAI_API_KEY"], "synthetic-key-b");
    let doc: toml_edit::DocumentMut = std::fs::read_to_string(&fixture.config)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        doc["model_providers"]["custom"]
            .get("requires_openai_auth")
            .and_then(toml_edit::Item::as_bool),
        Some(true)
    );
    assert!(doc["model_providers"]["custom"]
        .as_table()
        .unwrap()
        .get("experimental_bearer_token")
        .is_none());
    ProviderService::switch(&fixture.state, AppType::Codex, "native").unwrap();
    let restored: Value =
        serde_json::from_slice(&read_current(&fixture.auth).unwrap().unwrap()).unwrap();
    assert_eq!(
        restored,
        serde_json::from_slice::<Value>(&native.unwrap()).unwrap()
    );
    fixture.assert_current("native");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_faults_recover_original_intent_without_legacy_snapshot_rollback() {
    for point in [
        "pending",
        "published:0",
        "published:1",
        "published:4",
        "target",
    ] {
        let fixture = Fixture::new();
        let mut settings = crate::settings::get_settings();
        settings.preserve_codex_official_auth_on_switch = false;
        crate::settings::update_settings(settings).unwrap();
        let before = read_current(&fixture.auth).unwrap();
        {
            let _fault = Fault::at(point);
            assert!(
                ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err(),
                "{point}"
            );
        }
        assert!(fixture.pending().is_some(), "{point}");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "a").is_err());
        let result = super::codex_direct::recover_pending(&fixture.state).unwrap();
        if point == "pending" {
            assert_eq!(
                result,
                Some(crate::mode::operation::RecoveryOutcome::Discarded)
            );
            assert_eq!(read_current(&fixture.auth).unwrap(), before);
            fixture.assert_current("a");
        } else {
            assert_eq!(
                result,
                Some(crate::mode::operation::RecoveryOutcome::RolledForward)
            );
            let auth: Value =
                serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
            assert_eq!(auth["OPENAI_API_KEY"], "synthetic-key-b");
            fixture.assert_current("b");
        }
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_malformed_inputs_never_publish_a_partial_bundle() {
    for index in [0, 1, 2, 3, 4] {
        let fixture = Fixture::new();
        let files = super::codex_direct::files();
        let path = &files[index].path;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"{ malformed").unwrap();
        let before = files
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        assert!(
            ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err(),
            "file {index}"
        );
        let after = files
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
        fixture.assert_current("a");
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_external_changes_and_missing_staging_retain_barrier() {
    for external in [true, false] {
        let fixture = Fixture::new();
        {
            let _fault = Fault::at("published:1");
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err());
        }
        if external {
            std::fs::write(
                &fixture.config,
                b"# externally changed\nmodel = \"external\"\n",
            )
            .unwrap();
        } else {
            std::fs::remove_file(fixture.pending().unwrap().files[4].staged.as_ref().unwrap())
                .unwrap();
        }
        let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
        assert!(matches!(
            outcome,
            Some(crate::mode::operation::RecoveryOutcome::VerificationRequired { .. })
        ));
        assert!(fixture.pending().is_some());
        fixture.assert_current("a");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_unclaimed_external_catalog_is_preserved_by_default() {
    for filename in [
        "external-catalog.json",
        "loongport-model-catalog.json",
        "cc-switch-model-catalog.json",
    ] {
        let fixture = Fixture::new();
        let external = fixture._home.path().join(filename);
        std::fs::write(&external, b"external rich schema, never parse").unwrap();
        let before = std::fs::read_to_string(&fixture.config).unwrap();
        let encoded = toml_edit::Value::from(external.to_str().unwrap());
        std::fs::write(
            &fixture.config,
            format!("model_catalog_json = {encoded} # external owner\n{before}"),
        )
        .unwrap();
        let result = ProviderService::switch(&fixture.state, AppType::Codex, "b").unwrap();
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("模型映射未生效")));
        let text = std::fs::read_to_string(&fixture.config).unwrap();
        assert!(text.contains(&format!(
            "model_catalog_json = {encoded} # external owner\n"
        )));
        assert_eq!(
            std::fs::read(&external).unwrap(),
            b"external rich schema, never parse"
        );
        fixture.assert_current("b");
    }
}

fn managed_provider(id: &str, account: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.into(),
        id.into(),
        json!({"auth":{},"config":"model = \"gpt-5.5\"\n"}),
        None,
    );
    provider.category = Some("official".into());
    provider.meta = Some(crate::provider::ProviderMeta {
        auth_binding: Some(crate::provider::AuthBinding {
            source: crate::provider::AuthBindingSource::ManagedAccount,
            auth_provider: Some("codex_oauth".into()),
            account_id: Some(account.into()),
        }),
        ..Default::default()
    });
    provider
}
fn seed_managed(f: &Fixture, id: &str, account: &str, access: &str) -> Provider {
    crate::rt::block_on(
        f.state
            .codex_oauth_manager
            .add_test_account_with_access_token(account, access, Some("synthetic-id-token")),
    )
    .unwrap();
    let provider = managed_provider(id, account);
    f.state.db.save_provider("codex", &provider).unwrap();
    provider
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn managed_generation_advanced_after_prepare_is_not_overwritten() {
    let fixture = Fixture::new();
    let provider = seed_managed(&fixture, "managed", "synthetic-account", "access-old");
    let owner = super::codex_direct::Owner::None;
    let prepared =
        super::codex_direct::prepare(&fixture.state.codex_oauth_manager, &owner, &provider)
            .unwrap();
    let planned = super::codex_direct::plan(&fixture.state.db, &owner, &provider).unwrap();
    let before = super::codex_direct::files()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    seed_managed(&fixture, "managed", "synthetic-account", "access-newer");
    assert!(
        super::codex_direct::run(&fixture.state, planned, &prepared, &provider).is_err(),
        "a prepared bundle cannot overwrite a newer manager generation"
    );
    assert_eq!(
        before,
        super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>()
    );
    fixture.assert_current("a");
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn managed_token_refresh_during_prepare_does_not_publish_live_before_intent() {
    let fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "access-old");
    ProviderService::switch(&fixture.state, AppType::Codex, "managed").unwrap();
    let before = read_current(&fixture.auth).unwrap();
    crate::rt::block_on(fixture.state.codex_oauth_manager.test_refresh_next(
        "synthetic-account",
        "access-refreshed",
        "refresh-refreshed",
    ));
    let _fault = Fault::at("pending");
    assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
    assert!(fixture.pending().is_some());
    assert_eq!(
        read_current(&fixture.auth).unwrap(),
        before,
        "manager preparation must not publish native auth ahead of intent"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn managed_recovery_refuses_a_newer_manager_generation_before_more_effects() {
    let fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "access-old");
    {
        let _fault = Fault::at("published:0");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
    }
    seed_managed(&fixture, "managed", "synthetic-account", "access-newer");
    crate::rt::block_on(
        fixture
            .state
            .codex_oauth_manager
            .test_set_token_updated_at_ms(
                "synthetic-account",
                chrono::Utc::now().timestamp_millis() + 5000,
            ),
    );
    let before = super::codex_direct::files()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
    assert!(
        matches!(
            outcome,
            Some(crate::mode::operation::RecoveryOutcome::VerificationRequired { .. })
        ),
        "newer manager state must prevent stale forward recovery: {outcome:?}"
    );
    assert!(fixture.pending().is_some());
    assert_eq!(
        before,
        super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>()
    );
    fixture.assert_current("a");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn unproven_new_live_auth_without_cache_retains_verification_barrier() {
    let mut fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "access-old");
    {
        let _fault = Fault::at("target");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
    }
    let when = chrono::Utc::now() + chrono::Duration::seconds(10);
    let newer = crate::codex_config::codex_managed_oauth_auth_value(
        "synthetic-account",
        "cli-newer-access",
        Some("synthetic-id-token"),
        "cli-newer-refresh",
        &when.to_rfc3339(),
    );
    crate::config::write_json_file_private(&fixture.auth, &newer).unwrap();
    crate::rt::block_on(
        fixture
            .state
            .codex_oauth_manager
            .adopt_account_refresh_token(
                "synthetic-account",
                "cli-newer-refresh".into(),
                Some("synthetic-id-token".into()),
                Some(when.timestamp_millis()),
            ),
    )
    .unwrap();
    // A restart rebuilds AppState and ProxyService around the same new manager Arc.
    fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
    let before = std::fs::read(&fixture.auth).unwrap();
    let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
    assert!(
        matches!(
            outcome,
            Some(crate::mode::operation::RecoveryOutcome::VerificationRequired { .. })
        ),
        "refresh/id facts after restart do not prove unknown access bytes: {outcome:?}"
    );
    assert_eq!(std::fs::read(&fixture.auth).unwrap(), before);
    fixture.assert_current("managed");
    assert!(fixture.pending().is_some());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_catalog_takeover_uses_bound_revision_and_retains_previous_pointer() {
    let fixture = Fixture::new();
    let mut provider = fixture
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    provider.settings_config["modelCatalog"] = json!({"models":[{"model":"synthetic-model","displayName":"Synthetic","contextWindow":32768}]});
    fixture.state.db.save_provider("codex", &provider).unwrap();
    let original = std::fs::read_to_string(&fixture.config).unwrap();
    std::fs::write(
        &fixture.config,
        format!("model_catalog_json = \"unclaimed-external.json\" # retained owner\n{original}"),
    )
    .unwrap();
    let revision = super::codex_direct::CatalogRevision {
        app: "codex".into(),
        provider_id: "b".into(),
        config_digest: crate::live::engine::digest(
            read_current(&fixture.config).unwrap().as_deref(),
        ),
    };
    super::codex_direct::manage_catalog(&fixture.state, &revision).unwrap();
    let text = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(
        crate::live::project::codex::live_catalog_is_ours(&text),
        "explicit choice must replace the pointer only after revision admission"
    );
    let vault = fixture.state.db.secret_session().read().unwrap();
    let written = state::written(&DeviceStore::for_device(), &vault, "codex")
        .unwrap()
        .unwrap();
    let evidence = serde_json::to_value(written).unwrap();
    assert_eq!(
        evidence
            .pointer("/codex/catalog/previous_pointer")
            .and_then(Value::as_str),
        Some("unclaimed-external.json")
    );
    assert_eq!(
        evidence
            .pointer("/codex/catalog/config_pre")
            .and_then(Value::as_str),
        revision.config_digest.as_deref()
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn mixed_access_token_cannot_borrow_known_refresh_generation() {
    let fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "known-access");
    {
        let _fault = Fault::at("target");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
    }
    let mut mixed: Value = serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
    mixed["tokens"]["access_token"] = json!("unproven-access");
    mixed["last_refresh"] =
        json!((chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc3339());
    crate::config::write_json_file_private(&fixture.auth, &mixed).unwrap();
    let before = std::fs::read(&fixture.auth).unwrap();
    let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
    assert!(
        matches!(
            outcome,
            Some(crate::mode::operation::RecoveryOutcome::VerificationRequired { .. })
        ),
        "known refresh/id cannot authenticate arbitrary access material: {outcome:?}"
    );
    assert_eq!(std::fs::read(&fixture.auth).unwrap(), before);
    assert!(fixture.pending().is_some());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_generation_proof() -> Result<(), AppError> {
    let mixed = std::panic::catch_unwind(mixed_access_token_cannot_borrow_known_refresh_generation);
    let unknown =
        std::panic::catch_unwind(unproven_new_live_auth_without_cache_retains_verification_barrier);
    let later_live = std::panic::catch_unwind(
        same_refresh_material_does_not_authorize_overwriting_a_newer_live_access_token,
    );
    let restarted_proof = std::panic::catch_unwind(
        manager_known_newer_complete_bundle_can_finish_without_stale_replay,
    );
    assert!(
        mixed.is_ok() && unknown.is_ok() && later_live.is_ok() && restarted_proof.is_ok(),
        "both unknown-generation recovery boundaries must hold"
    );
    println!("PASS Codex mixed/cached and unknown/restarted access generation barriers");
    println!("PASS Codex manager-known complete newer bundle completes across restart without stale replay");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn manager_known_newer_complete_bundle_can_finish_without_stale_replay() {
    let mut fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "access-old");
    {
        let _fault = Fault::at("target");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
    }
    seed_managed(
        &fixture,
        "managed",
        "synthetic-account",
        "known-newer-access",
    );
    let time = chrono::Utc::now().timestamp_millis() + 10_000;
    crate::rt::block_on(
        fixture
            .state
            .codex_oauth_manager
            .test_set_bundle_time("synthetic-account", time),
    );
    let bundle = crate::rt::block_on(
        fixture
            .state
            .codex_oauth_manager
            .prepare_live_token_bundle("synthetic-account"),
    )
    .unwrap();
    let auth = crate::codex_config::codex_managed_oauth_auth_value(
        "synthetic-account",
        &bundle.access_token,
        bundle.id_token.as_deref(),
        &bundle.refresh_token,
        &bundle.last_refresh,
    );
    crate::config::write_json_file_private(&fixture.auth, &auth).unwrap();
    let before = std::fs::read(&fixture.auth).unwrap();
    {
        let _fault = Fault::at("recover:adopted");
        assert!(super::codex_direct::recover_pending(&fixture.state).is_err());
    }
    assert!(fixture.pending().is_some());
    assert_eq!(std::fs::read(&fixture.auth).unwrap(), before);
    // A restart rebuilds AppState and ProxyService around the same new manager Arc.
    fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
    let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
    assert_eq!(
        outcome,
        Some(crate::mode::operation::RecoveryOutcome::RolledForward)
    );
    assert_eq!(std::fs::read(&fixture.auth).unwrap(), before);
    fixture.assert_current("managed");
    assert!(fixture.pending().is_none());
}

fn catalog_fixture() -> (Fixture, super::codex_direct::CatalogRevision) {
    let fixture = Fixture::new();
    let mut provider = fixture
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    provider.settings_config["modelCatalog"] =
        json!({"models":[{"model":"synthetic-model","contextWindow":32768}]});
    fixture.state.db.save_provider("codex", &provider).unwrap();
    let original = std::fs::read_to_string(&fixture.config).unwrap();
    std::fs::write(
        &fixture.config,
        format!("model_catalog_json = \"unclaimed-external.json\" # external\n{original}"),
    )
    .unwrap();
    std::fs::write(
        fixture
            .config
            .parent()
            .unwrap()
            .join("unclaimed-external.json"),
        b"opaque external content",
    )
    .unwrap();
    let revision = super::codex_direct::CatalogRevision {
        app: "codex".into(),
        provider_id: "b".into(),
        config_digest: crate::live::engine::digest(
            read_current(&fixture.config).unwrap().as_deref(),
        ),
    };
    (fixture, revision)
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn catalog_restore_changes_only_the_pointer_under_a_fresh_revision() {
    let (fixture, revision) = catalog_fixture();
    super::codex_direct::manage_catalog(&fixture.state, &revision).unwrap();
    let mut restore_revision = super::codex_direct::CatalogRevision {
        app: "codex".into(),
        provider_id: "b".into(),
        config_digest: crate::live::engine::digest(
            read_current(&fixture.config).unwrap().as_deref(),
        ),
    };
    let text = std::fs::read_to_string(&fixture.config).unwrap();
    std::fs::write(&fixture.config, format!("# fresh external comment\n{text}")).unwrap();
    assert!(super::codex_direct::restore_catalog(&fixture.state, &restore_revision).is_err());
    restore_revision.config_digest =
        crate::live::engine::digest(read_current(&fixture.config).unwrap().as_deref());
    let before = super::codex_direct::files()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    super::codex_direct::restore_catalog(&fixture.state, &restore_revision).unwrap();
    for (index, file) in super::codex_direct::files().iter().enumerate() {
        if index != 1 {
            assert_eq!(read_current(&file.path).unwrap(), before[index]);
        }
    }
    let text = std::fs::read_to_string(&fixture.config).unwrap();
    assert!(text.starts_with("# fresh external comment\n"));
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    assert_eq!(
        doc["model_catalog_json"].as_str(),
        Some("unclaimed-external.json")
    );
    assert_eq!(
        std::fs::read(
            fixture
                .config
                .parent()
                .unwrap()
                .join("unclaimed-external.json")
        )
        .unwrap(),
        b"opaque external content"
    );
    fixture.assert_current("b");
    assert!(fixture.pending().is_none());
    let vault = fixture.state.db.secret_session().read().unwrap();
    assert!(state::written(&DeviceStore::for_device(), &vault, "codex")
        .unwrap()
        .unwrap()
        .codex
        .unwrap()
        .catalog
        .is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn catalog_takeover_faults_keep_original_pointer_evidence_through_recovery() {
    for point in ["pending", "published:1", "published:2", "target"] {
        let (fixture, revision) = catalog_fixture();
        {
            let _fault = Fault::at(point);
            assert!(super::codex_direct::manage_catalog(&fixture.state, &revision).is_err());
        }
        let pending = fixture.pending().unwrap();
        let catalog = pending
            .target
            .written
            .unwrap()
            .codex
            .unwrap()
            .catalog
            .unwrap();
        assert_eq!(catalog.previous_pointer, "unclaimed-external.json");
        assert_eq!(catalog.config_pre, revision.config_digest.unwrap());
        let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
        if point == "pending" {
            assert_eq!(
                outcome,
                Some(crate::mode::operation::RecoveryOutcome::Discarded)
            );
            fixture.assert_current("a");
        } else {
            assert_eq!(
                outcome,
                Some(crate::mode::operation::RecoveryOutcome::RolledForward)
            );
            fixture.assert_current("b");
            let vault = fixture.state.db.secret_session().read().unwrap();
            assert_eq!(
                state::written(&DeviceStore::for_device(), &vault, "codex")
                    .unwrap()
                    .unwrap()
                    .codex
                    .unwrap()
                    .catalog
                    .unwrap()
                    .previous_pointer,
                "unclaimed-external.json"
            );
        }
        assert_eq!(
            std::fs::read(
                fixture
                    .config
                    .parent()
                    .unwrap()
                    .join("unclaimed-external.json")
            )
            .unwrap(),
            b"opaque external content"
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn same_refresh_material_does_not_authorize_overwriting_a_newer_live_access_token() {
    let fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "known-access");
    ProviderService::switch(&fixture.state, AppType::Codex, "managed").unwrap();
    let mut external: Value =
        serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
    external["tokens"]["access_token"] = json!("newer-unknown-native-access");
    external["last_refresh"] =
        json!((chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc3339());
    crate::config::write_json_file_private(&fixture.auth, &external).unwrap();
    let before = std::fs::read(&fixture.auth).unwrap();
    assert!(
        ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err(),
        "same refresh/id must not conceal a later native access generation"
    );
    assert_eq!(std::fs::read(&fixture.auth).unwrap(), before);
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn managed_cross_account_publication_and_real_target_failures_recover() {
    for point in ["published:0", "published:3", "target", "database"] {
        let fixture = Fixture::new();
        seed_managed(&fixture, "managed-a", "account-a", "access-a");
        seed_managed(&fixture, "managed-b", "account-b", "access-b");
        ProviderService::switch(&fixture.state, AppType::Codex, "managed-a").unwrap();
        if point == "database" {
            fixture.state.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER synthetic_target_failure BEFORE UPDATE OF is_current ON providers WHEN NEW.app_type='codex' AND NEW.id='managed-b' AND NEW.is_current=1 BEGIN SELECT RAISE(ABORT, 'synthetic current failure'); END").unwrap();
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed-b").is_err());
            assert_eq!(
                crate::settings::get_current_provider(&AppType::Codex).as_deref(),
                Some("managed-b")
            );
            assert_eq!(
                fixture
                    .state
                    .db
                    .get_current_provider("codex")
                    .unwrap()
                    .as_deref(),
                Some("managed-a")
            );
            fixture
                .state
                .db
                .conn
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER synthetic_target_failure")
                .unwrap();
        } else {
            let _fault = Fault::at(point);
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed-b").is_err());
        }
        assert!(fixture.pending().is_some());
        let auth: Value = serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
        assert_eq!(
            auth["tokens"]["account_id"], "account-b",
            "old snapshot must not overwrite partial account B"
        );
        assert_eq!(
            super::codex_direct::recover_pending(&fixture.state).unwrap(),
            Some(crate::mode::operation::RecoveryOutcome::RolledForward)
        );
        fixture.assert_current("managed-b");
        let marker: Value = serde_json::from_slice(
            &std::fs::read(crate::codex_config::get_codex_managed_oauth_live_auth_marker_path())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["version"], 2);
        assert_eq!(marker["account_id"], "account-b");
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn ambiguous_cli_generations_and_missing_accounts_never_write() {
    for dated in [false, true] {
        let fixture = Fixture::new();
        seed_managed(&fixture, "managed", "account-a", "access-a");
        ProviderService::switch(&fixture.state, AppType::Codex, "managed").unwrap();
        let mut external: Value =
            serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
        external["tokens"]["refresh_token"] = json!("ambiguous-refresh");
        if !dated {
            external.as_object_mut().unwrap().remove("last_refresh");
        }
        crate::config::write_json_file_private(&fixture.auth, &external).unwrap();
        let before = std::fs::read(&fixture.auth).unwrap();
        for _ in 0..2 {
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
            assert_eq!(std::fs::read(&fixture.auth).unwrap(), before);
            assert!(fixture.pending().is_none());
        }
    }
    let fixture = Fixture::new();
    fixture
        .state
        .db
        .save_provider("codex", &managed_provider("missing", "not-present"))
        .unwrap();
    let before = read_current(&fixture.auth).unwrap();
    assert!(ProviderService::switch(&fixture.state, AppType::Codex, "missing").is_err());
    assert_eq!(read_current(&fixture.auth).unwrap(), before);
    fixture.assert_current("a");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn schema17_and_codex_image_keep_their_existing_paths() {
    let fixture = Fixture::with_schema(false);
    {
        let _fault = Fault::at("pending");
        ProviderService::switch(&fixture.state, AppType::Codex, "b").unwrap();
    }
    fixture.assert_current("b");
    assert!(fixture.pending().is_none());
    drop(fixture);
    let fixture = Fixture::new();
    let mut image = fixture
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    image.id = "image".into();
    fixture
        .state
        .db
        .save_provider("codex-image", &image)
        .unwrap();
    let before = super::codex_direct::files()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    ProviderService::switch(&fixture.state, AppType::CodexImage, "image").unwrap();
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("codex-image")
            .unwrap()
            .as_deref(),
        Some("image")
    );
    fixture.assert_current("a");
    assert_eq!(
        before,
        super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>()
    );
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn admission_failures_precede_even_credential_preparation() {
    for case in ["unknown", "checkpoint", "settings", "legacy"] {
        let fixture = Fixture::new();
        seed_managed(&fixture, "managed", "account-a", "access-a");
        crate::rt::block_on(fixture.state.codex_oauth_manager.test_refresh_next(
            "account-a",
            "must-not-resolve",
            "must-not-refresh",
        ));
        let store = DeviceStore::for_device();
        match case {
            "unknown" => {
                state::update(
                    &store,
                    &fixture.state.db.secret_session().read().unwrap(),
                    |live| {
                        live.apps.remove("codex");
                        Ok(())
                    },
                )
                .unwrap();
            }
            "checkpoint" => std::fs::write(
                store
                    .root()
                    .join(crate::secrets::owned_file::UPGRADE_CHECKPOINT_FILE),
                b"unknown checkpoint",
            )
            .unwrap(),
            "settings" => {
                std::fs::write(crate::settings::settings_path(), b"corrupt settings").unwrap();
                assert!(crate::settings::reload_settings().is_err());
            }
            "legacy" => {
                crate::rt::block_on(fixture.state.db.save_live_backup("codex", "{}")).unwrap();
                fixture.state.db.conn.lock().unwrap().execute("UPDATE proxy_live_backup SET original_config='corrupt-envelope' WHERE app_type='codex'", []).unwrap();
            }
            _ => unreachable!(),
        }
        let before = super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        assert!(
            ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err(),
            "{case}"
        );
        assert!(
            fixture.state.codex_oauth_manager.test_refresh_is_queued(),
            "{case} must stop before credential preparation"
        );
        assert_eq!(
            before,
            super::codex_direct::files()
                .iter()
                .map(|f| read_current(&f.path).unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider("codex")
                .unwrap()
                .as_deref(),
            Some("a")
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn catalog_choices_reject_wrong_revision_and_restore_faults_recover() {
    let (fixture, mut revision) = catalog_fixture();
    for wrong_app in [false, true] {
        revision.app = if wrong_app { "gemini" } else { "codex" }.into();
        let original_digest = revision.config_digest.clone();
        if !wrong_app {
            revision.config_digest = Some("0".repeat(64));
        }
        assert!(super::codex_direct::manage_catalog(&fixture.state, &revision).is_err());
        assert!(fixture.pending().is_none());
        fixture.assert_current("a");
        revision.config_digest = original_digest;
    }
    drop(fixture);
    for point in ["pending", "published:1", "target"] {
        let (fixture, revision) = catalog_fixture();
        super::codex_direct::manage_catalog(&fixture.state, &revision).unwrap();
        let revision = super::codex_direct::CatalogRevision {
            app: "codex".into(),
            provider_id: "b".into(),
            config_digest: crate::live::engine::digest(
                read_current(&fixture.config).unwrap().as_deref(),
            ),
        };
        {
            let _fault = Fault::at(point);
            assert!(super::codex_direct::restore_catalog(&fixture.state, &revision).is_err());
        }
        let outcome = super::codex_direct::recover_pending(&fixture.state).unwrap();
        assert_eq!(
            outcome,
            Some(if point == "pending" {
                crate::mode::operation::RecoveryOutcome::Discarded
            } else {
                crate::mode::operation::RecoveryOutcome::RolledForward
            })
        );
        fixture.assert_current("b");
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn logged_out_managed_marker_is_cleared_by_the_next_nonmanaged_transaction() {
    let fixture = Fixture::new();
    seed_managed(&fixture, "managed", "synthetic-account", "access-old");
    ProviderService::switch(&fixture.state, AppType::Codex, "managed").unwrap();
    let marker = crate::codex_config::get_codex_managed_oauth_live_auth_marker_path();
    let prior_marker = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&fixture.auth).unwrap(); // Actual CLI logout disposition.
    {
        let _fault = Fault::at("pending");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err());
    }
    let pending = fixture.pending().unwrap();
    assert!(
        pending.files[3].planned.is_none(),
        "missing auth's stale marker must be cleared inside the five-file intent"
    );
    assert!(
        marker.exists(),
        "intent publication cannot eagerly remove the marker"
    );
    super::codex_direct::recover_pending(&fixture.state).unwrap();
    ProviderService::switch(&fixture.state, AppType::Codex, "b").unwrap();
    assert!(!marker.exists());
    let native = crate::codex_config::codex_managed_oauth_auth_value(
        "other-native-account",
        "native-access",
        Some("native-id"),
        "native-refresh",
        &chrono::Utc::now().to_rfc3339(),
    );
    crate::config::write_json_file_private(&fixture.auth, &native).unwrap();
    ProviderService::switch(&fixture.state, AppType::Codex, "a").unwrap();
    assert_eq!(
        crate::config::read_json_file::<Value>(&fixture.auth).unwrap(),
        native
    );
    fixture.assert_current("a");
    // An existing auth with a contradictory marker still blocks before intent.
    std::fs::write(&marker, prior_marker).unwrap();
    assert!(ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err());
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn outgoing_provider_owned_catalog_moves_with_provider_but_unclaimed_pointer_is_preserved() {
    for filename in [
        "unclaimed-external.json",
        "loongport-model-catalog.json",
        "cc-switch-model-catalog.json",
    ] {
        for claimed in [true, false] {
            let (fixture, _) = catalog_fixture();
            let relative = filename == "unclaimed-external.json";
            let external = if relative {
                fixture.config.parent().unwrap().join(filename)
            } else {
                fixture._home.path().join(filename)
            };
            let pointer = if relative {
                filename
            } else {
                external.to_str().unwrap()
            };
            let encoded = toml_edit::Value::from(pointer);
            let bytes = b"opaque external content";
            std::fs::write(&external, bytes).unwrap();
            let mut live: toml_edit::DocumentMut = std::fs::read_to_string(&fixture.config)
                .unwrap()
                .parse()
                .unwrap();
            live["model_catalog_json"] = toml_edit::value(pointer);
            std::fs::write(&fixture.config, live.to_string()).unwrap();
            let mut owner = fixture
                .state
                .db
                .get_provider_by_id("a", "codex")
                .unwrap()
                .unwrap();
            let config = owner.settings_config["config"].as_str().unwrap().to_owned();
            owner.settings_config["config"] =
                json!(format!("model_catalog_json = {encoded}\n{config}"));
            if claimed {
                fixture.state.db.save_provider("codex", &owner).unwrap();
                ProviderService::switch(&fixture.state, AppType::Codex, "a").unwrap();
                let applied: toml_edit::DocumentMut = std::fs::read_to_string(&fixture.config)
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(applied["model_catalog_json"].as_str(), Some(pointer));
            } else {
                // A different database row claiming the same pointer is not current ownership.
                owner.id = "not-current".into();
                fixture.state.db.save_provider("codex", &owner).unwrap();
            }
            ProviderService::switch(&fixture.state, AppType::Codex, "b").unwrap();
            let doc: toml_edit::DocumentMut = std::fs::read_to_string(&fixture.config)
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                doc["model_catalog_json"].as_str(),
                Some(if claimed {
                    crate::live::project::codex::CATALOG_FILENAME
                } else {
                    pointer
                }),
                "only the actual outgoing row proves ownership"
            );
            assert_eq!(std::fs::read(external).unwrap(), bytes);
            fixture.assert_current("b");
        }
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_review_cases() -> Result<(), AppError> {
    codex_unclaimed_external_catalog_is_preserved_by_default();
    proxy_native_auth_and_no_current_exit_preserve_unowned_bytes();
    let marker = std::panic::catch_unwind(
        logged_out_managed_marker_is_cleared_by_the_next_nonmanaged_transaction,
    );
    let catalog = std::panic::catch_unwind(
        outgoing_provider_owned_catalog_moves_with_provider_but_unclaimed_pointer_is_preserved,
    );
    let auth_store = std::panic::catch_unwind(
        auth_store_semantics_cannot_authorize_unsupported_file_publication,
    );
    let staging =
        std::panic::catch_unwind(recovery_rejects_unowned_stage_name_before_reading_its_bytes);
    assert!(
        marker.is_ok() && catalog.is_ok() && auth_store.is_ok() && staging.is_ok(),
        "Codex reviewed marker and catalog ownership contracts"
    );
    println!("PASS Codex logout marker cleanup, outgoing catalog ownership and auth-store types");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn auth_store_semantics_cannot_authorize_unsupported_file_publication() {
    use crate::codex_config::{codex_config_auth_store_mode as mode, CodexAuthStoreMode};
    assert_eq!(mode(""), CodexAuthStoreMode::File);
    for (value, expected) in [
        ("file", CodexAuthStoreMode::File),
        ("keyring", CodexAuthStoreMode::Keyring),
        ("auto", CodexAuthStoreMode::Auto),
        ("ephemeral", CodexAuthStoreMode::Ephemeral),
        ("future", CodexAuthStoreMode::Unknown),
    ] {
        assert_eq!(
            mode(&format!("cli_auth_credentials_store = {value:?}")),
            expected
        );
    }
    for raw in [
        "cli_auth_credentials_store = 123",
        "cli_auth_credentials_store = []",
        "cli_auth_credentials_store = {}",
        "cli_auth_credentials_store = false",
        r#""\u0063li_auth_credentials_store" = "keyring""#,
    ] {
        let fixture = Fixture::new();
        seed_managed(&fixture, "managed", "synthetic-account", "synthetic-access");
        let config = std::fs::read_to_string(&fixture.config).unwrap();
        std::fs::write(&fixture.config, format!("{raw}\n{config}")).unwrap();
        let before = super::codex_direct::files()
            .iter()
            .map(|file| read_current(&file.path).unwrap())
            .collect::<Vec<_>>();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let result = ProviderService::switch(&fixture.state, AppType::Codex, "managed");
        assert!(
            matches!(result, Err(AppError::Config(ref code)) if code == "codex.auth_store_unavailable"),
            "unsupported auth store must not grant file publication: {raw}: {result:?}"
        );
        assert_eq!(
            super::codex_direct::files()
                .iter()
                .map(|file| read_current(&file.path).unwrap())
                .collect::<Vec<_>>(),
            before
        );
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        assert!(fixture.pending().is_none());
        fixture.assert_current("a");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn recovery_rejects_unowned_stage_name_before_reading_its_bytes() {
    for same_name_elsewhere in [false, true] {
        let fixture = Fixture::new();
        {
            let _fault = Fault::at("pending");
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err());
        }
        let unrelated = if same_name_elsewhere {
            fixture
                ._home
                .path()
                .join("other-private-parent/config.toml.tmp.1.2.3")
        } else {
            fixture.config.parent().unwrap().join("unrelated.json")
        };
        crate::config_file_io::write_durable(&unrelated, b"").unwrap();
        // A sparse, private temporary fixture: metadata alone rejects a byte read.
        // The expected error proves path ownership was checked before that reader.
        let size = 32 * 1024 * 1024 + 1;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&unrelated)
            .unwrap()
            .set_len(size)
            .unwrap();
        let store = DeviceStore::for_device();
        state::update(
            &store,
            &fixture.state.db.secret_session().read().unwrap(),
            |live| {
                live.apps
                    .get_mut("codex")
                    .unwrap()
                    .pending
                    .as_mut()
                    .unwrap()
                    .files[1]
                    .staged = Some(unrelated.clone());
                Ok(())
            },
        )
        .unwrap();
        let journal = std::fs::read(store.state_path()).unwrap();
        let before = super::codex_direct::files()
            .iter()
            .map(|file| read_current(&file.path).unwrap())
            .collect::<Vec<_>>();
        let result = super::codex_direct::recover_pending(&fixture.state);
        assert!(
            matches!(result, Err(AppError::Config(ref code)) if code == "live.invalid_pending"),
            "unowned stage must be rejected before invoking its byte reader: {result:?}"
        );
        assert_eq!(std::fs::metadata(unrelated).unwrap().len(), size);
        assert_eq!(std::fs::read(store.state_path()).unwrap(), journal);
        assert_eq!(
            super::codex_direct::files()
                .iter()
                .map(|file| read_current(&file.path).unwrap())
                .collect::<Vec<_>>(),
            before
        );
        fixture.assert_current("a");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn ordinary_refresh_waits_for_pending_operation_then_resumes() {
    for point in ["published:1", "pending", "target"] {
        let fixture = Fixture::new();
        let account = "synthetic-account";
        seed_managed(&fixture, "managed-a", account, "access-old");
        crate::rt::block_on(
            fixture
                .state
                .codex_oauth_manager
                .test_set_bundle_time(account, chrono::Utc::now().timestamp_millis() - 10_000),
        );
        ProviderService::switch(&fixture.state, AppType::Codex, "managed-a").unwrap();
        let mut target = managed_provider("managed-b", account);
        target.settings_config["config"] =
            json!("model = \"gpt-5.5\"\nmodel_reasoning_effort = \"low\"\n");
        fixture.state.db.save_provider("codex", &target).unwrap();
        {
            let _fault = Fault::at(point);
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed-b").is_err());
        }
        let pending = fixture
            .pending()
            .expect("the requested real fault must leave an intent");
        let config = pending
            .files
            .iter()
            .find(|f| f.path == fixture.config)
            .unwrap();
        assert_ne!(
            config.pre, config.planned,
            "this is a real configuration change"
        );
        assert_eq!(
            crate::live::engine::digest(read_current(&fixture.config).unwrap().as_deref()),
            if point == "pending" {
                config.pre.clone()
            } else {
                config.planned.clone()
            }
        );
        let before = super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        let journal = read_current(&DeviceStore::for_device().state_path()).unwrap();
        assert_eq!(
            crate::rt::block_on(
                fixture
                    .state
                    .codex_oauth_manager
                    .get_valid_token_for_account(account)
            )
            .unwrap(),
            "access-old"
        );
        assert_eq!(
            read_current(&DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        crate::rt::block_on(fixture.state.codex_oauth_manager.test_refresh_next(
            account,
            "access-refreshed",
            "refresh-refreshed",
        ));
        let result = crate::rt::block_on(
            fixture
                .state
                .codex_oauth_manager
                .get_valid_token_for_account(account),
        );
        assert!(
            super::codex_direct::files()
                .iter()
                .zip(&before)
                .all(|(file, bytes)| read_current(&file.path).unwrap() == *bytes),
            "ordinary refresh must not publish through a pending operation ({point}): {result:?}"
        );
        assert_eq!(
            read_current(&DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        assert!(
            result.is_err(),
            "an expired generation waits for explicit operation recovery"
        );
        assert!(
            fixture.state.codex_oauth_manager.test_refresh_is_queued(),
            "pending admission precedes network refresh"
        );
        super::codex_direct::recover_pending(&fixture.state).unwrap();
        assert!(
            fixture.pending().is_none(),
            "the retained original generation remains recoverable"
        );
        assert_eq!(
            crate::rt::block_on(
                fixture
                    .state
                    .codex_oauth_manager
                    .get_valid_token_for_account(account)
            )
            .unwrap(),
            "access-refreshed"
        );
        assert!(!fixture.state.codex_oauth_manager.test_refresh_is_queued());
        let auth: Value = serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
        assert_eq!(auth["tokens"]["access_token"], "access-refreshed");
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_pending_refresh() -> Result<(), AppError> {
    automatic_live_preparation_cannot_adopt_through_pending();
    catalog_and_target_only_intents_wait_for_inflight_refresh();
    ordinary_refresh_waits_for_pending_operation_then_resumes();
    new_live_intent_waits_for_every_in_flight_manager_refresh();
    another_apps_pending_does_not_block_codex_refresh();
    println!("PASS ordinary Codex refresh waits for pending recovery and resumes");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn new_live_intent_waits_for_every_in_flight_manager_refresh() {
    let fixture = Fixture::new();
    let account = "unselected-synthetic-account";
    seed_managed(&fixture, "unselected", account, "access-old");
    crate::rt::block_on(fixture.state.codex_oauth_manager.test_refresh_next(
        account,
        "access-refreshed",
        "refresh-refreshed",
    ));
    let manager = fixture.state.codex_oauth_manager.clone();
    let (started, release) = manager.test_pause_next_refresh();
    crate::rt::block_on(async {
        let refreshing = manager.clone();
        let task =
            tokio::spawn(async move { refreshing.get_valid_token_for_account(account).await });
        started.await.unwrap();
        // A non-managed target has no selected account ids. Its intent must still
        // wait for a refresh already in flight under the same existing manager.
        let guard = manager.with_live_auth_guard(&[], |_| {
            let _fault = Fault::at("pending");
            let vault = fixture.state.db.secret_session().read().unwrap();
            let app_guard = crate::live::engine::lock_app("codex");
            let patch =
                crate::live::patch::WholeFile::Write(b"model = \"synthetic-next\"\n".to_vec());
            assert!(crate::mode::operation::run(
                &DeviceStore::for_device(),
                &vault,
                &app_guard,
                state::op::APPLY,
                &[crate::mode::operation::FileChange {
                    file: crate::live::engine::LiveFile::private(&fixture.config),
                    patch: &patch
                }],
                Default::default(),
                &|_| Ok(()),
            )
            .is_err());
            Ok(())
        });
        futures::pin_mut!(guard);
        let first = futures::poll!(&mut guard);
        let intent_before_refresh = fixture.pending().is_some();
        release.send(()).unwrap();
        assert_eq!(task.await.unwrap().unwrap(), "access-refreshed");
        assert!(
            !intent_before_refresh,
            "intent must wait until the in-flight generation settles"
        );
        match first {
            std::task::Poll::Pending => guard.await.unwrap(),
            std::task::Poll::Ready(result) => {
                result.unwrap();
                panic!("new live intent passed an already in-flight account refresh");
            }
        }
        assert!(
            fixture.pending().is_some(),
            "the real operation starts after refresh has settled"
        );
    });
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn another_apps_pending_does_not_block_codex_refresh() {
    let fixture = Fixture::new();
    let account = "synthetic-account";
    seed_managed(&fixture, "managed", account, "access-old");
    ProviderService::switch(&fixture.state, AppType::Codex, "managed").unwrap();
    let store = DeviceStore::for_device();
    {
        let _fault = Fault::at("pending");
        let vault = fixture.state.db.secret_session().read().unwrap();
        let guard = crate::live::engine::lock_app("claude");
        let patch = crate::live::patch::WholeFile::Write(b"{}".to_vec());
        assert!(crate::mode::operation::run(
            &store,
            &vault,
            &guard,
            state::op::APPLY,
            &[crate::mode::operation::FileChange {
                file: crate::live::engine::LiveFile::private(
                    crate::config::get_claude_settings_path()
                ),
                patch: &patch
            }],
            Default::default(),
            &|_| Ok(()),
        )
        .is_err());
    }
    let journal = read_current(&store.state_path()).unwrap();
    assert!(state::pending(
        &store,
        &fixture.state.db.secret_session().read().unwrap(),
        "claude"
    )
    .unwrap()
    .is_some());
    crate::rt::block_on(fixture.state.codex_oauth_manager.test_refresh_next(
        account,
        "access-refreshed",
        "refresh-refreshed",
    ));
    assert_eq!(
        crate::rt::block_on(
            fixture
                .state
                .codex_oauth_manager
                .get_valid_token_for_account(account)
        )
        .unwrap(),
        "access-refreshed"
    );
    assert!(!fixture.state.codex_oauth_manager.test_refresh_is_queued());
    assert_eq!(read_current(&store.state_path()).unwrap(), journal);
    let auth: Value = serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
    assert_eq!(auth["tokens"]["access_token"], "access-refreshed");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn proxy_native_auth_and_no_current_exit_preserve_unowned_bytes() {
    for claimed in [true, false] {
        let fixture = Fixture::new();
        let external = fixture._home.path().join("loongport-model-catalog.json");
        std::fs::write(&external, b"opaque external catalog").unwrap();
        let pointer = external.to_str().unwrap();
        let mut route = fixture
            .state
            .db
            .get_provider_by_id("b", "codex")
            .unwrap()
            .unwrap();
        let encoded = toml_edit::Value::from(pointer);
        if claimed {
            route.settings_config["config"] = json!(format!(
                "model_catalog_json = {encoded}\n{}",
                route.settings_config["config"].as_str().unwrap()
            ));
            fixture.state.db.save_provider("codex", &route).unwrap();
        } else {
            let original = std::fs::read_to_string(&fixture.config).unwrap();
            std::fs::write(
                &fixture.config,
                format!("model_catalog_json = {encoded}\n{original}"),
            )
            .unwrap();
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let native = read_current(&fixture.auth).unwrap();
        runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("codex", true),
            )
            .unwrap();
        assert_eq!(
            read_current(&fixture.auth).unwrap(),
            native,
            "third-party proxy must not replace native auth with row key"
        );
        runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .switch_proxy_target("codex", "b"),
            )
            .unwrap();
        assert_eq!(read_current(&fixture.auth).unwrap(), native);
        let config = std::fs::read_to_string(&fixture.config).unwrap();
        let doc = config.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["model_catalog_json"].as_str(), Some(pointer));
        assert_eq!(doc["model"].as_str(), Some("model-b"));
        assert_eq!(
            doc["model_providers"]["custom"]
                .get("requires_openai_auth")
                .and_then(toml_edit::Item::as_bool),
            Some(true),
            "native login display and refresh must remain enabled beside proxy bearer auth"
        );
        assert!(doc["model_providers"]["custom"]["base_url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"));
        assert!(config.contains("keep = \"exact\" # untouched"));
        fixture.assert_current("a");
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE providers SET is_current=0 WHERE app_type='codex'",
                [],
            )
            .unwrap();
        crate::settings::set_current_provider(&AppType::Codex, None).unwrap();
        runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("codex", false),
            )
            .unwrap();
        assert_eq!(read_current(&fixture.auth).unwrap(), native);
        assert!(std::fs::read_to_string(&fixture.config)
            .unwrap()
            .contains("keep = \"exact\" # untouched"));
        let after: toml_edit::DocumentMut = std::fs::read_to_string(&fixture.config)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            after
                .get("model_catalog_json")
                .and_then(toml_edit::Item::as_str),
            if claimed { None } else { Some(pointer) },
            "only the actual outgoing route owns the catalog pointer without a direct provider"
        );
        assert_eq!(
            std::fs::read(&external).unwrap(),
            b"opaque external catalog"
        );
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn proxy_recovery_checks_managed_generation_before_committing_route() {
    let fixture = Fixture::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("codex", true),
        )
        .unwrap();
    seed_managed(&fixture, "managed", "synthetic-proxy-account", "access-old");
    {
        let _fault = Fault::at("published:1");
        assert!(runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .switch_proxy_target("codex", "managed")
            )
            .is_err());
    }
    assert!(fixture.pending().is_some());
    seed_managed(
        &fixture,
        "managed",
        "synthetic-proxy-account",
        "access-new-generation",
    );
    let before = super::codex_direct::files()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    let outcome =
        crate::mode::controller::recover_locked(&fixture.state.proxy_service, &AppType::Codex)
            .unwrap();
    assert!(
        matches!(
            outcome,
            Some(crate::mode::operation::RecoveryOutcome::VerificationRequired { .. })
        ),
        "generic recovery must not authorize an obsolete managed generation: {outcome:?}"
    );
    assert!(fixture.pending().is_some());
    assert_eq!(
        before,
        super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>()
    );
    fixture.assert_current("a");
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_proxy_mode() -> Result<(), AppError> {
    proxy_native_auth_store_modes_follow_original_lookup_semantics();
    proxy_native_auth_and_no_current_exit_preserve_unowned_bytes();
    managed_proxy_faults_restart_through_original_recovery();
    codex_target_only_recovery_needs_no_managed_account_or_auth_journal();
    println!("PASS Codex managed proxy enter/route/save/exit restart fault matrix and zero-file recovery");
    println!("PASS Codex proxy preserves native auth and no-current exit");
    proxy_recovery_checks_managed_generation_before_committing_route();
    println!("PASS Codex proxy recovery checks managed generation");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn automatic_live_preparation_cannot_adopt_through_pending() {
    for switch_away in [true, false] {
        let fixture = Fixture::new();
        let account = "synthetic-pending-adoption";
        seed_managed(&fixture, "managed", account, "access-old");
        crate::rt::block_on(
            fixture
                .state
                .codex_oauth_manager
                .test_set_bundle_time(account, chrono::Utc::now().timestamp_millis() - 10_000),
        );
        ProviderService::switch(&fixture.state, AppType::Codex, "managed").unwrap();
        let old: Value = serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
        {
            let _fault = Fault::at("pending");
            assert!(ProviderService::switch(&fixture.state, AppType::Codex, "managed").is_err());
        }
        let mut external = old.clone();
        external["tokens"]["refresh_token"] = json!("newer-native-refresh");
        external["last_refresh"] = json!(chrono::Utc::now().to_rfc3339());
        std::fs::write(&fixture.auth, serde_json::to_vec(&external).unwrap()).unwrap();
        if switch_away {
            let _ = super::live::prepare_codex_managed_oauth_live_auth_switch_away(
                fixture.state.codex_oauth_manager.clone(),
                account.into(),
            );
        } else {
            let _ = super::live::get_codex_managed_oauth_live_auth_value(
                fixture.state.codex_oauth_manager.clone(),
                account.into(),
            );
        }
        crate::rt::block_on(fixture.state.codex_oauth_manager.with_live_auth_guard(&[account.into()], |generation| {
            assert!(generation.matches_live_generation(account, &old), "automatic helper must not adopt native generation through pending (switch_away={switch_away})");
            Ok(())
        })).unwrap();
        assert!(fixture.pending().is_some());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn catalog_and_target_only_intents_wait_for_inflight_refresh() {
    for restore in [false, true] {
        let (fixture, revision) = if restore {
            let (fixture, revision) = catalog_fixture();
            super::codex_direct::manage_catalog(&fixture.state, &revision).unwrap();
            let revision = super::codex_direct::CatalogRevision {
                config_digest: crate::live::engine::digest(
                    read_current(&fixture.config).unwrap().as_deref(),
                ),
                ..revision
            };
            (fixture, Some(revision))
        } else {
            (Fixture::new(), None)
        };
        let account = "synthetic-unselected-refresh";
        seed_managed(&fixture, "unselected", account, "access-old");
        let manager = fixture.state.codex_oauth_manager.clone();
        crate::rt::block_on(manager.test_refresh_next(account, "access-new", "refresh-new"));
        let (started, release) = manager.test_pause_next_refresh();
        crate::rt::block_on(async {
            let refreshing = manager.clone();
            let refresh =
                tokio::spawn(async move { refreshing.get_valid_token_for_account(account).await });
            started.await.unwrap();
            let (early, pending) = std::thread::scope(|scope| {
                let (started_tx, started_rx) = std::sync::mpsc::channel();
                let (done_tx, done_rx) = std::sync::mpsc::channel();
                let fixture = &fixture;
                let operation = scope.spawn(move || {
                    let _fault = Fault::at("pending");
                    started_tx.send(()).unwrap();
                    let result = if let Some(revision) = revision {
                        super::codex_direct::restore_catalog(&fixture.state, &revision)
                    } else {
                        let mut row = fixture
                            .state
                            .db
                            .get_provider_by_id("b", "codex")
                            .unwrap()
                            .unwrap();
                        row.name = "updated inactive row".into();
                        ProviderService::update(&fixture.state, AppType::Codex, None, row)
                            .map(|_| ())
                    };
                    done_tx.send(()).unwrap();
                    assert!(result.is_err(), "fault should retain the real operation");
                });
                started_rx.recv().unwrap();
                let early = done_rx
                    .recv_timeout(std::time::Duration::from_millis(200))
                    .is_ok();
                let pending = fixture.pending().is_some();
                release.send(()).unwrap();
                operation.join().unwrap();
                (early, pending)
            });
            assert_eq!(refresh.await.unwrap().unwrap(), "access-new");
            assert!(
                !early && !pending,
                "actual intent passed an in-flight refresh (restore={restore})"
            );
            assert!(fixture.pending().is_some());
        });
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn managed_proxy_faults_restart_through_original_recovery() {
    for action in ["enter", "route", "save", "exit"] {
        for point in ["pending", "published:1", "target"] {
            let point = if action == "route" && point == "published:1" {
                "published:0"
            } else {
                point
            };
            let mut fixture = Fixture::new();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            seed_managed(&fixture, "managed-a", "synthetic-account-a", "access-a");
            seed_managed(&fixture, "managed-b", "synthetic-account-b", "access-b");
            // The test helper creates its cached token before inserting the
            // account. Pin one coherent synthetic generation after both inserts.
            let observed = chrono::Utc::now().timestamp_millis();
            for account in ["synthetic-account-a", "synthetic-account-b"] {
                crate::rt::block_on(
                    fixture
                        .state
                        .codex_oauth_manager
                        .test_set_bundle_time(account, observed),
                );
            }
            ProviderService::switch(&fixture.state, AppType::Codex, "managed-a").unwrap();
            if action != "enter" {
                runtime
                    .block_on(
                        fixture
                            .state
                            .proxy_service
                            .set_takeover_for_app("codex", true),
                    )
                    .unwrap();
            }
            if action == "exit" {
                runtime
                    .block_on(
                        fixture
                            .state
                            .proxy_service
                            .switch_proxy_target("codex", "managed-b"),
                    )
                    .unwrap();
            }
            let result = {
                let _fault = Fault::at(point);
                match action {
                    "enter" => runtime.block_on(
                        fixture
                            .state
                            .proxy_service
                            .set_takeover_for_app("codex", true),
                    ),
                    "route" => runtime.block_on(
                        fixture
                            .state
                            .proxy_service
                            .switch_proxy_target("codex", "managed-b"),
                    ),
                    "exit" => runtime.block_on(
                        fixture
                            .state
                            .proxy_service
                            .set_takeover_for_app("codex", false),
                    ),
                    "save" => {
                        let mut edited = managed_provider("managed-a", "synthetic-account-b");
                        edited.settings_config["config"] = json!(
                            "model = \"changed-route-model\"\nmodel_reasoning_effort = \"low\"\n"
                        );
                        ProviderService::update(&fixture.state, AppType::Codex, None, edited)
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    }
                    _ => unreachable!(),
                }
            };
            assert!(result.is_err(), "{action}/{point}");
            let pending = fixture.pending().expect("real fault must retain intent");
            assert_eq!(pending.files.len(), 5);
            // Stop only the test listener, then recreate original AppState/service
            // and manager together, as a real process restart does.
            runtime
                .block_on(fixture.state.proxy_service.stop())
                .unwrap();
            fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
            let outcome = crate::mode::controller::recover_locked(
                &fixture.state.proxy_service,
                &AppType::Codex,
            )
            .unwrap();
            assert_eq!(
                outcome,
                Some(if point == "pending" {
                    crate::mode::operation::RecoveryOutcome::Discarded
                } else {
                    crate::mode::operation::RecoveryOutcome::RolledForward
                }),
                "{action}/{point}"
            );
            assert!(fixture.pending().is_none(), "{action}/{point}");
            fixture.assert_current("managed-a");
            if point != "pending" {
                let auth: Value =
                    serde_json::from_slice(&std::fs::read(&fixture.auth).unwrap()).unwrap();
                let expected = if action == "route" || action == "save" {
                    "synthetic-account-b"
                } else {
                    "synthetic-account-a"
                };
                assert_eq!(auth["tokens"]["account_id"], expected, "{action}/{point}");
                let mode = state::load(
                    &DeviceStore::for_device(),
                    &fixture.state.db.secret_session().read().unwrap(),
                )
                .unwrap();
                assert_eq!(
                    mode.apps["codex"].mode,
                    Some(if action == "exit" {
                        Mode::Direct
                    } else {
                        Mode::Proxy
                    })
                );
            }
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_target_only_recovery_needs_no_managed_account_or_auth_journal() {
    for point in ["pending", "target"] {
        let fixture = Fixture::new();
        // No manager account exists: this is an inactive row save, not auth work.
        fixture
            .state
            .db
            .save_provider("codex", &managed_provider("a", "missing-account"))
            .unwrap();
        let files = super::codex_direct::files()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        {
            let mut row = fixture
                .state
                .db
                .get_provider_by_id("b", "codex")
                .unwrap()
                .unwrap();
            row.name = "saved while inactive".into();
            let _fault = Fault::at(point);
            assert!(ProviderService::update(&fixture.state, AppType::Codex, None, row).is_err());
        }
        assert!(fixture.pending().unwrap().files.is_empty());
        let outcome =
            crate::mode::controller::recover_locked(&fixture.state.proxy_service, &AppType::Codex)
                .unwrap();
        assert_eq!(
            outcome,
            Some(if point == "pending" {
                crate::mode::operation::RecoveryOutcome::Discarded
            } else {
                crate::mode::operation::RecoveryOutcome::RolledForward
            })
        );
        assert!(fixture.pending().is_none());
        assert_eq!(
            files,
            super::codex_direct::files()
                .iter()
                .map(|f| read_current(&f.path).unwrap())
                .collect::<Vec<_>>()
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn proxy_native_auth_store_modes_follow_original_lookup_semantics() {
    for (mode, present, expected) in [
        ("keyring", false, true),
        ("auto", false, true),
        ("future-store", false, true),
        ("ephemeral", true, false),
    ] {
        let fixture = Fixture::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        if !present {
            std::fs::remove_file(&fixture.auth).unwrap();
        }
        let auth = read_current(&fixture.auth).unwrap();
        let before = std::fs::read_to_string(&fixture.config).unwrap();
        std::fs::write(
            &fixture.config,
            format!("cli_auth_credentials_store = \"{mode}\"\n{before}"),
        )
        .unwrap();
        runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("codex", true),
            )
            .unwrap();
        let config = std::fs::read_to_string(&fixture.config)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(
            config["model_providers"]["custom"]
                .get("requires_openai_auth")
                .and_then(toml_edit::Item::as_bool),
            Some(expected),
            "{mode}"
        );
        assert_eq!(read_current(&fixture.auth).unwrap(), auth);
        assert_eq!(config["cli_auth_credentials_store"].as_str(), Some(mode));
    }
}
