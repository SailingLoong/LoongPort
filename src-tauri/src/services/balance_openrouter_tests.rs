use super::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Real loopback HTTP exercises the production request/status/parser path. Keys
// are synthetic; no official endpoint or credentials are used by these tests.
async fn query(replies: Vec<(u16, String)>) -> (Result<UsageResult, String>, Vec<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-openrouter-key\r\n"));
            captured
                .lock()
                .unwrap()
                .push(request.lines().next().unwrap().to_owned());
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let result = query_openrouter_with_client(&client, "synthetic-openrouter-key", &origin).await;
    server.abort();
    if let Err(error) = server.await {
        assert!(error.is_cancelled(), "fixture failed: {error}");
    }
    let captured = requests.lock().unwrap().clone();
    (result, captured)
}
fn forbidden() -> (u16, String) {
    (403, "{}".into())
}
fn key_body() -> String {
    json!({"data":{"label":"synthetic-key", "is_management_key":false, "limit":100, "limit_remaining":74.5, "usage":25.5}}).to_string()
}

#[tokio::test]
async fn normal_key_is_verified_without_fabricating_account_balance_or_inference_validity() {
    let (result, paths) = query(vec![forbidden(), (200, key_body())]).await;
    let result = result.unwrap();
    assert!(
        result.success,
        "normal inference key must not be marked expired by credits permission denial"
    );
    let data = &result.data.as_ref().unwrap()[0];
    assert_eq!(
        (data.total, data.used, data.remaining, data.unit.as_ref()),
        (None, None, None, None)
    );
    assert_eq!(data.is_valid, None);
    assert!(data.invalid_message.is_none());
    assert_eq!(
        data.extra.as_deref(),
        Some("API key verified; account balance unavailable (management key required).")
    );
    let wire = serde_json::to_value(data).unwrap();
    for field in [
        "remaining",
        "total",
        "used",
        "unit",
        "isValid",
        "invalidMessage",
    ] {
        assert!(wire.get(field).is_none(), "unexpected {field}");
    }
    assert_eq!(paths, ["GET /credits HTTP/1.1", "GET /key HTTP/1.1"]);
}

#[tokio::test]
async fn invalid_key_401_remains_invalid_without_a_fallback() {
    let (result, paths) = query(vec![(401, "{}".into())]).await;
    assert_eq!(result.unwrap().data.unwrap()[0].is_valid, Some(false));
    assert_eq!(paths, ["GET /credits HTTP/1.1"]);
}

#[tokio::test]
async fn verification_401_reports_invalid_key() {
    let (result, paths) = query(vec![forbidden(), (401, "{}".into())]).await;
    assert_eq!(result.unwrap().data.unwrap()[0].is_valid, Some(false));
    assert_eq!(paths.len(), 2);
}

#[tokio::test]
async fn verification_permission_denial_does_not_claim_expired_credentials() {
    let (result, _) = query(vec![forbidden(), forbidden()]).await;
    assert!(
        result.is_err(),
        "verification permission denial leaves credential state unknown"
    );
}

#[tokio::test]
async fn verification_unknown_or_unavailable_response_never_becomes_zero_balance() {
    for response in [
        (200, "{}"),
        (200, "{\"data\":{}}"),
        (200, "{\"data\":{\"label\":7}}"),
        (200, "not-json"),
        (429, "{}"),
        (500, "{}"),
    ] {
        let (result, _) = query(vec![forbidden(), (response.0, response.1.into())]).await;
        assert!(
            result.is_err(),
            "unknown verification response: {response:?}"
        );
    }
}

#[tokio::test]
async fn verification_transport_failure_is_unknown() {
    // The fixture closes its listener after the sole credits response.
    let (result, _) = query(vec![forbidden()]).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn management_key_retains_actual_credits_and_does_not_query_key() {
    let (result, paths) = query(vec![(
        200,
        json!({"data":{"total_credits":100.5,"total_usage":25.75}}).to_string(),
    )])
    .await;
    let data = result.unwrap().data.unwrap().remove(0);
    assert_eq!(
        (data.total, data.used, data.remaining),
        (Some(100.5), Some(25.75), Some(74.75))
    );
    assert_eq!(data.unit.as_deref(), Some("USD"));
    assert_eq!(paths, ["GET /credits HTTP/1.1"]);
}

#[tokio::test]
async fn missing_or_non_numeric_credits_are_not_zero_balance() {
    for body in [
        json!({}),
        json!({"data":{}}),
        json!({"data":{"total_credits":null,"total_usage":0}}),
        json!({"data":{"total_credits":"NaN","total_usage":0}}),
        json!({"data":{"total_credits":1,"total_usage":"Infinity"}}),
    ] {
        let (result, paths) = query(vec![(200, body.to_string())]).await;
        assert!(!result.unwrap().success, "invalid monetary payload: {body}");
        assert_eq!(paths.len(), 1);
    }
}

#[tokio::test]
async fn other_credits_errors_do_not_trigger_key_verification() {
    let (result, paths) = query(vec![(500, "{}".into())]).await;
    assert!(!result.unwrap().success);
    assert_eq!(paths.len(), 1);
}
