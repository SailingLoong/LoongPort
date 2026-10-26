//! Unix unit-test capability. No production endpoint/context override exists.
use super::super::{
    checkpoint::{ProfileCatalog, PROFILE_FILE},
    core::OAuthFamily,
    oauth_account::build_snapshot,
    official::{BusinessToken, OfficialUser, PollReady, ProviderAccessToken, StartJwt},
};
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Seal {
    session_address: usize,
    root: PathBuf,
    context: LibraryContext,
    storage: crate::secrets::storage::StorageIdentity,
    epoch: u64,
    accounts: Vec<(String, [u8; 32], String)>,
    _home: tempfile::TempDir,
}
pub(crate) struct FixtureOwner<'a> {
    seal: Arc<Seal>,
    // Always retain the original borrow, including unit-only builds.
    _session: &'a SecretSession,
}
impl FixtureOwner<'_> {
    pub(crate) fn account_id(&self, index: usize) -> &str {
        &self.seal.accounts[index].0
    }
    /// Generated fixture state only. The production selection reader remains the
    /// sole authority for effective identity; this does not call it or pre-freeze it.
    #[cfg(all(feature = "gui", feature = "test-hooks", unix))]
    pub(crate) fn seed_source_fixture(
        &self,
        db: &crate::database::Database,
        native: Option<&str>,
        selected: Option<&str>,
    ) {
        assert!(std::ptr::eq(self._session, db.secret_session()));
        let root = self.seal.context.data_root();
        crate::config_file_io::ensure_private_directory(root).unwrap();
        let path = root.join("telemetry-state.json");
        if let Some(id) = native {
            std::fs::write(&path, serde_json::json!({"deviceMid":id}).to_string()).unwrap();
        } else {
            assert!(!path.exists());
        }
        let ids = selected.into_iter().collect::<Vec<_>>();
        db.set_setting("zcode_device_ids_v1", &serde_json::json!({"scopes":{
            self.seal.context.context_id(): {"devices":ids,"selectedDeviceMid":selected,"revision":1}
        }}).to_string()).unwrap();
    }
    #[cfg(all(feature = "gui", feature = "test-hooks", unix))]
    pub(crate) fn assert_source_fixture_headers(
        &self,
        actual: &http::HeaderMap,
        effective: Option<&str>,
    ) {
        let version =
            super::super::native_context::library_app_version(self.seal.context.data_root());
        let expected = super::super::source_context::SourceContext::observe(Some(
            self.seal.context.data_root(),
        ))
        .with_device_mid(effective)
        .headers(version.as_deref());
        for (name, value) in &expected {
            assert!(
                actual.get(name) == Some(value),
                "original source header missing or changed: {name}"
            );
        }
        assert!(
            actual.get("x-device-mid").and_then(|v| v.to_str().ok()) == effective,
            "effective device selection must be original-owner frozen, including absence"
        );
    }
    /// These values are generated synthetic tokens, never read from an account.
    pub(crate) fn synthetic_token(&self, index: usize) -> &str {
        &self.seal.accounts[index].2
    }
}

#[derive(Clone)]
pub(crate) struct FixtureTransport {
    seal: Arc<Seal>,
    origin: String,
    client: reqwest::Client,
    attempts: Arc<AtomicUsize>,
}
impl FixtureTransport {
    /// Issue a capability only after sealing generated accounts into an empty,
    /// private temporary session root. Existing credentials are never adopted.
    pub(crate) async fn seed_accounts(
        session: &SecretSession,
    ) -> Result<FixtureOwner<'_>, Failure> {
        let temporary = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| Failure::UnsupportedContext)?;
        let root = session
            .root()
            .canonicalize()
            .map_err(|_| Failure::UnsupportedContext)?;
        if !root.starts_with(&temporary) || root == temporary {
            return Err(Failure::UnsupportedContext);
        }
        let home = tempfile::tempdir_in(&temporary).map_err(|_| Failure::StorageUnavailable)?;
        let home_path = home
            .path()
            .canonicalize()
            .map_err(|_| Failure::UnsupportedContext)?;
        let context = LibraryContext::from_os_identity(
            home_path.to_str().ok_or(Failure::UnsupportedContext)?,
            "synthetic-zcode-user",
            &home_path.join("zcode-library"),
        )
        .map_err(|_| Failure::UnsupportedContext)?;
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
        if root.join(PROFILE_FILE).exists() {
            return Err(Failure::StaleIdentity);
        }
        crate::config_file_io::ensure_private_directory(&root)
            .map_err(|_| Failure::StorageUnavailable)?;
        let native = context.cipher().map_err(|_| Failure::UnsupportedContext)?;
        let store =
            VaultAccountStore::new(&root, &vault).map_err(|_| Failure::StorageUnavailable)?;
        let status = store.status().map_err(|_| Failure::StorageUnavailable)?;
        if status.pending || status.native_unconfirmed {
            return Err(Failure::StaleIdentity);
        }
        let mut catalog = ProfileCatalog::default();
        let mut accounts = Vec::new();
        for (index, family) in [OAuthFamily::Zai, OAuthFamily::BigModel]
            .into_iter()
            .enumerate()
        {
            let token = format!("e30.{}.synthetic-signature", URL_SAFE_NO_PAD.encode(serde_json::json!({"exp":u64::MAX,"fixture":"zcode-loopback-only","index":index}).to_string()));
            let ready = PollReady {
                family,
                user: OfficialUser {
                    id: format!("synthetic-zcode-{index}"),
                    name: None,
                    email: None,
                },
                start_jwt: StartJwt::new(&token).map_err(|_| Failure::UnsupportedContext)?,
                provider_access_token: ProviderAccessToken::new("synthetic-provider-token")
                    .map_err(|_| Failure::UnsupportedContext)?,
                refresh_token: None,
            };
            let snapshot = build_snapshot(
                &native,
                &ready,
                &BusinessToken::from_stored(family, "synthetic-business-token")
                    .map_err(|_| Failure::UnsupportedContext)?,
                None,
            )
            .map_err(|_| Failure::UnsupportedContext)?;
            accounts.push((
                snapshot.identity().opaque_id(),
                saved_session_binding(&native, &snapshot)
                    .map_err(|_| Failure::UnsupportedContext)?,
                token,
            ));
            catalog.upsert(snapshot);
        }
        let sealed = catalog
            .seal(&vault, &native)
            .map_err(|_| Failure::StorageUnavailable)?;
        crate::config_file_io::write_durable(&root.join(PROFILE_FILE), sealed.as_bytes())
            .map_err(|_| Failure::StorageUnavailable)?;
        Ok(FixtureOwner {
            seal: Arc::new(Seal {
                session_address: session as *const SecretSession as usize,
                root,
                context,
                storage: vault.identity(),
                epoch: session.lifecycle_epoch(),
                accounts,
                _home: home,
            }),
            _session: session,
        })
    }

    pub(crate) fn loopback(owner: &FixtureOwner<'_>, origin: &str) -> Result<Self, Failure> {
        let url = url::Url::parse(origin).map_err(|_| Failure::UnsupportedContext)?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .and_then(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .is_some_and(|ip| ip.is_loopback())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Failure::UnsupportedContext);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Failure::UnsupportedContext)?;
        Ok(Self {
            seal: owner.seal.clone(),
            origin: origin.trim_end_matches('/').into(),
            client,
            attempts: Arc::new(AtomicUsize::new(0)),
        })
    }
    pub(crate) fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
    fn matches_session(&self, session: &SecretSession) -> Result<(), Failure> {
        if session
            .root()
            .canonicalize()
            .map_err(|_| Failure::StaleIdentity)?
            != self.seal.root
            || session.lifecycle_epoch() != self.seal.epoch
        {
            return Err(Failure::StaleIdentity);
        }
        let vault = session.read().map_err(|_| Failure::StaleIdentity)?;
        if vault.identity() != self.seal.storage {
            return Err(Failure::StaleIdentity);
        }
        Ok(())
    }
    fn matches(&self, lease: &Lease) -> Result<(), Failure> {
        self.matches_session(&lease.session)?;
        if lease.context.context_id() != self.seal.context.context_id()
            || !self.seal.accounts.iter().any(|(id, binding, _)| {
                id == &lease.binding.account_id && binding == &lease.binding.session_binding
            })
        {
            return Err(Failure::StaleIdentity);
        }
        Ok(())
    }
    /// Synthetic catalog replacement, not evidence of the native archive UI.
    pub(crate) async fn retire_fixture_accounts(
        &self,
        session: &SecretSession,
    ) -> Result<(), Failure> {
        self.retire_fixture_selection(session, None).await
    }
    pub(crate) async fn retire_first_fixture_account(
        &self,
        session: &SecretSession,
    ) -> Result<(), Failure> {
        self.retire_fixture_selection(session, Some(&self.seal.accounts[0].0))
            .await
    }
    async fn retire_fixture_selection(
        &self,
        session: &SecretSession,
        selected: Option<&str>,
    ) -> Result<(), Failure> {
        self.matches_session(session)?;
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        self.matches_session(session)?;
        let vault = session.read().map_err(|_| Failure::StaleIdentity)?;
        let native = self
            .seal
            .context
            .cipher()
            .map_err(|_| Failure::UnsupportedContext)?;
        let store = VaultAccountStore::new(&self.seal.root, &vault)
            .map_err(|_| Failure::StorageUnavailable)?;
        let status = store
            .catalog_status(&native)
            .map_err(|_| Failure::StaleIdentity)?;
        let catalog = store
            .import_catalog(&native, &status.revision)
            .map_err(|_| Failure::StaleIdentity)?;
        for snapshot in catalog.profiles() {
            let binding =
                saved_session_binding(&native, snapshot).map_err(|_| Failure::StaleIdentity)?;
            if !self.seal.accounts.iter().any(|(id, expected, _)| {
                id == &snapshot.identity().opaque_id() && expected == &binding
            }) {
                return Err(Failure::StaleIdentity);
            }
        }
        let mut remaining = ProfileCatalog::default();
        if let Some(selected) = selected {
            for snapshot in catalog.profiles() {
                if snapshot.identity().opaque_id() != selected {
                    // This fixture seeds native-captured synthetic profiles only.
                    remaining.upsert(snapshot.clone());
                }
            }
        }
        let sealed = remaining
            .seal(&vault, &native)
            .map_err(|_| Failure::StorageUnavailable)?;
        crate::config_file_io::write_durable(&self.seal.root.join(PROFILE_FILE), sealed.as_bytes())
            .map_err(|_| Failure::StorageUnavailable)
    }
}
impl StreamTransport for FixtureTransport {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            let key = request
                .headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok());
            let bearer = request
                .headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            if request.url != START_MESSAGES_URL
                || !self.seal.accounts.iter().any(|(_, _, token)| {
                    key == Some(token.as_str())
                        && bearer == Some(format!("Bearer {token}").as_str())
                })
            {
                return Err(owner_error(Failure::UnsupportedContext));
            }
            self.attempts.fetch_add(1, Ordering::SeqCst);
            self.client
                .post(format!(
                    "{}/api/v1/zcode-plan/anthropic/v1/messages",
                    self.origin
                ))
                .headers(request.headers)
                .body(request.body)
                .send()
                .await
                .map(ProxyResponse::Reqwest)
                .map_err(|_| {
                    ProxyError::ForwardFailed("zcode.inference.fixture_transport_failed".into())
                })
        })
    }
}
impl Lease {
    pub(crate) async fn acquire_fixture(
        db: Arc<crate::database::Database>,
        account_id: &str,
        request_session_id: &str,
        fixture: &FixtureTransport,
    ) -> Result<Self, Failure> {
        let session = db.secret_session();
        fixture.matches_session(session)?;
        if !fixture
            .seal
            .accounts
            .iter()
            .any(|(id, _, _)| id == account_id)
        {
            return Err(Failure::StaleIdentity);
        }
        let lease = Self::acquire_in_context(
            db,
            fixture.seal.context.clone(),
            account_id,
            request_session_id,
        )
        .await?;
        fixture.matches(&lease)?;
        Ok(lease)
    }
    pub(crate) async fn dispatch_fixture(
        self,
        body: Value,
        timeout: Duration,
        fixture: &FixtureTransport,
    ) -> Result<DispatchResponse, ProxyError> {
        fixture.matches(&self).map_err(owner_error)?;
        self.dispatch_with(body, timeout, fixture).await
    }
}

// Test-only capability registry: only seed_accounts can mint Seal. It accepts no
// caller-chosen context, OS identity or credential and never changes production
// library selection. The scoped guard owns the registration and borrows the
// original session, so its memory cannot be reused while the capability is live.
fn routing_capabilities(
) -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<Seal>>> {
    static CAPS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<Seal>>>,
    > = std::sync::OnceLock::new();
    CAPS.get_or_init(Default::default)
}
#[cfg(feature = "test-hooks")]
pub(crate) struct RoutingCapability<'a> {
    seal: Arc<Seal>,
    _session: &'a SecretSession,
}
#[cfg(feature = "test-hooks")]
impl<'a> FixtureOwner<'a> {
    pub(crate) fn install_routing_capability(
        &self,
        session: &SecretSession,
    ) -> Result<RoutingCapability<'a>, Failure> {
        if !std::ptr::eq(self._session, session) {
            return Err(Failure::StaleIdentity);
        }
        let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
        validate_routing_session(&self.seal, session, &vault)?;
        let mut caps = routing_capabilities()
            .lock()
            .map_err(|_| Failure::StorageUnavailable)?;
        if caps
            .get(&self.seal.root)
            .and_then(std::sync::Weak::upgrade)
            .is_some()
        {
            return Err(Failure::StaleIdentity);
        }
        caps.insert(self.seal.root.clone(), Arc::downgrade(&self.seal));
        Ok(RoutingCapability {
            seal: self.seal.clone(),
            _session: self._session,
        })
    }
}
#[cfg(feature = "test-hooks")]
impl Drop for RoutingCapability<'_> {
    fn drop(&mut self) {
        if let Ok(mut caps) = routing_capabilities().lock() {
            if caps
                .get(&self.seal.root)
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|seal| Arc::ptr_eq(&seal, &self.seal))
            {
                caps.remove(&self.seal.root);
            }
        }
    }
}
fn validate_routing_session(
    seal: &Seal,
    session: &SecretSession,
    vault: &crate::secrets::storage::StorageContext,
) -> Result<(), Failure> {
    if session as *const SecretSession as usize != seal.session_address
        || session
            .root()
            .canonicalize()
            .map_err(|_| Failure::UnsupportedContext)?
            != seal.root
        || session.lifecycle_epoch() != seal.epoch
        || vault.identity() != seal.storage
    {
        return Err(Failure::StaleIdentity);
    }
    Ok(())
}
pub(super) fn routing_context(
    session: &SecretSession,
    vault: &crate::secrets::storage::StorageContext,
) -> Result<LibraryContext, Failure> {
    let root = session
        .root()
        .canonicalize()
        .map_err(|_| Failure::UnsupportedContext)?;
    let seal = routing_capabilities()
        .lock()
        .map_err(|_| Failure::StorageUnavailable)?
        .get(&root)
        .and_then(std::sync::Weak::upgrade)
        .ok_or(Failure::UnsupportedContext)?;
    validate_routing_session(&seal, session, vault)?;
    Ok(seal.context.clone())
}
