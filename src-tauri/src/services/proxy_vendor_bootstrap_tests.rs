//! Unexecuted regression specification for explicit empty-app VendorApiKey ENTER.
//! Register under cfg(all(test, feature = "gui")); reuse the original sealed
//! OpenCode fixture, real secret session/database, service, journal and router.
//! No real account, provisioning command, upstream HTTP or native user home.
use crate::{
    app_config::AppType,
    live::engine::DeviceStore,
    mode::{
        controller::InitialProxyRoute,
        current, operation,
        state::{self, Mode},
    },
    provider::{AuthBindingSource, Provider},
    proxy::{application_routing, opencode_vendor_fixture::FixtureOwner},
    services::application_selection,
};

type DbImage = Vec<(String, Vec<Vec<rusqlite::types::Value>>)>;

struct Fixture {
    app: AppType,
    owner: FixtureOwner,
}
impl Fixture {
    async fn new(app: AppType) -> Self {
        let owner = FixtureOwner::with_uninitialized_chain();
        if app == AppType::Claude {
            // The original fixture seeds credential-free Claude source rows and
            // binds Codex references. Rebind only those synthetic Claude rows
            // through the same original owner, using the existing Codex sources.
            let db = &owner.state().db;
            for id in owner.ids() {
                db.conn
                    .lock()
                    .unwrap()
                    .execute(
                        "DELETE FROM providers WHERE id=?1 AND app_type='claude'",
                        [id],
                    )
                    .unwrap();
                assert!(
                    crate::services::vendor_source_binding::bind_vendor_source(db, id, &app)
                        .unwrap()
                );
            }
        }
        let mut config = owner.state().db.get_proxy_config().await.unwrap();
        config.listen_address = "127.0.0.1".into();
        config.listen_port = 0;
        owner.state().db.update_proxy_config(config).await.unwrap();
        Self { app, owner }
    }
    fn row(&self, index: usize) -> Provider {
        self.owner
            .state()
            .db
            .get_provider_by_id(&self.owner.ids()[index], self.app.as_str())
            .unwrap()
            .unwrap()
    }
    fn save(&self, row: &Provider) {
        self.owner
            .state()
            .db
            .save_provider(self.app.as_str(), row)
            .unwrap();
    }
    async fn try_prepare(&self, id: &str) -> Result<InitialProxyRoute, String> {
        Ok(InitialProxyRoute {
            provider_id: id.into(),
            request: self
                .owner
                .state()
                .proxy_service
                .prepare_proxy_takeover_request(
                    self.app.as_str(),
                    id,
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await?,
        })
    }
    async fn prepare(&self, index: usize) -> InitialProxyRoute {
        self.try_prepare(&self.owner.ids()[index]).await.unwrap()
    }
    async fn apply(&self, initial: InitialProxyRoute) -> Result<(), String> {
        self.owner
            .state()
            .proxy_service
            .set_takeover_for_app_with_initial_route(self.app.as_str(), true, Some(initial))
            .await
    }
    fn bytes(&self) -> Vec<Option<Vec<u8>>> {
        crate::mode::controller::files(&self.app)
            .unwrap()
            .iter()
            .map(|file| std::fs::read(&file.path).ok())
            .collect()
    }
    fn mode(&self) -> state::ModeState {
        current::validate_known_mode(
            &DeviceStore::for_device(),
            &self.owner.state().db.secret_session().read().unwrap(),
            &self.app,
        )
        .unwrap()
    }
    fn receipt(&self, initial: &InitialProxyRoute) -> String {
        application_selection::query_routing_request(
            self.owner.state(),
            &self.app,
            initial.request.clone(),
        )
        .unwrap()
        .status
    }
    fn inventory(&self) -> DbImage {
        let conn = self.owner.state().db.conn.lock().unwrap();
        ["loongport_vendor", "providers"]
            .into_iter()
            .map(|table| {
                let mut statement = conn
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .unwrap();
                let columns = statement.column_count();
                let rows = statement
                    .query_map([], |row| {
                        (0..columns).map(|column| row.get(column)).collect()
                    })
                    .unwrap()
                    .collect::<Result<Vec<Vec<rusqlite::types::Value>>, _>>()
                    .unwrap();
                (table.into(), rows)
            })
            .collect()
    }
    fn assert_empty(&self) {
        let mode = self.mode();
        assert_eq!(mode.mode, Some(Mode::Direct));
        assert!(!mode.attached);
        assert!(mode.proxy_route.is_none());
        assert_eq!(
            self.owner
                .state()
                .db
                .get_current_provider(self.app.as_str())
                .unwrap(),
            None
        );
        assert_eq!(current::local_direct_pointer(&self.app).unwrap(), None);
        assert!(state::pending(
            &DeviceStore::for_device(),
            &self.owner.state().db.secret_session().read().unwrap(),
            self.app.as_str()
        )
        .unwrap()
        .is_none());
    }
    fn assert_no_key_projection(&self) {
        let rows = self
            .owner
            .state()
            .db
            .get_all_providers(self.app.as_str())
            .unwrap();
        let inventory = serde_json::to_string(&rows).unwrap();
        for i in 0..2 {
            assert!(!inventory.contains(self.owner.synthetic_key(i)));
            for bytes in self.bytes().into_iter().flatten() {
                assert!(!String::from_utf8_lossy(&bytes).contains(self.owner.synthetic_key(i)));
            }
        }
    }
    fn clear_key(&self, index: usize) {
        let row = self.row(index);
        let id = row
            .meta
            .unwrap()
            .auth_binding
            .unwrap()
            .account_id
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let vault = self.owner.state().db.secret_session().read().unwrap();
        let conn = self.owner.state().db.conn.lock().unwrap();
        crate::vendor::creds::set_api_key(&conn, &vault, id, "").unwrap();
    }
    async fn stop(&self) {
        self.owner
            .state()
            .proxy_service
            .set_takeover_for_app(self.app.as_str(), false)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_first_enter_preserves_key_owner_and_uses_original_router() {
    for app in [AppType::Claude, AppType::Codex] {
        let f = Fixture::new(app.clone()).await;
        f.assert_empty();
        let row = f.row(0);
        assert_eq!(
            row.meta
                .as_ref()
                .unwrap()
                .auth_binding
                .as_ref()
                .unwrap()
                .source,
            AuthBindingSource::VendorApiKey
        );
        assert!(f.owner.state().db.resolve_vendor_auth(&app, &row).is_ok());
        let inventory = f.inventory();
        let native = f.bytes();
        let chain = application_routing::chain_ids(&f.owner.state().db, app.as_str()).unwrap();
        let initial = f.prepare(0).await;
        // Preparation must not provision, publish an intent, select or start.
        assert_eq!(f.receipt(&initial), "notRecorded");
        assert_eq!(f.inventory(), inventory);
        assert_eq!(f.bytes(), native);
        f.assert_empty();
        assert!(!f.owner.state().proxy_service.is_running().await);
        f.apply(initial.clone()).await.unwrap();
        assert_eq!(f.receipt(&initial), "completed");
        assert!(f.mode().routes_to(&row.id));
        assert_eq!(f.inventory(), inventory);
        assert_eq!(
            application_routing::chain_ids(&f.owner.state().db, app.as_str()).unwrap(),
            chain
        );
        assert_eq!(
            f.owner
                .state()
                .db
                .get_current_provider(app.as_str())
                .unwrap(),
            None
        );
        assert_eq!(current::local_direct_pointer(&app).unwrap(), None);
        let routes = crate::proxy::provider_router::ProviderRouter::new(f.owner.state().db.clone())
            .select_providers(app.as_str())
            .await
            .unwrap();
        assert_eq!(routes[0].id, row.id);
        f.assert_no_key_projection();
        let written = f.bytes();
        f.apply(initial.clone()).await.unwrap();
        assert_eq!(f.bytes(), written);
        assert_eq!(f.receipt(&initial), "completed");
        f.stop().await;
    }
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_saved_references_never_implicitly_enter_or_attach() {
    let f = Fixture::new(AppType::Codex).await;
    let before = f.bytes();
    assert!(f
        .owner
        .state()
        .proxy_service
        .set_takeover_for_app("codex", true)
        .await
        .is_err());
    {
        let _guard = f
            .owner
            .state()
            .proxy_service
            .lock_switch_for_app("codex")
            .await;
        assert!(crate::mode::controller::enter_locked(
            &f.owner.state().proxy_service,
            &f.app,
            state::op::ATTACH
        )
        .await
        .is_err());
    }
    f.assert_empty();
    assert_eq!(f.bytes(), before);
    assert!(!f.owner.state().proxy_service.is_running().await);
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_missing_key_and_locked_vault_reject_before_prepare() {
    for locked in [false, true] {
        let f = Fixture::new(AppType::Codex).await;
        let before = f.bytes();
        if locked {
            f.owner.state().db.secret_session().set_blocked(true);
        } else {
            f.clear_key(0);
        }
        let result = f.try_prepare(&f.owner.ids()[0]).await;
        f.owner.state().db.secret_session().set_blocked(false);
        assert!(result.is_err());
        assert_eq!(f.bytes(), before);
        f.assert_empty();
        assert!(!f.owner.state().proxy_service.is_running().await);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_key_removed_or_vault_locked_after_prepare_rejects_enter() {
    for locked in [false, true] {
        let f = Fixture::new(AppType::Codex).await;
        let initial = f.prepare(0).await;
        let before = f.bytes();
        if locked {
            f.owner.state().db.secret_session().set_blocked(true);
        } else {
            f.clear_key(0);
        }
        let result = f.apply(initial.clone()).await;
        f.owner.state().db.secret_session().set_blocked(false);
        assert!(result.is_err());
        assert_eq!(f.receipt(&initial), "notRecorded");
        assert_eq!(f.bytes(), before);
        f.assert_empty();
        assert!(!f.owner.state().proxy_service.is_running().await);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_invalid_identity_plan_and_destinations_never_admit() {
    for case in 0..8 {
        let f = Fixture::new(AppType::Codex).await;
        let mut row = f.row(0);
        match case {
            0 => {
                row.meta
                    .as_mut()
                    .unwrap()
                    .auth_binding
                    .as_mut()
                    .unwrap()
                    .account_id = Some("702".into())
            }
            1 => {
                row.meta
                    .as_mut()
                    .unwrap()
                    .auth_binding
                    .as_mut()
                    .unwrap()
                    .auth_provider = Some("deepseek".into())
            }
            2 => {
                row.meta.as_mut().unwrap().loongport_vendor_account =
                    Some("synthetic-unowned".into())
            }
            3 => row.website_url = Some("https://untrusted.invalid".into()),
            4 => row.settings_config["base_url"] = "https://untrusted.invalid/v1".into(),
            5 => {
                let config = row.settings_config["config"].as_str().unwrap();
                row.settings_config["config"] = config
                    .replace("https://opencode.ai/zen/v1", "https://untrusted.invalid/v1")
                    .into();
            }
            6 => {
                // A real Go identity paired with Zen settings is still wrong.
                let account = row
                    .meta
                    .as_ref()
                    .unwrap()
                    .loongport_vendor_account
                    .as_ref()
                    .unwrap();
                row.id = crate::vendor::provision::provider_id_for("opencode-go", account);
            }
            _ => row.id = "unowned-provider-with-vendor-binding".into(),
        }
        f.save(&row);
        let before = f.bytes();
        let inventory = f.inventory();
        assert!(
            f.owner
                .state()
                .db
                .resolve_vendor_auth(&f.app, &row)
                .is_err(),
            "fixture case {case}"
        );
        assert!(f.try_prepare(&row.id).await.is_err(), "case {case}");
        assert_eq!(f.inventory(), inventory);
        assert_eq!(f.bytes(), before);
        f.assert_empty();
        assert!(!f.owner.state().proxy_service.is_running().await);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_raw_key_preset_does_not_become_a_saved_account_reference() {
    let f = Fixture::new(AppType::Codex).await;
    let mut row = f.row(0);
    row.meta.as_mut().unwrap().auth_binding = None;
    assert!(crate::relay::provider_config::patch_api_key(
        &mut row.settings_config,
        &f.app,
        "SYNTHETIC_RAW_PRESET_KEY",
    ));
    f.save(&row);
    let before = f.bytes();
    assert!(f.try_prepare(&row.id).await.is_err());
    assert_eq!(f.bytes(), before);
    f.assert_empty();
    assert!(!f.owner.state().proxy_service.is_running().await);
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_stale_provider_cas_preserves_preexisting_listener() {
    let f = Fixture::new(AppType::Codex).await;
    f.owner.state().proxy_service.start().await.unwrap();
    let initial = f.prepare(0).await;
    let before = f.bytes();
    let mut row = f.row(0);
    row.name.push_str(" changed after review");
    f.save(&row);
    assert!(f.apply(initial.clone()).await.is_err());
    assert_eq!(f.receipt(&initial), "notRecorded");
    assert_eq!(f.bytes(), before);
    f.assert_empty();
    assert!(f.owner.state().proxy_service.is_running().await);
    f.owner.state().proxy_service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_stale_native_cas_is_not_overwritten() {
    let f = Fixture::new(AppType::Codex).await;
    let initial = f.prepare(0).await;
    let file = crate::codex_config::get_codex_config_path();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, b"# synthetic newer native edit\n").unwrap();
    let before = f.bytes();
    assert!(f.apply(initial.clone()).await.is_err());
    assert_eq!(f.receipt(&initial), "notRecorded");
    assert_eq!(f.bytes(), before);
    assert!(!f.owner.state().proxy_service.is_running().await);
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_two_windows_use_existing_cas_and_single_receipt() {
    let f = Fixture::new(AppType::Codex).await;
    let first = f.prepare(0).await;
    let second = f.prepare(1).await;
    let (a, b) = tokio::join!(f.apply(first.clone()), f.apply(second.clone()));
    assert_ne!(a.is_ok(), b.is_ok());
    let winner = if a.is_ok() { first } else { second };
    assert_eq!(f.receipt(&winner), "completed");
    assert!(f.mode().routes_to(&winner.provider_id));
    f.assert_no_key_projection();
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_empty_existing_listener_survives_original_writer_failure() {
    let f = Fixture::new(AppType::Codex).await;
    f.owner.state().proxy_service.start().await.unwrap();
    let initial = f.prepare(0).await;
    let before = f.bytes();
    operation::failpoint::crash_at(Some("staged"));
    let result = f.apply(initial).await;
    operation::failpoint::crash_at(None);
    assert!(result.is_err());
    assert_eq!(f.bytes(), before);
    f.assert_empty();
    assert!(f.owner.state().proxy_service.is_running().await);
    f.owner.state().proxy_service.stop().await.unwrap();
}
