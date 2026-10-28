//! Narrow debug/test-hook fixtures exercising the production relay boundary.
//! Only loopback HTTP and an in-memory provider database are used.
use super::*;
use axum::{body::Body, extract::Request, routing::any, Json, Router};
use serde_json::{json, Value};

#[derive(Default)]
struct FixtureState {
    keys: Vec<Value>,
    tokens: Vec<Value>,
    requests: Vec<(String, String)>,
    inactive: bool,
    models: Option<Vec<String>>,
    invalid_models: bool,
}

struct Fixture {
    origin: String,
    state: Arc<Mutex<FixtureState>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn start() -> Self {
        static LOOPBACK: std::sync::Once = std::sync::Once::new();
        LOOPBACK.call_once(|| {
            for name in ["NO_PROXY", "no_proxy"] {
                let previous = std::env::var(name).unwrap_or_default();
                std::env::set_var(name, format!("{previous},127.0.0.1,localhost,::1"));
            }
        });
        let state = Arc::new(Mutex::new(FixtureState::default()));
        let shared = state.clone();
        let app = Router::new().fallback(any(move |request: Request<Body>| {
            let state = shared.clone();
            async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_string();
                let body = axum::body::to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
                let mut state = state.lock().unwrap();
                state.requests.push((method.clone(), path.clone()));
                let data = match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/models") => {
                        if state.invalid_models {
                            return Json(json!({"data":[{"id":42}]}));
                        }
                        let models = state.models.clone().unwrap_or_else(|| vec!["gpt-5.4".into(), "claude-sonnet-4-5".into()]);
                        return Json(json!({"data": models.iter().map(|id| json!({"id":id})).collect::<Vec<_>>()}));
                    }
                    ("GET", "/.well-known/loongport.json") => return Json(json!({})),
                    ("GET", "/api/user/self") => return Json(json!({"success":true,"data":{"id":7,"username":"fixture","display_name":"Fixture","email":"fixture@example.test","group":"default","quota":0,"used_quota":0}})),
                    ("GET", "/api/user/self/groups") => return Json(json!({"success":true,"data":{"alpha":{"ratio":1,"desc":"Alpha"},"beta":{"ratio":1,"desc":"Beta"}}})),
                    ("GET", "/api/token/") => return Json(json!({"success":true,"data":{"page":1,"page_size":100,"total":state.tokens.len(),"items":state.tokens}})),
                    ("POST", "/api/token/") => {
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        let id = state.tokens.len() as i64 + 100;
                        state.tokens.push(json!({"id":id,"key":"masked-fixture-key","name":body["name"],"group":body["group"],"status":1}));
                        return Json(json!({"success":true}));
                    }
                    ("POST", path) if path.starts_with("/api/token/") && path.ends_with("/key") => {
                        let id = path.trim_start_matches("/api/token/").trim_end_matches("/key");
                        return Json(json!({"success":true,"data":{"key":format!("fixture-revealed-{id}")}}));
                    }
                    ("DELETE", path) if path.starts_with("/api/token/") => {
                        let id = path.trim_start_matches("/api/token/").parse::<i64>().unwrap();
                        state.tokens.retain(|token| token["id"].as_i64() != Some(id));
                        return Json(json!({"success":true}));
                    }
                    ("GET", "/api/v1/groups/available") => json!([
                        {"id":1,"name":"Alpha","platform":"openai","status":"active","rate_multiplier":1},
                        {"id":2,"name":"Beta","platform":"anthropic","status":if state.inactive { "inactive" } else { "active" },"rate_multiplier":1},
                        {"id":3,"name":"Shared","platform":"composite","status":"active","rate_multiplier":1}
                    ]),
                    ("GET", "/api/v1/keys") => json!({"items":state.keys,"pages":1}),
                    ("POST", "/api/v1/keys") => {
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        let id = body["group_id"].as_i64().unwrap();
                        let key = json!({"id":id,"key":format!("fixture-key-{id}"),"name":body["name"],"status":"active"});
                        state.keys.push(key.clone());
                        key
                    }
                    ("GET", "/api/v1/subscriptions") => json!([]),
                    ("GET", "/api/v1/groups/rates") => json!({}),
                    ("DELETE", path) if path.starts_with("/api/v1/keys/") => json!(true),
                    _ => panic!("unexpected fixture request {method} {path}"),
                };
                Json(json!({"code":0,"message":"success","data":data}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            origin,
            state,
            server,
        }
    }

    fn account(&self, backend: discovery::BackendKind) -> creds::RelayAccount {
        let mut result = account(7);
        result.site_origin = self.origin.clone();
        result.backend_kind = backend;
        result
    }
}

fn account(id: i64) -> creds::RelayAccount {
    creds::RelayAccount {
        id,
        site_origin: "https://relay.example".into(),
        site_name: "Fixture relay".into(),
        backend_kind: discovery::BackendKind::NewApi,
        api_base_url: String::new(),
        account_id: Some(id),
        account_label: format!("fixture-{id}"),
        login_identifier: format!("fixture-{id}"),
        auth_token: "fixture-session".into(),
        refresh_token: None,
        token_expires_at: None,
        user_agent: None,
        cf_clearance: None,
        pricing_synced_at: None,
        sort_index: 0,
    }
}

fn provider(id: String, owner: Option<i64>) -> Provider {
    Provider {
        id,
        name: "Fixture".into(),
        settings_config: json!({}),
        website_url: Some("https://relay.example".into()),
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: owner.map(|owner| managed_meta(&AppType::Codex, Some(owner), None)),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
        available_models: None,
    }
}

fn batch(account: &creds::RelayAccount, group: &str) -> ManagedProvisionBatch {
    let group = newapi_provision::ReconciledGroup {
        identity: newapi::GroupIdentity(group.into()),
        name: group.into(),
        rate_multiplier: Some(1.0),
        description: "Fixture".into(),
        api_key: format!("fixture-{group}-key"),
        token_was_created: false,
    };
    ManagedProvisionBatch {
        account_id: account.account_id,
        candidates: newapi_candidates_for_group(
            &account.site_origin,
            account.account_id.unwrap(),
            &group,
            &[
                "gpt-5.4".into(),
                "claude-sonnet-4-5".into(),
                "gemini-2.5-pro".into(),
            ],
            &model_selection::ModelSelectionTables::builtin(),
        ),
        ..Default::default()
    }
}

fn verify_persistence() {
    let account = account(7);
    let db = Arc::new(crate::database::Database::memory().unwrap());
    let state = AppState::new(db.clone()).unwrap();
    persist_provision_batch_scope(
        &state,
        &account,
        batch(&account, "old"),
        ProvisionPersistenceIntent::SelectedPrepare,
    )
    .unwrap();
    let old_id = managed::newapi_provider_id_for(&account.site_origin, 7, "old");
    let old_codex = db.get_provider_by_id(&old_id, "codex").unwrap().unwrap();
    let mut next = batch(&account, "selected");
    restrict_batch_to_selection(
        &mut next,
        &[RelayTierSelection {
            app: "codex".into(),
            group_id: "selected".into(),
        }],
    );
    let summary = persist_provision_batch_scope(
        &state,
        &account,
        next,
        ProvisionPersistenceIntent::SelectedPrepare,
    )
    .unwrap();
    assert_eq!(summary.tiers.len(), 1);
    for app in ["claude", "codex", "gemini"] {
        assert!(db.get_provider_by_id(&old_id, app).unwrap().is_some());
    }
    assert_eq!(
        db.get_provider_by_id(&old_id, "codex")
            .unwrap()
            .unwrap()
            .settings_config,
        old_codex.settings_config
    );
    let new_id = managed::newapi_provider_id_for(&account.site_origin, 7, "selected");
    assert!(db.get_provider_by_id(&new_id, "codex").unwrap().is_some());
    assert!(db.get_provider_by_id(&new_id, "claude").unwrap().is_none());
    assert!(db.get_provider_by_id(&new_id, "gemini").unwrap().is_none());
}

fn verify_selected_isolation() {
    let account = account(7);
    let db = Arc::new(crate::database::Database::memory().unwrap());
    let state = AppState::new(db.clone()).unwrap();
    let mut selected = batch(&account, "selected");
    restrict_batch_to_selection(
        &mut selected,
        &[RelayTierSelection {
            app: "codex".into(),
            group_id: "selected".into(),
        }],
    );
    let id = selected.candidates[0].provider_id.clone();
    let mut original = provider(id.clone(), Some(7));
    original.settings_config =
        json!({"auth":{"OPENAI_API_KEY":"fixture-manual-key"},"config":"model = \"manual-model\""});
    original.notes = Some("retain manually edited routing".into());
    original.in_failover_queue = true;
    db.save_provider("codex", &original).unwrap();
    db.set_available_models("codex", &id, &["manual-model".into()])
        .unwrap();
    db.set_current_provider("codex", &id).unwrap();
    db.set_user_edited("codex", &id, true).unwrap();
    db.set_setting(
        "application_priority_codex",
        &serde_json::to_string(&[&id]).unwrap(),
    )
    .unwrap();
    let before = db.get_provider_by_id(&id, "codex").unwrap().unwrap();
    let before_chain = db.get_setting("application_priority_codex").unwrap();
    let before_mcp = db.get_all_mcp_servers().unwrap();
    let summary = persist_provision_batch_scope(
        &state,
        &account,
        selected,
        ProvisionPersistenceIntent::SelectedPrepare,
    )
    .unwrap();
    let after = db.get_provider_by_id(&id, "codex").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert_eq!(before.available_models, after.available_models);
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some(id.as_str())
    );
    assert_eq!(
        db.get_setting("application_priority_codex").unwrap(),
        before_chain
    );
    assert_eq!(
        serde_json::to_value(db.get_all_mcp_servers().unwrap()).unwrap(),
        serde_json::to_value(before_mcp).unwrap()
    );
    assert!(db.get_user_edited("codex", &id).unwrap());
    assert!(summary.merged_providers.is_empty());
    assert_eq!(summary.tiers[0].model, "manual-model");
}

/// Exercise the new-row branch against side effects that would otherwise be
/// observable: a mergeable current duplicate, registered MCP, and live files.
fn verify_selected_new_projection_isolation() {
    let account = account(7);
    let db = Arc::new(crate::database::Database::memory().unwrap());
    let state = AppState::new(db.clone()).unwrap();
    let mut selected = batch(&account, "selected-new");
    restrict_batch_to_selection(
        &mut selected,
        &[RelayTierSelection {
            app: "codex".into(),
            group_id: "selected-new".into(),
        }],
    );
    let candidate = &selected.candidates[0];
    let id = candidate.provider_id.clone();
    let mut duplicate = provider("fixture-unmanaged-current".into(), None);
    duplicate.settings_config = provider_config::settings_config_for(
        &AppType::Codex,
        &candidate.api_key,
        "Imported duplicate",
        &sub2api::base_url_for(
            &AppType::Codex,
            &account.site_origin,
            &candidate.api_base_url,
        ),
        "manual-model",
    )
    .unwrap();
    duplicate.in_failover_queue = true;
    db.save_provider("codex", &duplicate).unwrap();
    db.set_current_provider("codex", &duplicate.id).unwrap();
    db.set_user_edited("codex", &duplicate.id, true).unwrap();
    let chain = format!("[ \"{}\" ]", duplicate.id);
    db.set_setting("application_priority_codex", &chain)
        .unwrap();
    db.save_mcp_server(&crate::app_config::McpServer {
        id: crate::relay::imagegen_mcp::IMAGEGEN_MCP_ID.into(),
        name: "Existing image tool sentinel".into(),
        server: json!({"type":"stdio", "command":"fixture-never-executed"}),
        apps: crate::app_config::McpApps::default(),
        description: Some("preserve registration until explicit apply".into()),
        homepage: None,
        docs: None,
        tags: vec!["fixture-sentinel".into()],
    })
    .unwrap();

    // Integration support installs a per-process synthetic home before entry.
    assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
    struct LiveSentinel(std::path::PathBuf, Option<Vec<u8>>);
    impl Drop for LiveSentinel {
        fn drop(&mut self) {
            match &self.1 {
                Some(bytes) => std::fs::write(&self.0, bytes).unwrap(),
                None => {
                    let _ = std::fs::remove_file(&self.0);
                }
            }
        }
    }
    let mut live = Vec::new();
    for (path, content) in [
        (
            crate::codex_config::get_codex_config_path(),
            b"# selected preparation must not publish live\nmodel = \"manual-model\"\n".as_slice(),
        ),
        (
            crate::codex_config::get_codex_auth_path(),
            br#"{"OPENAI_API_KEY":"fixture-live-sentinel"}"#.as_slice(),
        ),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        live.push((
            LiveSentinel(path.clone(), std::fs::read(&path).ok()),
            content.to_vec(),
        ));
        std::fs::write(path, content).unwrap();
    }
    let before = db
        .get_provider_by_id(&duplicate.id, "codex")
        .unwrap()
        .unwrap();
    let before_mcp = serde_json::to_value(db.get_all_mcp_servers().unwrap()).unwrap();
    let summary = persist_provision_batch_scope(
        &state,
        &account,
        selected,
        ProvisionPersistenceIntent::SelectedPrepare,
    )
    .unwrap();
    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!(summary.tiers.len(), 1);
    assert!(summary.merged_providers.is_empty());
    let prepared = db.get_provider_by_id(&id, "codex").unwrap().unwrap();
    assert_eq!(
        provider_fingerprint::for_provider(&prepared, &AppType::Codex),
        provider_fingerprint::for_provider(&before, &AppType::Codex),
        "fixture duplicate must really be mergeable",
    );
    assert!(provider_fingerprint::for_provider(&prepared, &AppType::Codex).is_some());
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(
            db.get_provider_by_id(&duplicate.id, "codex")
                .unwrap()
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some(duplicate.id.as_str())
    );
    assert!(db.get_user_edited("codex", &duplicate.id).unwrap());
    assert!(!prepared.in_failover_queue);
    assert_eq!(
        db.get_setting("application_priority_codex")
            .unwrap()
            .as_deref(),
        Some(chain.as_str())
    );
    assert_eq!(
        serde_json::to_value(db.get_all_mcp_servers().unwrap()).unwrap(),
        before_mcp
    );
    for (guard, bytes) in &live {
        assert_eq!(std::fs::read(&guard.0).unwrap(), *bytes);
    }
}

fn verify_selected_metadata_rollback() {
    // AppState construction performs the admitted legacy routing migration, so a
    // fresh Database no longer implies an absent chain after startup. Exercise
    // both transaction branches explicitly rather than assuming startup is inert.
    for initial_chain in [None, Some("[  ]")] {
        verify_selected_metadata_rollback_with_chain(initial_chain);
    }
}

fn verify_selected_metadata_rollback_with_chain(initial_chain: Option<&str>) {
    let account = account(7);
    let db = Arc::new(crate::database::Database::memory().unwrap());
    let state = AppState::new(db.clone()).unwrap();
    match initial_chain {
        Some(raw) => db.set_setting("application_priority_codex", raw).unwrap(),
        None => {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "DELETE FROM settings WHERE key='application_priority_codex'",
                    [],
                )
                .unwrap();
        }
    }
    let mut selected = batch(&account, "rollback");
    restrict_batch_to_selection(
        &mut selected,
        &[RelayTierSelection {
            app: "codex".into(),
            group_id: "rollback".into(),
        }],
    );
    let id = selected.candidates[0].provider_id.clone();
    assert_eq!(
        db.get_setting("application_priority_codex")
            .unwrap()
            .as_deref(),
        initial_chain
    );
    db.conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_initial_rate BEFORE UPDATE OF tier_rate_multiplier ON providers
         BEGIN SELECT RAISE(ABORT, 'fixture initial rate failure'); END;",
        )
        .unwrap();
    let summary = persist_provision_batch_scope(
        &state,
        &account,
        selected,
        ProvisionPersistenceIntent::SelectedPrepare,
    )
    .unwrap();
    assert_eq!(summary.failures.len(), 1);
    assert!(summary.failures[0]
        .reason
        .contains("fixture initial rate failure"));
    assert!(summary.tiers.is_empty());
    assert!(
        db.get_provider_by_id(&id, "codex").unwrap().is_none(),
        "initial metadata failure must not leave a partially prepared provider"
    );
    assert_eq!(
        db.get_setting("application_priority_codex")
            .unwrap()
            .as_deref(),
        initial_chain,
        "provider failure must roll back new chain materialization and preserve existing bytes"
    );
}

fn verify_legacy() {
    let account = account(7);
    let id = managed::newapi_provider_id_for(&account.site_origin, 7, "active");
    let providers = vec![
        provider(id.clone(), None),
        provider(
            managed::newapi_provider_id_for(&account.site_origin, 8, "active"),
            Some(8),
        ),
        provider(
            managed::newapi_provider_id_for(&account.site_origin, 7, "removed"),
            Some(7),
        ),
    ];
    let choices = vec![discovered_tier(
        AppType::Codex,
        id,
        "active".into(),
        "Renamed".into(),
    )];
    let selected = configured_selection(&account, &AppType::Codex, &providers, &choices);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].group_id, "active");
    assert!(configured_selection(&account, &AppType::Claude, &providers, &choices).is_empty());
}

async fn verify_sub2api(inactive: bool) {
    let fixture = Fixture::start().await;
    let client =
        sub2api::Client::new(&fixture.origin, "fixture-session", Some(7), None, None).unwrap();
    let tables = model_selection::ModelSelectionTables::builtin();
    if inactive {
        fixture.state.lock().unwrap().inactive = true;
        assert!(provision::provision_selected(&client, &tables, &[1, 2])
            .await
            .is_err());
        assert!(fixture
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|(method, _)| method == "GET"));
        return;
    }
    let empty = provision::provision_selected(&client, &tables, &[])
        .await
        .unwrap();
    assert!(empty.tiers.is_empty());
    assert!(fixture.state.lock().unwrap().requests.is_empty());
    assert!(provision::provision_selected(&client, &tables, &[1, 99])
        .await
        .is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|(method, _)| method == "GET"));
    fixture.state.lock().unwrap().keys = [2, 99].into_iter().map(|id| json!({"id":id,"key":format!("fixture-old-{id}"),"name":format!("LoongPort/a7/anthropic/{id}"),"status":"active"})).collect();
    let first = provision::provision_selected(&client, &tables, &[1, 1])
        .await
        .unwrap();
    let repeated = provision::provision_selected(&client, &tables, &[1])
        .await
        .unwrap();
    assert_eq!(first.tiers.len(), 1);
    assert_eq!(first.tiers[0].tier.group_id, 1);
    assert!(first.tiers[0].tier.key_was_created);
    assert!(!repeated.tiers[0].tier.key_was_created);
    assert_eq!(first.tiers[0].tier.api_key, repeated.tiers[0].tier.api_key);
    // The unselected composite resource must still be absent before its own
    // later confirmation, not merely filtered out of the returned projections.
    assert_eq!(fixture.state.lock().unwrap().keys.len(), 3);
    assert!(!fixture
        .state
        .lock()
        .unwrap()
        .keys
        .iter()
        .any(|key| key["id"] == 3));
    let composite = provision::provision_selected(&client, &tables, &[3, 3])
        .await
        .unwrap();
    assert_eq!(composite.tiers.len(), 2);
    assert!(composite
        .tiers
        .iter()
        .all(|tier| tier.tier.api_key == composite.tiers[0].tier.api_key));
    let state = fixture.state.lock().unwrap();
    assert_eq!(
        state
            .requests
            .iter()
            .filter(|(method, path)| method == "POST" && path == "/api/v1/keys")
            .count(),
        2
    );
    assert!(!state.requests.iter().any(|(method, _)| method == "DELETE"));
    assert_eq!(state.keys.len(), 4);
}

async fn verify_newapi() {
    let fixture = Fixture::start().await;
    let client = newapi::NewApiClient::new(&fixture.origin, "fixture-session").unwrap();
    let selected = [
        newapi::GroupIdentity("alpha".into()),
        newapi::GroupIdentity("alpha".into()),
    ];
    let empty = newapi_provision::reconcile_selected_for_account(&client, 7, &[])
        .await
        .unwrap();
    assert!(empty.groups.is_empty());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|(method, _)| method == "GET"));
    assert!(newapi_provision::reconcile_selected_for_account(
        &client,
        7,
        &[
            newapi::GroupIdentity("alpha".into()),
            newapi::GroupIdentity("missing".into())
        ]
    )
    .await
    .is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|(method, _)| method == "GET"));
    fixture.state.lock().unwrap().tokens = ["beta", "removed"].into_iter().enumerate().map(|(id, group)| json!({"id":id+10,"key":"masked","name":if group == "beta" { "LoongPort/napi/a7/9E5k5185SOn3P436lHIcTOjLtPJlxHkM" } else { "LoongPort/napi/a7/4feXWMxC5v5gN5QSBdH7w39XgPE6oHfQ" },"group":group,"status":1})).collect();
    let first = newapi_provision::reconcile_selected_for_account(&client, 7, &selected)
        .await
        .unwrap();
    let second = newapi_provision::reconcile_selected_for_account(&client, 7, &selected)
        .await
        .unwrap();
    assert_eq!(first.tokens_created, 1);
    assert_eq!(first.groups.len(), 1);
    assert_eq!(first.groups[0].identity.0, "alpha");
    assert_eq!(first.observed_groups.len(), 2);
    assert_eq!(second.tokens_created, 0);
    assert_eq!(first.groups[0].api_key, second.groups[0].api_key);
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.tokens.len(), 3);
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|(method, path)| method == "POST" && path == "/api/token/")
                .count(),
            1
        );
        assert!(!state.requests.iter().any(|(method, _)| method == "DELETE"));
    }
    // The removed fixture is a genuine managed token: full reconciliation must
    // delete it. Otherwise the selected-path retention check would be vacuous.
    let full = newapi_provision::reconcile_for_account(&client, 7)
        .await
        .unwrap();
    assert_eq!(full.stale_tokens_deleted, 1);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.tokens.len(), 2);
    assert!(state
        .requests
        .iter()
        .any(|(method, path)| method == "DELETE" && path == "/api/token/11"));
}

async fn verify_discovery() {
    for backend in [
        discovery::BackendKind::Sub2Api,
        discovery::BackendKind::NewApi,
    ] {
        let fixture = Fixture::start().await;
        let choices = discover_backend_choices(&fixture.account(backend), None)
            .await
            .unwrap();
        assert!(!choices.is_empty());
        assert!(choices.iter().all(|choice| !choice.prepared));
        for choice in &choices {
            assert_eq!(
                serde_json::to_value(choice).unwrap()["compatibility"],
                "unknown"
            );
        }
        let state = fixture.state.lock().unwrap();
        assert!(state.requests.iter().all(|(method, path)| method == "GET"
            && (path.ends_with("/groups/available")
                || path == "/api/user/self"
                || path == "/api/user/self/groups")));
        assert!(state.keys.is_empty() && state.tokens.is_empty());
    }
}

async fn verify_capabilities() {
    // A group listing cannot establish model compatibility before a key-backed
    // catalog read. Exercise the real backend projection and persistence policy
    // after the user's explicit choice, including recoverable partial creation.
    for backend in [
        discovery::BackendKind::Sub2Api,
        discovery::BackendKind::NewApi,
    ] {
        for (models, selected_app, expected_success) in [
            (vec!["gpt-5.4"], "codex-image", false),
            (vec!["gpt-5.4", "gpt-image-2"], "codex-image", false),
            (vec!["gpt-image-2"], "codex-image", true),
            (vec!["gpt-image-2"], "codex", false),
            (vec!["gpt-5.4", "gpt-image-2"], "codex", true),
        ] {
            let fixture = Fixture::start().await;
            fixture.state.lock().unwrap().models =
                Some(models.iter().map(|model| (*model).into()).collect());
            let account = fixture.account(backend);
            let group = if matches!(backend, discovery::BackendKind::Sub2Api) {
                "1"
            } else {
                "alpha"
            };
            let choices = discover_backend_choices(&account, None).await.unwrap();
            let choice = choices
                .iter()
                .find(|choice| choice.app == selected_app && choice.group_id == group)
                .unwrap();

            assert!(fixture
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|(method, _)| method == "GET"));
            let id = choice.provider_id.clone();
            let selected = [RelayTierSelection {
                app: selected_app.into(),
                group_id: group.into(),
            }];
            let db = Arc::new(crate::database::Database::memory().unwrap());
            let state = Arc::new(AppState::new(db.clone()).unwrap());
            for expected_created in [1, 0] {
                let batch = provision_backend_scope(&account, None, Some(&selected))
                    .await
                    .unwrap();
                assert_eq!(batch.keys_created, expected_created);
                assert_eq!(batch.candidates.len(), usize::from(expected_success));
                assert!(batch
                    .candidates
                    .iter()
                    .all(|candidate| candidate.app_type.as_str() == selected_app));
                if !expected_success {
                    assert!(!batch.failures.is_empty());
                }
                let persist_account = account.clone();
                let persist_state = state.clone();
                let summary = tokio::task::spawn_blocking(move || {
                    persist_provision_batch_scope(&persist_state, &persist_account, batch, ProvisionPersistenceIntent::SelectedPrepare)
                }).await.unwrap().expect("selected partial creation must return a structured receipt, not erase progress as an error");
                assert_eq!(summary.keys_created, expected_created);
                assert_eq!(summary.tiers.len(), usize::from(expected_success));
                if !expected_success {
                    assert!(!summary.failures.is_empty());
                }
            }
            for app in ["claude", "codex", "gemini", "codex-image"] {
                assert_eq!(
                    db.get_provider_by_id(&id, app).unwrap().is_some(),
                    expected_success && app == selected_app
                );
            }
            let state = fixture.state.lock().unwrap();
            assert_eq!(
                state
                    .requests
                    .iter()
                    .filter(|(method, path)| method == "POST"
                        && (path == "/api/token/" || path == "/api/v1/keys"))
                    .count(),
                1
            );
            assert!(!state.requests.iter().any(|(method, _)| method == "DELETE"));
        }
    }
}

async fn verify_composite_partial() {
    for invalid_models in [false, true] {
        let fixture = Fixture::start().await;
        {
            let mut state = fixture.state.lock().unwrap();
            state.models = Some(Vec::new());
            state.invalid_models = invalid_models;
        }
        let account = fixture.account(discovery::BackendKind::Sub2Api);
        let selected = [RelayTierSelection {
            app: "codex".into(),
            group_id: "3".into(),
        }];
        let db = Arc::new(crate::database::Database::memory().unwrap());
        let state = Arc::new(AppState::new(db.clone()).unwrap());
        for expected_created in [1, 0] {
            let batch = provision_backend_scope(&account, None, Some(&selected))
                .await
                .expect(
                    "a post-create composite catalog failure must preserve the partial receipt",
                );
            assert_eq!(batch.keys_created, expected_created);
            assert!(batch.candidates.is_empty());
            assert!(!batch.failures.is_empty());
            let persist_state = state.clone();
            let persist_account = account.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                persist_provision_batch_scope(
                    &persist_state,
                    &persist_account,
                    batch,
                    ProvisionPersistenceIntent::SelectedPrepare,
                )
            })
            .await
            .unwrap()
            .unwrap();
            assert!(receipt.tiers.is_empty());
            assert_eq!(receipt.keys_created, expected_created);
            assert!(!receipt.failures.is_empty());
        }
        let id = managed::provider_id_for(&account.site_origin, Some(7), 3);
        for app in ["claude", "codex", "gemini", "codex-image"] {
            assert!(db.get_provider_by_id(&id, app).unwrap().is_none());
        }
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.keys.len(), 1);
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|(method, path)| method == "POST" && path == "/api/v1/keys")
                .count(),
            1
        );
        assert!(!state.requests.iter().any(|(method, _)| method == "DELETE"));
    }
}

/// Invoked only through the existing integration-test support boundary.
pub(crate) async fn verify(case: &str) {
    match case {
        "composite_partial" => verify_composite_partial().await,
        "capabilities" => verify_capabilities().await,
        "sub2api_selected" => verify_sub2api(false).await,
        "sub2api_inactive" => verify_sub2api(true).await,
        "newapi_selected" => verify_newapi().await,
        "discovery" => verify_discovery().await,
        "unresolved_identity" => {
            for backend in [
                discovery::BackendKind::Sub2Api,
                discovery::BackendKind::NewApi,
            ] {
                let fixture = Fixture::start().await;
                let mut account = fixture.account(backend);
                account.account_id = None;
                assert!(discover_backend_choices(&account, None).await.is_err());
                assert!(fixture.state.lock().unwrap().requests.is_empty());
            }
        }
        "selected_metadata_rollback" => {
            tokio::task::spawn_blocking(verify_selected_metadata_rollback)
                .await
                .unwrap()
        }
        "selected_new_isolation" => {
            tokio::task::spawn_blocking(verify_selected_new_projection_isolation)
                .await
                .unwrap()
        }
        "selected_isolation" => tokio::task::spawn_blocking(verify_selected_isolation)
            .await
            .unwrap(),
        "persistence" => tokio::task::spawn_blocking(verify_persistence)
            .await
            .unwrap(),
        "legacy" => verify_legacy(),
        "lock" => {
            let account = account(7);
            let first = relay_provision_lock(&account).unwrap();
            let second = relay_provision_lock(&creds::RelayAccount {
                id: 999,
                ..account.clone()
            })
            .unwrap();
            let other = relay_provision_lock(&self::account(8)).unwrap();
            assert!(Arc::ptr_eq(&first, &second));
            assert!(!Arc::ptr_eq(&first, &other));
            let guard = first.lock().await;
            assert!(second.try_lock().is_err());
            assert!(other.try_lock().is_ok());
            drop(guard);
            assert!(second.try_lock().is_ok());
        }
        _ => panic!("unknown relay provision fixture case: {case}"),
    }
}
