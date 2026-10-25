//! Synthetic loopback regression for the real Codex proxy request path.
use cc_switch_lib::Provider;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[path = "support.rs"]
mod support;

async fn upstream() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let router = axum::Router::new().route(
        "/v1/responses",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({"id":"resp_fixture", "output":[]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, hits, handle)
}

// Both endpoints are loopback-only, including the deliberately stale endpoint.
// Stop the real proxy and fixture listeners before asserting a routing failure.
async fn assert_route(config: impl FnOnce(&str, &str) -> String, expect_selected: bool) {
    support::reset_test_fs();
    let state = support::create_test_state().unwrap();
    let (selected, selected_hits, selected_task) = upstream().await;
    let (stale, stale_hits, stale_task) = upstream().await;
    let provider = Provider::with_id(
        "codex-endpoint-fixture".into(),
        "Synthetic endpoint".into(),
        json!({"config": config(&selected, &stale), "auth":{"OPENAI_API_KEY":"synthetic-fixture-key"}}),
        None,
    );
    state.db.save_provider("codex", &provider).unwrap();
    state
        .db
        .set_current_provider("codex", &provider.id)
        .unwrap();
    let mut cfg = state.db.get_proxy_config().await.unwrap();
    cfg.listen_address = "127.0.0.1".into();
    cfg.listen_port = 0;
    cfg.enable_logging = false;
    cfg.non_streaming_timeout = 3;
    state.db.update_proxy_config(cfg).await.unwrap();
    let info = state.proxy_service.start().await.unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/responses", info.port))
        .header(reqwest::header::CONNECTION, "close")
        .json(&json!({"model":"fixture-model", "stream":false, "input":"fixture"}))
        .send()
        .await;
    let response = match response {
        Ok(response) => {
            let status = response.status();
            response.text().await.map(|body| (status, body))
        }
        Err(error) => Err(error),
    };
    state.proxy_service.stop().await.unwrap();
    selected_task.abort();
    stale_task.abort();
    let _ = selected_task.await;
    let _ = stale_task.await;
    let (status, body) = response.expect("complete loopback proxy response");
    assert_eq!(
        stale_hits.load(Ordering::SeqCst),
        0,
        "must never send to an unselected or commented endpoint"
    );
    assert_eq!(
        selected_hits.load(Ordering::SeqCst),
        usize::from(expect_selected)
    );
    if expect_selected {
        assert!(status.is_success());
    } else {
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert!(body.contains("base_url"));
    }
}

macro_rules! route_case {
    ($name:ident, $config:expr, $ok:expr) => {
        #[allow(clippy::await_holding_lock)]
        #[tokio::test]
        async fn $name() {
            let _guard = support::test_mutex()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            assert_route($config, $ok).await;
        }
    };
}
route_case!(
    selected_provider_after_stale,
    |selected, stale| {
        format!("model_provider = 'selected'\n[model_providers.stale]\nbase_url = \"{stale}\"\n[model_providers.selected]\nbase_url = \"{selected}\"\nwire_api = 'responses'")
    },
    true
);
route_case!(
    selected_provider_before_stale,
    |selected, stale| {
        format!("model_provider = 'selected'\n[model_providers.selected]\nbase_url = \"{selected}\"\nwire_api = 'responses'\n[model_providers.stale]\nbase_url = \"{stale}\"")
    },
    true
);
route_case!(
    commented_old_endpoint,
    |selected, stale| {
        format!("# base_url = \"{stale}\"\nmodel_provider = 'selected'\n[model_providers.selected]\nbase_url = \"{selected}\"\nwire_api = 'responses'")
    },
    true
);
route_case!(
    compact_assignment,
    |selected, _| {
        format!("model_provider='selected'\n[model_providers.selected]\nbase_url=\"{selected}\"\nwire_api='responses'")
    },
    true
);
route_case!(
    missing_selected_provider_fails_closed,
    |_, stale| format!("model_provider='missing'\n[model_providers.stale]\nbase_url = \"{stale}\""),
    false
);
route_case!(
    selected_without_url_fails_closed,
    |_, stale| {
        format!("model_provider='selected'\n[model_providers.stale]\nbase_url = \"{stale}\"\n[model_providers.selected]\nwire_api='responses'")
    },
    false
);
route_case!(
    invalid_toml_fails_closed,
    |_, stale| format!("base_url = \"{stale}\"\n[invalid"),
    false
);
route_case!(
    top_level_legacy_url,
    |selected, _| format!("base_url = '{selected}/'"),
    true
);
route_case!(
    selected_over_top_level,
    |selected, stale| {
        format!("base_url = \"{stale}\"\nmodel_provider='selected'\n[model_providers.selected]\nbase_url = \"{selected}\"\nwire_api='responses'")
    },
    true
);
route_case!(
    grok_selected_model_preserved,
    |selected, _| {
        format!("[models]\ndefault='fixture'\n[model.fixture]\nmodel='fixture-model'\nbase_url='{selected}/'\nname='Fixture'\napi_backend='responses'\ncontext_window=8192\napi_key='synthetic-fixture-key'")
    },
    true
);
route_case!(
    unselected_single_provider_fails_closed,
    |_, stale| format!("[model_providers.stale]\nbase_url = \"{stale}\""),
    false
);
route_case!(
    missing_selected_uses_top_level_legacy_url,
    |selected, stale| {
        format!("model_provider='missing'\nbase_url = \"{selected}\"\n[model_providers.stale]\nbase_url = \"{stale}\"")
    },
    true
);
