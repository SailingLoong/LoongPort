//! Synthetic encrypted-catalog/transport tests. These are NOT ProxyServer tests,
//! live account checks, or evidence of non-macOS production support.
use super::super::{
    checkpoint::{ProfileCatalog, PROFILE_FILE},
    core::OAuthFamily,
    oauth_account::build_snapshot,
    official::{BusinessToken, OfficialUser, PollReady, ProviderAccessToken, StartJwt},
};
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::{collections::VecDeque, sync::Mutex};

fn jwt(exp: u64, marker: &str) -> String {
    format!(
        "e30.{}.signature",
        URL_SAFE_NO_PAD.encode(serde_json::json!({"exp":exp,"fixture":marker}).to_string())
    )
}
struct Fixture {
    _root: tempfile::TempDir,
    session: Arc<SecretSession>,
    context: LibraryContext,
    ids: Vec<String>,
    tokens: Vec<String>,
}
impl Fixture {
    fn new(exp: u64) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap();
        let context = LibraryContext::from_os_identity(
            home.to_str().unwrap(),
            "fixture",
            &home.join("zcode"),
        )
        .unwrap();
        let native = context.cipher().unwrap();
        let session = SecretSession::ephemeral().unwrap();
        crate::config_file_io::ensure_private_directory(session.root()).unwrap();
        let tokens = vec![jwt(exp, "a"), jwt(exp, "b")];
        let mut catalog = ProfileCatalog::default();
        let mut ids = vec![];
        for (index, family) in [OAuthFamily::Zai, OAuthFamily::BigModel]
            .into_iter()
            .enumerate()
        {
            let ready = PollReady {
                family,
                user: OfficialUser {
                    id: format!("fixture-{index}"),
                    name: None,
                    email: None,
                },
                start_jwt: StartJwt::new(&tokens[index]).unwrap(),
                provider_access_token: ProviderAccessToken::new("fixture-provider-token").unwrap(),
                refresh_token: None,
            };
            let account = build_snapshot(
                &native,
                &ready,
                &BusinessToken::from_stored(family, "fixture-business").unwrap(),
                None,
            )
            .unwrap();
            ids.push(account.identity().opaque_id());
            catalog.upsert(account);
        }
        let vault = session.read().unwrap();
        crate::config_file_io::write_durable(
            &session.root().join(PROFILE_FILE),
            catalog.seal(&vault, &native).unwrap().as_bytes(),
        )
        .unwrap();
        drop(vault);
        Self {
            _root: root,
            session,
            context,
            ids,
            tokens,
        }
    }
    async fn lease(&self, index: usize) -> Result<Lease, Failure> {
        Lease::acquire_in_context(
            self.session.clone(),
            self.context.clone(),
            &self.ids[index],
            "fixture-session",
        )
        .await
    }
    async fn mutate(&self, edit: impl FnOnce(&mut ProfileCatalog, &NativeCipher)) {
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let vault = self.session.read().unwrap();
        let native = self.context.cipher().unwrap();
        let store = VaultAccountStore::new(self.session.root(), &vault).unwrap();
        let status = store.catalog_status(&native).unwrap();
        let mut catalog = store.import_catalog(&native, &status.revision).unwrap();
        edit(&mut catalog, &native);
        crate::config_file_io::write_durable(
            &self.session.root().join(PROFILE_FILE),
            catalog.seal(&vault, &native).unwrap().as_bytes(),
        )
        .unwrap();
    }
}
#[derive(Default)]
struct TransportFixture {
    calls: Mutex<Vec<StreamRequest>>,
    responses: Mutex<VecDeque<ProxyResponse>>,
}
impl StreamTransport for TransportFixture {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProxyError::ForwardFailed("fixture exhausted".into()))
        })
    }
}
fn response(status: u16) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::from_u16(status).unwrap(),
        http::HeaderMap::new(),
        Bytes::from_static(b"fixture-body"),
    )
}
fn body() -> Value {
    serde_json::json!({"model":"GLM-5.2","max_tokens":32,"stream":true,"messages":[{"role":"user","content":"fixture"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}],"thinking":{"type":"enabled","budget_tokens":16}})
}

#[test]
fn expiry_is_only_a_local_rejection_hint_never_an_authorization_proof() {
    assert!(jwt_is_expired(&jwt(100, "a"), 100));
    assert!(!jwt_is_expired(&jwt(101, "a"), 100));
    for token in [
        "opaque-token",
        "a.not-json.c",
        "a.e30.c",
        "a.eyJleHAiOiJub3QtbmF0ZSIifQ.c",
    ] {
        assert!(!jwt_is_expired(token, 100));
    }
}

#[tokio::test]
async fn sealed_fixture_rejects_existing_catalog_cross_root_and_non_loopback() {
    let session = SecretSession::ephemeral().unwrap();
    let owner = FixtureTransport::seed_accounts(&session).await.unwrap();
    assert!(FixtureTransport::seed_accounts(&session).await.is_err());
    for origin in [
        "https://127.0.0.1:9",
        "http://example.com",
        "http://127.0.0.1:9/path",
        "http://user@127.0.0.1:9",
        "http://127.0.0.1:9/?x=1",
    ] {
        assert!(FixtureTransport::loopback(&owner, origin).is_err());
    }
    let transport = FixtureTransport::loopback(&owner, "http://127.0.0.1:9").unwrap();
    let other = SecretSession::ephemeral().unwrap();
    assert!(
        Lease::acquire_fixture(other, owner.account_id(0), "fixture-session", &transport)
            .await
            .is_err()
    );
    assert!(Lease::acquire_fixture(
        session.clone(),
        "unknown-account",
        "fixture-session",
        &transport
    )
    .await
    .is_err());
    let lease = Lease::acquire_fixture(
        session.clone(),
        owner.account_id(0),
        "fixture-session",
        &transport,
    )
    .await
    .unwrap();
    assert_eq!(transport.attempts(), 0);
    session.set_blocked(true);
    session.set_blocked(false);
    assert!(lease
        .dispatch_fixture(body(), Duration::from_secs(1), &transport)
        .await
        .is_err());
    assert_eq!(transport.attempts(), 0);
}
#[tokio::test]
async fn two_existing_accounts_supply_their_own_start_jwt_and_preserve_messages() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    net.responses
        .lock()
        .unwrap()
        .extend([response(200), response(200)]);
    for index in 0..2 {
        let out = f
            .lease(index)
            .await
            .unwrap()
            .dispatch_with(body(), Duration::from_secs(3), &net)
            .await
            .unwrap();
        assert_eq!(
            out.response.bytes_with_limit(1024).await.unwrap(),
            Bytes::from_static(b"fixture-body")
        );
    }
    let calls = net.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for (index, call) in calls.iter().enumerate() {
        assert_eq!(call.url, START_MESSAGES_URL);
        assert!(call.streaming);
        assert_eq!(call.timeout, Duration::from_secs(3));
        assert_eq!(call.headers["x-api-key"], f.tokens[index]);
        assert_eq!(
            call.headers[http::header::AUTHORIZATION],
            format!("Bearer {}", f.tokens[index])
        );
        assert!(call.headers["x-api-key"].is_sensitive());
        assert!(call.headers[http::header::AUTHORIZATION].is_sensitive());
        assert_eq!(call.headers["anthropic-version"], "2023-06-01");
        assert_eq!(call.headers["accept-encoding"], "identity");
        assert_eq!(serde_json::from_slice::<Value>(&call.body).unwrap(), body());
        assert!(!String::from_utf8_lossy(&call.body).contains("fixture-business"));
    }
}

#[tokio::test]
async fn success_headers_are_normalized_and_never_echo_credentials_or_cookies() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "content-type",
        format!("application/json; token={}", f.tokens[0])
            .parse()
            .unwrap(),
    );
    headers.insert("authorization", f.tokens[0].parse().unwrap());
    headers.insert("x-api-key", f.tokens[0].parse().unwrap());
    headers.insert(
        "set-cookie",
        format!("token={}", f.tokens[0]).parse().unwrap(),
    );
    headers.insert("x-request-id", f.tokens[0].parse().unwrap());
    headers.insert("content-encoding", "identity".parse().unwrap());
    net.responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::buffered(
            http::StatusCode::OK,
            headers,
            Bytes::from_static(b"normal answer"),
        ));
    let response = f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .unwrap();
    assert_eq!(response.response.headers().len(), 1);
    assert_eq!(
        response.response.headers()["content-type"],
        "application/json"
    );
    assert_eq!(
        response.response.bytes_with_limit(1024).await.unwrap(),
        "normal answer"
    );
}

#[tokio::test]
async fn encoded_or_multi_encoded_success_is_rejected_without_poisoning_account() {
    for encodings in [
        vec!["gzip"],
        vec!["identity", "gzip"],
        vec!["identity, gzip"],
        vec![""],
    ] {
        let f = Fixture::new(u64::MAX);
        let net = TransportFixture::default();
        let mut headers = http::HeaderMap::new();
        for encoding in encodings {
            headers.append("content-encoding", encoding.parse().unwrap());
        }
        net.responses
            .lock()
            .unwrap()
            .push_back(ProxyResponse::buffered(
                http::StatusCode::OK,
                headers,
                Bytes::from(f.tokens[0].clone()),
            ));
        assert!(f
            .lease(0)
            .await
            .unwrap()
            .dispatch_with(body(), Duration::from_secs(3), &net)
            .await
            .is_err());
        assert!(
            f.lease(0).await.is_ok(),
            "encoding failure is not a login failure"
        );
        assert_eq!(net.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn current_start_jwt_is_blocked_across_every_chunk_split_and_buffered_body() {
    let f = Fixture::new(u64::MAX);
    for split in 0..=f.tokens[0].len() {
        let token = f.tokens[0].as_bytes();
        let net = TransportFixture::default();
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(b"safe prefix ")),
            Ok(Bytes::copy_from_slice(&token[..split])),
            Ok(Bytes::copy_from_slice(&token[split..])),
            Ok(Bytes::from_static(b"ignored suffix")),
        ];
        net.responses
            .lock()
            .unwrap()
            .push_back(ProxyResponse::streamed(
                http::StatusCode::OK,
                http::HeaderMap::new(),
                futures::stream::iter(chunks),
            ));
        let response = f
            .lease(0)
            .await
            .unwrap()
            .dispatch_with(body(), Duration::from_secs(3), &net)
            .await
            .unwrap();
        let mut stream = Box::pin(response.response.bytes_stream());
        let mut emitted = Vec::new();
        let mut blocked = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => emitted.extend_from_slice(&bytes),
                Err(error) => {
                    assert!(!error.to_string().contains(&f.tokens[0]));
                    blocked = true;
                }
            }
        }
        assert!(blocked, "split {split}");
        assert_eq!(
            emitted, b"safe prefix ",
            "no token prefix is released at split {split}"
        );
    }
    let net = TransportFixture::default();
    net.responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::buffered(
            http::StatusCode::OK,
            http::HeaderMap::new(),
            Bytes::from(format!("echo {}", f.tokens[0])),
        ));
    assert!(f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .unwrap()
        .response
        .bytes_with_limit(1024)
        .await
        .is_err());
}
#[tokio::test]
async fn expired_jwt_blocks_before_transport_and_persists_only_auth_failure() {
    let f = Fixture::new(1);
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
    let vault = f.session.read().unwrap();
    let native = f.context.cipher().unwrap();
    let status = VaultAccountStore::new(f.session.root(), &vault)
        .unwrap()
        .catalog_status(&native)
        .unwrap();
    let check = status
        .profiles
        .iter()
        .find(|p| p.id == f.ids[0])
        .unwrap()
        .capabilities
        .as_ref()
        .unwrap();
    assert_eq!(
        check.start.check.reason,
        Some(super::super::session_checks::CheckReason::AuthRejected)
    );
    assert_eq!(
        check.start.entitlement,
        super::super::session_checks::EntitlementState::Unknown
    );
    assert!(check.start.buckets.is_empty());
}
#[tokio::test]
async fn unauthorized_is_one_send_no_refresh_and_blocks_same_credentials_on_reacquire() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    net.responses
        .lock()
        .unwrap()
        .extend([response(401), response(200)]);
    let out = f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .unwrap();
    assert_eq!(out.response.status(), http::StatusCode::UNAUTHORIZED);
    assert!(
        !String::from_utf8_lossy(&out.response.bytes_with_limit(1024).await.unwrap())
            .contains("fixture-body")
    );
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
    assert!(f.lease(1).await.is_ok());
}
#[tokio::test]
async fn rate_limit_does_not_mark_credentials_invalid_or_retry_locally() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    net.responses.lock().unwrap().push_back(response(429));
    let out = f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .unwrap();
    assert_eq!(out.response.status(), http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(out.fence.check().await, Ok(()));
    let _ = out.response.bytes_with_limit(1024).await.unwrap();
    assert_eq!(out.fence.check().await, Ok(()));
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    assert!(f.lease(0).await.is_ok());
}
#[tokio::test]
async fn unknown_transport_failure_does_not_change_catalog() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    let before = std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap();
    assert!(f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .is_err());
    assert_eq!(
        before,
        std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap()
    );
}
#[tokio::test]
async fn catalog_replacement_between_acquire_and_dispatch_prevents_any_send() {
    let f = Fixture::new(u64::MAX);
    let lease = f.lease(0).await.unwrap();
    let net = TransportFixture::default();
    f.mutate(|catalog, _| {
        *catalog = ProfileCatalog::default();
    })
    .await;
    assert!(lease
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .is_err());
    assert!(net.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn vault_lock_then_unlock_never_revives_an_old_lease() {
    let f = Fixture::new(u64::MAX);
    let lease = f.lease(0).await.unwrap();
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    assert!(matches!(lease.check().await, Err(Failure::StaleIdentity)));
    assert!(f.lease(0).await.is_ok());
}
#[tokio::test]
async fn response_body_is_fenced_after_headers_and_catalog_retirement() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    net.responses.lock().unwrap().push_back(response(200));
    let out = f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .unwrap();
    f.mutate(|catalog, _| *catalog = ProfileCatalog::default())
        .await;
    assert!(out.response.bytes_with_limit(1024).await.is_err());
}
#[tokio::test]
async fn wrong_context_cannot_borrow_the_selected_vault_authority() {
    let f = Fixture::new(u64::MAX);
    let home = f._root.path().canonicalize().unwrap();
    let wrong = LibraryContext::from_os_identity(
        home.to_str().unwrap(),
        "different-user",
        &home.join("zcode"),
    )
    .unwrap();
    assert!(
        Lease::acquire_in_context(f.session.clone(), wrong, &f.ids[0], "request")
            .await
            .is_err()
    );
}
#[tokio::test]
async fn malformed_messages_never_use_transport() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    assert!(f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(serde_json::json!({"model":"GLM-5.2"}), Duration::ZERO, &net)
        .await
        .is_err());
    assert!(net.calls.lock().unwrap().is_empty());
}

async fn replace_start_token(f: &Fixture, token: &str) {
    let target = f.ids[0].clone();
    f.mutate(|catalog, native| {
        let original = catalog
            .profiles()
            .find(|s| s.identity().opaque_id() == target)
            .unwrap()
            .clone();
        let mut fields: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&original.scoped_document().to_bytes().unwrap()).unwrap();
        for slot in [4, 6] {
            fields.insert(
                original.identity().credential_keys()[slot].clone(),
                native.encrypt(token).unwrap(),
            );
        }
        let updated = native
            .inspect(
                &super::super::core::CredentialDocument::parse(
                    &serde_json::to_vec(&fields).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(updated.identity().opaque_id(), target);
        catalog.upsert(updated);
    })
    .await;
}

#[tokio::test]
async fn replaced_start_credential_for_same_identity_clears_rejection_without_coding_key() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    net.responses.lock().unwrap().push_back(response(401));
    f.lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::ZERO, &net)
        .await
        .unwrap();
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
    replace_start_token(&f, &jwt(u64::MAX, "relogin")).await;
    assert!(f.lease(0).await.is_ok());
    let vault = f.session.read().unwrap();
    let native = f.context.cipher().unwrap();
    let store = VaultAccountStore::new(f.session.root(), &vault).unwrap();
    let status = store.catalog_status(&native).unwrap();
    let catalog = store.import_catalog(&native, &status.revision).unwrap();
    let account = catalog
        .profiles()
        .find(|s| s.identity().opaque_id() == f.ids[0])
        .unwrap();
    assert!(super::super::oauth_account::saved_coding_key(&native, account).is_none());
}

struct Delayed401 {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl StreamTransport for Delayed401 {
    fn request(
        &self,
        _: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            Ok(response(401))
        })
    }
}

#[tokio::test]
async fn late_unauthorized_cannot_mark_a_new_login_as_rejected() {
    let f = Fixture::new(u64::MAX);
    let net = Arc::new(Delayed401 {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let request_net = net.clone();
    let lease = f.lease(0).await.unwrap();
    let request = tokio::spawn(async move {
        lease
            .dispatch_with(body(), Duration::ZERO, request_net.as_ref())
            .await
    });
    net.started.notified().await;
    replace_start_token(&f, &jwt(u64::MAX, "new-login")).await;
    let before = std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap();
    net.release.notify_one();
    assert!(request.await.unwrap().is_err());
    assert_eq!(
        before,
        std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap()
    );
    assert!(f.lease(0).await.is_ok());
}

#[tokio::test]
async fn expired_global_jwt_never_falls_back_to_fresh_account_jwt() {
    let f = Fixture::new(u64::MAX);
    let target = f.ids[0].clone();
    f.mutate(|catalog, native| {
        let original = catalog
            .profiles()
            .find(|s| s.identity().opaque_id() == target)
            .unwrap()
            .clone();
        let mut fields: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&original.scoped_document().to_bytes().unwrap()).unwrap();
        fields.insert(
            original.identity().credential_keys()[4].clone(),
            native.encrypt(&jwt(1, "expired-global")).unwrap(),
        );
        let updated = native
            .inspect(
                &super::super::core::CredentialDocument::parse(
                    &serde_json::to_vec(&fields).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        catalog.upsert(updated);
    })
    .await;
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
}

#[tokio::test]
async fn rejection_fence_tracks_only_its_own_write_and_never_revives_request_leases() {
    let f = Fixture::new(u64::MAX);
    let lease = f.lease(0).await.unwrap();
    let original = lease.clone();
    let previously_acquired_other = f.lease(1).await.unwrap();
    let net = TransportFixture::default();
    net.responses.lock().unwrap().push_back(response(401));
    let result = lease
        .dispatch_with(body(), Duration::ZERO, &net)
        .await
        .unwrap();
    assert_eq!(result.response.status(), http::StatusCode::UNAUTHORIZED);
    assert_ne!(
        result.fence.lease.binding.catalog_revision,
        original.binding.catalog_revision
    );
    let updated = &result.fence.lease.binding;
    let before = &original.binding;
    assert_eq!(updated.storage, before.storage);
    assert_eq!(updated.epoch, before.epoch);
    assert_eq!(updated.account_id, before.account_id);
    assert_eq!(updated.session_binding, before.session_binding);
    assert_eq!(
        result.fence.lease.context.context_id(),
        original.context.context_id()
    );
    let committed = std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap();
    // Both the status check and the post-error-body check are read-only.
    assert_eq!(result.fence.check().await, Ok(()));
    let sanitized = result.response.bytes_with_limit(1024).await.unwrap();
    assert!(!String::from_utf8_lossy(&sanitized).contains(&f.tokens[0]));
    assert!(!String::from_utf8_lossy(&sanitized).contains("fixture-body"));
    assert_eq!(result.fence.check().await, Ok(()));
    assert_eq!(
        committed,
        std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap()
    );
    assert_eq!(original.check().await, Err(Failure::StaleIdentity));
    assert_eq!(
        previously_acquired_other.check().await,
        Err(Failure::StaleIdentity)
    );
    assert!(original
        .dispatch_with(body(), Duration::ZERO, &net)
        .await
        .is_err());
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
    assert!(f.lease(1).await.is_ok());
}

async fn reject_with_fence(f: &Fixture, index: usize) -> DispatchResponse {
    let net = TransportFixture::default();
    net.responses.lock().unwrap().push_back(response(401));
    let result = f
        .lease(index)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::ZERO, &net)
        .await
        .unwrap();
    assert_eq!(result.response.status(), http::StatusCode::UNAUTHORIZED);
    assert_eq!(result.fence.check().await, Ok(()));
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    result
}

#[tokio::test]
async fn rejection_fence_remains_closed_after_external_catalog_retirement() {
    let f = Fixture::new(u64::MAX);
    let result = reject_with_fence(&f, 0).await;
    f.mutate(|catalog, _| *catalog = ProfileCatalog::default())
        .await;
    assert_eq!(result.fence.check().await, Err(Failure::StaleIdentity));
    assert_eq!(result.fence.check().await, Err(Failure::StaleIdentity));
}

#[tokio::test]
async fn rejection_fence_remains_closed_after_lock_and_unlock() {
    let f = Fixture::new(u64::MAX);
    let result = reject_with_fence(&f, 0).await;
    f.session.set_blocked(true);
    assert_eq!(result.fence.check().await, Err(Failure::StaleIdentity));
    f.session.set_blocked(false);
    assert_eq!(result.fence.check().await, Err(Failure::StaleIdentity));
    assert!(f.lease(1).await.is_ok());
}

#[tokio::test]
async fn rejection_fence_cannot_adopt_same_identity_relogin() {
    let f = Fixture::new(u64::MAX);
    let result = reject_with_fence(&f, 0).await;
    replace_start_token(&f, &jwt(u64::MAX, "new-login-after-rejection")).await;
    assert_eq!(result.fence.check().await, Err(Failure::StaleIdentity));
    assert!(f.lease(0).await.is_ok());
}

#[tokio::test]
async fn rejection_fence_cannot_adopt_another_profiles_catalog_write() {
    let f = Fixture::new(u64::MAX);
    let result = reject_with_fence(&f, 0).await;
    let other = f.ids[1].clone();
    f.mutate(|catalog, native| {
        let identity = catalog
            .profiles()
            .find(|snapshot| snapshot.identity().opaque_id() == other)
            .unwrap()
            .identity()
            .clone();
        catalog
            .set_label(&identity, native, Some("Changed other profile".into()))
            .unwrap();
    })
    .await;
    let changed = std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap();
    assert_eq!(result.fence.check().await, Err(Failure::StaleIdentity));
    assert_eq!(
        changed,
        std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap()
    );
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
    assert!(f.lease(1).await.is_ok());
}

#[tokio::test]
async fn separate_account_rejections_never_advance_an_earlier_response_fence() {
    let f = Fixture::new(u64::MAX);
    let first = reject_with_fence(&f, 0).await;
    let second = reject_with_fence(&f, 1).await;
    assert_eq!(first.fence.check().await, Err(Failure::StaleIdentity));
    assert_eq!(second.fence.check().await, Ok(()));
    assert!(matches!(f.lease(0).await, Err(Failure::NeedsLogin)));
    assert!(matches!(f.lease(1).await, Err(Failure::NeedsLogin)));
}

#[tokio::test]
async fn late_unauthorized_after_lock_unlock_does_not_publish_rejection() {
    let f = Fixture::new(u64::MAX);
    let net = Arc::new(Delayed401 {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let request_net = net.clone();
    let lease = f.lease(0).await.unwrap();
    let request = tokio::spawn(async move {
        lease
            .dispatch_with(body(), Duration::ZERO, request_net.as_ref())
            .await
    });
    net.started.notified().await;
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    let before = std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap();
    net.release.notify_one();
    assert!(request.await.unwrap().is_err());
    assert_eq!(
        before,
        std::fs::read(f.session.root().join(PROFILE_FILE)).unwrap()
    );
    assert!(f.lease(0).await.is_ok());
    assert!(f.lease(1).await.is_ok());
}

#[tokio::test]
async fn explicit_denial_401_does_not_mark_original_account_needs_login() {
    let f = Fixture::new(u64::MAX);
    let net = TransportFixture::default();
    net.responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::buffered(
            http::StatusCode::UNAUTHORIZED,
            http::HeaderMap::new(),
            Bytes::from_static(
                br#"{"error":{"type":"permission_error","message":"fixture-secret"}}"#,
            ),
        ));
    let out = f
        .lease(0)
        .await
        .unwrap()
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .unwrap();
    assert_eq!(out.response.status().as_u16(), 401);
    let bytes = out.response.bytes_with_limit(4096).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["type"], "access_denied");
    assert!(!String::from_utf8_lossy(&bytes).contains("fixture-secret"));
    assert!(!String::from_utf8_lossy(&bytes).contains("needs_login"));
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    assert!(f.lease(0).await.is_ok());
    assert!(f.lease(1).await.is_ok());
}

#[tokio::test]
async fn error_body_epoch_change_cannot_reject_original_account() {
    let f = Fixture::new(u64::MAX);
    let lease = f.lease(0).await.unwrap();
    let session = f.session.clone();
    let net = TransportFixture::default();
    net.responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::streamed(
            http::StatusCode::UNAUTHORIZED,
            http::HeaderMap::new(),
            futures::stream::once(async move {
                session.set_blocked(true);
                session.set_blocked(false);
                Ok(Bytes::from_static(b"{}"))
            }),
        ));
    assert!(lease
        .dispatch_with(body(), Duration::from_secs(3), &net)
        .await
        .is_err());
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    assert!(f.lease(0).await.is_ok());
}
