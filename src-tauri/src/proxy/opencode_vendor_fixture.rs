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
const PERSONAL_KEYS: [&str; 2] = [
    "oc_sk_fixture_SyntheticPersonalSecretOnly1234",
    "oc_sk_fixture_OtherSyntheticPersonalSecret1234",
];
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
    accounts: [String; 2],
    personal_go: bool,
    _home: TestHome,
}
impl FixtureOwner {
    /// Generates its own empty physical temporary home and original vault/DB.
    pub(crate) fn new() -> Self {
        Self::new_inner(true, 2, false)
    }
    pub(crate) fn with_uninitialized_chain() -> Self {
        Self::new_inner(false, 2, false)
    }
    pub(crate) fn with_one_binding() -> Self {
        Self::new_inner(true, 1, false)
    }
    pub(crate) fn personal_go() -> Self {
        Self::new_inner(true, 2, true)
    }
    #[cfg(feature = "test-hooks")]
    pub(crate) fn personal_go_plaintext() -> Self {
        Self::new_inner_with_database(true, 2, true, || {
            crate::services::native_source_guard::storage_fixture::fresh_plaintext_database()
        })
    }
    fn new_inner(initialize_chain: bool, bind_count: usize, personal_go: bool) -> Self {
        Self::new_inner_with_database(initialize_chain, bind_count, personal_go, || {
            crate::secrets::testing::initialize_database().unwrap()
        })
    }
    fn new_inner_with_database(
        initialize_chain: bool,
        bind_count: usize,
        personal_go: bool,
        initialize: impl FnOnce() -> Database,
    ) -> Self {
        let accounts: [String; 2] = std::array::from_fn(|i| {
            if personal_go {
                vendor::opencode_personal::identity(PERSONAL_KEYS[i]).unwrap()
            } else {
                ACCOUNT_IDS[i].into()
            }
        });
        let keys = if personal_go { PERSONAL_KEYS } else { KEYS };
        let plan = if personal_go {
            vendor::opencode_personal::GO
        } else {
            "opencode"
        };
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(initialize());
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
                conn.execute("INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(?1,'opencode',?2)", rusqlite::params![ROW_IDS[i], accounts[i]]).unwrap();
                vendor::creds::set_api_key(&conn, &vault, ROW_IDS[i], keys[i]).unwrap();
            }
        }
        let ids: [String; 2] =
            std::array::from_fn(|i| vendor::provision::provider_id_for(plan, &accounts[i]));
        for i in 0..2 {
            // This pure projection is not the provisioning command or key lifecycle.
            // Existing source inventory is credential-free; the original vendor vault owns the key.
            let zen = vendor::provision::plan_rows_for(Vendor::OpenCode, &accounts[i], "")
                .into_iter()
                .find(|p| p.plan.id_segment == plan)
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
                loongport_vendor_account: Some(accounts[i].clone()),
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
                if personal_go {
                    // Codex now holds the original source reference. Re-bind
                    // Claude through the same owner, not a hand-built auth DTO.
                    db.delete_provider("claude", &ids[i]).unwrap();
                    assert!(crate::services::vendor_source_binding::bind_vendor_source(
                        &db,
                        &ids[i],
                        &AppType::Claude,
                    )
                    .unwrap());
                }
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
            accounts,
            personal_go,
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
        if self.personal_go {
            PERSONAL_KEYS[index]
        } else {
            KEYS[index]
        }
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
            origin: format!("http://{destination}"),
            accounts: self.accounts.clone(),
            personal_go: self.personal_go,
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
    origin: String,
    accounts: [String; 2],
    personal_go: bool,
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
    app_type: &AppType,
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
        || !(if fixture.personal_go {
            match app_type {
                AppType::Codex => {
                    url == "https://opencode.ai/inference/go/openai/v1/chat/completions"
                }
                AppType::Claude => url == "https://opencode.ai/inference/go/anthropic/v1/messages",
                _ => false,
            }
        } else {
            url == OFFICIAL_OUTPUT
        })
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
            != Some(fixture.accounts[index].as_str())
        || crate::relay::provider_config::extract_api_key(&provider.settings_config, app_type)
            .as_deref()
            != Some(if fixture.personal_go {
                PERSONAL_KEYS[index]
            } else {
                KEYS[index]
            })
    {
        return Err(invalid());
    }
    Ok(Some(FixtureOutput {
        url: format!(
            "{}{}",
            fixture.origin,
            url::Url::parse(url).map_err(|_| invalid())?.path()
        ),
        client: fixture.client.clone(),
    }))
}
