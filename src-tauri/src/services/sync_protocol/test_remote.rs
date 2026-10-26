//! Original publication-test conditional object server, shared only by synthetic tests.
use super::*;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use std::{
    future::IntoFuture,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct StoredObject {
    pub(crate) body: Vec<u8>,
    pub(crate) revision: u64,
}
#[derive(Default)]
pub(crate) struct Store {
    pub(crate) objects: BTreeMap<String, StoredObject>,
    pub(crate) ignore_conditions: bool,
    pub(crate) race_on_manifest_publish: bool,
    // Trace routing only, never authentication headers or request payloads.
    pub(crate) requests: Vec<(String, String)>,
}

async fn object(
    State(state): State<Arc<Mutex<Store>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut store = state.lock().unwrap();
    store
        .requests
        .push((method.as_str().into(), uri.path().into()));
    if store.race_on_manifest_publish
        && method == Method::PUT
        && uri.path().ends_with("/manifest.json")
        && headers.contains_key("if-match")
    {
        store.objects.get_mut(uri.path()).unwrap().revision += 1;
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    let current = store.objects.get(uri.path());
    if method == Method::GET || method == Method::HEAD {
        return match current {
            Some(object) => (
                StatusCode::OK,
                [("etag", format!("\"{}\"", object.revision))],
                object.body.clone(),
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    let etag = current.map(|o| format!("\"{}\"", o.revision));
    if !store.ignore_conditions
        && (headers.get("if-none-match").is_some_and(|v| v == "*") && current.is_some()
            || headers
                .get("if-match")
                .is_some_and(|v| Some(v.as_bytes()) != etag.as_ref().map(|e| e.as_bytes())))
    {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    if method == Method::DELETE {
        store.objects.remove(uri.path());
        return StatusCode::NO_CONTENT.into_response();
    }
    let revision = current.map_or(1, |o| o.revision + 1);
    store.objects.insert(
        uri.path().into(),
        StoredObject {
            body: body.to_vec(),
            revision,
        },
    );
    (StatusCode::CREATED, [("etag", format!("\"{revision}\""))]).into_response()
}

pub(crate) struct TestRemote {
    pub(crate) use_s3: bool,
    pub(crate) endpoint: String,
    pub(crate) state: Arc<Mutex<Store>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for TestRemote {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl TestRemote {
    pub(crate) async fn new(use_s3: bool) -> Self {
        let state = Arc::new(Mutex::new(Store::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(any(object))
            .with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).into_future().await.unwrap();
        });
        Self {
            use_s3,
            endpoint,
            state,
            server,
        }
    }
    pub(crate) fn creds(&self) -> super::super::s3::S3Credentials {
        super::super::s3::S3Credentials {
            endpoint: self.endpoint.clone(),
            bucket: "bucket".into(),
            region: "test".into(),
            access_key_id: "test".into(),
            secret_access_key: "test".into(),
        }
    }
    pub(crate) async fn artifact(&self, path: &str, bytes: &[u8]) -> Result<(), AppError> {
        if self.use_s3 {
            super::super::s3::put_object(
                &self.creds(),
                path,
                bytes.to_vec(),
                "application/octet-stream",
            )
            .await
        } else {
            super::super::webdav::put_bytes(
                &format!("{}/{path}", self.endpoint),
                &super::super::webdav::auth_from_credentials("test", "test"),
                bytes.to_vec(),
                "application/octet-stream",
            )
            .await
        }
    }
    pub(crate) async fn publish(
        &self,
        path: &str,
        bytes: &[u8],
        condition: &PutCondition,
    ) -> Result<Option<String>, AppError> {
        if self.use_s3 {
            super::super::s3::put_object_conditional(
                &self.creds(),
                path,
                bytes.to_vec(),
                "application/json",
                condition,
            )
            .await
        } else {
            super::super::webdav::put_bytes_conditional(
                &format!("{}/{path}", self.endpoint),
                &super::super::webdav::auth_from_credentials("test", "test"),
                bytes.to_vec(),
                "application/json",
                condition,
            )
            .await
        }
    }
    pub(crate) async fn read(&self, path: &str) -> Vec<u8> {
        if self.use_s3 {
            super::super::s3::get_object(&self.creds(), path, 4096)
                .await
                .unwrap()
                .unwrap()
                .0
        } else {
            super::super::webdav::get_bytes(
                &format!("{}/{path}", self.endpoint),
                &super::super::webdav::auth_from_credentials("test", "test"),
                4096,
            )
            .await
            .unwrap()
            .unwrap()
            .0
        }
    }
}
