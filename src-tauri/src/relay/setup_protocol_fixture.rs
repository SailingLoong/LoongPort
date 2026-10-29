//! Loopback-only tests of the actual protocol owner and non-secret SQLite journal.
use super::setup_contract::*;
use super::setup_journal::{KeyEvent, KeyPreparationJournal, PreparationIssue};
use super::{model_selection::ModelSelectionTables, newapi, newapi_provision, provision, sub2api};
use crate::database::relay_setup_operations as journal;
use crate::error::AppError;
use axum::{extract::Request, http::StatusCode, routing::any, Json, Router};
use rusqlite::Connection;
use serde_json::json;
use std::sync::{Arc, Mutex};
const OP: &str = "f8d02319-67c1-426b-a931-c836de11320e";
const BIND: &str = "fixture-storage/generation";
struct State {
    conn: Connection,
    posts: usize,
    observe_existing: bool,
    requests: Vec<(String, String)>,
    post_saw_requesting: bool,
    read_saw_created: bool,
}
struct Observer(Arc<Mutex<State>>);
impl KeyPreparationJournal for Observer {
    fn record_issue(
        &self,
        group: &str,
        app: Option<&str>,
        issue: PreparationIssue,
    ) -> Result<(), AppError> {
        journal::preparation_issue_on(
            &mut self.0.lock().unwrap().conn,
            BIND,
            OP,
            group,
            app,
            issue,
            2,
        )
        .map(|_| ())
    }
    fn record_model_support(&self, group: &str, apps: &[String]) -> Result<(), AppError> {
        journal::model_support_on(&mut self.0.lock().unwrap().conn, BIND, OP, group, apps, 2)
            .map(|_| ())
    }
    fn record(&self, group: &str, event: KeyEvent) -> Result<(), AppError> {
        journal::key_event_on(&mut self.0.lock().unwrap().conn, BIND, OP, group, event, 2)
            .map(|_| ())
    }
}
struct Fixture {
    origin: String,
    state: Arc<Mutex<State>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn start(newapi: bool, lost: bool) -> Self {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            for n in ["NO_PROXY", "no_proxy"] {
                let old = std::env::var(n).unwrap_or_default();
                std::env::set_var(n, format!("{old},localhost,127.0.0.1,::1"));
            }
        });
        let group = if newapi { "alpha" } else { "3" };
        let d = RelayDiscovery {
            schema_version: 1,
            relay_id: 7,
            account: RelayAccountIdentity {
                identity: "fixture/account7".into(),
                site_origin: "https://fixture.example".into(),
                account_label: "Fixture".into(),
            },
            revision: "r1".into(),
            choices: vec![RelayChoice {
                app: "codex".into(),
                group_id: group.into(),
                group_name: "Fixture".into(),
                provider_id: "fixture-provider".into(),
                local_configuration: LocalConfiguration::Absent,
                compatibility: ChoiceCompatibility::Unknown,
                key_status: UnknownKeyStatus::Unknown,
            }],
        };
        let mut conn = Connection::open_in_memory().unwrap();
        journal::create_schema(&conn).unwrap();
        journal::begin_on(
            &mut conn,
            BIND,
            &initial_receipt(
                OP,
                &[RelaySelection {
                    app: "codex".into(),
                    group_id: group.into(),
                }],
                &d,
                1,
            )
            .unwrap(),
        )
        .unwrap();
        let state = Arc::new(Mutex::new(State {
            conn,
            posts: 0,
            observe_existing: false,
            requests: Vec::new(),
            post_saw_requesting: false,
            read_saw_created: false,
        }));
        let shared = state.clone();
        let app=Router::new().fallback(any(move |request:Request|{let state=shared.clone();async move {
            let method=request.method().to_string();let path=request.uri().path().to_string();
            let mut s=state.lock().unwrap();
            s.requests.push((method.clone(),path.clone()));
            let r=journal::read_on(&s.conn,OP).unwrap().unwrap();
            let fact=&r.groups[0];
            let data=match (method.as_str(),path.as_str()) {
                ("GET","/api/v1/groups/available")=>json!([{"id":3,"name":"Fixture","platform":"composite","status":"active","rate_multiplier":1}]),
                ("GET","/api/v1/keys")=>json!({"items":if s.observe_existing {vec![json!({"id":31,"key":"fixture-only-key","name":"LoongPort/a7/composite/3","status":"active"})]} else {vec![]},"pages":1}),
                ("GET","/api/v1/subscriptions")=>json!([]),
                ("GET","/api/v1/groups/rates")=>json!({}),
                ("GET","/api/user/self")=>return (StatusCode::OK,Json(json!({"success":true,"data":{"id":7,"username":"fixture","display_name":"Fixture","email":"fixture@example.test","group":"default","quota":0,"used_quota":0}}))),
                ("GET","/api/user/self/groups")=>return (StatusCode::OK,Json(json!({"success":true,"data":{"alpha":{"ratio":1,"desc":"Fixture"}}}))),
                ("GET","/api/token/")=>{
                    if s.observe_existing {return (StatusCode::OK,Json(json!({"success":true,"data":{"page":1,"page_size":100,"total":1,"items":[{"id":31,"key":"masked-fixture","name":newapi_provision::managed_token_name(7,&newapi::GroupIdentity("alpha".into())),"group":"alpha","status":1}]}})));}
                    if s.posts>0 {s.read_saw_created=fact.state==KeyState::Ready && fact.origin==Some(KeyOrigin::Created);return (StatusCode::SERVICE_UNAVAILABLE,Json(json!({"success":false})));}
                    return (StatusCode::OK,Json(json!({"success":true,"data":{"page":1,"page_size":100,"total":0,"items":[]}})));
                }
                ("POST","/api/v1/keys")|("POST","/api/token/")=>{
                    s.post_saw_requesting=fact.state==KeyState::Requesting;s.posts+=1;
                    if lost {return (StatusCode::SERVICE_UNAVAILABLE,Json(json!({"code":500,"message":"fixture reply lost"})));}
                    if newapi {return (StatusCode::OK,Json(json!({"success":true})));}
                    json!({"id":31,"key":"fixture-only-key","name":"LoongPort/a7/composite/3","status":"active"})
                }
                ("GET","/v1/models")=>{
                    s.read_saw_created=fact.state==KeyState::Ready && fact.origin==Some(KeyOrigin::Created);
                    return (StatusCode::OK,Json(json!({"data":[{"id":42}]})));
                }
                _=>return (StatusCode::NOT_FOUND,Json(json!({"unexpected":path}))),
            };
            (StatusCode::OK,Json(json!({"code":0,"message":"success","data":data})))
        }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            origin,
            state,
            server,
        }
    }
    fn receipt(&self) -> RelaySetupReceipt {
        journal::read_on(&self.state.lock().unwrap().conn, OP)
            .unwrap()
            .unwrap()
    }
}
#[tokio::test]
async fn sub2api_records_before_post_and_immediately_after_success_before_catalog_failure() {
    let f = Fixture::start(false, false).await;
    let client = sub2api::Client::new(&f.origin, "fixture-session", Some(7), None, None).unwrap();
    let _ = provision::provision_selected_recorded(
        &client,
        &ModelSelectionTables::builtin(),
        &[3],
        &Observer(f.state.clone()),
    )
    .await;
    let state = f.state.lock().unwrap();
    assert!(state.post_saw_requesting);
    assert!(state.read_saw_created);
    drop(state);
    let receipt = f.receipt();
    assert_eq!(receipt.created_key_count, 1);
    assert_eq!(receipt.groups[0].remote_key_id.as_deref(), Some("31"));
    assert_eq!(
        receipt.targets[0].reason_code.as_deref(),
        Some("modelCatalogUnavailable")
    );
    assert_eq!(receipt.targets[0].compatibility, Compatibility::Unknown);
}
#[tokio::test]
async fn newapi_create_ack_is_recorded_even_when_relist_fails() {
    let f = Fixture::start(true, false).await;
    let client = newapi::NewApiClient::with_account_id(&f.origin, "fixture-session", 7).unwrap();
    let _ = newapi_provision::reconcile_selected_recorded(
        &client,
        7,
        &[newapi::GroupIdentity("alpha".into())],
        &Observer(f.state.clone()),
    )
    .await;
    let state = f.state.lock().unwrap();
    assert!(state.post_saw_requesting);
    assert!(state.read_saw_created);
    drop(state);
    let r = f.receipt();
    assert_eq!(r.created_key_count, 1);
    assert_eq!(r.groups[0].state, KeyState::Ready);
    assert_eq!(r.groups[0].remote_key_id, None);
    assert_eq!(
        r.targets[0].reason_code.as_deref(),
        Some("remoteReadFailed")
    );
}
#[tokio::test]
async fn uncertain_post_and_empty_relist_cannot_create_again() {
    let f = Fixture::start(false, true).await;
    let client = sub2api::Client::new(&f.origin, "fixture-session", Some(7), None, None).unwrap();
    let observer = Observer(f.state.clone());
    let _ = provision::provision_selected_recorded(
        &client,
        &ModelSelectionTables::builtin(),
        &[3],
        &observer,
    )
    .await;
    assert_eq!(f.receipt().groups[0].state, KeyState::Unknown);
    let _ = provision::provision_selected_recorded(
        &client,
        &ModelSelectionTables::builtin(),
        &[3],
        &observer,
    )
    .await;
    assert_eq!(f.state.lock().unwrap().posts, 1);
}
#[tokio::test]
async fn failed_journal_write_results_in_zero_remote_creates() {
    let f = Fixture::start(false, false).await;
    f.state.lock().unwrap().conn.execute_batch("CREATE TRIGGER deny_attempt BEFORE UPDATE ON relay_setup_operations BEGIN SELECT RAISE(ABORT,'fixture journal failure'); END;").unwrap();
    let client = sub2api::Client::new(&f.origin, "fixture-session", Some(7), None, None).unwrap();
    let _ = provision::provision_selected_recorded(
        &client,
        &ModelSelectionTables::builtin(),
        &[3],
        &Observer(f.state.clone()),
    )
    .await;
    assert_eq!(f.state.lock().unwrap().posts, 0);
}

#[tokio::test]
async fn sub2api_reconcile_observes_managed_resource_without_creating_or_claiming_ownership() {
    let f = Fixture::start(false, false).await;
    let observer = Observer(f.state.clone());
    observer.record("3", KeyEvent::BeforeCreate).unwrap();
    observer.record("3", KeyEvent::OutcomeUnknown).unwrap();
    let client = sub2api::Client::new(&f.origin, "fixture-session", Some(7), None, None).unwrap();
    provision::observe_selected_keys(&client, &[3], &observer)
        .await
        .unwrap();
    assert_eq!(
        f.receipt().groups[0].state,
        KeyState::Unknown,
        "empty inventory cannot prove non-creation"
    );
    f.state.lock().unwrap().observe_existing = true;
    provision::observe_selected_keys(&client, &[3], &observer)
        .await
        .unwrap();
    let r = f.receipt();
    assert_eq!(r.groups[0].state, KeyState::Ready);
    assert_eq!(r.groups[0].origin, Some(KeyOrigin::Observed));
    assert_eq!(r.created_key_count, 0);
    assert!(f
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|(method, _)| method == "GET"));
}
#[tokio::test]
async fn newapi_reconcile_only_lists_metadata_and_never_reveals_or_creates() {
    let f = Fixture::start(true, false).await;
    let observer = Observer(f.state.clone());
    observer.record("alpha", KeyEvent::BeforeCreate).unwrap();
    observer.record("alpha", KeyEvent::OutcomeUnknown).unwrap();
    f.state.lock().unwrap().observe_existing = true;
    let client = newapi::NewApiClient::with_account_id(&f.origin, "fixture-session", 7).unwrap();
    newapi_provision::observe_selected_tokens(
        &client,
        7,
        &[newapi::GroupIdentity("alpha".into())],
        &observer,
    )
    .await
    .unwrap();
    let r = f.receipt();
    assert_eq!(r.groups[0].state, KeyState::Ready);
    assert_eq!(r.groups[0].origin, Some(KeyOrigin::Observed));
    assert_eq!(r.created_key_count, 0);
    assert!(f
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|(method, _)| method == "GET"));
}

#[tokio::test]
async fn sub2api_maintenance_cannot_create_without_a_confirmed_operation() {
    let f = Fixture::start(false, false).await;
    let client = sub2api::Client::new(&f.origin, "fixture-session", Some(7), None, None).unwrap();
    let result = provision::provision_selected(&client, &ModelSelectionTables::builtin(), &[3])
        .await
        .unwrap();
    assert_eq!(f.state.lock().unwrap().posts, 0);
    assert_eq!(result.keys_created, 0);
    assert!(!result.failures.is_empty());
    assert_eq!(f.receipt().groups[0].state, KeyState::NotStarted);
}
#[tokio::test]
async fn newapi_maintenance_cannot_create_without_a_confirmed_operation() {
    let f = Fixture::start(true, false).await;
    let client = newapi::NewApiClient::with_account_id(&f.origin, "fixture-session", 7).unwrap();
    let result = newapi_provision::reconcile_selected_for_account(
        &client,
        7,
        &[newapi::GroupIdentity("alpha".into())],
    )
    .await
    .unwrap();
    assert_eq!(f.state.lock().unwrap().posts, 0);
    assert_eq!(result.tokens_created, 0);
    assert!(!result.failures.is_empty());
    assert_eq!(f.receipt().groups[0].state, KeyState::NotStarted);
}
