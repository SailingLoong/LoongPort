//! Original service/controller/journal/router consumers in a synthetic HOME.
//! Requires the real runtime feature; no fixture claims native platform support.
use crate::live::engine::DeviceStore;
use crate::mode::controller::InitialProxyRoute;
use crate::mode::{
    current, operation,
    state::{self, Mode},
};
use crate::secrets::testing::TestHome;
use crate::services::{application_selection, ProviderService};
use crate::{
    app_config::AppType,
    database::Database,
    provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta},
    store::AppState,
};
use std::sync::Arc;

struct Fixture {
    state: AppState,
    app: AppType,
    _home: TestHome,
}
impl Fixture {
    async fn new(app: AppType) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(crate::secrets::testing::initialize_database().unwrap());
        Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap()).unwrap();
        let mut config = db.get_proxy_config().await.unwrap();
        config.listen_address = "127.0.0.1".into();
        config.listen_port = 0;
        db.update_proxy_config(config).await.unwrap();
        let state = AppState::new(db).unwrap();
        state::update(
            &DeviceStore::for_device(),
            &state.db.secret_session().read().unwrap(),
            |live| {
                // All applications are known Direct so a standalone listener
                // has no accidental unknown-mode retention in cleanup tests.
                for known in crate::mode::controller::PROXY_APPS {
                    live.apps.entry(known.as_str().into()).or_default().mode = Some(Mode::Direct);
                }
                Ok(())
            },
        )
        .unwrap();
        Self {
            state,
            app,
            _home: home,
        }
    }
    fn row(&self, id: &str) -> Provider {
        let mut row = Provider::with_id(
            id.into(),
            format!("Synthetic {id}"),
            match self.app {
                AppType::Claude => serde_json::json!({"env":{"ANTHROPIC_MODEL":"synthetic-model"}}),
                AppType::Codex => {
                    serde_json::json!({"auth":{},"config":"model = \"synthetic-model\"\n"})
                }
                _ => unreachable!(),
            },
            None,
        );
        row.category = Some("custom".into());
        row.meta = Some(ProviderMeta {
            common_config_enabled: Some(false),
            api_format: Some("anthropic".into()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("zcode_start_plan".into()),
                account_id: Some(format!("synthetic-{id}")),
            }),
            ..Default::default()
        });
        row
    }
    fn add(&self, id: &str) {
        ProviderService::add(&self.state, self.app.clone(), self.row(id), false).unwrap();
    }
    async fn prepare(&self, id: &str) -> InitialProxyRoute {
        InitialProxyRoute {
            provider_id: id.into(),
            request: self
                .state
                .proxy_service
                .prepare_proxy_takeover_request(
                    self.app.as_str(),
                    id,
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await
                .unwrap(),
        }
    }
    async fn apply(&self, initial: InitialProxyRoute) -> Result<(), String> {
        self.state
            .proxy_service
            .set_takeover_for_app_with_initial_route(self.app.as_str(), true, Some(initial))
            .await
    }
    fn bytes(&self) -> Vec<Option<Vec<u8>>> {
        crate::mode::controller::files(&self.app)
            .unwrap()
            .iter()
            .map(|f| std::fs::read(&f.path).ok())
            .collect()
    }
    fn mode(&self) -> state::ModeState {
        current::validate_known_mode(
            &DeviceStore::for_device(),
            &self.state.db.secret_session().read().unwrap(),
            &self.app,
        )
        .unwrap()
    }
    fn query(&self, initial: &InitialProxyRoute) -> String {
        application_selection::query_routing_request(
            &self.state,
            &self.app,
            initial.request.clone(),
        )
        .unwrap()
        .status
    }
    async fn stop(&self) {
        self.state
            .proxy_service
            .set_takeover_for_app(self.app.as_str(), false)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn explicit_first_route_uses_original_enter_and_router_without_direct_pointer() {
    for app in [AppType::Claude, AppType::Codex] {
        let f = Fixture::new(app.clone()).await;
        f.add("chosen");
        f.add("other");
        let before = f.bytes();
        assert!(f
            .state
            .proxy_service
            .set_takeover_for_app(app.as_str(), true)
            .await
            .is_err());
        assert_eq!(f.bytes(), before);
        let initial = f.prepare("chosen").await;
        assert_eq!(f.query(&initial), "notRecorded");
        f.apply(initial.clone()).await.unwrap();
        assert_eq!(f.query(&initial), "completed");
        assert!(f.mode().routes_to("chosen"));
        assert_eq!(f.state.db.get_current_provider(app.as_str()).unwrap(), None);
        assert_eq!(current::local_direct_pointer(&app).unwrap(), None);
        let routes = crate::proxy::provider_router::ProviderRouter::new(f.state.db.clone())
            .select_providers(app.as_str())
            .await
            .unwrap();
        assert_eq!(routes[0].id, "chosen");
        for bytes in f.bytes().into_iter().flatten() {
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("synthetic-chosen"));
            assert!(!text.contains("zcode_start_plan"));
        }
        f.stop().await;
    }
}

#[tokio::test]
#[serial_test::serial]
async fn changed_selection_or_source_revision_rejects_before_listener_and_native_write() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    f.add("second");
    let before = f.bytes();
    let ordinary = application_selection::prepare_routing_request(
        &f.state,
        &f.app,
        &application_selection::ApplicationRoutingChange {
            order: None,
            selection: Some(application_selection::TierSelection {
                provider_id: "first".into(),
                model: None,
            }),
            catalog_decision: None,
            catalog_preview_only: false,
        },
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    assert!(f
        .apply(InitialProxyRoute {
            provider_id: "first".into(),
            request: ordinary
        })
        .await
        .is_err());
    assert_eq!(f.bytes(), before);
    let initial = f.prepare("first").await;
    let mut changed = initial.clone();
    changed.provider_id = "second".into();
    assert!(f.apply(changed).await.is_err());
    let mut row = f.row("first");
    row.settings_config["env"]["ANTHROPIC_MODEL"] = "changed-model".into();
    f.state
        .db
        .update_provider_settings_config("claude", "first", &row.settings_config)
        .unwrap();
    assert!(f.apply(initial).await.is_err());
    assert_eq!(f.bytes(), before);
    assert!(!f.state.proxy_service.is_running().await);
    let initial = f.prepare("first").await;
    let file = crate::config::get_claude_settings_path();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"{\"userEdit\":true}").unwrap();
    let changed = f.bytes();
    assert!(f.apply(initial).await.is_err());
    assert_eq!(f.bytes(), changed);
    assert!(!f.state.proxy_service.is_running().await);
}

#[tokio::test]
#[serial_test::serial]
async fn two_windows_and_duplicate_confirmation_share_original_app_lock_and_receipt() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    f.add("second");
    let first = f.prepare("first").await;
    let second = f.prepare("second").await;
    let (a, b) = tokio::join!(f.apply(first.clone()), f.apply(second.clone()));
    assert_ne!(a.is_ok(), b.is_ok());
    let winner = if a.is_ok() { first } else { second };
    let bytes = f.bytes();
    f.apply(winner.clone()).await.unwrap();
    assert_eq!(f.bytes(), bytes);
    assert_eq!(f.query(&winner), "completed");
    assert_eq!(f.state.db.get_all_providers("claude").unwrap().len(), 2);
    assert_eq!(f.state.db.get_current_provider("claude").unwrap(), None);
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn failed_first_listener_preserves_missing_native_and_retry_does_not_save_rows_again() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = f.state.db.get_proxy_config().await.unwrap();
    config.listen_port = occupied.local_addr().unwrap().port();
    f.state.db.update_proxy_config(config).await.unwrap();
    let before = f.bytes();
    let initial = f.prepare("first").await;
    assert!(f.apply(initial.clone()).await.is_err());
    assert_eq!(f.bytes(), before);
    assert_eq!(f.query(&initial), "notRecorded");
    assert_eq!(f.mode().mode, Some(Mode::Direct));
    drop(occupied);
    f.apply(f.prepare("first").await).await.unwrap();
    assert_eq!(f.state.db.get_all_providers("claude").unwrap().len(), 1);
    assert_eq!(f.state.db.get_current_provider("claude").unwrap(), None);
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn original_pending_enter_is_not_replayed_and_original_recovery_retains_selected_route() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    f.add("second");
    let initial = f.prepare("second").await;
    operation::failpoint::crash_at(Some("published:0"));
    let result = f.apply(initial.clone()).await;
    operation::failpoint::crash_at(None);
    assert!(result.is_err());
    assert_eq!(f.query(&initial), "verificationRequired");
    let bytes = f.bytes();
    assert!(f.apply(initial.clone()).await.is_err());
    assert_eq!(f.bytes(), bytes);
    assert!(f.state.proxy_service.is_running().await);
    {
        let _guard = f.state.proxy_service.lock_switch_for_app("claude").await;
        crate::mode::controller::recover_locked(&f.state.proxy_service, &f.app).unwrap();
    }
    assert_eq!(f.query(&initial), "completed");
    assert!(f.mode().routes_to("second"));
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn unknown_mode_and_attach_without_recorded_route_never_guess_an_account() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    let before = f.bytes();
    {
        let _guard = f.state.proxy_service.lock_switch_for_app("claude").await;
        assert!(crate::mode::controller::enter_locked(
            &f.state.proxy_service,
            &f.app,
            state::op::ATTACH
        )
        .await
        .is_err());
    }
    state::update(
        &DeviceStore::for_device(),
        &f.state.db.secret_session().read().unwrap(),
        |live| {
            live.apps.get_mut("claude").unwrap().mode = None;
            Ok(())
        },
    )
    .unwrap();
    assert!(f
        .state
        .proxy_service
        .prepare_proxy_takeover_request("claude", "first", &uuid::Uuid::new_v4().to_string())
        .await
        .is_err());
    assert_eq!(f.bytes(), before);
    assert!(!f.state.proxy_service.is_running().await);
}

#[tokio::test]
#[serial_test::serial]
async fn stale_bootstrap_does_not_stop_a_preexisting_standalone_listener() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    f.state.proxy_service.start().await.unwrap();
    let initial = f.prepare("first").await;
    let before = f.bytes();
    f.state
        .db
        .update_provider_settings_config(
            "claude",
            "first",
            &serde_json::json!({"env":{"ANTHROPIC_MODEL":"changed-model"}}),
        )
        .unwrap();
    assert!(f.apply(initial).await.is_err());
    assert!(f.state.proxy_service.is_running().await);
    assert_eq!(f.bytes(), before);
    f.state.proxy_service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn failed_bootstrap_cleans_only_its_created_listener_when_no_pending_dependency() {
    let f = Fixture::new(AppType::Claude).await;
    f.add("first");
    let initial = f.prepare("first").await;
    let before = f.bytes();
    operation::failpoint::crash_at(Some("staged"));
    let result = f.apply(initial).await;
    operation::failpoint::crash_at(None);
    assert!(result.is_err());
    assert!(!f.state.proxy_service.is_running().await);
    assert_eq!(f.bytes(), before);
    assert!(state::pending(
        &DeviceStore::for_device(),
        &f.state.db.secret_session().read().unwrap(),
        "claude"
    )
    .unwrap()
    .is_none());
}
