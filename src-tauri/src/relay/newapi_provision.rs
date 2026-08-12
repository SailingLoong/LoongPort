//! NewAPI remote group/token reconciliation.
//!
//! This module owns only the backend-specific remote lifecycle. Local provider
//! persistence deliberately remains with the command layer.

use std::collections::{HashMap, HashSet};
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::relay::newapi::{Group, GroupIdentity, NewApiClient, Token};

const MANAGED_TOKEN_PREFIX: &str = "LoongPort/napi";
const NEWAPI_TOKEN_NAME_LIMIT: usize = 50;

/// The remote operation that failed for one group or stale token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileStage {
    Create,
    Relist,
    Reveal,
    DeleteStale,
}

/// A non-secret per-group reconciliation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileFailure {
    pub group_identity: Option<GroupIdentity>,
    pub stage: ReconcileStage,
    pub reason: String,
}

/// A remote NewAPI group with its revealed credential.
///
/// `Debug` intentionally omits `api_key`: callers must handle that value as a
/// secret rather than allowing diagnostics to print it.
#[derive(Clone, PartialEq)]
pub struct ReconciledGroup {
    pub identity: GroupIdentity,
    pub name: String,
    pub rate_multiplier: Option<f64>,
    pub description: String,
    pub api_key: String,
    pub token_was_created: bool,
}

impl fmt::Debug for ReconciledGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconciledGroup")
            .field("identity", &self.identity)
            .field("name", &self.name)
            .field("rate_multiplier", &self.rate_multiplier)
            .field("description", &self.description)
            .field("token_was_created", &self.token_was_created)
            .finish_non_exhaustive()
    }
}

/// Backend-neutral input for the later local-provider persistence slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconcileResult {
    pub account_id: i64,
    /// Complete raw group inventory, including groups whose token reveal failed.
    pub observed_groups: Vec<GroupIdentity>,
    pub groups: Vec<ReconciledGroup>,
    pub failures: Vec<ReconcileFailure>,
    pub tokens_created: usize,
    pub stale_tokens_deleted: usize,
}

#[derive(Clone)]
struct CanonicalToken {
    group: Group,
    token: Token,
    duplicate_tokens: Vec<Token>,
    token_was_created: bool,
}

/// Returns the NewAPI-only managed token name for one account and raw group.
///
/// The digest is encoded as URL-safe base64 and clipped to the remaining
/// server-side name budget. This retains at least 78 bits for every valid i64
/// account id while never embedding arbitrary group text in the token name.
fn managed_token_name(account_id: i64, group: &GroupIdentity) -> String {
    let prefix = format!("{MANAGED_TOKEN_PREFIX}/a{account_id}/");
    let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(group.0.as_bytes()));
    let digest_len = NEWAPI_TOKEN_NAME_LIMIT.saturating_sub(prefix.len());
    format!("{prefix}{}", &digest[..digest_len])
}

fn is_enabled(token: &Token) -> bool {
    token.status == 1
}

fn canonical_token(
    tokens: &[Token],
    account_id: i64,
    group: &Group,
) -> Option<(Token, Vec<Token>)> {
    let name = managed_token_name(account_id, &group.identity);
    let mut matches = tokens
        .iter()
        .filter(|token| token.name == name && token.group == group.identity.0)
        .cloned()
        .collect::<Vec<_>>();
    let canonical = matches
        .iter()
        .filter(|token| is_enabled(token))
        .max_by_key(|token| token.id)
        .cloned()?;
    matches.retain(|token| token.id != canonical.id);
    Some((canonical, matches))
}

fn belongs_to_current_account(token: &Token, account_id: i64) -> bool {
    token.name == managed_token_name(account_id, &GroupIdentity(token.group.clone()))
}

fn failure(
    group: Option<GroupIdentity>,
    stage: ReconcileStage,
    error: impl fmt::Display,
) -> ReconcileFailure {
    ReconcileFailure {
        group_identity: group,
        stage,
        reason: error.to_string(),
    }
}

/// Reconciles remote NewAPI tokens with the authenticated account's current
/// raw group inventory. It never writes local cc-switch providers.
#[cfg(test)]
async fn reconcile(client: &NewApiClient) -> Result<ReconcileResult, AppError> {
    let account = client.account().await?;
    reconcile_for_account(client, account.id).await
}

/// Reconcile after the caller has verified which persisted relay account owns
/// this authenticated NewAPI session. No group or token request is made before
/// that account preflight succeeds.
pub async fn reconcile_for_account(
    client: &NewApiClient,
    account_id: i64,
) -> Result<ReconcileResult, AppError> {
    let groups = client.groups().await?;
    let observed_groups = groups
        .iter()
        .map(|group| group.identity.clone())
        .collect::<Vec<_>>();
    let observed_set = observed_groups.iter().cloned().collect::<HashSet<_>>();
    let initial_tokens = client.list_tokens().await?;

    let mut result = ReconcileResult {
        account_id,
        observed_groups,
        groups: Vec::new(),
        failures: Vec::new(),
        tokens_created: 0,
        stale_tokens_deleted: 0,
    };
    let mut canonical = Vec::new();
    let mut missing_groups = Vec::new();

    for group in &groups {
        if let Some((token, duplicates)) = canonical_token(&initial_tokens, account_id, group) {
            canonical.push(CanonicalToken {
                group: group.clone(),
                token,
                duplicate_tokens: duplicates,
                token_was_created: false,
            });
        } else {
            missing_groups.push(group.clone());
        }
    }

    let mut created_groups = Vec::new();
    for group in missing_groups {
        match client
            .create_token(
                &managed_token_name(account_id, &group.identity),
                &group.identity.0,
            )
            .await
        {
            Ok(()) => {
                result.tokens_created += 1;
                created_groups.push(group);
            }
            Err(error) => {
                result
                    .failures
                    .push(failure(Some(group.identity), ReconcileStage::Create, error))
            }
        }
    }

    let mut cleanup_tokens = initial_tokens.clone();
    if !created_groups.is_empty() {
        match client.list_tokens().await {
            Ok(tokens) => {
                cleanup_tokens = tokens.clone();
                for group in created_groups {
                    if let Some((token, duplicates)) = canonical_token(&tokens, account_id, &group)
                    {
                        canonical.push(CanonicalToken {
                            group,
                            token,
                            duplicate_tokens: duplicates,
                            token_was_created: true,
                        });
                    } else {
                        result.failures.push(ReconcileFailure {
                            group_identity: Some(group.identity),
                            stage: ReconcileStage::Relist,
                            reason: "newapi token create 后未找到可用 token".into(),
                        });
                    }
                }
            }
            Err(error) => {
                for group in created_groups {
                    result.failures.push(failure(
                        Some(group.identity),
                        ReconcileStage::Relist,
                        &error,
                    ));
                }
            }
        }
    }

    let removed_group_tokens = cleanup_tokens
        .into_iter()
        .filter(|token| {
            belongs_to_current_account(token, account_id)
                && !observed_set.contains(&GroupIdentity(token.group.clone()))
        })
        .collect::<Vec<_>>();
    delete_stale_tokens(client, &mut result, removed_group_tokens).await;

    let mut revealed_duplicate_tokens = Vec::new();
    for item in canonical {
        match client.reveal_token(item.token.id).await {
            Ok(api_key) => {
                revealed_duplicate_tokens.extend(item.duplicate_tokens);
                result.groups.push(ReconciledGroup {
                    identity: item.group.identity,
                    name: item.group.name,
                    rate_multiplier: item.group.rate_multiplier,
                    description: item.group.description,
                    api_key,
                    token_was_created: item.token_was_created,
                });
            }
            Err(error) => result.failures.push(failure(
                Some(item.group.identity),
                ReconcileStage::Reveal,
                error,
            )),
        }
    }
    delete_stale_tokens(client, &mut result, revealed_duplicate_tokens).await;

    Ok(result)
}

async fn delete_stale_tokens(
    client: &NewApiClient,
    result: &mut ReconcileResult,
    tokens: Vec<Token>,
) {
    let tokens = tokens
        .into_iter()
        .map(|token| (token.id, token))
        .collect::<HashMap<_, _>>();
    let mut token_ids = tokens.keys().copied().collect::<Vec<_>>();
    token_ids.sort_unstable();
    for token_id in token_ids {
        match client.delete_token(token_id).await {
            Ok(()) => result.stale_tokens_deleted += 1,
            Err(error) => result.failures.push(failure(
                tokens
                    .get(&token_id)
                    .map(|token| GroupIdentity(token.group.clone())),
                ReconcileStage::DeleteStale,
                error,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use bytes::Bytes;
    use http::{Response, StatusCode};
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;
    use crate::relay::newapi::{GroupIdentity, NewApiClient};

    #[derive(Clone)]
    struct TestToken {
        id: i64,
        name: String,
        group: String,
        status: i64,
    }

    struct TestState {
        account_id: i64,
        groups: Vec<(String, f64, String)>,
        tokens: Vec<TestToken>,
        next_token_id: i64,
        created: Vec<(String, String)>,
        deleted: Vec<i64>,
        create_failures: HashSet<String>,
        reveal_failures: HashSet<i64>,
        delete_failures: HashSet<i64>,
        relist_failure: bool,
        token_list_requests: usize,
    }

    impl TestState {
        fn new(account_id: i64, groups: &[&str], tokens: Vec<TestToken>) -> Self {
            let next_token_id = tokens.iter().map(|token| token.id).max().unwrap_or(0) + 1;
            Self {
                account_id,
                groups: groups
                    .iter()
                    .map(|group| ((*group).to_string(), 1.0, format!("{group} description")))
                    .collect(),
                tokens,
                next_token_id,
                created: Vec::new(),
                deleted: Vec::new(),
                create_failures: HashSet::new(),
                reveal_failures: HashSet::new(),
                delete_failures: HashSet::new(),
                relist_failure: false,
                token_list_requests: 0,
            }
        }
    }

    struct TestServer {
        origin: String,
        state: Arc<Mutex<TestState>>,
    }

    impl TestServer {
        async fn spawn(state: TestState) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test listener");
            let origin = format!("http://{}", listener.local_addr().expect("local addr"));
            let state = Arc::new(Mutex::new(state));
            let state_for_task = Arc::clone(&state);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let state = Arc::clone(&state_for_task);
                    tokio::spawn(async move {
                        let service = service_fn(move |request| {
                            let state = Arc::clone(&state);
                            async move {
                                Ok::<_, std::convert::Infallible>(handle(request, state).await)
                            }
                        });
                        let _ = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });
            Self { origin, state }
        }
    }

    async fn handle(
        request: http::Request<Incoming>,
        state: Arc<Mutex<TestState>>,
    ) -> Response<Full<Bytes>> {
        let path = request.uri().path().to_string();
        let method = request.method().clone();
        let body = request
            .into_body()
            .collect()
            .await
            .expect("read request body")
            .to_bytes();
        let mut state = state.lock().await;

        let response = match (method.as_str(), path.as_str()) {
            ("GET", "/api/user/self") => json!({
                "success": true,
                "data": {
                    "id": state.account_id,
                    "username": "relay-user",
                    "display_name": "Relay User",
                    "email": "relay@example.test",
                    "group": "default",
                    "quota": 0,
                    "used_quota": 0
                }
            }),
            ("GET", "/api/user/self/groups") => {
                let groups = state
                    .groups
                    .iter()
                    .map(|(name, ratio, description)| {
                        (name.clone(), json!({ "ratio": ratio, "desc": description }))
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({ "success": true, "data": groups })
            }
            ("GET", "/api/token/") => {
                state.token_list_requests += 1;
                if state.relist_failure && state.token_list_requests == 2 {
                    json!({ "success": false, "message": "relist failed" })
                } else {
                    let items = state
                        .tokens
                        .iter()
                        .map(|token| {
                            json!({
                                "id": token.id,
                                "name": token.name,
                                "key": "sk-masked",
                                "status": token.status,
                                "group": token.group
                            })
                        })
                        .collect::<Vec<_>>();
                    json!({
                        "success": true,
                        "data": {
                            "page": 1,
                            "page_size": 100,
                            "total": items.len(),
                            "items": items
                        }
                    })
                }
            }
            ("POST", "/api/token/") => {
                let payload: serde_json::Value =
                    serde_json::from_slice(&body).expect("valid create payload");
                let name = payload["name"].as_str().expect("create name").to_string();
                let group = payload["group"].as_str().expect("create group").to_string();
                if state.create_failures.contains(&group) {
                    json!({ "success": false, "message": "create failed" })
                } else {
                    let id = state.next_token_id;
                    state.next_token_id += 1;
                    state.created.push((name.clone(), group.clone()));
                    state.tokens.push(TestToken {
                        id,
                        name,
                        group,
                        status: 1,
                    });
                    json!({ "success": true })
                }
            }
            ("GET", path) if path.starts_with("/api/token/") && path.ends_with("/key") => {
                let id = path
                    .trim_start_matches("/api/token/")
                    .trim_end_matches("/key")
                    .trim_end_matches('/')
                    .parse::<i64>()
                    .expect("token id");
                if state.reveal_failures.contains(&id) {
                    json!({
                        "success": false,
                        "message": "revealed-token-secret-must-not-leak",
                        "data": { "key": "revealed-token-secret-must-not-leak" }
                    })
                } else {
                    json!({ "success": true, "data": { "key": format!("revealed-{id}") } })
                }
            }
            ("DELETE", path) if path.starts_with("/api/token/") => {
                let id = path
                    .trim_start_matches("/api/token/")
                    .parse::<i64>()
                    .expect("token id");
                state.deleted.push(id);
                if state.delete_failures.contains(&id) {
                    json!({ "success": false, "message": "delete failed" })
                } else {
                    state.tokens.retain(|token| token.id != id);
                    json!({ "success": true })
                }
            }
            _ => {
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from("not found")))
                    .expect("not found response");
            }
        };

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(response.to_string())))
            .expect("json response")
    }

    fn token(id: i64, name: String, group: &str, status: i64) -> TestToken {
        TestToken {
            id,
            name,
            group: group.into(),
            status,
        }
    }

    #[test]
    fn managed_token_names_preserve_raw_unicode_group_identity_without_exceeding_limit() {
        let names = [
            "tier with spaces",
            "tier/slash",
            "中文分组",
            "emoji-🚀",
            "very-long-group-name-very-long-group-name-very-long-group-name",
            " vip ",
            "vip",
        ]
        .map(|raw| managed_token_name(41, &GroupIdentity(raw.into())));

        assert!(names
            .iter()
            .all(|name| name.starts_with("LoongPort/napi/a41/")));
        assert!(names.iter().all(|name| name.len() <= 50));
        assert_eq!(
            names[5],
            managed_token_name(41, &GroupIdentity(" vip ".into()))
        );
        assert_ne!(names[5], names[6]);
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), names.len());
    }

    #[test]
    fn account_scoped_managed_token_names_do_not_overlap() {
        let identity = GroupIdentity("shared group".into());
        assert_ne!(
            managed_token_name(7, &identity),
            managed_token_name(8, &identity)
        );
    }

    #[tokio::test]
    async fn reconcile_claims_existing_exact_enabled_token_without_creating_on_repeat() {
        let group = GroupIdentity("alpha".into());
        let server = TestServer::spawn(TestState::new(
            7,
            &["alpha"],
            vec![token(10, managed_token_name(7, &group), "alpha", 1)],
        ))
        .await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let first = reconcile(&client).await.unwrap();
        let second = reconcile(&client).await.unwrap();

        assert_eq!(first.groups.len(), 1);
        assert!(!first.groups[0].token_was_created);
        assert_eq!(second.groups.len(), 1);
        assert!(!second.groups[0].token_was_created);
        assert!(second.groups[0].api_key.starts_with("revealed-"));
        assert!(server.state.lock().await.created.is_empty());
    }

    #[tokio::test]
    async fn reconcile_does_not_claim_disabled_or_wrong_group_same_name_tokens() {
        let group = GroupIdentity("alpha".into());
        let name = managed_token_name(7, &group);
        let server = TestServer::spawn(TestState::new(
            7,
            &["alpha"],
            vec![
                token(10, name.clone(), "other", 1),
                token(11, name, "alpha", 0),
            ],
        ))
        .await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();

        assert_eq!(result.groups.len(), 1);
        assert!(result.groups[0].token_was_created);
        assert_eq!(server.state.lock().await.created.len(), 1);
    }

    #[tokio::test]
    async fn reconcile_deletes_disabled_matching_token_after_replacement_reveal() {
        let group = GroupIdentity("alpha".into());
        let name = managed_token_name(7, &group);
        let server = TestServer::spawn(TestState::new(
            7,
            &["alpha"],
            vec![token(11, name, "alpha", 0)],
        ))
        .await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();
        let state = server.state.lock().await;

        assert_eq!(result.groups.len(), 1);
        assert!(result.groups[0].token_was_created);
        assert_eq!(state.deleted, vec![11]);
        assert!(!state.tokens.iter().any(|token| token.id == 11));
    }

    #[tokio::test]
    async fn reconcile_retains_disabled_matching_token_when_replacement_reveal_fails() {
        let group = GroupIdentity("alpha".into());
        let name = managed_token_name(7, &group);
        let mut state = TestState::new(7, &["alpha"], vec![token(11, name, "alpha", 0)]);
        state.reveal_failures.insert(12);
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();
        let state = server.state.lock().await;

        assert!(result.groups.is_empty());
        assert_eq!(result.failures[0].stage, ReconcileStage::Reveal);
        assert!(state.deleted.is_empty());
        assert!(state.tokens.iter().any(|token| token.id == 11));
        assert!(state.tokens.iter().any(|token| token.id == 12));
    }

    #[tokio::test]
    async fn reconcile_chooses_highest_duplicate_and_deletes_other_enabled_match() {
        let group = GroupIdentity("alpha".into());
        let name = managed_token_name(7, &group);
        let server = TestServer::spawn(TestState::new(
            7,
            &["alpha"],
            vec![
                token(10, name.clone(), "alpha", 1),
                token(12, name, "alpha", 1),
            ],
        ))
        .await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let first = reconcile(&client).await.unwrap();
        let second = reconcile(&client).await.unwrap();
        let state = server.state.lock().await;

        assert!(first.groups[0].api_key.ends_with("12"));
        assert_eq!(state.deleted, vec![10]);
        assert_eq!(
            state
                .tokens
                .iter()
                .map(|token| token.id)
                .collect::<Vec<_>>(),
            vec![12]
        );
        assert_eq!(second.stale_tokens_deleted, 0);
    }

    #[tokio::test]
    async fn reconcile_creates_all_missing_groups_then_relists_once_before_revealing() {
        let server = TestServer::spawn(TestState::new(7, &["alpha", "beta"], Vec::new())).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();
        let state = server.state.lock().await;

        assert_eq!(result.groups.len(), 2);
        assert!(result.groups.iter().all(|group| group.token_was_created));
        assert_eq!(state.created.len(), 2);
        assert_eq!(state.token_list_requests, 2);
    }

    #[tokio::test]
    async fn reconcile_keeps_reveal_failure_in_observed_inventory_without_staling_canonical_token()
    {
        let alpha = GroupIdentity("alpha".into());
        let beta = GroupIdentity("beta".into());
        let mut state = TestState::new(
            7,
            &["alpha", "beta"],
            vec![
                token(10, managed_token_name(7, &alpha), "alpha", 1),
                token(11, managed_token_name(7, &beta), "beta", 1),
            ],
        );
        state.reveal_failures.insert(10);
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();

        assert_eq!(result.observed_groups, vec![alpha, beta]);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].stage, ReconcileStage::Reveal);
        assert!(server.state.lock().await.deleted.is_empty());
    }

    #[tokio::test]
    async fn reconcile_retains_enabled_duplicate_when_higher_canonical_reveal_fails() {
        let alpha = GroupIdentity("alpha".into());
        let beta = GroupIdentity("beta".into());
        let alpha_name = managed_token_name(7, &alpha);
        let mut state = TestState::new(
            7,
            &["alpha", "beta"],
            vec![
                token(10, alpha_name.clone(), "alpha", 1),
                token(12, alpha_name, "alpha", 1),
                token(13, managed_token_name(7, &beta), "beta", 1),
            ],
        );
        state.reveal_failures.insert(12);
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();
        let state = server.state.lock().await;

        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.failures[0].stage, ReconcileStage::Reveal);
        assert!(state.deleted.is_empty());
        assert!(state.tokens.iter().any(|token| token.id == 10));
    }

    #[tokio::test]
    async fn reconcile_reports_one_group_create_failure_while_reconciling_another_group() {
        let mut state = TestState::new(7, &["alpha", "beta"], Vec::new());
        state.create_failures.insert("alpha".into());
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();

        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].identity, GroupIdentity("beta".into()));
        assert_eq!(result.failures.len(), 1);
        assert_eq!(
            result.failures[0].group_identity,
            Some(GroupIdentity("alpha".into()))
        );
        assert_eq!(result.failures[0].stage, ReconcileStage::Create);
    }

    #[tokio::test]
    async fn reconcile_returns_observed_inventory_when_every_group_create_fails() {
        let mut state = TestState::new(7, &["alpha"], Vec::new());
        state.create_failures.insert("alpha".into());
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();

        assert_eq!(result.observed_groups, vec![GroupIdentity("alpha".into())]);
        assert!(result.groups.is_empty());
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].stage, ReconcileStage::Create);
    }

    #[tokio::test]
    async fn reconcile_keeps_existing_success_when_post_create_relist_fails() {
        let alpha = GroupIdentity("alpha".into());
        let mut state = TestState::new(
            7,
            &["alpha", "beta"],
            vec![token(10, managed_token_name(7, &alpha), "alpha", 1)],
        );
        state.relist_failure = true;
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();
        let state = server.state.lock().await;

        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].identity, alpha);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(
            result.failures[0].group_identity,
            Some(GroupIdentity("beta".into()))
        );
        assert_eq!(result.failures[0].stage, ReconcileStage::Relist);
        assert_eq!(state.token_list_requests, 2);
    }

    #[tokio::test]
    async fn reconcile_deletes_managed_token_for_removed_group() {
        let alpha = GroupIdentity("alpha".into());
        let removed = GroupIdentity("removed".into());
        let server = TestServer::spawn(TestState::new(
            7,
            &["alpha"],
            vec![
                token(10, managed_token_name(7, &alpha), "alpha", 1),
                token(11, managed_token_name(7, &removed), "removed", 1),
            ],
        ))
        .await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();

        assert_eq!(result.groups.len(), 1);
        assert_eq!(server.state.lock().await.deleted, vec![11]);
    }

    #[tokio::test]
    async fn reconcile_reports_stale_deletion_failure_without_losing_success() {
        let alpha = GroupIdentity("alpha".into());
        let removed = GroupIdentity("removed".into());
        let mut state = TestState::new(
            7,
            &["alpha"],
            vec![
                token(10, managed_token_name(7, &alpha), "alpha", 1),
                token(11, managed_token_name(7, &removed), "removed", 1),
            ],
        );
        state.delete_failures.insert(11);
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();

        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].stage, ReconcileStage::DeleteStale);
    }

    #[tokio::test]
    async fn reconcile_debug_and_errors_do_not_expose_access_or_revealed_token_secrets() {
        let alpha = GroupIdentity("alpha".into());
        let beta = GroupIdentity("beta".into());
        let mut state = TestState::new(
            7,
            &["alpha", "beta"],
            vec![
                token(10, managed_token_name(7, &alpha), "alpha", 1),
                token(11, managed_token_name(7, &beta), "beta", 1),
            ],
        );
        state.reveal_failures.insert(10);
        let server = TestServer::spawn(state).await;
        let client = NewApiClient::new(&server.origin, "access-token-secret").unwrap();

        let result = reconcile(&client).await.unwrap();
        let debug = format!("{result:?}");

        assert!(!debug.contains("access-token-secret"));
        assert!(!debug.contains("revealed-11"));
        assert!(!debug.contains("revealed-token-secret-must-not-leak"));
        assert!(!result.failures[0]
            .reason
            .contains("revealed-token-secret-must-not-leak"));
    }
}
