use super::*;
use crate::secrets::VaultContext;
use std::collections::VecDeque;
struct Fixture {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    _root: tempfile::TempDir,
    session: Arc<SecretSession>,
    id: String,
}
impl Fixture {
    async fn new(realm: Realm, expired: bool) -> Self {
        let guard = store::fixture_guard().await;
        operations().lock().unwrap().clear();
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        let mut account = super::super::import::synthetic_material().account;
        account.id = "synthetic-model-account".into();
        account.realm = realm;
        account.device.auth_host = realm.auth().into();
        if expired {
            account.grant.as_mut().unwrap().expires = 1;
        }
        let id = account.id.clone();
        store::access(&session, None, |d, _| {
            d.accounts.push(account);
            Ok(((), true))
        })
        .await
        .unwrap();
        Self {
            _guard: guard,
            _root: root,
            session,
            id,
        }
    }
    async fn begin(&self) -> String {
        begin(self.session.clone(), &self.id)
            .await
            .unwrap()
            .request_id
    }
}
#[derive(Default)]
struct Wire {
    calls: Mutex<Vec<inference::Request>>,
    responses: Mutex<VecDeque<ProxyResponse>>,
}
impl Transport for Wire {
    fn send(
        &self,
        r: inference::Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProxyError::Internal("synthetic exhausted".into()))
        })
    }
}
fn response(status: u16, body: Value) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::from_u16(status).unwrap(),
        http::HeaderMap::new(),
        Bytes::from(body.to_string()),
    )
}
fn catalog() -> Value {
    json!({"config_info_list":[{"config_name":"text-one","display_config":{"display_name":"Text One"},"usage":"chat_completion"}]})
}
#[tokio::test]
async fn models_fixed_region_single_chat_v3_and_safe_dto() {
    for realm in [Realm::Cn, Realm::Sg, Realm::Us] {
        let f = Fixture::new(realm, false).await;
        let id = f.begin().await;
        let wire = Wire::default();
        assert!(wire.calls.lock().unwrap().is_empty());
        wire.responses
            .lock()
            .unwrap()
            .push_back(response(200, catalog()));
        let found = list_with(f.session.clone(), &id, &wire).await.unwrap();
        assert_eq!(
            found,
            [ModelView {
                id: "text-one".into(),
                name: "Text One".into()
            }]
        );
        let dto = serde_json::to_value(&found).unwrap();
        assert_eq!(dto[0].as_object().unwrap().len(), 2);
        let calls = wire.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, format!("{}{PATH}", realm.api()));
        assert_eq!(calls[0].headers["accept"], "application/json");
        assert_eq!(
            calls[0].headers["authorization"],
            "Cloud-IDE-JWT synthetic-access"
        );
        assert_eq!(
            calls[0].body,
            json!({"function":"chat_v3","config_names":null,"need_prompt":false,"current_config_info":null,"poly_prompt":true,"mode_type":null,"agent_type":null})
        );
    }
}
#[tokio::test]
async fn models_cancel_before_list_and_single_use_do_not_send_again() {
    let f = Fixture::new(Realm::Sg, false).await;
    let id = f.begin().await;
    let wire = Wire::default();
    cancel(&f.session, &id).await.unwrap();
    assert_eq!(
        list_with(f.session.clone(), &id, &wire).await.unwrap_err(),
        Error::Cancelled
    );
    assert!(wire.calls.lock().unwrap().is_empty());
    let id = f.begin().await;
    wire.responses
        .lock()
        .unwrap()
        .push_back(response(200, catalog()));
    list_with(f.session.clone(), &id, &wire).await.unwrap();
    assert!(list_with(f.session.clone(), &id, &wire).await.is_err());
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn models_cn_missing_crossroot_and_changed_realm_send_zero_http() {
    let f = Fixture::new(Realm::Cn, false).await;
    // Previously stored CN rows cannot acquire a consumer by upgrading alone.
    store::access(&f.session, None, |d, _| {
        d.accounts[0].device.native_package = None;
        Ok(((), true))
    })
    .await
    .unwrap();
    assert_eq!(
        begin(f.session.clone(), &f.id).await.unwrap_err(),
        Error::Unsupported
    );
    drop(f);
    let f = Fixture::new(Realm::Sg, false).await;
    let id = f.begin().await;
    let wire = Wire::default();
    assert!(begin(f.session.clone(), "missing").await.is_err());
    let root = tempfile::tempdir().unwrap();
    let other =
        SecretSession::from_storage_context(root.path().into(), f.session.read().unwrap().clone());
    assert_eq!(
        list_with(other, &id, &wire).await.unwrap_err(),
        Error::Cancelled
    );
    store::access(&f.session, None, |d, _| {
        d.accounts[0].realm = Realm::Us;
        Ok(((), true))
    })
    .await
    .unwrap();
    assert_eq!(
        list_with(f.session.clone(), &id, &wire).await.unwrap_err(),
        Error::Cancelled
    );
    assert!(wire.calls.lock().unwrap().is_empty());
}
struct Late {
    session: Arc<SecretSession>,
    id: String,
    request: String,
    action: u8,
    calls: Mutex<usize>,
}
impl Transport for Late {
    fn send(
        &self,
        _: inference::Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            tokio::task::yield_now().await;
            match self.action {
                0 => cancel(&self.session, &self.request).await.unwrap(),
                1 => store::retire(&self.session, &self.id).await.unwrap(),
                2 => {
                    self.session.set_blocked(true);
                    self.session.set_blocked(false);
                }
                _ => {
                    store::access(&self.session, None, |d, _| {
                        d.accounts[0].generation += 1;
                        Ok(((), true))
                    })
                    .await
                    .unwrap();
                }
            }
            Ok(response(200, catalog()))
        })
    }
}
#[tokio::test]
async fn models_late_cancel_retire_epoch_and_generation_never_publish() {
    for action in 0..4 {
        let f = Fixture::new(Realm::Sg, false).await;
        let request = f.begin().await;
        let wire = Late {
            session: f.session.clone(),
            id: f.id.clone(),
            request: request.clone(),
            action,
            calls: Mutex::new(0),
        };
        assert!(list_with(f.session.clone(), &request, &wire).await.is_err());
        assert_eq!(*wire.calls.lock().unwrap(), 1);
    }
}
#[test]
fn models_parser_filters_helpers_custom_invisible_and_keeps_only_returned_models() {
    let v = json!({"data":{"config_info_list":[{"config_name":"text-one","display_name":"One"},{"config_name":"text-two","display_model_name":"Two"},{"config_name":"text-one","display_name":"One"},{"config_name":"custom_model_slot"},{"config_name":"disabled","config_switch":false},{"config_name":"helper","usage":"summary"},{"config_name":"hidden","is_invisible_to_user":true},{"config_name":"custom","display_config":{"is_custom_model":true}}]}});
    assert_eq!(
        parse(&v).unwrap(),
        [
            ModelView {
                id: "text-one".into(),
                name: "One".into()
            },
            ModelView {
                id: "text-two".into(),
                name: "Two".into()
            }
        ]
    );
    assert!(parse(&json!({"function_configs":[{"function":"solo_agent","config_info_list":[{"config_name":"solo-only"}]}]})).is_err());
    assert!(parse(&json!({"config_info_list":[]})).unwrap().is_empty());
}
#[test]
fn models_parser_rejects_malformed_limit_duplicate_and_custom_shape() {
    for v in [
        json!({}),
        json!({"config_info_list":{}}),
        json!({"config_info_list":[{"config_name":"x"}],"ResponseMetadata":{"Error":{"Message":"synthetic unknown error"}}}),
        json!({"config_info_list":[{"config_name":2}]}),
        json!({"config_info_list":[{"config_name":"x","config_switch":"false"}]}),
        json!({"config_info_list":[{"config_name":"x","display_config":{"is_custom_model":"true"}}]}),
        json!({"config_info_list":[{"config_name":"x","display_name":"a"},{"config_name":"x","display_name":"b"}]}),
        json!({"config_info_list":[{"config_name":"x".repeat(257)}]}),
        json!({"config_info_list":vec![json!({"config_name":"x"});513]}),
    ] {
        assert!(parse(&v).is_err(), "malformed response accepted");
    }
}
#[tokio::test]
async fn models_denials_do_not_fallback_or_forward_raw_error() {
    for (status, v, expected) in [
        (
            200,
            json!({"code":11004,"message":"synthetic-refresh"}),
            Error::PermissionDenied,
        ),
        (
            401,
            json!({"message":"denied"}),
            Error::AuthorizationUnavailable,
        ),
        (429, json!({"code":4008}), Error::RateLimited),
        (500, json!({"message":"server"}), Error::Unavailable),
    ] {
        let f = Fixture::new(Realm::Sg, false).await;
        let id = f.begin().await;
        let wire = Wire::default();
        wire.responses
            .lock()
            .unwrap()
            .push_back(response(status, v));
        assert_eq!(
            list_with(f.session.clone(), &id, &wire).await.unwrap_err(),
            expected
        );
        assert_eq!(wire.calls.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn models_malformed_compressed_and_oversized_responses_fail_closed() {
    for case in 0..3 {
        let f = Fixture::new(Realm::Sg, false).await;
        let id = f.begin().await;
        let wire = Wire::default();
        let mut headers = http::HeaderMap::new();
        if case == 1 {
            headers.insert("content-encoding", http::HeaderValue::from_static("gzip"));
        }
        let bytes = match case {
            0 => b"<html>unexpected</html>".to_vec(),
            1 => catalog().to_string().into_bytes(),
            _ => vec![b'x'; MAX_BYTES + 1],
        };
        wire.responses
            .lock()
            .unwrap()
            .push_back(ProxyResponse::buffered(
                http::StatusCode::OK,
                headers,
                Bytes::from(bytes),
            ));
        assert_eq!(
            list_with(f.session.clone(), &id, &wire).await.unwrap_err(),
            Error::ResponseInvalid
        );
        assert_eq!(wire.calls.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn models_escaped_and_semantic_known_credentials_never_enter_dto() {
    for raw in [
        r#"{"config_info_list":[{"config_name":"safe","display_name":"synthetic\u002drefresh"}]}"#,
        r#"{"config_info_list":[{"config_name":"synthetic-","display_name":"refresh"}]}"#,
    ] {
        let f = Fixture::new(Realm::Sg, false).await;
        let id = f.begin().await;
        let wire = Wire::default();
        wire.responses
            .lock()
            .unwrap()
            .push_back(ProxyResponse::buffered(
                http::StatusCode::OK,
                http::HeaderMap::new(),
                Bytes::copy_from_slice(raw.as_bytes()),
            ));
        assert_eq!(
            list_with(f.session.clone(), &id, &wire).await.unwrap_err(),
            Error::ResponseInvalid
        );
    }
}
#[tokio::test]
async fn models_expiry_and_bounded_request_registry_keep_no_catalog() {
    let f = Fixture::new(Realm::Sg, false).await;
    let first = f.begin().await;
    {
        let mut map = operations().lock().unwrap();
        Arc::get_mut(map.get_mut(&first).unwrap()).unwrap().expires = store::now() - 1;
    }
    let wire = Wire::default();
    assert_eq!(
        list_with(f.session.clone(), &first, &wire)
            .await
            .unwrap_err(),
        Error::Expired
    );
    for _ in 0..LIMIT {
        f.begin().await;
    }
    assert_eq!(
        begin(f.session.clone(), &f.id).await.unwrap_err(),
        Error::Busy
    );
    assert!(wire.calls.lock().unwrap().is_empty());
}
fn refresh_reply() -> Value {
    json!({"Result":{"Token":"synthetic-new-access","RefreshToken":"synthetic-new-refresh","TokenExpireAt":4102444800000_i64,"BoundDeviceID":"1234567890123456","DeviceBindStatus":"BOUND"}})
}
#[tokio::test]
async fn models_expired_token_uses_original_refresh_profile_and_same_host_catalog() {
    let f = Fixture::new(Realm::Sg, true).await;
    let id = f.begin().await;
    let wire = Wire::default();
    wire.responses.lock().unwrap().extend([
        response(200, refresh_reply()),
        response(
            200,
            json!({"Result":{"UserID":"synthetic-user","AIRegion":"SG"}}),
        ),
        response(200, catalog()),
    ]);
    let result = list_with(f.session.clone(), &id, &wire).await.unwrap();
    assert_eq!(result.len(), 1);
    {
        let calls = wire.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls[0]
            .url
            .starts_with("https://growsg-normal.trae.ai/trae/api/v3/oauth/"));
        assert!(calls[1].url.ends_with("/GetUserInfo"));
        assert_eq!(calls[2].url, format!("{}{PATH}", Realm::Sg.api()));
        assert_eq!(calls[2].headers["x-cloudide-token"], "synthetic-new-access");
    }

    assert!(
        !store::snapshot(&f.session, &f.id)
            .await
            .unwrap()
            .account
            .refresh_pending
    );
}
struct CancelRefresh {
    session: Arc<SecretSession>,
    id: String,
    calls: Mutex<usize>,
}
impl Transport for CancelRefresh {
    fn send(
        &self,
        _: inference::Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            tokio::task::yield_now().await;
            cancel(&self.session, &self.id).await.unwrap();
            Ok(response(200, refresh_reply()))
        })
    }
}
#[tokio::test]
async fn models_cancel_after_exchange_never_starts_profile_or_catalog_or_replays() {
    let f = Fixture::new(Realm::Sg, true).await;
    let id = f.begin().await;
    let wire = CancelRefresh {
        session: f.session.clone(),
        id: id.clone(),
        calls: Mutex::new(0),
    };
    assert!(list_with(f.session.clone(), &id, &wire).await.is_err());
    assert_eq!(*wire.calls.lock().unwrap(), 1);
    assert!(list_with(f.session.clone(), &id, &wire).await.is_err());
    assert_eq!(*wire.calls.lock().unwrap(), 1);
    assert_eq!(
        store::list(&f.session).await.unwrap()[0].status,
        store::AccountStatus::RefreshPending
    );
}
