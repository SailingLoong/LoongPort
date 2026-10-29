//! Regressions through the real listener, router and final forwarder.
use super::*;
use crate::provider::{Provider, ProviderMeta};
use axum::http::StatusCode;
use serde_json::{json, Value};

/// Regression for #3787: a provider accidentally pointing at the proxy must
/// fail before it creates another request, even with a dynamically bound port.
#[tokio::test]
async fn loopback_final_upstream_is_rejected() {
    assert_rejected(
        "127.0.0.1",
        &["127.0.0.1", "LOCALHOST.", "[::ffff:127.0.0.1]"],
    )
    .await;
}

#[tokio::test]
async fn loopback_ipv6_and_wildcard_listeners_are_rejected() {
    assert_rejected("::1", &["[::1]", "localhost"]).await;
    assert_rejected("::", &["[::1]"]).await;
    assert_rejected("0.0.0.0", &["localhost", "127.0.0.1"]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn loopback_wildcard_rejects_local_interface_dns_alias() {
    // Discover an existing local hosts alias; never edit DNS or hosts files.
    let hosts = std::fs::read_to_string("/etc/hosts").unwrap();
    for line in hosts.lines() {
        let mut fields = line.split('#').next().unwrap_or("").split_whitespace();
        let Some(ip) = fields
            .next()
            .and_then(|value| value.parse::<std::net::IpAddr>().ok())
        else {
            continue;
        };
        if !ip.is_ipv4() || ip.is_loopback() || ip.is_unspecified() {
            continue;
        }
        if std::net::UdpSocket::bind((ip, 0)).is_err() {
            continue;
        }
        if let Some(alias) = fields.next() {
            assert_rejected("0.0.0.0", &[alias]).await;
            eprintln!("tested an existing non-loopback local hosts alias");
            return;
        }
    }
    eprintln!("no non-loopback local hosts alias exists on this test machine");
}

async fn assert_rejected(listen_address: &str, hosts: &[&str]) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let db = Arc::new(Database::memory().expect("memory database"));
    let proxy = ProxyServer::new(
        ProxyConfig {
            listen_port: 0,
            listen_address: listen_address.into(),
            ..ProxyConfig::default()
        },
        db.clone(),
        #[cfg(feature = "gui")]
        None,
        crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
        Arc::new(super::super::model_alignment::ModelAlignmentAlerts::new()),
    );
    let info = proxy.start().await.expect("start proxy");
    // A second live instance changes the legacy global proxy port. The guard
    // must keep using this request's actual listener owner.
    let other = ProxyServer::new(
        ProxyConfig {
            listen_port: 0,
            listen_address: listen_address.into(),
            ..Default::default()
        },
        Arc::new(Database::memory().unwrap()),
        #[cfg(feature = "gui")]
        None,
        crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
        Arc::new(super::super::model_alignment::ModelAlignmentAlerts::new()),
    );
    let other_info = other.start().await.unwrap();
    assert_ne!(info.port, other_info.port);
    let client_host = if listen_address.contains(':') {
        "[::1]"
    } else {
        "127.0.0.1"
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    for (index, host) in hosts.iter().enumerate() {
        let full_url = index % 2 == 1;
        let stream = index % 2 == 0;
        let base_url = if full_url {
            format!("http://synthetic-user:private-password@{host}:{}/private-path/v1/responses?key=private-query", info.port)
        } else {
            format!("http://{host}:{}/v1", info.port)
        };
        let mut provider = Provider::with_id(
            "self-forwarding".to_string(),
            "Self forwarding".to_string(),
            json!({
                "base_url": base_url,
                "auth": {"OPENAI_API_KEY": "upstream-secret"},
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            is_full_url: Some(full_url),
            ..ProviderMeta::default()
        });
        db.save_provider("codex", &provider).unwrap();
        db.set_current_provider("codex", &provider.id).unwrap();
        let before = proxy.get_status().await.total_requests;
        let request = client
            .post(format!("http://{}:{}/v1/responses", client_host, info.port))
            .json(&json!({
                "model": "gpt-5",
                "stream": stream,
                "input": [{"role": "user", "content": "x".repeat(256 * 1024)}],
            }));
        // Bound the regression itself if the guard is ever removed: stop
        // after the first internal request instead of letting the loop grow.
        let response = tokio::select! {
            response = request.send() => match response {
                Ok(response) => response,
                Err(error) => {
                    let status = proxy.get_status().await;
                    proxy.stop().await.unwrap();
                    panic!("request failed: {error}; total_requests={}, active_connections={}", status.total_requests, status.active_connections);
                }
            },
            _ = async {
                while proxy.get_status().await.total_requests < before + 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            } => {
                proxy.stop().await.unwrap();
                panic!("provider request looped back into the proxy");
            }
        };
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: Value = response.json().await.unwrap();
        assert!(error.to_string().contains("own proxy listener"), "{error}");
        for secret in [
            "upstream-secret",
            "synthetic-user",
            "private-password",
            "private-path",
            "private-query",
        ] {
            assert!(
                !error.to_string().contains(secret),
                "rejection disclosed upstream data"
            );
        }
        assert_eq!(proxy.get_status().await.total_requests, before + 1);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while proxy.get_status().await.active_connections != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("rejected request releases its connection");
    }
    other.stop().await.unwrap();
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn loopback_other_port_upstream_remains_available() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .fallback(|| async { axum::Json(json!({"id":"synthetic-response","output":[]})) }),
        )
        .await
        .unwrap();
    });
    let db = Arc::new(Database::memory().unwrap());
    let provider = Provider::with_id(
        "local-model".into(),
        "Local model".into(),
        json!({"base_url":format!("http://{upstream}/v1"),
            "auth":{"OPENAI_API_KEY":"synthetic-token"}}),
        None,
    );
    db.save_provider("codex", &provider).unwrap();
    db.set_current_provider("codex", &provider.id).unwrap();
    let proxy = ProxyServer::new(
        ProxyConfig {
            listen_port: 0,
            ..Default::default()
        },
        db,
        #[cfg(feature = "gui")]
        None,
        crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
        Arc::new(super::super::model_alignment::ModelAlignmentAlerts::new()),
    );
    let info = proxy.start().await.unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/responses", info.port))
        .json(&json!({"model":"synthetic-model","stream":false,"input":"hello"}))
        .send()
        .await;
    proxy.stop().await.unwrap();
    task.abort();
    let response = response.expect("local upstream at another port remains reachable");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "synthetic-response"
    );
}
