use super::super::key_intent::{IntentState, KeyIntentLedger};
use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    ledger: KeyIntentLedger,
    events: Vec<&'static str>,
    cancel_on_reserve: Option<(LoginFlowStore, String)>,
    cancel_on_save: Option<(LoginFlowStore, String)>,
    save_error: Option<StoreFailure>,
    catalog_revision: String,
    duplicate: bool,
    receipt_error: bool,
    receipt: Option<super::super::checkpoint::LoginReceipt>,
    commit_unknown: bool,
    clear_error: bool,
    clear_commit_unknown: bool,
    expire_on_save: Option<LoginFlowStore>,
    intent_error: bool,
    checks: usize,
    cancel_after_check: Option<(usize, LoginFlowStore, String)>,
    expire_on_check: bool,
    saved_completion_target: Option<String>,
    saved_revision: Option<String>,
}
#[derive(Clone, Default)]
struct Store(Arc<Mutex<State>>);
impl LoginPersistence for Store {
    fn check(&self, _: &VaultBinding) -> Result<(), StoreFailure> {
        let mut state = self.0.lock().unwrap();
        state.checks += 1;
        if state
            .cancel_after_check
            .as_ref()
            .is_some_and(|(at, _, _)| *at == state.checks)
        {
            let (_, flows, id) = state.cancel_after_check.take().unwrap();
            if state.expire_on_check {
                flows
                    .expire_due(Instant::now() + std::time::Duration::from_secs(601))
                    .unwrap();
            } else {
                flows.cancel(&id, Instant::now()).unwrap();
            }
        }
        Ok(())
    }
    fn catalog<'a>(
        &'a self,
        _: &'a VaultBinding,
        _: &'a LibraryContext,
        _: &'a AccountSnapshot,
    ) -> StoreFuture<'a, CatalogView> {
        Box::pin(async {
            let state = self.0.lock().unwrap();
            Ok(CatalogView {
                revision: if state.catalog_revision.is_empty() {
                    "synthetic-revision".into()
                } else {
                    state.catalog_revision.clone()
                },
                duplicate: state.duplicate,
            })
        })
    }
    fn reserve<'a>(&'a self, _: &'a VaultBinding, scope: KeyScope) -> StoreFuture<'a, Reservation> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.events.push("reserve");
            let result = state.ledger.reserve(scope).unwrap();
            if let Some((flows, id)) = state.cancel_on_reserve.take() {
                flows.cancel(&id, Instant::now()).unwrap();
            }
            Ok(result)
        })
    }
    fn intent<'a>(
        &'a self,
        _: &'a VaultBinding,
        scope: &'a KeyScope,
    ) -> StoreFuture<'a, Option<KeyIntent>> {
        Box::pin(async move {
            let state = self.0.lock().unwrap();
            if state.intent_error {
                Err(StoreFailure::Unavailable)
            } else {
                Ok(state.ledger.get(scope).cloned())
            }
        })
    }
    fn mark_created<'a>(
        &'a self,
        _: &'a VaultBinding,
        grant: Arc<FreshIntent>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.events.push("created");
            state.ledger.mark_created(&grant).unwrap();
            Ok(())
        })
    }
    fn clear_unsubmitted<'a>(
        &'a self,
        _: &'a VaultBinding,
        grant: Arc<FreshIntent>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.events.push("not-sent");
            if state.clear_error {
                return Err(StoreFailure::Unavailable);
            }
            if state.ledger.contains_request(&grant.receipt()) {
                state.ledger.clear_unsubmitted(&grant).unwrap();
            }
            if state.clear_commit_unknown {
                return Err(StoreFailure::Unknown);
            }
            Ok(())
        })
    }
    fn clear_resolved<'a>(
        &'a self,
        _: &'a VaultBinding,
        intent: &'a KeyIntent,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.clear_error {
                return Err(StoreFailure::Unavailable);
            }
            if state.ledger.contains_request(intent) {
                state.ledger.clear_resolved(intent).unwrap();
            }
            if state.clear_commit_unknown {
                return Err(StoreFailure::Unknown);
            }
            Ok(())
        })
    }
    fn saved_receipt<'a>(
        &'a self,
        _: &'a VaultBinding,
        _: &'a LibraryContext,
        _: &'a str,
    ) -> StoreFuture<'a, Option<super::super::checkpoint::LoginReceipt>> {
        Box::pin(async {
            let state = self.0.lock().unwrap();
            if state.receipt_error {
                Err(StoreFailure::Unavailable)
            } else {
                Ok(state.receipt.clone())
            }
        })
    }
    fn save<'a>(&'a self, input: SaveInput<'a>) -> StoreFuture<'a, CaptureCommitOutcome> {
        let SaveInput {
            revision,
            request_id,
            snapshot,
            completion,
            ..
        } = input;
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.events.push("save");
            state.saved_completion_target =
                completion.map(|target| target.snapshot.identity().opaque_id());
            state.saved_revision = Some(revision.into());
            if !state.catalog_revision.is_empty() && state.catalog_revision != revision {
                return Err(StoreFailure::Changed);
            }
            if state.commit_unknown {
                state.receipt = Some(super::super::checkpoint::LoginReceipt {
                    request_id: request_id.into(),
                    account_id: snapshot.identity().opaque_id(),
                    candidate_revision: "0".repeat(64),
                    outcome: CaptureCommitOutcome::Refreshed,
                });
            }
            if let Some((flows, id)) = state.cancel_on_save.take() {
                flows.cancel(&id, Instant::now()).unwrap();
            }
            if let Some(flows) = state.expire_on_save.take() {
                flows
                    .expire_due(Instant::now() + std::time::Duration::from_secs(601))
                    .unwrap();
            }
            match state.save_error {
                Some(err) => Err(err),
                None => Ok(CaptureCommitOutcome::Refreshed),
            }
        })
    }
}
#[derive(Clone)]
struct Transport(
    Arc<Mutex<Vec<OfficialRequest>>>,
    Arc<Mutex<Vec<Result<OfficialResponse, OfficialError>>>>,
);
impl Transport {
    fn new(results: &[Result<&str, OfficialError>]) -> Self {
        Self(
            Arc::default(),
            Arc::new(Mutex::new(
                results
                    .iter()
                    .rev()
                    .map(|r| {
                        r.map(|body| OfficialResponse {
                            status: 200,
                            body: zeroize::Zeroizing::new(body.as_bytes().to_vec()),
                        })
                    })
                    .collect(),
            )),
        )
    }
}
impl OfficialTransport for Transport {
    fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
        self.0.lock().unwrap().push(request);
        Box::pin(async {
            self.1
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(Err(OfficialError::Transport))
        })
    }
}
fn fixture(
    results: &[Result<&str, OfficialError>],
) -> (LoginService<Transport, Store>, String, KeyScope) {
    fixture_family(results, OAuthFamily::BigModel)
}
fn fixture_family(
    results: &[Result<&str, OfficialError>],
    family: OAuthFamily,
) -> (LoginService<Transport, Store>, String, KeyScope) {
    let service = LoginService::new(Transport::new(results), Store::default());
    let now = Instant::now();
    let binding = VaultBinding {
        root: super::super::synthetic_test_path("vault"),
        vault_id: "vault".into(),
        key_id: "key".into(),
        revision: 1,
    };
    let id = service
        .flows
        .begin(
            binding,
            family,
            PollToken::new("synthetic-poll").unwrap(),
            now,
        )
        .unwrap();
    let ready = Arc::new(PollReady {
        family,
        user: OfficialUser {
            id: "account".into(),
            name: Some("Sample".into()),
            email: None,
        },
        start_jwt: StartJwt::new("synthetic-jwt").unwrap(),
        provider_access_token: ProviderAccessToken::new("synthetic-access").unwrap(),
        refresh_token: None,
    });
    let home = super::super::synthetic_test_path("home");
    let context = LibraryContext::from_os_identity(
        home.to_str().unwrap(),
        "synthetic-user",
        &home.join(".zcode/v2"),
    )
    .unwrap();
    let business = BusinessToken::from_stored(family, "synthetic-business").unwrap();
    let snapshot = super::super::oauth_account::build_snapshot(
        &context.cipher().unwrap(),
        &ready,
        &business,
        None,
    )
    .unwrap();
    let project = PersonalProject {
        organization_id: "org".into(),
        project_id: "project".into(),
        organization_name: None,
        project_name: None,
    };
    let init = service.flows.acquire(&id, WorkKind::Init, now).unwrap();
    init.finish(FlowStage::KeyRequired, |draft| {
        draft.ready = Some(ready);
        draft.business = Some(business);
        draft.project = Some(project);
        draft.context = Some(context);
        draft.snapshot = Some(snapshot);
        draft.catalog_revision = Some("synthetic-revision".into());
    })
    .unwrap();
    let scope = KeyScope::new(family, "account", "org", "project").unwrap();
    (service, id, scope)
}

const SAVED_CUSTOMER: &str = r#"{"code":200,"data":{"customerNumber":"business-owner","organizations":[{"organizationId":"org","projects":[{"projectId":"project","projectType":1}]}]}}"#;

#[tokio::test]
async fn saved_coding_does_not_send_unchanged_start_jwt_when_native_version_is_known() {
    let (service, id, _) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[{"name":"zcode-api-key","apiKey":"saved-key"}]}"#),
        Ok(r#"{"code":200,"data":{"secretKey":"saved-secret"}}"#),
        Ok(SAVED_CUSTOMER),
        Ok(
            r#"{"code":200,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":200,"data":{"limits":[]}}"#),
    ]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    lease
        .update(|draft, _| draft.app_version = Some("3.14.4".into()))
        .unwrap();
    service.prepare(lease).await.unwrap();
    let requests = service.client.transport.0.lock().unwrap();
    assert!(!requests
        .iter()
        .any(|request| request.url.contains("/billing/balance")));
    assert!(!requests.iter().any(|request| request
        .authorization
        .as_ref()
        .is_some_and(|secret| secret.expose().contains("synthetic-jwt"))));
    assert_eq!(requests.len(), 6);
}

fn saved_fixture(
    results: &[Result<&str, OfficialError>],
) -> (LoginService<Transport, Store>, String, AccountSnapshot) {
    let (service, old_id, _) = fixture(results);
    let lease = service
        .flows
        .acquire(&old_id, WorkKind::Prepare, Instant::now())
        .unwrap();
    let (binding, draft) = lease.draft().unwrap();
    let snapshot = draft.snapshot.unwrap();
    drop(lease);
    service.flows.cancel(&old_id, Instant::now()).unwrap();
    service.persistence.0.lock().unwrap().duplicate = true;
    let begin = service
        .flows
        .begin_saved(
            binding,
            SavedCodingInput {
                target: SavedCodingTarget {
                    snapshot: snapshot.clone(),
                    revision: "synthetic-revision".into(),
                    details: super::super::checkpoint::ProfileDetails::default(),
                },
                context: draft.context.unwrap(),
                app_version: None,
            },
            Instant::now(),
        )
        .unwrap();
    (service, begin.flow_id, snapshot)
}

#[tokio::test]
async fn saved_coding_reuses_business_session_and_existing_key_without_oauth_or_source_upgrade() {
    let (service, id, original) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[{"name":"zcode-api-key","apiKey":"saved-key"}]}"#),
        Ok(r#"{"code":200,"data":{"secretKey":"saved-secret"}}"#),
        Ok(SAVED_CUSTOMER),
        Ok(
            r#"{"code":200,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":200,"data":{"limits":[]}}"#),
    ]);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "review");
    assert_eq!(progress.account.unwrap().identity_source, "packageDeclared");
    assert_eq!(progress.connections.unwrap().coding, "ready");
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    let (_, draft) = lease.draft().unwrap();
    assert!(draft.ready.is_none());
    assert!(super::super::oauth_account::coding_only_change(
        &original,
        draft.snapshot.as_ref().unwrap()
    ));
    service.save(lease, true).await.unwrap();
    let state = service.persistence.0.lock().unwrap();
    assert_eq!(
        state.saved_completion_target.as_deref(),
        Some(original.identity().opaque_id().as_str())
    );
    assert_eq!(state.saved_revision.as_deref(), Some("synthetic-revision"));
    let requests = service.client.transport.0.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests
        .iter()
        .all(|request| request.method == OfficialMethod::Get
            && !request.url.contains("/oauth/")
            && !request.url.contains("/auth/z/login")));
    assert_eq!(
        requests[0].authorization.as_ref().unwrap().expose(),
        "synthetic-business"
    );
}

#[tokio::test]
async fn saved_coding_decline_keeps_the_complete_original_start_session() {
    let (service, id, original) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Ok(SAVED_CUSTOMER),
        Ok(SAVED_CUSTOMER),
    ]);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "keyRequired"
    );
    service
        .decline_key(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    let (_, draft) = lease.draft().unwrap();
    assert_eq!(
        draft
            .snapshot
            .unwrap()
            .scoped_document()
            .to_bytes()
            .unwrap(),
        original.scoped_document().to_bytes().unwrap()
    );
    assert!(service
        .client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == OfficialMethod::Get));
}

#[tokio::test]
async fn saved_coding_rejects_stale_catalog_before_network_and_does_not_adopt_new_revision() {
    let (service, id, _) = saved_fixture(&[]);
    service.persistence.0.lock().unwrap().catalog_revision = "changed-revision".into();
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(
        progress.error.unwrap().code,
        "zcode.account.catalog_changed"
    );
    assert!(service.client.transport.0.lock().unwrap().is_empty());
    assert_eq!(
        progress.source_catalog_revision.as_deref(),
        Some("synthetic-revision")
    );
}

#[tokio::test]
async fn saved_coding_original_project_intent_is_read_only_even_with_another_declared_identity() {
    let (service, id, _) = saved_fixture(&[Ok(SAVED_CUSTOMER), Ok(r#"{"code":200,"data":[]}"#)]);
    service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .reserve(
            KeyScope::new(
                OAuthFamily::BigModel,
                "other-declared-identity",
                "org",
                "project",
            )
            .unwrap(),
        )
        .unwrap();
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "keyRequired");
    assert!(progress.key_may_exist);
    assert_eq!(
        progress.error.unwrap().code,
        "zcode.account.key_result_unknown"
    );
    assert!(service
        .client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == OfficialMethod::Get));
}

#[tokio::test]
async fn saved_coding_cancelled_uncertainty_queries_original_project_and_never_reposts() {
    let (service, id, _) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Err(OfficialError::Transport),
        // Recovery must read the original key list first, even if the current
        // customer response would now select a different default project.
        Ok(r#"{"code":200,"data":[{"name":"zcode-api-key","apiKey":"created-key"}]}"#),
        Ok(r#"{"code":200,"data":{"secretKey":"created-secret"}}"#),
        Ok(
            r#"{"code":200,"data":{"customerNumber":"business-owner","organizations":[{"organizationId":"other-org","projects":[{"projectId":"different-default","projectType":1}]}]}}"#,
        ),
        Ok(
            r#"{"code":200,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":200,"data":{"limits":[]}}"#),
    ]);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    service
        .create_key(
            service
                .flows
                .acquire(&id, WorkKind::CreateKey, Instant::now())
                .unwrap(),
            "org",
            "project",
        )
        .await
        .unwrap();
    assert!(
        service
            .flows
            .progress(&id, Instant::now())
            .unwrap()
            .key_may_exist
    );
    let read = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    let (binding, draft) = read.draft().unwrap();
    drop(read);
    service.flows.cancel(&id, Instant::now()).unwrap();
    let resumed = service
        .flows
        .begin_saved(
            binding,
            SavedCodingInput {
                target: draft.completion.unwrap(),
                context: draft.context.unwrap(),
                app_version: None,
            },
            Instant::now(),
        )
        .unwrap();
    assert_eq!(resumed.flow_id, id);
    assert!(resumed.needs_prepare);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "review");
    assert_eq!(progress.project.unwrap().project_id, "project");
    let requests = service.client.transport.0.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == OfficialMethod::Post)
            .count(),
        1
    );
    assert!(requests[5]
        .url
        .ends_with("/organization/org/projects/project/api_keys"));
    assert!(requests[6]
        .url
        .ends_with("/organization/org/projects/project/api_keys/copy/created-key"));
}

#[tokio::test]
async fn saved_coding_committed_save_receipt_survives_cancel_and_lost_reply() {
    let (service, id, original) = saved_fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    {
        let mut state = service.persistence.0.lock().unwrap();
        state.commit_unknown = true;
        state.save_error = Some(StoreFailure::Unknown);
        state.cancel_on_save = Some((service.flows.clone(), id.clone()));
    }
    service
        .save(
            service
                .flows
                .acquire(&id, WorkKind::Save, Instant::now())
                .unwrap(),
            true,
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "saved");
    assert_eq!(progress.purpose, LoginPurpose::CompleteCoding);
    assert_eq!(progress.saved.unwrap().id, original.identity().opaque_id());
    assert_eq!(
        service
            .persistence
            .0
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|event| **event == "save")
            .count(),
        1
    );
    assert!(service.client.transport.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn create_consent_must_match_current_project_before_any_reservation_or_request() {
    let (service, id, _) = fixture(&[]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service
        .create_key(lease, "other-org", "project")
        .await
        .unwrap();
    assert!(service.persistence.0.lock().unwrap().events.is_empty());
    assert!(service.client.transport.0.lock().unwrap().is_empty());
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(view.phase, "keyRequired");
    assert_eq!(view.error.unwrap().remedy, "retryKeyConsent");
}
#[tokio::test]
async fn cancel_after_fresh_durable_intent_clears_only_that_unsubmitted_attempt() {
    let (service, id, scope) = fixture(&[]);
    service.persistence.0.lock().unwrap().cancel_on_reserve =
        Some((service.flows.clone(), id.clone()));
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    let _ = service.create_key(lease, "org", "project").await;
    let state = service.persistence.0.lock().unwrap();
    assert_eq!(state.events, vec!["reserve", "not-sent"]);
    assert!(state.ledger.get(&scope).is_none());
    assert!(service.client.transport.0.lock().unwrap().is_empty());
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "cancelled"
    );
}
#[tokio::test]
async fn prior_uncertain_project_intent_cannot_generate_a_second_post() {
    let (service, id, scope) = fixture(&[Ok(r#"{"code":0,"data":[]}"#)]);
    service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .reserve(scope)
        .unwrap();
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    assert!(service
        .client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method == OfficialMethod::Get));
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert!(view.key_may_exist);
    assert_eq!(view.error.unwrap().remedy, "queryOriginal");
}
#[tokio::test]
async fn remote_post_timeout_retains_intent_and_does_not_repeat_post() {
    let (service, id, scope) =
        fixture(&[Ok(r#"{"code":0,"data":[]}"#), Err(OfficialError::Timeout)]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    assert_eq!(
        service
            .persistence
            .0
            .lock()
            .unwrap()
            .ledger
            .get(&scope)
            .unwrap()
            .state,
        IntentState::Pending
    );
    assert_eq!(
        service
            .client
            .transport
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == OfficialMethod::Post)
            .count(),
        1
    );
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert!(view.key_may_exist);
    assert!(!view.key_created);
    assert_eq!(view.error.unwrap().remedy, "queryOriginal");
}
#[tokio::test]
async fn committed_save_survives_cancel_during_writer_and_preserves_exact_outcome() {
    let (service, id, _) = fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    service.persistence.0.lock().unwrap().cancel_on_save =
        Some((service.flows.clone(), id.clone()));
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    service.save(lease, false).await.unwrap();
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(view.phase, "saved");
    assert_eq!(view.saved.unwrap().outcome, CaptureCommitOutcome::Refreshed);
    assert!(view.authorization.is_none());
    assert!(service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .is_err());
}

#[tokio::test]
async fn created_remote_key_with_copy_failure_retains_created_fact_after_cancel() {
    let (service, id, scope) = fixture(&[
        Ok(r#"{"code":0,"data":[]}"#),
        Ok(r#"{"code":0,"data":{"apiKey":"synthetic-key-reference","name":"zcode-api-key"}}"#),
        Err(OfficialError::Timeout),
    ]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert!(view.key_created && view.key_may_exist);
    assert_eq!(
        service
            .persistence
            .0
            .lock()
            .unwrap()
            .ledger
            .get(&scope)
            .unwrap()
            .state,
        IntentState::Created
    );
    service.flows.cancel(&id, Instant::now()).unwrap();
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(view.phase, "cancelled");
    assert!(view.key_created && view.key_may_exist);
}
#[tokio::test]
async fn dropping_save_waiter_keeps_owned_writer_and_saved_result() {
    let (service, id, _) = fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    let service = Arc::new(service);
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    let owned = Arc::clone(&service);
    let waiter = tokio::spawn(async move { owned.save(lease, false).await });
    drop(waiter);
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let view = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(view.phase, "saved");
    assert_eq!(service.persistence.0.lock().unwrap().events, vec!["save"]);
}
#[tokio::test]
async fn init_expiry_uses_official_deadline_and_does_not_start_a_poll_for_expired_url() {
    let (service, id, _) = fixture(&[Ok(
        r#"{"code":0,"data":{"authorize_url":"https://zcode.z.ai/authorize?flow=synthetic","flow_id":"synthetic-flow","expires_at":1,"poll_interval_sec":2}}"#,
    )]);
    let binding = VaultBinding {
        root: super::super::synthetic_test_path("vault"),
        vault_id: "vault".into(),
        key_id: "key".into(),
        revision: 1,
    };
    service.flows.cancel(&id, Instant::now()).unwrap();
    let id = service
        .flows
        .begin(
            binding,
            OAuthFamily::BigModel,
            PollToken::new("synthetic-poll").unwrap(),
            Instant::now(),
        )
        .unwrap();
    let home = super::super::synthetic_test_path("home");
    let context = LibraryContext::from_os_identity(
        home.to_str().unwrap(),
        "synthetic-user",
        &home.join(".zcode/v2"),
    )
    .unwrap();
    let lease = service
        .flows
        .acquire(&id, WorkKind::Init, Instant::now())
        .unwrap();
    service.initialize(lease, context, None).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "expired"
    );
    assert!(service
        .flows
        .acquire(&id, WorkKind::Poll, Instant::now())
        .is_err());
    assert_eq!(service.client.transport.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn review_transient_poll_preserves_same_flow_for_later_pending_response() {
    for failure in [
        OfficialError::Timeout,
        OfficialError::Transport,
        OfficialError::Http(408),
        OfficialError::Http(429),
        OfficialError::Http(503),
    ] {
        let (service, id, _) = fixture(&[
            Err(failure),
            Ok(r#"{"code":0,"data":{"status":"pending"}}"#),
        ]);
        let lease = service
            .flows
            .acquire(&id, WorkKind::Prepare, Instant::now())
            .unwrap();
        lease
            .finish(FlowStage::Waiting, |draft| {
                draft.init = Some(OAuthInit {
                    authorize_url: "https://zcode.z.ai/authorize?flow=synthetic".into(),
                    flow_id: "original-remote-flow".into(),
                    expires_at: u64::MAX,
                    poll_interval_sec: 2,
                });
                draft.poll_deadline = Some(Instant::now() + std::time::Duration::from_secs(60));
            })
            .unwrap();
        let lease = service
            .flows
            .acquire(&id, WorkKind::Poll, Instant::now())
            .unwrap();
        assert!(!service.poll(lease).await.unwrap());
        assert_eq!(
            service.flows.progress(&id, Instant::now()).unwrap().phase,
            "waiting"
        );
        let lease = service
            .flows
            .acquire(&id, WorkKind::Poll, Instant::now())
            .unwrap();
        assert_eq!(
            lease.draft().unwrap().1.init.unwrap().flow_id,
            "original-remote-flow"
        );
        assert!(!service.poll(lease).await.unwrap());
    }
}
#[tokio::test]
async fn review_transient_normalization_preserves_authorized_ready_credentials() {
    let (service, id, _) = fixture_family(&[Err(OfficialError::Timeout)], OAuthFamily::Zai);
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    lease
        .finish(FlowStage::Preparing, |draft| draft.business = None)
        .unwrap();
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    service.prepare(lease).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "preparing"
    );
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    assert_eq!(
        lease
            .draft()
            .unwrap()
            .1
            .ready
            .unwrap()
            .provider_access_token
            .expose(),
        "synthetic-access"
    );
}
#[tokio::test]
async fn review_successful_creation_and_save_clears_exact_durable_key_intent() {
    let (service, id, scope) = fixture(&[
        Ok(r#"{"code":0,"data":[]}"#),
        Ok(r#"{"code":0,"data":{"apiKey":"new-key-reference","name":"zcode-api-key"}}"#),
        Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
        Ok(r#"{"code":0,"data":{"customerNumber":"account"}}"#),
        Ok(
            r#"{"code":0,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":0,"data":{"limits":[]}}"#),
    ]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    service.save(lease, false).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "saved"
    );
    assert!(service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .get(&scope)
        .is_none());
}
#[tokio::test]
async fn review_confirm_reuses_a_key_created_while_consent_dialog_was_open() {
    let (service, id, _) = fixture(&[
        Ok(r#"{"code":0,"data":[{"apiKey":"existing-key-reference","name":"zcode-api-key"}]}"#),
        Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
        Ok(r#"{"code":0,"data":{"customerNumber":"account"}}"#),
        Ok(
            r#"{"code":0,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":0,"data":{"limits":[]}}"#),
    ]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    assert!(service
        .client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == OfficialMethod::Get));
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "review"
    );
}

#[tokio::test]
async fn review_changed_catalog_returns_fresh_duplicate_preview_before_explicit_save_retry() {
    let (service, id, _) = fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    {
        let mut state = service.persistence.0.lock().unwrap();
        state.catalog_revision = "changed-by-other-account".into();
        state.duplicate = true;
    }
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    service.save(lease, false).await.unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "review");
    assert!(progress.account.unwrap().duplicate);
    assert_eq!(service.persistence.0.lock().unwrap().events, vec!["save"]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    service.save(lease, false).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "saved"
    );
    assert_eq!(
        service.persistence.0.lock().unwrap().events,
        vec!["save", "save"]
    );
}
#[tokio::test]
async fn review_cancelled_save_with_unreadable_receipt_can_later_recover_without_replaying_write() {
    let (service, id, _) = fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    {
        let mut state = service.persistence.0.lock().unwrap();
        state.commit_unknown = true;
        state.save_error = Some(StoreFailure::Unknown);
        state.receipt_error = true;
        state.cancel_on_save = Some((service.flows.clone(), id.clone()));
    }
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    let _ = service.save(lease, false).await;
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "cancelled"
    );
    service.persistence.0.lock().unwrap().receipt_error = false;
    let kind = service
        .flows
        .recovery_kind(&id)
        .unwrap()
        .expect("committed possibility remains queryable after cancellation");
    let lease = service.flows.acquire(&id, kind, Instant::now()).unwrap();
    service.recover(lease).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "saved"
    );
    assert_eq!(service.persistence.0.lock().unwrap().events, vec!["save"]);
}

#[tokio::test]
async fn review_expired_unknown_save_retains_only_receipt_query_after_secret_expiry() {
    let (service, id, _) = fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    {
        let mut state = service.persistence.0.lock().unwrap();
        state.commit_unknown = true;
        state.save_error = Some(StoreFailure::Unknown);
        state.receipt_error = true;
        state.expire_on_save = Some(service.flows.clone());
    }
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    service.save(lease, false).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "expired"
    );
    service
        .flows
        .expire_due(Instant::now() + std::time::Duration::from_secs(1801))
        .unwrap();
    service.persistence.0.lock().unwrap().receipt_error = false;
    let kind = service.flows.recovery_kind(&id).unwrap().unwrap();
    let lease = service.flows.acquire(&id, kind, Instant::now()).unwrap();
    assert!(lease.draft().is_err());
    service.recover(lease).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "saved"
    );
    assert_eq!(service.persistence.0.lock().unwrap().events, vec!["save"]);
}
#[tokio::test]
async fn review_key_cleanup_failure_stays_queryable_until_exact_marker_is_cleared() {
    let (service, id, scope) = fixture(&[
        Ok(r#"{"code":0,"data":[]}"#),
        Ok(r#"{"code":0,"data":{"apiKey":"key-reference","name":"zcode-api-key"}}"#),
        Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
        Ok(r#"{"code":0,"data":{"customerNumber":"account"}}"#),
        Ok(
            r#"{"code":0,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":0,"data":{"limits":[]}}"#),
    ]);
    service.persistence.0.lock().unwrap().clear_error = true;
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "keyRequired");
    assert_eq!(
        progress.error.unwrap().code,
        "zcode.account.key_cleanup_pending"
    );
    assert!(service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .get(&scope)
        .is_some());
    service.persistence.0.lock().unwrap().clear_error = false;
    let kind = service.flows.recovery_kind(&id).unwrap().unwrap();
    let lease = service.flows.acquire(&id, kind, Instant::now()).unwrap();
    service.recover(lease).await.unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "review"
    );
    assert!(service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .get(&scope)
        .is_none());
    assert_eq!(
        service
            .client
            .transport
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == OfficialMethod::Post)
            .count(),
        1
    );
}
#[tokio::test]
async fn review_failed_key_lookup_never_posts_or_leaves_a_false_uncertain_intent() {
    let (service, id, scope) = fixture(&[Err(OfficialError::Timeout)]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    service.create_key(lease, "org", "project").await.unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "keyRequired");
    assert_eq!(progress.error.unwrap().remedy, "retryKeyConsent");
    assert!(!progress.key_may_exist);
    assert!(service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .get(&scope)
        .is_none());
    assert!(service
        .client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == OfficialMethod::Get));
}

#[tokio::test]
async fn review_receipt_capacity_is_a_known_no_write_limit_not_an_uncertain_save() {
    let (service, id, _) = fixture(&[]);
    let prepare = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    {
        let mut state = service.persistence.0.lock().unwrap();
        state.save_error = Some(StoreFailure::Capacity);
        state.receipt_error = true;
    }
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .unwrap();
    service.save(lease, false).await.unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.error.unwrap().code, "zcode.account.resource_limit");
    assert!(service.flows.recovery_kind(&id).unwrap().is_none());
}

#[tokio::test]
async fn review_cancel_after_existing_key_copy_still_cleans_exact_no_post_reservation() {
    let (service, id, scope) = fixture(&[
        Ok(r#"{"code":0,"data":[{"apiKey":"existing-key-reference","name":"zcode-api-key"}]}"#),
        Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
    ]);
    service.persistence.0.lock().unwrap().cancel_after_check =
        Some((5, service.flows.clone(), id.clone()));
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .unwrap();
    let _ = service.create_key(lease, "org", "project").await;
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "cancelled"
    );
    assert_eq!(service.client.transport.0.lock().unwrap().len(), 2);
    assert!(service
        .client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == OfficialMethod::Get));
    assert!(service
        .persistence
        .0
        .lock()
        .unwrap()
        .ledger
        .get(&scope)
        .is_none());
}
#[tokio::test]
async fn review_unreadable_key_intent_cannot_silently_clear_cleanup_pending_state() {
    let (service, id, _) = fixture(&[]);
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    lease
        .update(|_, display| {
            display.error = Some(LoginError {
                code: "zcode.account.key_cleanup_pending",
                remedy: "queryOriginal",
                committed: false,
            })
        })
        .unwrap();
    lease.finish(FlowStage::KeyRequired, |_| {}).unwrap();
    service.persistence.0.lock().unwrap().intent_error = true;
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    service.recover(lease).await.unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "keyRequired");
    assert_eq!(
        progress.error.unwrap().code,
        "zcode.account.key_cleanup_pending"
    );
    assert!(service.client.transport.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn review_cancelled_copy_with_failed_local_cleanup_retries_only_local_io() {
    for expired in [false, true] {
        for created in [false, true] {
            let responses = if created {
                vec![
                    Ok(r#"{"code":0,"data":[]}"#),
                    Ok(r#"{"code":0,"data":{"apiKey":"key-reference","name":"zcode-api-key"}}"#),
                    Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
                ]
            } else {
                vec![
                    Ok(r#"{"code":0,"data":[{"apiKey":"key-reference","name":"zcode-api-key"}]}"#),
                    Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
                ]
            };
            let (service, id, scope) = fixture(&responses);
            {
                let mut state = service.persistence.0.lock().unwrap();
                state.clear_error = true;
                state.expire_on_check = expired;
                state.cancel_after_check = Some((
                    if created { 7 } else { 5 },
                    service.flows.clone(),
                    id.clone(),
                ));
            }
            let lease = service
                .flows
                .acquire(&id, WorkKind::CreateKey, Instant::now())
                .unwrap();
            service.create_key(lease, "org", "project").await.unwrap();
            let progress = service.flows.progress(&id, Instant::now()).unwrap();
            assert_eq!(
                progress.phase,
                if expired { "expired" } else { "cancelled" }
            );
            assert_eq!(
                progress.error.unwrap().code,
                "zcode.account.key_cleanup_pending"
            );
            assert!(service
                .persistence
                .0
                .lock()
                .unwrap()
                .ledger
                .get(&scope)
                .is_some());
            let count = service.client.transport.0.lock().unwrap().len();
            service.persistence.0.lock().unwrap().clear_error = false;
            let kind = service.flows.recovery_kind(&id).unwrap().unwrap();
            let lease = service.flows.acquire(&id, kind, Instant::now()).unwrap();
            assert!(lease.draft().is_err());
            service.recover(lease).await.unwrap();
            assert!(service
                .persistence
                .0
                .lock()
                .unwrap()
                .ledger
                .get(&scope)
                .is_none());
            let progress = service.flows.progress(&id, Instant::now()).unwrap();
            assert_eq!(
                progress.phase,
                if expired { "expired" } else { "cancelled" }
            );
            assert_eq!(progress.key_created, created);
            assert!(!progress.key_may_exist);
            assert!(progress.error.is_none());
            assert_eq!(service.client.transport.0.lock().unwrap().len(), count);
        }
    }
}

#[tokio::test]
async fn review_lost_cleanup_acknowledgment_resolves_without_more_http_after_cancel() {
    for created in [false, true] {
        let responses = if created {
            vec![
                Ok(r#"{"code":0,"data":[]}"#),
                Ok(r#"{"code":0,"data":{"apiKey":"key-reference","name":"zcode-api-key"}}"#),
                Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
            ]
        } else {
            vec![
                Ok(r#"{"code":0,"data":[{"apiKey":"key-reference","name":"zcode-api-key"}]}"#),
                Ok(r#"{"code":0,"data":{"secretKey":"synthetic-secret"}}"#),
            ]
        };
        let (service, id, scope) = fixture(&responses);
        {
            let mut state = service.persistence.0.lock().unwrap();
            state.clear_commit_unknown = true;
            state.cancel_after_check = Some((
                if created { 7 } else { 5 },
                service.flows.clone(),
                id.clone(),
            ));
            let other =
                KeyScope::new(OAuthFamily::BigModel, "account", "org", "other-project").unwrap();
            state.ledger.reserve(other).unwrap();
        }
        let lease = service
            .flows
            .acquire(&id, WorkKind::CreateKey, Instant::now())
            .unwrap();
        service.create_key(lease, "org", "project").await.unwrap();
        let progress = service.flows.progress(&id, Instant::now()).unwrap();
        assert_eq!(progress.phase, "cancelled");
        assert_eq!(
            progress.error.unwrap().code,
            "zcode.account.key_cleanup_pending"
        );
        let count = service.client.transport.0.lock().unwrap().len();
        {
            let mut state = service.persistence.0.lock().unwrap();
            assert!(state.ledger.get(&scope).is_none());
            assert!(!state.ledger.is_empty());
            state.clear_commit_unknown = false;
        }
        let kind = service.flows.recovery_kind(&id).unwrap().unwrap();
        let lease = service.flows.acquire(&id, kind, Instant::now()).unwrap();
        assert!(lease.draft().is_err());
        service.recover(lease).await.unwrap();
        let progress = service.flows.progress(&id, Instant::now()).unwrap();
        assert_eq!(progress.phase, "cancelled");
        assert!(progress.error.is_none());
        assert!(!progress.key_may_exist);
        assert_eq!(progress.key_created, created);
        assert!(service.flows.recovery_kind(&id).unwrap().is_none());
        assert_eq!(service.client.transport.0.lock().unwrap().len(), count);
        assert!(!service.persistence.0.lock().unwrap().ledger.is_empty());
    }
}

#[tokio::test]
async fn review_saved_catalog_replacement_during_key_discovery_blocks_post() {
    let (service, id, _) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Ok(r#"{"code":200,"data":{"name":"zcode-api-key","apiKey":"created"}}"#),
        Ok(r#"{"code":200,"data":{"secretKey":"created-secret"}}"#),
    ]);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        service.flows.progress(&id, Instant::now()).unwrap().phase,
        "keyRequired"
    );
    struct ReplaceCatalogAfterGet {
        inner: Transport,
        store: Store,
    }
    impl OfficialTransport for ReplaceCatalogAfterGet {
        fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
            let replace =
                request.method == OfficialMethod::Get && request.url.ends_with("/api_keys");
            let sent = self.inner.send(request);
            Box::pin(async move {
                let result = sent.await;
                if replace {
                    let mut state = self.store.0.lock().unwrap();
                    state.catalog_revision = "replacement-catalog".into();
                    state.duplicate = false;
                }
                result
            })
        }
    }
    let transport = ReplaceCatalogAfterGet {
        inner: service.client.transport,
        store: service.persistence.clone(),
    };
    let service = LoginService {
        flows: service.flows,
        client: OfficialClient::new(transport),
        persistence: service.persistence,
    };
    service
        .create_key(
            service
                .flows
                .acquire(&id, WorkKind::CreateKey, Instant::now())
                .unwrap(),
            "org",
            "project",
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    let posts = service
        .client
        .transport
        .inner
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.method == OfficialMethod::Post)
        .count();
    println!(
        "Post count after catalog replacement: {posts}; final error: {}",
        progress.error.unwrap().code
    );
    assert!(
        service
            .persistence
            .0
            .lock()
            .unwrap()
            .events
            .contains(&"not-sent"),
        "Exact unsubmitted reservation must be cleared"
    );
    assert_eq!(posts, 0, "The selected catalog row was removed before POST, so saved Coding consent must be invalidated before transmission");
}

#[tokio::test]
async fn review_saved_coding_label_change_recovers_original_project_once() {
    let (service, id, _) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Err(OfficialError::Transport),
        // Recovery must read the original key list first, even if the current
        // customer response would now select a different default project.
        Ok(r#"{"code":200,"data":[{"name":"zcode-api-key","apiKey":"created-key"}]}"#),
        Ok(r#"{"code":200,"data":{"secretKey":"created-secret"}}"#),
        Ok(
            r#"{"code":200,"data":{"customerNumber":"business-owner","organizations":[{"organizationId":"other-org","projects":[{"projectId":"different-default","projectType":1}]}]}}"#,
        ),
        Ok(
            r#"{"code":200,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":200,"data":{"limits":[]}}"#),
    ]);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    service
        .create_key(
            service
                .flows
                .acquire(&id, WorkKind::CreateKey, Instant::now())
                .unwrap(),
            "org",
            "project",
        )
        .await
        .unwrap();
    assert!(
        service
            .flows
            .progress(&id, Instant::now())
            .unwrap()
            .key_may_exist
    );
    let read = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    let (binding, draft) = read.draft().unwrap();
    drop(read);
    service.flows.cancel(&id, Instant::now()).unwrap();
    let mut target = draft.completion.unwrap();
    target.revision = "after-label-edit".into();
    target.details.label = Some("Renamed saved account".into());
    service.persistence.0.lock().unwrap().catalog_revision = target.revision.clone();
    let resumed = service
        .flows
        .begin_saved(
            binding,
            SavedCodingInput {
                target,
                context: draft.context.unwrap(),
                app_version: None,
            },
            Instant::now() + std::time::Duration::from_secs(3600),
        )
        .unwrap();
    assert_eq!(resumed.flow_id, id);
    assert!(resumed.needs_prepare);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let progress = service.flows.progress(&id, Instant::now()).unwrap();
    assert_eq!(progress.phase, "review");
    assert_eq!(progress.project.unwrap().project_id, "project");
    let requests = service.client.transport.0.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == OfficialMethod::Post)
            .count(),
        1
    );
    assert!(requests[5]
        .url
        .ends_with("/organization/org/projects/project/api_keys"));
    assert!(requests[6]
        .url
        .ends_with("/organization/org/projects/project/api_keys/copy/created-key"));
}

#[tokio::test]
async fn review_saved_coding_changed_session_cannot_reopen_uncertain_creation() {
    let (service, id, _) = saved_fixture(&[
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Ok(SAVED_CUSTOMER),
        Ok(r#"{"code":200,"data":[]}"#),
        Err(OfficialError::Transport),
        // Recovery must read the original key list first, even if the current
        // customer response would now select a different default project.
        Ok(r#"{"code":200,"data":[{"name":"zcode-api-key","apiKey":"created-key"}]}"#),
        Ok(r#"{"code":200,"data":{"secretKey":"created-secret"}}"#),
        Ok(
            r#"{"code":200,"data":{"customerNumber":"business-owner","organizations":[{"organizationId":"other-org","projects":[{"projectId":"different-default","projectType":1}]}]}}"#,
        ),
        Ok(
            r#"{"code":200,"data":[{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]}"#,
        ),
        Ok(r#"{"code":200,"data":{"limits":[]}}"#),
    ]);
    service
        .prepare(
            service
                .flows
                .acquire(&id, WorkKind::Prepare, Instant::now())
                .unwrap(),
        )
        .await
        .unwrap();
    service
        .create_key(
            service
                .flows
                .acquire(&id, WorkKind::CreateKey, Instant::now())
                .unwrap(),
            "org",
            "project",
        )
        .await
        .unwrap();
    assert!(
        service
            .flows
            .progress(&id, Instant::now())
            .unwrap()
            .key_may_exist
    );
    let read = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .unwrap();
    let (binding, draft) = read.draft().unwrap();
    drop(read);
    service.flows.cancel(&id, Instant::now()).unwrap();
    let mut target = draft.completion.unwrap();
    target.revision = "after-label-edit".into();
    target.details.label = Some("Renamed saved account".into());
    service.persistence.0.lock().unwrap().catalog_revision = target.revision.clone();
    let context = draft.context.unwrap();
    let native = context.cipher().unwrap();
    let mut fields: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&target.snapshot.scoped_document().to_bytes().unwrap()).unwrap();
    fields.insert(
        target.snapshot.identity().credential_keys()[1].clone(),
        native.encrypt("different-saved-business").unwrap(),
    );
    target.snapshot = native
        .inspect(
            &super::super::core::CredentialDocument::parse(&serde_json::to_vec(&fields).unwrap())
                .unwrap(),
        )
        .unwrap();
    let result = service.flows.begin_saved(
        binding,
        SavedCodingInput {
            target,
            context,
            app_version: None,
        },
        Instant::now(),
    );
    assert!(matches!(result, Err(FlowError::StaleWork)));
    let requests = service.client.transport.0.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method == OfficialMethod::Post)
            .count(),
        1
    );
    assert!(
        service
            .flows
            .progress(&id, Instant::now())
            .unwrap()
            .key_may_exist
    );
}
