//! Test-only transport for the original OpenCode vendor-key owner.
//! No caller-supplied database, session, key, or public destination can mint it.
use crate::{
    app_config::AppType,
    database::Database,
    live::engine::DeviceStore,
    mode::state::{self, Mode},
    provider::{AuthBindingSource, Provider, ProviderMeta},
    proxy::ProxyError,
    secrets::{session::SecretSession, testing::TestHome},
    store::AppState,
    vendor::{self, Vendor},
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

const ACCOUNT_IDS: [&str; 2] = [
    "synthetic-opencode-workspace-a",
    "synthetic-opencode-workspace-b",
];
const ROW_IDS: [i64; 2] = [701, 702];
const KEYS: [&str; 2] = ["SYNTHETIC_OPENCODE_KEY_A", "SYNTHETIC_OPENCODE_KEY_B"];
const OFFICIAL_OUTPUT: &str = "https://opencode.ai/zen/v1/responses";

fn invalid() -> ProxyError {
    ProxyError::ConfigError("synthetic_vendor_transport_unavailable".into())
}

pub(crate) struct FixtureOwner {
    state: AppState,
    ids: [String; 2],
    session: Arc<SecretSession>,
    root: PathBuf,
    epoch: u64,
    _home: TestHome,
}
impl FixtureOwner {
    /// Generates its own empty physical temporary home and original vault/DB.
    pub(crate) fn new() -> Self {
        Self::new_inner(true, 2)
    }
    pub(crate) fn with_uninitialized_chain() -> Self {
        Self::new_inner(false, 2)
    }
    pub(crate) fn with_one_binding() -> Self {
        Self::new_inner(true, 1)
    }
    fn new_inner(initialize_chain: bool, bind_count: usize) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(crate::secrets::testing::initialize_database().unwrap());
        Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap()).unwrap();
        // Existing-app routing fixtures start from a deliberately initialized empty chain.
        // The separate missing-priority regression below exercises the original gap.
        if initialize_chain {
            crate::proxy::application_routing::migrate(&db, "codex").unwrap();
        }
        let session = db.secrets.clone();
        let root = session.root().canonicalize().unwrap();
        assert!(root.starts_with(home.path().canonicalize().unwrap()));
        let epoch = session.lifecycle_epoch();
        let state = AppState::new(db.clone()).unwrap();
        if !initialize_chain {
            assert!(
                db.get_setting("application_priority_codex")
                    .unwrap()
                    .is_none(),
                "modern AppState must not be mistaken for a legacy routing migration"
            );
        }
        {
            let vault = session.read().unwrap();
            let conn = db.conn.lock().unwrap();
            for table in ["providers", "loongport_vendor"] {
                let count: i64 = conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 0, "fixture constructor only seeds an empty DB");
            }
            for i in 0..2 {
                conn.execute("INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(?1,'opencode',?2)", rusqlite::params![ROW_IDS[i], ACCOUNT_IDS[i]]).unwrap();
                vendor::creds::set_api_key(&conn, &vault, ROW_IDS[i], KEYS[i]).unwrap();
            }
        }
        let ids: [String; 2] =
            std::array::from_fn(|i| vendor::provision::provider_id_for("opencode", ACCOUNT_IDS[i]));
        for i in 0..2 {
            // This pure projection is not the provisioning command or key lifecycle.
            // Existing source inventory is credential-free; the original vendor vault owns the key.
            let zen = vendor::provision::plan_rows_for(Vendor::OpenCode, ACCOUNT_IDS[i], "")
                .into_iter()
                .find(|p| p.plan.id_segment == "opencode")
                .unwrap();
            let settings = zen
                .rows
                .into_iter()
                .find(|(app, _)| *app == AppType::Claude)
                .unwrap()
                .1;
            let mut source = Provider::with_id(
                ids[i].clone(),
                "Synthetic existing Zen source".into(),
                settings,
                None,
            );
            source.meta = Some(ProviderMeta {
                loongport_vendor_account: Some(ACCOUNT_IDS[i].into()),
                ..Default::default()
            });
            db.save_provider("claude", &source).unwrap();
            if i < bind_count {
                assert!(crate::services::vendor_source_binding::bind_vendor_source(
                    &db,
                    &ids[i],
                    &AppType::Codex
                )
                .unwrap());
            }
        }
        state::update(
            &DeviceStore::for_device(),
            &session.read().unwrap(),
            |live| {
                for app in crate::mode::controller::PROXY_APPS {
                    live.apps.entry(app.as_str().into()).or_default().mode = Some(Mode::Direct);
                }
                Ok(())
            },
        )
        .unwrap();
        Self {
            state,
            ids,
            session,
            root,
            epoch,
            _home: home,
        }
    }
    pub(crate) fn state(&self) -> &AppState {
        &self.state
    }
    pub(crate) fn ids(&self) -> &[String; 2] {
        &self.ids
    }
    pub(crate) fn model(&self) -> String {
        vendor::config_for(Vendor::OpenCode, "opencode", &AppType::Codex)
            .unwrap()
            .1
    }
    pub(crate) fn synthetic_key(&self, index: usize) -> &'static str {
        KEYS[index]
    }
    pub(crate) fn transport(
        &self,
        destination: SocketAddr,
    ) -> Result<FixtureTransport, ProxyError> {
        if destination.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            || destination.port() == 0
        {
            return Err(invalid());
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| invalid())?;
        Ok(FixtureTransport {
            session: self.session.clone(),
            root: self.root.clone(),
            epoch: self.epoch,
            ids: self.ids.clone(),
            url: format!("http://{destination}/zen/v1/responses"),
            client,
        })
    }
}

#[derive(Clone)]
pub(crate) struct FixtureTransport {
    session: Arc<SecretSession>,
    root: PathBuf,
    epoch: u64,
    ids: [String; 2],
    url: String,
    client: reqwest::Client,
}
pub(crate) struct FixtureOutput {
    url: String,
    client: reqwest::Client,
}
impl FixtureOutput {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
    pub(crate) fn client(&self) -> reqwest::Client {
        self.client.clone()
    }
}

/// Call only after the real resolve_vendor_auth and immediately before sending.
/// Production has no registration for this module or function.
pub(crate) fn prepare_output(
    extensions: &http::Extensions,
    session: &Arc<SecretSession>,
    provider: &Provider,
    url: &str,
) -> Result<Option<FixtureOutput>, ProxyError> {
    let binding = provider.meta.as_ref().and_then(|m| m.auth_binding.as_ref());
    let Some(binding) = binding.filter(|b| {
        b.source == AuthBindingSource::VendorApiKey
            && b.auth_provider.as_deref() == Some("opencode")
    }) else {
        return Ok(None);
    };
    let fixture = extensions.get::<FixtureTransport>().ok_or_else(invalid)?;
    if !Arc::ptr_eq(session, &fixture.session)
        || session.root().canonicalize().map_err(|_| invalid())? != fixture.root
        || session.lifecycle_epoch() != fixture.epoch
        || url != OFFICIAL_OUTPUT
    {
        return Err(invalid());
    }
    let _vault = session.read().map_err(|_| invalid())?;
    let index = fixture
        .ids
        .iter()
        .position(|id| id == &provider.id)
        .ok_or_else(invalid)?;
    if binding.account_id.as_deref() != Some(ROW_IDS[index].to_string().as_str())
        || provider
            .meta
            .as_ref()
            .and_then(|m| m.loongport_vendor_account.as_deref())
            != Some(ACCOUNT_IDS[index])
        || crate::relay::provider_config::extract_api_key(
            &provider.settings_config,
            &AppType::Codex,
        )
        .as_deref()
            != Some(KEYS[index])
    {
        return Err(invalid());
    }
    Ok(Some(FixtureOutput {
        url: fixture.url.clone(),
        client: fixture.client.clone(),
    }))
}
