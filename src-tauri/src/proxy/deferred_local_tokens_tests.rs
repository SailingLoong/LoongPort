//! No sockets, accounts, environment mutation, or production test hooks.
use super::RequestContext;
use crate::proxy::{
    failover_switch::FailoverSwitchManager,
    provider_router::ProviderRouter,
    providers::{codex_chat_history::CodexChatHistoryStore, gemini_shadow::GeminiShadowStore},
    server::ProxyState,
    types::{ProxyConfig, ProxyStatus},
};
use crate::{app_config::AppType, database::Database, provider::Provider};
use axum::http::HeaderMap;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

fn build_state(db: Arc<Database>) -> ProxyState {
    ProxyState {
        db: db.clone(),
        config: Arc::new(RwLock::new(ProxyConfig::default())),
        status: Arc::new(RwLock::new(ProxyStatus::default())),
        start_time: Arc::new(RwLock::new(None)),
        current_providers: Arc::new(RwLock::new(HashMap::new())),
        provider_router: Arc::new(ProviderRouter::new(db.clone())),
        gemini_shadow: Arc::new(GeminiShadowStore::default()),
        codex_chat_history: Arc::new(CodexChatHistoryStore::default()),
        codex_tool_carriers: std::sync::Arc::new(
            crate::proxy::providers::codex_tool_carriers::CodexToolCarrierStore::default(),
        ),
        #[cfg(feature = "gui")]
        app_handle: None,
        service_owner: std::sync::Weak::new(),
        failover_manager: Arc::new(FailoverSwitchManager::new(db)),
        passive_ingress: crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
        model_alignment: std::sync::Arc::new(
            crate::proxy::model_alignment::ModelAlignmentAlerts::new(),
        ),
    }
}

#[tokio::test]
async fn context_construction_defers_local_token_sampling() {
    let db = Arc::new(Database::memory().expect("memory database"));
    let provider = Provider::with_id(
        "deferred-synthetic".into(),
        "Deferred synthetic".into(),
        json!({"base_url":"https://synthetic.invalid/v1", "auth":{"OPENAI_API_KEY":"synthetic"}}),
        None,
    );
    db.save_provider("codex", &provider)
        .expect("save synthetic provider");
    db.set_current_provider("codex", &provider.id)
        .expect("select synthetic provider");
    let state = build_state(db);
    let body = json!({"model":"original-model", "input":"synthetic local sample"});
    let observation = crate::proxy::local_tokens::test_observation::CountScope::start();
    let context = RequestContext::new(
        &state,
        &body,
        &HeaderMap::new(),
        AppType::Codex,
        "Codex",
        "codex",
    )
    .await
    .expect("construct context without network");
    assert_eq!(context.request_model, "original-model");
    assert_eq!(observation.calls(), 0, "context construction must not run synchronous BPE; only usage consumers may resolve the sample");
}
