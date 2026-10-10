//! Synthetic localhost HTTP fixtures for authenticated redirect regressions.
//! Reuses the Axum + ephemeral-listener pattern from auto_mode_e2e_tests.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub(crate) struct ReceivedRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Default)]
struct MockState {
    routes: Arc<Mutex<BTreeMap<String, (StatusCode, String)>>>,
    received: Arc<Mutex<Vec<ReceivedRequest>>>,
}

pub(crate) struct MockServer {
    pub base_url: String,
    state: MockState,
    task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    pub async fn spawn() -> Self {
        let state = MockState::default();
        let app = axum::Router::new()
            .fallback(handle_request)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic redirect upstream");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url,
            state,
            task,
        }
    }

    pub fn redirect(&self, path: &str, status: u16, location: &str) {
        self.state.routes.lock().unwrap().insert(
            path.to_string(),
            (StatusCode::from_u16(status).unwrap(), location.to_string()),
        );
    }

    pub fn received(&self) -> Vec<ReceivedRequest> {
        self.state.received.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.state.received.lock().unwrap().clear();
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_request(
    State(state): State<MockState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.received.lock().unwrap().push(ReceivedRequest {
        method,
        uri: uri.clone(),
        headers,
        body,
    });
    if let Some((status, location)) = state.routes.lock().unwrap().get(uri.path()) {
        return (*status, [("location", location.clone())]).into_response();
    }
    axum::Json(serde_json::json!({"data": [{"id": "synthetic-model"}]})).into_response()
}

/// Isolate process-global proxy/environment mutations from parallel lib tests.
/// Returns true in the parent after the one exact original test has passed.
pub(crate) fn run_in_isolated_process(test_name: &str) -> bool {
    const CHILD_TEST: &str = "LOONGPORT_AUTH_REDIRECT_TEST_CHILD";
    if std::env::var(CHILD_TEST).as_deref() == Ok(test_name) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(CHILD_TEST, test_name)
        .output()
        .expect("run isolated original lib test");
    assert!(
        output.status.success(),
        "isolated test {test_name} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    true
}
