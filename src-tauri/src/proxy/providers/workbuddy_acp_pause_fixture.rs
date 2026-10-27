//! Synthetic loopback only, real explicit ACP entry; never an official account.
use super::*;
pub(crate) async fn verify() {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = hits.clone();
    let router = axum::Router::new().route(
        "/session",
        axum::routing::any(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { http::StatusCode::BAD_REQUEST }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/session", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut provider = Provider::with_id(
        "synthetic-acp".into(),
        "Synthetic ACP".into(),
        json!({"workbuddy_acp":{
            "account_id":"synthetic-account", "session_id":"synthetic-session", "link":origin,
            "gateway_ticket":"synthetic-ticket", "declared_model":"synthetic-model"
        }}),
        None,
    );
    provider.meta = Some(
        serde_json::from_value(json!({"authBinding":{
            "source":"managed_account","authProvider":"workbuddy","accountId":"synthetic-account"
        }}))
        .unwrap(),
    );
    let before = serde_json::to_value(&provider).unwrap();
    for stream in [true, false] {
        let result = forward(&AppType::Claude, &http::Method::POST, &provider, "/v1/messages",
            &json!({"model":"synthetic-model","stream":stream,"messages":[{"role":"user","content":"synthetic"}]})).await;
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "paused ACP must not open GET or POST"
        );
        assert!(
            matches!(result, Err(ProxyError::InvalidRequest(reason)) if reason == "workbuddy.inference_paused")
        );
    }
    assert_eq!(serde_json::to_value(&provider).unwrap(), before);
    server.abort();
}
