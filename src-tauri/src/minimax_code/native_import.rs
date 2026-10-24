//! Read-only native snapshot -> explicit profile verification -> separately
//! confirmed encrypted access-only import. Never refresh or write the source app.
use super::{
    auth,
    catalog::{
        self, Account, AccountView, Binding, CredentialKind, Credentials, Failure, NativeSource,
        NativeSourceFormat, Region, Status,
    },
    http::Transport,
    native_format,
};
use crate::secrets::session::SecretSession;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Stage {
    Preview,
    Verified,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub preview_id: String,
    pub source: NativeSourceFormat,
    pub region: Region,
    pub masked_subject: Option<String>,
    pub expires_at_ms: i64,
    pub stage: Stage,
    pub verified_user_id: Option<String>,
    pub label: Option<String>,
    pub can_save: bool,
}
struct Inspected {
    preview: Preview,
    credential: native_format::NativeCredential,
    fingerprint: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preview,
    Verifying,
    Verified,
    Publishing,
    Completed,
}
#[derive(Clone)]
struct Flow {
    root: PathBuf,
    path: PathBuf,
    binding: Binding,
    fingerprint: String,
    deadline: i64,
    phase: Phase,
    preview: Preview,
    identities: HashMap<String, (u64, u64)>,
}
fn flows() -> &'static Mutex<HashMap<String, Flow>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Flow>>> = OnceLock::new();
    FLOWS.get_or_init(Default::default)
}
pub(super) fn clear_flows() {
    if let Ok(mut flows) = flows().lock() {
        flows.clear();
    }
}
fn flow_for(session: &SecretSession, id: &str, phase: Phase) -> Result<Flow, Failure> {
    let map = flows().lock().map_err(|_| Failure::Busy)?;
    let f = map.get(id).ok_or(Failure::ExpiredAuthorization)?;
    if f.root != session.root() {
        return Err(Failure::UnsupportedContext);
    }
    if f.phase == Phase::Completed {
        return Err(Failure::AuthorizationCompleted);
    }
    if f.deadline <= catalog::now_ms() {
        return Err(Failure::ExpiredAuthorization);
    }
    if f.phase != phase {
        return Err(Failure::Busy);
    }
    Ok(f.clone())
}
fn current(
    session: &SecretSession,
    id: &str,
    flow: &Flow,
    phase: Phase,
    binding: &Binding,
) -> Result<(), Failure> {
    let f = flow_for(session, id, phase)?;
    if f.binding != *binding || f.binding != flow.binding || f.fingerprint != flow.fingerprint {
        return Err(Failure::StaleIdentity);
    }
    Ok(())
}
fn same_source(flow: &Flow) -> Result<Inspected, Failure> {
    let inspected = inspect(&flow.path, flow.preview.region, catalog::now_ms())?;
    if inspected.fingerprint != flow.fingerprint {
        return Err(Failure::StaleIdentity);
    }
    Ok(inspected)
}
fn inspect(path: &Path, region: Region, now: i64) -> Result<Inspected, Failure> {
    use std::fs;
    if !path.is_absolute()
        || path.components().any(|p| {
            matches!(
                p,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(Failure::UnsupportedContext);
    }
    let suffix = match region {
        Region::Cn => "prod/cn/mcode-public/auth.json",
        Region::Global => "prod/en/mcode-public/auth.json",
    };
    if !path.ends_with(suffix) {
        return Err(Failure::UnsupportedContext);
    }
    let home = path.ancestors().nth(4).ok_or(Failure::UnsupportedContext)?;
    for part in path.ancestors() {
        let m = fs::symlink_metadata(part).map_err(|_| Failure::UnsupportedContext)?;
        if m.file_type().is_symlink() {
            return Err(Failure::UnsupportedContext);
        }
    }
    let m = fs::metadata(path).map_err(|_| Failure::UnsupportedContext)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err(Failure::UnsupportedContext);
        }
    }
    if !m.is_file() {
        return Err(Failure::UnsupportedContext);
    }
    let bytes = zeroize::Zeroizing::new(
        crate::config_file_io::read_regular_file(path, 1024 * 1024)
            .map_err(|_| Failure::UnsupportedContext)?
            .ok_or(Failure::UnsupportedContext)?,
    );
    let region_format = match region {
        Region::Cn => native_format::Region::Cn,
        Region::Global => native_format::Region::Global,
    };
    let c = native_format::parse_native(
        &bytes,
        home.to_str().ok_or(Failure::UnsupportedContext)?,
        region_format,
        now,
    )
    .map_err(|e| match e {
        native_format::Failure::Expired => Failure::NeedsLogin,
        _ => Failure::UnsupportedContext,
    })?;
    catalog::validate_access_token(&c.access_token)?;
    let masked_subject = c.subject.as_ref().map(|s| {
        let mut chars = s.chars();
        let first = chars.next().unwrap_or('*');
        let last = chars.last().unwrap_or('*');
        format!("{first}***{last}")
    });
    Ok(Inspected {
        preview: Preview {
            preview_id: String::new(),
            source: NativeSourceFormat::PublicCliOAuthV1,
            region,
            masked_subject,
            expires_at_ms: c.expires_at_ms,
            stage: Stage::Preview,
            verified_user_id: None,
            label: None,
            can_save: false,
        },
        credential: c,
        fingerprint: hex::encode(Sha256::digest(&bytes)),
    })
}
pub(crate) async fn preview(
    session: &SecretSession,
    path: &Path,
    region: Region,
    source: NativeSourceFormat,
    confirmed: bool,
) -> Result<Preview, Failure> {
    if !confirmed {
        return Err(Failure::UnsupportedContext);
    }
    let (binding, identities) = catalog::access(session, None, |d, b| {
        Ok((
            (
                b.clone(),
                d.accounts
                    .iter()
                    .filter(|a| a.region == region)
                    .map(|a| {
                        (
                            a.real_user_id.clone(),
                            (a.auth_generation, a.credential_version),
                        )
                    })
                    .collect(),
            ),
            false,
        ))
    })
    .await?;
    // Only the declared public CLI v1 format is supported. No Desktop/broker guess.
    let NativeSourceFormat::PublicCliOAuthV1 = source;
    let mut inspected = inspect(path, region, catalog::now_ms())?;
    inspected.preview.preview_id = uuid::Uuid::new_v4().to_string();
    catalog::access(session, Some(&binding), |_, _| {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let now = catalog::now_ms();
        map.retain(|_, f| f.deadline > now);
        if map.len() >= 128 {
            return Err(Failure::Busy);
        }
        map.insert(
            inspected.preview.preview_id.clone(),
            Flow {
                root: session.root().to_path_buf(),
                path: path.to_path_buf(),
                binding: binding.clone(),
                fingerprint: inspected.fingerprint.clone(),
                deadline: inspected
                    .preview
                    .expires_at_ms
                    .min(now.saturating_add(300_000)),
                phase: Phase::Preview,
                preview: inspected.preview.clone(),
                identities,
            },
        );
        Ok((inspected.preview, false))
    })
    .await
}
pub(crate) async fn verify<T: Transport>(
    session: &SecretSession,
    transport: &T,
    id: &str,
    confirmed: bool,
) -> Result<Preview, Failure> {
    if !confirmed {
        return Err(Failure::UnsupportedContext);
    }
    let flow = flow_for(session, id, Phase::Preview)?;
    let inspected = catalog::access(session, Some(&flow.binding), |_, b| {
        current(session, id, &flow, Phase::Preview, b)?;
        let inspected = same_source(&flow)?;
        flows()
            .lock()
            .map_err(|_| Failure::Busy)?
            .get_mut(id)
            .ok_or(Failure::ExpiredAuthorization)?
            .phase = Phase::Verifying;
        Ok((inspected, false))
    })
    .await?;
    let result = auth::verify_access_token(
        session,
        transport,
        flow.preview.region,
        &inspected.credential.access_token,
        &flow.binding,
        |_, b| {
            current(session, id, &flow, Phase::Verifying, b)?;
            same_source(&flow)?;
            Ok(())
        },
    )
    .await;
    let (uid, label) = match result {
        Ok(identity) => identity,
        Err(error) => {
            if let Ok(mut map) = flows().lock() {
                if let Some(f) = map.get_mut(id) {
                    if f.phase == Phase::Verifying {
                        f.phase = Phase::Preview;
                    }
                }
            }
            return Err(error);
        }
    };
    catalog::access(session, Some(&flow.binding), |_, b| {
        current(session, id, &flow, Phase::Verifying, b)?;
        same_source(&flow)?;
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let f = map.get_mut(id).ok_or(Failure::ExpiredAuthorization)?;
        f.phase = Phase::Verified;
        f.preview.stage = Stage::Verified;
        f.preview.verified_user_id = Some(uid);
        f.preview.label = Some(label);
        f.preview.can_save = true;
        Ok((f.preview.clone(), false))
    })
    .await
}
pub(crate) async fn save(
    session: &SecretSession,
    id: &str,
    confirmed: bool,
) -> Result<AccountView, Failure> {
    if !confirmed {
        return Err(Failure::UnsupportedContext);
    }
    let flow = flow_for(session, id, Phase::Verified)?;
    catalog::access_after(
        session,
        Some(&flow.binding),
        |d, b| {
            current(session, id, &flow, Phase::Verified, b)?;
            let inspected = same_source(&flow)?;
            let uid = flow
                .preview
                .verified_user_id
                .as_ref()
                .ok_or(Failure::InvalidResponse)?;
            let label = flow
                .preview
                .label
                .as_ref()
                .ok_or(Failure::InvalidResponse)?;
            let existing = d
                .accounts
                .iter()
                .position(|a| a.region == flow.preview.region && a.real_user_id == *uid);
            if existing.map(|i| {
                (
                    d.accounts[i].auth_generation,
                    d.accounts[i].credential_version,
                )
            }) != flow.identities.get(uid).copied()
            {
                return Err(Failure::StaleIdentity);
            }
            if existing.is_some_and(|i| {
                d.accounts[i]
                    .credentials
                    .as_ref()
                    .is_some_and(|c| c.kind == CredentialKind::DeviceOAuth)
            }) {
                // Import cannot silently downgrade an independently owned refresh grant.
                return Err(Failure::UnsupportedContext);
            }
            let credentials = Credentials {
                access_token: inspected.credential.access_token.to_string(),
                refresh_token: None,
                expires_at: inspected.credential.expires_at_ms,
                kind: CredentialKind::NativeAccessOnly,
                native_source: Some(NativeSource {
                    format: flow.preview.source,
                    generation: inspected.credential.generation,
                    fingerprint: inspected.fingerprint,
                    rejected: false,
                }),
            };
            catalog::validate_credentials(&credentials)?;
            let index = if let Some(i) = existing {
                let a = &mut d.accounts[i];
                a.auth_generation = a
                    .auth_generation
                    .checked_add(1)
                    .ok_or(Failure::StorageUnavailable)?;
                a.credential_version = a
                    .credential_version
                    .checked_add(1)
                    .ok_or(Failure::StorageUnavailable)?;
                a.auto_claim_revision = a
                    .auto_claim_revision
                    .checked_add(1)
                    .ok_or(Failure::StorageUnavailable)?;
                a.auto_claim_enabled = false;
                a.auto_claim_failure = None;
                a.credentials = Some(credentials);
                a.label = label.clone();
                a.status = if a.unresolved() {
                    Status::Unknown
                } else {
                    Status::Unavailable
                };
                i
            } else {
                d.accounts.push(Account {
                    quota_observation: None,
                    id: catalog::account_id(&b.vault.vault_id, flow.preview.region, uid),
                    label: label.clone(),
                    region: flow.preview.region,
                    build_env: "prod".into(),
                    real_user_id: uid.clone(),
                    auth_generation: 1,
                    credential_version: 1,
                    credentials: Some(credentials),
                    auto_claim_enabled: false,
                    auto_claim_revision: 0,
                    auto_claim_failure: None,
                    status: Status::Unavailable,
                    operation: None,
                    last_checked_at: None,
                    last_observed_status: None,
                });
                d.accounts.len() - 1
            };
            let view = d.accounts[index].view();
            flows()
                .lock()
                .map_err(|_| Failure::Busy)?
                .get_mut(id)
                .ok_or(Failure::ExpiredAuthorization)?
                .phase = Phase::Publishing;
            Ok((view, true))
        },
        |_| {
            flows()
                .lock()
                .map_err(|_| Failure::Busy)?
                .get_mut(id)
                .ok_or(Failure::ExpiredAuthorization)?
                .phase = Phase::Completed;
            Ok(())
        },
    )
    .await
}
pub(crate) async fn cancel(session: &SecretSession, id: &str) -> Result<(), Failure> {
    let _runtime = super::admit()?;
    let mut map = flows().lock().map_err(|_| Failure::Busy)?;
    let f = map.get(id).ok_or(Failure::ExpiredAuthorization)?;
    if f.root != session.root() {
        return Err(Failure::UnsupportedContext);
    }
    if f.phase == Phase::Completed {
        return Err(Failure::AuthorizationCompleted);
    }
    if f.phase == Phase::Publishing {
        return Err(Failure::StorageUnavailable);
    }
    map.remove(id);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    fn source(root: &Path) -> std::path::PathBuf {
        let home = root.canonicalize().unwrap().join("auth");
        let file = home.join("prod/cn/mcode-public/auth.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let key = format!(
            "com.minimax.mcode.oauth.prod.cn\0{}",
            URL_SAFE_NO_PAD.encode(Sha256::digest(
                format!("{}\0mcode-public", home.display()).as_bytes()
            ))
        );
        std::fs::write(&file,serde_json::to_vec(&json!({"schemaVersion":1,"records":{key:{"schemaVersion":1,"accessToken":"synthetic-access","refreshToken":"synthetic-refresh","tokenType":"Bearer","clientId":"mcode-public","scopes":["agent.default"],"audience":"agent-backend","expiresAtMs":200000,"generation":1,"subject":"fixture-user"}}})).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        file
    }
    #[test]
    fn masked_preview_never_claims_account_verified_or_saveable() {
        let t = tempfile::tempdir().unwrap();
        let file = source(t.path());
        let p = inspect(&file, Region::Cn, 100000).unwrap();
        assert_eq!(p.preview.masked_subject.as_deref(), Some("f***r"));
        assert!(!p.preview.can_save);
        let dto = serde_json::to_string(&p.preview).unwrap();
        assert!(!dto.contains("synthetic"));
        assert!(!dto.contains("fixture-user"));
        assert!(!dto.contains("refreshToken"));
    }
    #[test]
    fn cross_region_rejected() {
        let t = tempfile::tempdir().unwrap();
        let f = source(t.path());
        assert!(matches!(
            inspect(&f, Region::Global, 100000),
            Err(Failure::UnsupportedContext)
        ));
    }
    #[test]
    fn empty_file_is_not_identity() {
        let t = tempfile::tempdir().unwrap();
        let f = source(t.path());
        std::fs::write(&f, b"{}").unwrap();
        assert!(inspect(&f, Region::Cn, 100000).is_err());
    }
    #[test]
    fn expired_source_rejected() {
        let t = tempfile::tempdir().unwrap();
        let f = source(t.path());
        assert!(matches!(
            inspect(&f, Region::Cn, 200000),
            Err(Failure::NeedsLogin)
        ));
    }
    #[tokio::test]
    async fn missing_read_consent_never_opens_source() {
        let _guard = super::super::isolated_fixture_guard().await;
        let t = tempfile::tempdir().unwrap();
        let s = SecretSession::from_context(
            t.path().into(),
            crate::secrets::VaultContext::generate().unwrap(),
        );
        assert!(matches!(
            preview(
                &s,
                Path::new("/does-not-exist"),
                Region::Cn,
                NativeSourceFormat::PublicCliOAuthV1,
                false
            )
            .await,
            Err(Failure::UnsupportedContext)
        ));
    }
    #[test]
    fn source_change_requires_new_preview() {
        let t = tempfile::tempdir().unwrap();
        let f = source(t.path());
        assert!(inspect(&f, Region::Cn, 100000).is_ok());
        std::fs::write(&f, b"{}").unwrap();
        assert!(inspect(&f, Region::Cn, 100000).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinked_file_is_rejected_without_read() {
        let t = tempfile::tempdir().unwrap();
        let f = source(t.path());
        let original = f.with_extension("saved");
        std::fs::rename(&f, &original).unwrap();
        std::os::unix::fs::symlink(&original, &f).unwrap();
        assert!(inspect(&f, Region::Cn, 100000).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn public_file_permissions_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let f = source(t.path());
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(inspect(&f, Region::Cn, 100000).is_err());
    }
    #[tokio::test]
    async fn locked_vault_blocks_before_source_read() {
        let _guard = super::super::isolated_fixture_guard().await;
        let t = tempfile::tempdir().unwrap();
        let s = SecretSession::from_context(
            t.path().into(),
            crate::secrets::VaultContext::generate().unwrap(),
        );
        s.set_blocked(true);
        assert!(matches!(
            preview(
                &s,
                Path::new("/does-not-exist"),
                Region::Cn,
                NativeSourceFormat::PublicCliOAuthV1,
                true
            )
            .await,
            Err(Failure::StorageUnavailable)
        ));
    }
    use super::super::http::{Endpoint, Request, Response};
    use std::{future::Future, pin::Pin, sync::Arc};
    struct NativeFixture {
        _serial: tokio::sync::OwnedMutexGuard<()>,
        root: tempfile::TempDir,
        session: Arc<SecretSession>,
        file: PathBuf,
        region: Region,
    }
    impl NativeFixture {
        async fn new(region: Region) -> Self {
            let serial = super::super::isolated_fixture_guard().await;
            let root = tempfile::tempdir().unwrap();
            let session = SecretSession::from_context(
                root.path().into(),
                crate::secrets::VaultContext::generate().unwrap(),
            );
            let home = root.path().canonicalize().unwrap().join("native-auth");
            let realm = if region == Region::Cn { "cn" } else { "en" };
            let file = home.join(format!("prod/{realm}/mcode-public/auth.json"));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            let key = format!(
                "com.minimax.mcode.oauth.prod.{realm}\0{}",
                URL_SAFE_NO_PAD.encode(Sha256::digest(
                    format!("{}\0mcode-public", home.display()).as_bytes()
                ))
            );
            std::fs::write(&file, serde_json::to_vec(&json!({"schemaVersion":1,"records":{key:{"schemaVersion":1,"accessToken":"synthetic-native-access","refreshToken":"synthetic-source-refresh-never-copy","tokenType":"Bearer","clientId":"mcode-public","scopes":["agent.default"],"audience":"agent-backend","expiresAtMs":catalog::now_ms()+3600000,"generation":9}}})).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            Self {
                _serial: serial,
                root,
                session,
                file,
                region,
            }
        }
        async fn preview(&self) -> Preview {
            preview(
                &self.session,
                &self.file,
                self.region,
                NativeSourceFormat::PublicCliOAuthV1,
                true,
            )
            .await
            .unwrap()
        }
        async fn import(&self, transport: &NativeTransport) -> AccountView {
            let p = self.preview().await;
            verify(&self.session, transport, &p.preview_id, true)
                .await
                .unwrap();
            save(&self.session, &p.preview_id, true).await.unwrap()
        }
        fn change_source(&self, key: &str, value: serde_json::Value) {
            let mut source: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&self.file).unwrap()).unwrap();
            source["records"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap()[key] = value;
            std::fs::write(&self.file, serde_json::to_vec(&source).unwrap()).unwrap();
        }
    }
    #[derive(Default)]
    struct NativeTransport {
        calls: Mutex<Vec<Request>>,
        gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        status: Option<u16>,
    }
    impl Transport for NativeTransport {
        fn request(
            &self,
            request: Request,
        ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
            Box::pin(async move {
                let region = request.region;
                self.calls.lock().unwrap().push(request);
                if let Some((started, release)) = &self.gate {
                    started.notify_one();
                    release.notified().await;
                }
                Ok(Response {
                    status: self.status.unwrap_or(200),
                    body: json!({"statusInfo":{"code":0},"data":{"userInfo":{"realUserID":if region == Region::Cn {"verified-cn-user"} else {"verified-global-user"},"name":"Synthetic verified account"}}}),
                })
            })
        }
    }
    #[tokio::test]
    async fn public_cli_import_without_subject_is_separately_confirmed_access_only_and_encrypted() {
        for region in [Region::Cn, Region::Global] {
            let f = NativeFixture::new(region).await;
            let before = std::fs::read(&f.file).unwrap();
            let transport = NativeTransport::default();
            let p = f.preview().await;
            assert!(p.masked_subject.is_none());
            assert!(!p.can_save);
            assert!(transport.calls.lock().unwrap().is_empty());
            assert!(catalog::list(&f.session).await.unwrap().is_empty());
            assert!(matches!(
                verify(&f.session, &transport, &p.preview_id, false).await,
                Err(Failure::UnsupportedContext)
            ));
            assert!(matches!(
                save(&f.session, &p.preview_id, true).await,
                Err(Failure::Busy)
            ));
            assert!(transport.calls.lock().unwrap().is_empty());
            let v = verify(&f.session, &transport, &p.preview_id, true)
                .await
                .unwrap();
            assert_eq!(
                v.verified_user_id.as_deref(),
                Some(if region == Region::Cn {
                    "verified-cn-user"
                } else {
                    "verified-global-user"
                })
            );
            assert!(v.can_save);
            assert!(matches!(
                save(&f.session, &p.preview_id, false).await,
                Err(Failure::UnsupportedContext)
            ));
            assert!(catalog::list(&f.session).await.unwrap().is_empty());
            let account = save(&f.session, &p.preview_id, true).await.unwrap();
            assert!(!account.auto_claim_enabled);
            assert!(account.authorized && account.can_refresh && !account.can_refresh_credentials);
            assert_eq!(
                account.credential_kind,
                Some(CredentialKind::NativeAccessOnly)
            );
            assert_eq!(account.credential_expires_at, Some(p.expires_at_ms));
            let s = catalog::snapshot(&f.session, &account.id).await.unwrap();
            let c = s.account.credentials.as_ref().unwrap();
            assert!(c.refresh_token.is_none());
            assert_eq!(c.expires_at, p.expires_at_ms);
            assert_eq!(c.native_source.as_ref().unwrap().generation, 9);
            let raw = std::fs::read(f.root.path().join(catalog::FILE)).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("synthetic-native-access"));
            assert!(!serde_json::to_string(c)
                .unwrap()
                .contains("synthetic-source-refresh"));
            let dto = serde_json::to_string(&account).unwrap();
            assert!(
                !dto.contains("synthetic-native-access")
                    && !dto.contains("fingerprint")
                    && !dto.contains("refreshToken")
            );
            {
                let calls = transport.calls.lock().unwrap();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].endpoint, Endpoint::Profile);
                assert_eq!(calls[0].region, region);
                assert_eq!(
                    calls[0].access_token.as_deref(),
                    Some("synthetic-native-access")
                );
                assert!(calls[0].body.is_none());
            }
            assert_eq!(before, std::fs::read(&f.file).unwrap());
            assert!(matches!(
                save(&f.session, &p.preview_id, true).await,
                Err(Failure::AuthorizationCompleted)
            ));
        }
    }
    #[tokio::test]
    async fn native_changed_snapshot_or_wrong_session_never_dispatches() {
        let f = NativeFixture::new(Region::Cn).await;
        let p = f.preview().await;
        let transport = NativeTransport::default();
        f.change_source("accessToken", json!("synthetic-new-user-token"));
        assert!(matches!(
            verify(&f.session, &transport, &p.preview_id, true).await,
            Err(Failure::StaleIdentity)
        ));
        assert!(transport.calls.lock().unwrap().is_empty());
        let other = tempfile::tempdir().unwrap();
        let other_session = SecretSession::from_context(
            other.path().into(),
            crate::secrets::VaultContext::generate().unwrap(),
        );
        assert!(matches!(
            cancel(&other_session, &p.preview_id).await,
            Err(Failure::UnsupportedContext)
        ));
        assert!(matches!(
            verify(&other_session, &transport, &p.preview_id, true).await,
            Err(Failure::UnsupportedContext)
        ));
    }
    #[tokio::test]
    async fn native_late_identity_cannot_cross_cancel_source_lock_or_shutdown() {
        for action in ["cancel", "source", "locked", "shutdown"] {
            let f = NativeFixture::new(Region::Cn).await;
            let p = f.preview().await;
            let started = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let transport = Arc::new(NativeTransport {
                gate: Some((started.clone(), release.clone())),
                ..Default::default()
            });
            let s = f.session.clone();
            let t = transport.clone();
            let id = p.preview_id.clone();
            let task = tokio::spawn(async move { verify(&s, &*t, &id, true).await });
            started.notified().await;
            match action {
                "cancel" => cancel(&f.session, &p.preview_id).await.unwrap(),
                "source" => f.change_source("subject", json!("different-source-user")),
                "locked" => f.session.set_blocked(true),
                "shutdown" => super::super::shutdown(),
                _ => unreachable!(),
            }
            release.notify_one();
            assert!(task.await.unwrap().is_err(), "{action}");
            assert!(
                save(&f.session, &p.preview_id, true).await.is_err(),
                "{action}"
            );
            assert!(!f.root.path().join(catalog::FILE).exists());
        }
    }
    #[tokio::test]
    async fn native_verified_snapshot_cannot_cross_expiry_retirement_or_new_authorization() {
        for action in [
            "expiry",
            "retire",
            "new-authorization",
            "new-credential",
            "changed-source",
        ] {
            let f = NativeFixture::new(Region::Cn).await;
            let t = NativeTransport::default();
            let first = f.import(&t).await;
            let p = f.preview().await;
            verify(&f.session, &t, &p.preview_id, true).await.unwrap();
            match action {
                "expiry" => {
                    flows()
                        .lock()
                        .unwrap()
                        .get_mut(&p.preview_id)
                        .unwrap()
                        .deadline = catalog::now_ms() - 1;
                }
                "retire" => super::super::checkin::Engine {
                    session: &f.session,
                    transport: &t,
                }
                .retire(&first.id)
                .await
                .unwrap(),
                "new-authorization" | "new-credential" => {
                    catalog::access(&f.session, None, |d, _| {
                        if action == "new-authorization" {
                            d.accounts[0].auth_generation += 1;
                        } else {
                            d.accounts[0].credential_version += 1;
                        }
                        Ok(((), true))
                    })
                    .await
                    .unwrap();
                }
                "changed-source" => {
                    f.change_source("accessToken", json!("synthetic-other-source-user"))
                }
                _ => unreachable!(),
            }
            let before = std::fs::read(f.root.path().join(catalog::FILE)).unwrap();
            assert!(
                save(&f.session, &p.preview_id, true).await.is_err(),
                "{action}"
            );
            assert_eq!(
                before,
                std::fs::read(f.root.path().join(catalog::FILE)).unwrap()
            );
        }
    }
    #[tokio::test]
    async fn native_reimport_preserves_unknown_receipt_and_disables_automatic_consent() {
        let f = NativeFixture::new(Region::Cn).await;
        let t = NativeTransport::default();
        let first = f.import(&t).await;
        let operation = catalog::Operation {
            attempt_id: "synthetic-unknown".into(),
            started_at: catalog::now_ms() - 1000,
            day: catalog::day(catalog::now_ms()),
            bound_auth_generation: 1,
            claim_id: None,
            status: catalog::AttemptStatus::Unknown,
            credit_amount: None,
        };
        catalog::access(&f.session, None, |d, _| {
            let a = &mut d.accounts[0];
            a.operation = Some(operation.clone());
            a.status = Status::NeedsLogin;
            a.auto_claim_enabled = true;
            a.credentials.as_mut().unwrap().expires_at = catalog::now_ms() - 1;
            Ok(((), true))
        })
        .await
        .unwrap();
        let expired = catalog::list(&f.session).await.unwrap().remove(0);
        assert!(expired.requires_reimport && !expired.authorized && expired.can_retire);
        let next = f.import(&t).await;
        assert_eq!(next.auth_generation, first.auth_generation + 1);
        assert_eq!(next.credential_version, first.credential_version + 1);
        assert_eq!(next.operation, Some(operation));
        assert_eq!(next.status, Status::Unknown);
        assert!(next.authorized && !next.requires_reimport && !next.auto_claim_enabled);
        assert!(!next.can_claim);
    }
    #[tokio::test]
    async fn native_import_never_downgrades_owned_refresh_or_duplicates_same_identity() {
        let f = NativeFixture::new(Region::Cn).await;
        let t = NativeTransport::default();
        let p1 = f.preview().await;
        let p2 = f.preview().await;
        verify(&f.session, &t, &p1.preview_id, true).await.unwrap();
        verify(&f.session, &t, &p2.preview_id, true).await.unwrap();
        save(&f.session, &p1.preview_id, true).await.unwrap();
        assert!(matches!(
            save(&f.session, &p2.preview_id, true).await,
            Err(Failure::StaleIdentity)
        ));
        catalog::access(&f.session, None, |d, _| {
            let c = d.accounts[0].credentials.as_mut().unwrap();
            c.kind = CredentialKind::DeviceOAuth;
            c.native_source = None;
            c.refresh_token = Some("synthetic-owned-refresh".into());
            Ok(((), true))
        })
        .await
        .unwrap();
        let p3 = f.preview().await;
        verify(&f.session, &t, &p3.preview_id, true).await.unwrap();
        let before = std::fs::read(f.root.path().join(catalog::FILE)).unwrap();
        assert!(matches!(
            save(&f.session, &p3.preview_id, true).await,
            Err(Failure::UnsupportedContext)
        ));
        assert_eq!(
            before,
            std::fs::read(f.root.path().join(catalog::FILE)).unwrap()
        );
        assert_eq!(catalog::list(&f.session).await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn native_bad_identity_or_expired_source_never_becomes_saveable() {
        let f = NativeFixture::new(Region::Cn).await;
        let t = NativeTransport {
            status: Some(401),
            ..Default::default()
        };
        let p = f.preview().await;
        assert!(matches!(
            verify(&f.session, &t, &p.preview_id, true).await,
            Err(Failure::NeedsLogin)
        ));
        assert!(save(&f.session, &p.preview_id, true).await.is_err());
        f.change_source("expiresAtMs", json!(catalog::now_ms() - 1));
        let t = NativeTransport::default();
        assert!(matches!(
            verify(&f.session, &t, &p.preview_id, true).await,
            Err(Failure::NeedsLogin)
        ));
        assert!(t.calls.lock().unwrap().is_empty());
        assert!(catalog::list(&f.session).await.unwrap().is_empty());
    }

    #[test]
    fn native_import_commands_are_registered_and_reexported() {
        let registration = include_str!("../lib.rs");
        let exports = include_str!("../commands/mod.rs");
        assert!(
            exports.contains("pub(crate) use minimax_code::*;")
                || exports.contains("pub use minimax_code::*;")
        );
        for name in [
            "preview_minimax_native_file",
            "verify_minimax_native_import",
            "save_minimax_native_import",
            "cancel_minimax_native_import",
        ] {
            assert!(
                registration.contains(&format!("commands::{name},")),
                "unregistered command: {name}"
            );
        }
    }

    #[tokio::test]
    async fn native_imported_account_enters_original_inference_consumer_without_refresh() {
        use super::super::inference::{Lease, StreamRequest, StreamTransport};
        use crate::proxy::{hyper_client::ProxyResponse, ProxyError};
        struct SyntheticStream(Mutex<Vec<StreamRequest>>);
        impl StreamTransport for SyntheticStream {
            fn request(
                &self,
                request: StreamRequest,
            ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>>
            {
                Box::pin(async move {
                    self.0.lock().unwrap().push(request);
                    let mut headers = http::HeaderMap::new();
                    headers.insert(
                        http::header::CONTENT_TYPE,
                        http::HeaderValue::from_static("text/event-stream"),
                    );
                    Ok(ProxyResponse::streamed(http::StatusCode::OK,headers,futures::stream::iter(vec![Ok::<_,std::io::Error>(bytes::Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"))])))
                })
            }
        }
        for region in [Region::Cn, Region::Global] {
            let f = NativeFixture::new(region).await;
            let auth = NativeTransport::default();
            let account = f.import(&auth).await;
            let stream = SyntheticStream(Mutex::default());
            let lease = Lease::acquire(
                f.session.clone(),
                &account.id,
                "synthetic-native-import-consumer",
            )
            .await
            .unwrap();
            let response=lease.dispatch_with(json!({"model":"synthetic-model","messages":[{"role":"user","content":"synthetic message"}],"stream":true}),std::time::Duration::from_secs(3),&auth,&stream).await.unwrap();
            assert!(!response
                .response
                .bytes_with_limit(4096)
                .await
                .unwrap()
                .is_empty());
            let calls = stream.0.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].headers[http::header::AUTHORIZATION],
                "Bearer synthetic-native-access"
            );
            assert_eq!(
                calls[0].url,
                if region == Region::Cn {
                    "https://agent.minimax.cn/mavis/api/v1/llm/v1/messages"
                } else {
                    "https://agent.minimax.io/mavis/api/v1/llm/v1/messages"
                }
            );
            assert_eq!(auth.calls.lock().unwrap().len(), 1); // Explicit profile verification only.
        }
    }

    #[tokio::test]
    async fn native_cancel_before_queued_commit_cannot_persist_verified_access() {
        let f = NativeFixture::new(Region::Cn).await;
        let transport = NativeTransport::default();
        let p = f.preview().await;
        verify(&f.session, &transport, &p.preview_id, true)
            .await
            .unwrap();
        let owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let mut saving = Box::pin(save(&f.session, &p.preview_id, true));
        let first_poll =
            std::future::poll_fn(|cx| std::task::Poll::Ready(saving.as_mut().poll(cx))).await;
        assert!(first_poll.is_pending());
        cancel(&f.session, &p.preview_id).await.unwrap();
        drop(owner);
        assert!(matches!(saving.await, Err(Failure::ExpiredAuthorization)));
        assert!(catalog::list(&f.session).await.unwrap().is_empty());
        assert!(!f.root.path().join(catalog::FILE).exists());
    }
}
