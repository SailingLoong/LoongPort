//! 跨域共享的测试脚手架（凭证构造、mock 服务、seed helper、model_verification
//! 类型导入）。各领域模块自己的测试只搬自己的；这里收被两个以上域引用的部分，
//! 以及原 tests.rs 顶部的公共 use（以 pub(crate) use 再导出，各域测试经 glob 取用）。
//! 各域测试 mod 的头部统一 `use crate::commands::relay::test_support::*;`。
use super::*;

pub(crate) fn purchase_capability_relay(backend_kind: creds::BackendKind) -> creds::RelayAccount {
    creds::RelayAccount {
        id: 1,
        site_origin: "https://relay.example".into(),
        site_name: "Relay".into(),
        backend_kind,
        api_base_url: "https://relay.example/v1".into(),
        account_id: Some(1),
        account_label: "account".into(),
        login_identifier: "account".into(),
        auth_token: "session".into(),
        refresh_token: None,
        token_expires_at: None,
        user_agent: None,
        cf_clearance: None,
        pricing_synced_at: None,
        sort_index: 0,
    }
}

pub(crate) use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(crate) use futures::channel::oneshot;

pub(crate) use crate::relay::model_verification::{
    coordinator::{
        ActiveVerifier, ModelVerificationCoordinator, PreparedVerification, ProbeProgress,
    },
    types::{EvidenceLevel, RunFailureKind, TargetKey, Verdict, VerificationReport, RULES_VERSION},
};

pub(crate) fn tier(id: &str) -> TierInfo {
    TierInfo {
        provider_id: id.into(),
        // 归属测试只关心「哪条属于哪个站/账号」，与落在哪个 CLI 无关。
        app_id: AppType::Codex.as_str().to_string(),
        group_name: id.into(),
        display_name: id.into(),
        model: "gpt-5.6-sol".into(),
        models: vec!["gpt-5.6-sol".into()],
        rate_multiplier: None,
        is_current: false,
        can_verify_models: true,
        // 归属测试不关心它 —— `tiers_of_site` 会自己算出来覆盖掉这个值。
        user_edited: None,
        // 同上：归属判定与生图无关。
        allow_image_generation: None,
        site_declared_origin: None,
    }
}

pub(crate) fn test_newapi_relay(account_id: i64) -> creds::RelayAccount {
    creds::RelayAccount {
        id: account_id,
        site_origin: "https://newapi.example".into(),
        site_name: "NewAPI".into(),
        backend_kind: discovery::BackendKind::NewApi,
        api_base_url: String::new(),
        account_id: Some(account_id),
        account_label: format!("account-{account_id}"),
        login_identifier: format!("account-{account_id}"),
        auth_token: "access-token".into(),
        refresh_token: None,
        token_expires_at: None,
        user_agent: None,
        cf_clearance: None,
        pricing_synced_at: None,
        sort_index: 0,
    }
}

pub(crate) fn newapi_discovery_body() -> serde_json::Value {
    serde_json::json!({
        "success": true,
        "data": {
            "version": "1.0.0",
            "system_name": "NewAPI",
            "theme": "default",
            "register_enabled": true,
            "password_login_enabled": true
        }
    })
}

pub(crate) fn sub2api_discovery_body() -> serde_json::Value {
    serde_json::json!({
        "code": 0,
        "message": "success",
        "data": {
            "site_name": "Sub2API",
            "version": "1.0.0",
            "api_base_url": "",
            "registration_enabled": true,
            "promo_code_enabled": false,
            "invitation_code_enabled": false
        }
    })
}

pub(crate) fn saved_relay_app(
    site_origin: &str,
    backend_kind: discovery::BackendKind,
) -> (tauri::App<tauri::test::MockRuntime>, i64) {
    let db = Arc::new(crate::database::Database::memory().expect("memory database"));
    let relay_id = {
        let conn = db.conn.lock().expect("lock memory database");
        let relay_id = creds::save_site_with_backend(
            &conn,
            site_origin,
            "Saved relay",
            site_origin,
            backend_kind,
        )
        .expect("save relay");
        creds::save_credentials(
            &conn,
            relay_id,
            creds::AccountIdentity {
                id: 7,
                label: "Saved Account",
                login_identifier: "saved-account",
            },
            "saved-access-token",
            Some("saved-refresh-token"),
            None,
            creds::SessionEnvironment::default(),
        )
        .expect("save relay credentials");
        relay_id
    };
    let app = tauri::test::mock_builder()
        .manage(AppState::new(db))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock app");
    (app, relay_id)
}

pub(crate) fn relay_credentials(
    app: &tauri::App<tauri::test::MockRuntime>,
    relay_id: i64,
) -> creds::RelayAccount {
    let state = app.state::<AppState>();
    with_conn(&state, |conn| creds::get(conn, relay_id))
        .expect("read saved relay")
        .expect("saved relay exists")
}

/// 起一个只回余额相关端点的本地 server（手法照 `relay/backend.rs` 的先例）。
pub(crate) async fn spawn_balance_server(
    app: axum::Router,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, task)
}

/// 造一条 provider。`site` 进 `website_url`（归属依据），`id` 决定它是否被认作托管项。
pub(crate) fn seeded(id: &str, name: &str, site: Option<&str>) -> Provider {
    Provider {
        id: id.to_string(),
        name: name.to_string(),
        settings_config: serde_json::json!({ "env": {} }),
        website_url: site.map(str::to_string),
        category: Some("aggregator".to_string()),
        created_at: Some(1),
        sort_index: Some(0),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
        available_models: None,
    }
}

/// 带账号归属的那种（provision 从此都写它，见 `managed_meta`）。
pub(crate) fn seeded_owned(id: &str, name: &str, site: Option<&str>, account_id: i64) -> Provider {
    Provider {
        meta: Some(managed_meta(&AppType::Codex, Some(account_id), None)),
        ..seeded(id, name, site)
    }
}

pub(crate) struct ResetVerifier {
    pub(crate) senders: Mutex<HashMap<TargetKey, DiagCompletion>>,
}

pub(crate) type DiagCompletion = (
    oneshot::Sender<
        Result<
            (
                VerificationReport,
                Vec<crate::relay::model_verification::types::ProbeDiagnostic>,
            ),
            RunFailureKind,
        >,
    >,
    ProbeProgress,
);

impl ResetVerifier {
    pub(crate) fn new() -> Self {
        Self {
            senders: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn complete(&self, target: &TargetKey, report: VerificationReport) -> bool {
        self.senders
            .lock()
            .unwrap()
            .remove(target)
            .is_some_and(|(sender, _)| sender.send(Ok((report, Vec::new()))).is_ok())
    }
}

impl ActiveVerifier for ResetVerifier {
    fn prepare(
        &self,
        target: TargetKey,
        progress: ProbeProgress,
    ) -> Result<PreparedVerification, RunFailureKind> {
        let (sender, receiver) = oneshot::channel();
        let progress_for_store = progress.clone();
        self.senders
            .lock()
            .unwrap()
            .insert(target, (sender, progress_for_store));
        let future = Box::pin(async move { receiver.await.unwrap() });
        let future = Box::pin(async move {
            let result = future.await;
            if result.is_ok() {
                for completed in 1..=3 {
                    progress(completed);
                }
            }
            result
        });
        Ok(PreparedVerification {
            total_checks: 3,
            future,
        })
    }
}
