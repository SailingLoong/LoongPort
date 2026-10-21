//! Sealed test capability. No production URL override or credential resolver.
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(crate) struct FixtureOwner {
    root: PathBuf,
    binding: store::Binding,
    accounts: Arc<Vec<store::Account>>,
    ids: Vec<String>,
}
impl FixtureOwner {
    pub(crate) fn account_ids(&self) -> &[String] {
        &self.ids
    }
}
#[derive(Clone)]
pub(crate) struct FixtureTransport {
    root: PathBuf,
    binding: store::Binding,
    accounts: Arc<Vec<store::Account>>,
    origin: String,
    client: reqwest::Client,
    attempts: Arc<AtomicUsize>,
}
impl FixtureTransport {
    /// Reuse the owner test serial barrier and its existing runtime reset.
    /// Keep this guard until requests, responses and fixture teardown finish.
    pub(crate) async fn serial() -> tokio::sync::OwnedMutexGuard<()> {
        store::fixture_guard().await
    }
    pub(crate) async fn seed_accounts(
        session: &SecretSession,
        accounts: &[(&str, store::Realm)],
    ) -> Result<FixtureOwner, Failure> {
        let root = session
            .root()
            .canonicalize()
            .map_err(|_| Failure::Storage)?;
        let temporary = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| Failure::Storage)?;
        if root == temporary
            || !root.starts_with(&temporary)
            || accounts.is_empty()
            || accounts.len() > 8
        {
            return Err(Failure::Unsupported);
        }
        let mut rows = Vec::new();
        for (id, realm) in accounts {
            if !id.starts_with("synthetic-")
                || !store::text(id, 128)
                || !matches!(realm, store::Realm::Sg | store::Realm::Us)
                || rows.iter().any(|a: &store::Account| a.id == *id)
            {
                return Err(Failure::Unsupported);
            }
            rows.push(store::Account {
                id: (*id).into(),
                uid: format!("{id}-user"),
                realm: *realm,
                generation: 1,
                version: 1,
                device: store::Device {
                    auth_host: realm.auth().into(),
                    native_package: None,
                    uid: format!("{id}-user"),
                    id: format!("{id}-device"),
                    machine: format!("{id}-machine"),
                    client: "ono9krqynydwx5".into(),
                    version: "3.6.2".into(),
                    version_code: "20260917".into(),
                    os: "linux".into(),
                    os_version: "Linux synthetic".into(),
                    source_digest: "a".repeat(64),
                    private_key: String::new(),
                    public_key: String::new(),
                },
                grant: Some(store::Grant {
                    access: format!("{id}-access"),
                    refresh: format!("{id}-refresh"),
                    expires: i64::MAX,
                }),
                refresh_pending: false,
                checkin_day: None,
                imported_refresh_hash: hex::encode(Sha256::digest(
                    format!("{id}-refresh").as_bytes(),
                )),
            });
        }
        store::access(session, None, |d, binding| {
            if !d.accounts.is_empty() {
                return Err(Failure::Busy);
            }
            d.accounts = rows.clone();
            Ok((
                FixtureOwner {
                    root,
                    binding: binding.clone(),
                    accounts: Arc::new(rows),
                    ids: accounts.iter().map(|(id, _)| (*id).into()).collect(),
                },
                true,
            ))
        })
        .await
    }
    pub(crate) fn loopback(owner: &FixtureOwner, origin: &str) -> Result<Self, Failure> {
        let url = reqwest::Url::parse(origin).map_err(|_| Failure::Unsupported)?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Failure::Unsupported);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Failure::Network)?;
        Ok(Self {
            root: owner.root.clone(),
            binding: owner.binding.clone(),
            accounts: owner.accounts.clone(),
            origin: origin.trim_end_matches('/').into(),
            client,
            attempts: Arc::default(),
        })
    }
    pub(crate) fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
    fn matches(&self, lease: &Lease) -> bool {
        self.binding == lease.snapshot.binding
            && lease
                .session
                .root()
                .canonicalize()
                .is_ok_and(|r| r == self.root)
            && self.accounts.iter().any(|a| {
                let row = &lease.snapshot.account;
                a.id == row.id
                    && a.realm == row.realm
                    && a.uid == row.uid
                    && a.generation == row.generation
                    && a.version == row.version
                    && a.device == row.device
                    && !row.refresh_pending
                    && a.grant
                        .as_ref()
                        .zip(row.grant.as_ref())
                        .is_some_and(|(a, b)| {
                            a.access == b.access && a.refresh == b.refresh && a.expires == b.expires
                        })
            })
    }
}
impl Transport for FixtureTransport {
    fn send(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            let account = self
                .accounts
                .iter()
                .find(|a| {
                    let Some(g) = &a.grant else {
                        return false;
                    };
                    r.url == format!("{}/api/agent/v3/llm_utils_chat", a.realm.api())
                        && r.headers.get("authorization").and_then(|h| h.to_str().ok())
                            == Some(format!("Cloud-IDE-JWT {}", g.access).as_str())
                        && r.headers
                            .get("x-cloudide-token")
                            .and_then(|h| h.to_str().ok())
                            == Some(g.access.as_str())
                        && r.headers.get("x-ide-token").and_then(|h| h.to_str().ok())
                            == Some(g.access.as_str())
                        && r.headers.get("x-uid").and_then(|h| h.to_str().ok())
                            == Some(a.uid.as_str())
                        && r.headers.get("x-device-id").and_then(|h| h.to_str().ok())
                            == Some(a.device.id.as_str())
                })
                .ok_or_else(|| ProxyError::ConfigError("trae.fixture_binding_mismatch".into()))?;
            let realm = match account.realm {
                store::Realm::Sg => "sg",
                store::Realm::Us => "us",
                _ => return Err(network_error()),
            };
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let response = self
                .client
                .post(format!(
                    "{}/{realm}/api/agent/v3/llm_utils_chat",
                    self.origin
                ))
                .headers(r.headers)
                .timeout(r.timeout)
                .json(&r.body)
                .send()
                .await
                .map_err(|_| network_error())?;
            Ok(ProxyResponse::Reqwest(response))
        })
    }
}
impl Lease {
    pub(crate) async fn dispatch_fixture(
        self,
        body: Value,
        timeout: Duration,
        fixture: &FixtureTransport,
    ) -> Result<DispatchResponse, ProxyError> {
        if !fixture.matches(&self) {
            return Err(ProxyError::ConfigError(
                "trae.fixture_binding_mismatch".into(),
            ));
        }
        self.dispatch_with(body, timeout, fixture).await
    }
    /// CN protocol regression only; not issued by any production acquisition path.
    pub(crate) async fn checkin_fixture(
        session: Arc<SecretSession>,
        id: &str,
    ) -> Result<Self, Failure> {
        let snapshot = store::snapshot(&session, id).await?;
        if snapshot.account.realm != store::Realm::Cn
            || !snapshot
                .account
                .grant
                .as_ref()
                .is_some_and(|g| g.access == "synthetic-access" && g.refresh == "synthetic-refresh")
        {
            return Err(Failure::Unsupported);
        }
        Ok(Self {
            session,
            snapshot,
            request_session: "synthetic-checkin".into(),
        })
    }
}
