use super::*;
use crate::relay::login;
use crate::relay::provision;

/// 契约闸：前端 TS 类型手写断言了这份 wire 形状（`src/lib/api/relay.ts` 的
/// `SwitchTierCommandResult`），serde 的 enum 级 `rename_all` 只转变体名、
/// 不转变体字段 —— 没有这条闸的话 casing 分叉编译期完全静默
/// （2026-08-16 线上事故：`target_name` 蛇形下发，确认弹窗永不打开）。
#[test]
fn switch_tier_command_result_wire_contract_is_camel_case() {
    let confirmation = serde_json::to_value(SwitchTierCommandResult::ConfirmationRequired {
        target_name: "站点 · 分组".into(),
    })
    .unwrap();
    assert_eq!(confirmation["status"], "confirmationRequired");
    assert!(
        confirmation.get("targetName").is_some(),
        "变体字段必须驼峰下发：{confirmation}"
    );
    assert!(
        confirmation.get("target_name").is_none(),
        "蛇形键意味着前端读到 undefined：{confirmation}"
    );

    let switched = serde_json::to_value(SwitchTierCommandResult::Switched {
        result: SwitchTierResult {
            provider_name: "p".into(),
            chatgpt_was_running: false,
            chatgpt_relaunched: false,
            warnings: vec![],
        },
    })
    .unwrap();
    assert_eq!(switched["status"], "switched");
    assert!(
        switched.get("providerName").is_some(),
        "flatten 的结构体字段同样是驼峰契约：{switched}"
    );
}

fn purchase_capability_relay(backend_kind: creds::BackendKind) -> creds::Relay {
    creds::Relay {
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

fn sub2api_with_session() -> creds::Relay {
    purchase_capability_relay(creds::BackendKind::Sub2Api)
}

fn newapi_with_refresh_cookie() -> creds::Relay {
    creds::Relay {
        refresh_token: Some("refresh-cookie".into()),
        ..purchase_capability_relay(creds::BackendKind::NewApi)
    }
}

fn newapi_without_refresh_cookie() -> creds::Relay {
    purchase_capability_relay(creds::BackendKind::NewApi)
}

#[test]
fn purchase_capability_requires_login_config_and_backend_credentials() {
    assert!(can_open_site_window(&sub2api_with_session(), true, true));
    assert!(can_open_site_window(
        &newapi_with_refresh_cookie(),
        true,
        true
    ));
    assert!(!can_open_site_window(
        &newapi_without_refresh_cookie(),
        true,
        true
    ));
    assert!(!can_open_site_window(&sub2api_with_session(), false, true));
    assert!(!can_open_site_window(&sub2api_with_session(), true, false));

    let newapi_with_blank_refresh_cookie = creds::Relay {
        refresh_token: Some("   ".into()),
        ..newapi_with_refresh_cookie()
    };
    assert!(!can_open_site_window(
        &newapi_with_blank_refresh_cookie,
        true,
        true
    ));
}

#[test]
fn directory_update_event_matches_the_frontend_constant() {
    let frontend = include_str!("../../../../src/config/constants.ts");

    assert!(frontend.contains(RELAY_DIRECTORY_UPDATED_EVENT));
}
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use futures::channel::oneshot;

use crate::relay::model_verification::{
    coordinator::{
        ActiveVerifier, ModelVerificationCoordinator, PreparedVerification, ProbeProgress,
    },
    types::{EvidenceLevel, RunFailureKind, TargetKey, Verdict, VerificationReport, RULES_VERSION},
};

#[test]
fn browser_entry_url_preserves_user_path_and_query_but_forces_https() {
    let url = browser_entry_url("http://api.example.com/register?aff=ABC123")
        .expect("valid browser entry URL");

    assert_eq!(url.as_str(), "https://api.example.com/register?aff=ABC123");
}

#[test]
fn browser_entry_url_accepts_bare_hosts_with_paths() {
    let url = browser_entry_url("api.example.com/login?next=%2Fdashboard")
        .expect("valid browser entry URL");

    assert_eq!(
        url.as_str(),
        "https://api.example.com/login?next=%2Fdashboard"
    );
}

#[test]
fn directory_entry_source_accepts_only_policy_owned_entries() {
    let config = crate::relay::remote_config::RemoteConfig {
        relay_directory: crate::relay::remote_config::RelayDirectoryPolicy {
            blocked_hosts: vec![],
            sites: std::collections::BTreeMap::from([
                (
                    "790053500.com".into(),
                    crate::relay::remote_config::RelayDirectorySite {
                        veridrop_host: Some("api.790053500.com".into()),
                        entry_url: Some("https://790053500.com/keys".into()),
                        purchase_url: None,
                        usage_url: None,
                        display_name: Some("鑫旺".into()),
                    },
                ),
                (
                    "plain.example".into(),
                    crate::relay::remote_config::RelayDirectorySite::default(),
                ),
                (
                    "broken.example".into(),
                    crate::relay::remote_config::RelayDirectorySite {
                        veridrop_host: None,
                        entry_url: Some("http://broken.example/keys".into()),
                        purchase_url: None,
                        usage_url: None,
                        display_name: None,
                    },
                ),
            ]),
        },
        ..crate::relay::remote_config::RemoteConfig::default()
    };

    assert_eq!(
        directory_entry_source(&config, "https://790053500.com/keys"),
        Some(BrowserEntrySource::SignedDirectory)
    );
    assert_eq!(
        directory_entry_source(&config, "https://plain.example"),
        Some(BrowserEntrySource::Manual)
    );
    assert_eq!(
        directory_entry_source(&config, "https://unknown.example"),
        None
    );
    assert_eq!(
        directory_entry_source(&config, "https://790053500.com/other"),
        None
    );
    assert_eq!(
        directory_entry_source(&config, "https://broken.example"),
        Some(BrowserEntrySource::Manual)
    );
    assert_eq!(
        directory_entry_source(&config, "https://broken.example/keys"),
        None
    );
}

/// wawapi.top 实测踩出的洞：aff / sponsor 名单里的站进了广场（曝光集合），
/// 导入闸却只认 relay_directory ⇒ 点「接入」被 NotInDirectory 拒。第二段回落
/// 补上：受管全集按 Manual 放行裸 origin；带 path / blocked / 名单外仍拒。
#[test]
fn directory_entry_source_falls_back_to_the_full_managed_set() {
    let config = crate::relay::remote_config::RemoteConfig {
        relay_directory: crate::relay::remote_config::RelayDirectoryPolicy {
            blocked_hosts: vec!["blocked.example".into()],
            sites: std::collections::BTreeMap::new(),
        },
        sponsors: vec![crate::relay::remote_config::Sponsor {
            site_origin: "https://www.WawAPII.com".into(),
            display_name: "WawAPI".into(),
            tagline: String::new(),
        }],
        aff_codes: std::collections::BTreeMap::from([
            ("wawapi.top".to_string(), "AFF".to_string()),
            ("blocked.example".to_string(), "AFF".to_string()),
        ]),
        ..crate::relay::remote_config::RemoteConfig::default()
    };

    assert_eq!(
        directory_entry_source(&config, "https://wawapi.top"),
        Some(BrowserEntrySource::Manual)
    );
    // sponsor 的 www / 大小写变体归一后也要命中
    assert_eq!(
        directory_entry_source(&config, "https://wawapii.com"),
        Some(BrowserEntrySource::Manual)
    );
    // Manual 回落只认裸 origin —— 带 path 的输入不给受信处理
    assert_eq!(
        directory_entry_source(&config, "https://wawapi.top/register"),
        None
    );
    assert_eq!(
        directory_entry_source(&config, "https://blocked.example"),
        None
    );
    assert_eq!(
        directory_entry_source(&config, "https://unknown.example"),
        None
    );
}

fn detected_sub2api() -> discovery::DetectedSite {
    discovery::DetectedSite {
        backend_kind: discovery::BackendKind::Sub2Api,
        site_name: "Example".into(),
        api_base_url: String::new(),
        final_origin: None,
    }
}

fn detected_newapi() -> discovery::DetectedSite {
    discovery::DetectedSite {
        backend_kind: discovery::BackendKind::NewApi,
        site_name: "NewAPI".into(),
        api_base_url: String::new(),
        final_origin: None,
    }
}

#[test]
fn import_anchor_origin_prefers_the_probe_final_origin() {
    let redirected = discovery::DetectedSite {
        final_origin: Some("https://panel.example".into()),
        ..detected_sub2api()
    };
    assert_eq!(
        import_anchor_origin("https://apex.example".into(), Some(&redirected)),
        "https://panel.example"
    );
    // 浏览器路径的回传不带 final_origin（守卫保证同源），探针也没跑成时同理：
    // 都保持用户输入的 origin。
    assert_eq!(
        import_anchor_origin("https://apex.example".into(), Some(&detected_sub2api())),
        "https://apex.example"
    );
    assert_eq!(
        import_anchor_origin("https://apex.example".into(), None),
        "https://apex.example"
    );
    let same_origin = discovery::DetectedSite {
        final_origin: Some("https://apex.example".into()),
        ..detected_sub2api()
    };
    assert_eq!(
        import_anchor_origin("https://apex.example".into(), Some(&same_origin)),
        "https://apex.example"
    );
}

#[test]
fn browser_start_url_uses_origin_when_protocol_is_unknown_even_for_non_page_path() {
    let url = browser_start_url(
        "https://api.example.com/custom/subscription-token",
        "https://api.example.com",
        None,
        BrowserEntrySource::Manual,
    )
    .expect("valid browser start URL");

    assert_eq!(url.as_str(), "https://api.example.com/");
}

#[test]
fn browser_start_url_preserves_auth_link_while_protocol_is_unknown() {
    let url = browser_start_url(
        "https://api.example.com/register?aff=ABC123",
        "https://api.example.com",
        None,
        BrowserEntrySource::Manual,
    )
    .expect("valid browser start URL");

    assert_eq!(url.as_str(), "https://api.example.com/register?aff=ABC123");
}

#[test]
fn browser_start_url_replaces_non_page_path_after_native_detection() {
    let detected = detected_sub2api();
    let url = browser_start_url(
        "https://api.example.com/custom/subscription-token",
        "https://api.example.com",
        Some(&detected),
        BrowserEntrySource::Manual,
    )
    .expect("valid browser start URL");

    assert_eq!(url.as_str(), "https://api.example.com/register");
}

#[test]
fn browser_start_url_preserves_a_signed_directory_entry_path() {
    let detected = detected_sub2api();
    let url = browser_start_url(
        "https://790053500.com/keys",
        "https://790053500.com",
        Some(&detected),
        BrowserEntrySource::SignedDirectory,
    )
    .expect("valid signed directory entry URL");

    assert_eq!(url.as_str(), "https://790053500.com/keys");
}

#[test]
fn browser_start_url_preserves_invitation_link_after_native_detection() {
    let detected = detected_sub2api();
    let url = browser_start_url(
        "http://api.example.com/register?aff=ABC123",
        "https://api.example.com",
        Some(&detected),
        BrowserEntrySource::Manual,
    )
    .expect("valid browser start URL");

    assert_eq!(url.as_str(), "https://api.example.com/register?aff=ABC123");
}

#[test]
fn browser_start_url_uses_protocol_registration_page_for_known_bare_origin() {
    let detected = detected_sub2api();
    let url = browser_start_url(
        "api.example.com",
        "https://api.example.com",
        Some(&detected),
        BrowserEntrySource::Manual,
    )
    .expect("valid browser start URL");

    assert_eq!(url.as_str(), "https://api.example.com/register");
}

#[test]
fn browser_start_url_uses_newapi_legacy_registration_page_for_known_bare_origin() {
    let detected = detected_newapi();
    let url = browser_start_url(
        "api.example.com",
        "https://api.example.com",
        Some(&detected),
        BrowserEntrySource::Manual,
    )
    .expect("valid browser start URL");

    assert_eq!(
        url.as_str(),
        backend::browser_login_url(
            "https://api.example.com",
            discovery::BackendKind::NewApi,
            ""
        )
    );
}

#[test]
fn native_protocol_conflict_is_terminal_while_unsupported_site_can_fall_back() {
    let conflict = recoverable_native_discovery_error(discovery::DiscoveryError {
        kind: discovery::DiscoveryErrorKind::ProtocolConflict,
        message: "conflict".into(),
    });
    assert_eq!(
        conflict
            .expect_err("conflict must not open browser fallback")
            .message,
        "conflict"
    );

    let unsupported = recoverable_native_discovery_error(discovery::DiscoveryError {
        kind: discovery::DiscoveryErrorKind::UnsupportedSite,
        message: "unsupported".into(),
    })
    .expect("unsupported site can use browser fallback");
    assert_eq!(unsupported.to_string(), "unsupported");
}

#[test]
fn new_site_import_close_is_a_typed_cancellation() {
    let error = incomplete_new_site_import_error(IncompleteImportReason::Closed);

    assert_eq!(error.kind, Some(RelayImportErrorKind::Cancelled));
    assert_eq!(error.message, "注册或登录尚未完成");
}

fn bare_mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock app")
}

#[tokio::test]
async fn stale_login_window_destroy_returns_without_waiting_when_none_exists() {
    // 没有残留窗口是绝大多数路径（上一轮窗口早就关了）：helper 必须立即返回，
    // 不进等待循环 —— 否则每次导入/重登都白等一个轮询周期。
    let app = bare_mock_app();
    let start = std::time::Instant::now();
    destroy_stale_login_window_with_timeout(app.handle(), std::time::Duration::from_secs(2)).await;
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "没有残留窗口时不该等待，实际等了 {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn stale_login_window_destroy_is_bounded_when_the_label_never_frees() {
    // MockRuntime 不驱动事件循环 ⇒ destroy 后 manager 注册表里的 label 永不释放
    // （与 newapi_purchase 超时用例同款不可观测性）。能钉住的不变量是「有界返回」：
    // 到点必须继续走，否则真实运行时里事件循环卡一下，导入就永久卡死在等待里。
    let app = bare_mock_app();
    tauri::WebviewWindowBuilder::new(
        app.handle(),
        login::LOGIN_WINDOW_LABEL,
        tauri::WebviewUrl::External(url::Url::parse("about:blank").unwrap()),
    )
    .build()
    .expect("预建残留登录窗");

    let start = std::time::Instant::now();
    destroy_stale_login_window_with_timeout(app.handle(), std::time::Duration::from_millis(150))
        .await;
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "label 永不释放时也必须有界返回，实际等了 {:?}",
        start.elapsed()
    );
}

#[test]
fn new_site_import_timeout_is_a_typed_cancellation() {
    let error = incomplete_new_site_import_error(IncompleteImportReason::TimedOut);

    assert_eq!(error.kind, Some(RelayImportErrorKind::Cancelled));
    assert_eq!(error.message, "注册或登录等待超时，请重试");
}

/// ⭐ **kind 的线上格式必须与前端 union 逐字一致。**
///
/// 前端 `src/lib/api/relay.ts` 的 `ImportErrorKind` 按这些字面量匹配（switch
/// 的是字符串，不是共享类型）。serde enum 的 rename 只在结构体字段上踩过 casing
/// 雷（PR #153 的 `target_name`），枚举变体同理 —— 这里把每个变体的线上值钉死。
#[test]
fn import_error_kinds_serialize_to_the_wire_names_the_frontend_matches() {
    for (kind, wire) in [
        (
            RelayImportErrorKind::UnsupportedSite,
            "\"unsupported_site\"",
        ),
        (RelayImportErrorKind::NotInDirectory, "\"not_in_directory\""),
        (
            RelayImportErrorKind::ProtocolConflict,
            "\"protocol_conflict\"",
        ),
        (RelayImportErrorKind::Transport, "\"transport\""),
        (RelayImportErrorKind::Cancelled, "\"cancelled\""),
    ] {
        assert_eq!(
            serde_json::to_string(&kind).expect("kind 可序列化"),
            wire,
            "{kind:?} 的线上名变了，前端 union 要跟着改"
        );
    }
}

/// `RelayRowStatus` 的线上名由 `RelayRow.tsx` 的 `RowStatus` switch 直接消费
/// （裸字符串比较，无编译器把守）。这里把每个变体的 serde 输出钉死 ——
/// 改枚举变体名 / 改 rename 规则时这条会红，提醒同步前端 union。
#[test]
fn relay_row_statuses_serialize_to_the_wire_names_the_frontend_matches() {
    for (status, wire) in [
        (RelayRowStatus::NotLoggedIn, "\"notLoggedIn\""),
        (RelayRowStatus::SessionExpired, "\"sessionExpired\""),
        (
            RelayRowStatus::SessionExpiredUsable,
            "\"sessionExpiredUsable\"",
        ),
        (RelayRowStatus::NoTiers, "\"noTiers\""),
        (RelayRowStatus::Ready, "\"ready\""),
    ] {
        assert_eq!(
            serde_json::to_string(&status).expect("status 可序列化"),
            wire,
            "{status:?} 的线上名变了，src/lib/api/relay.ts 的 union 与 RowStatus 要跟着改"
        );
    }
}

#[test]
fn new_site_import_discovery_stays_in_memory_until_authentication() {
    let context = browser_login_context(
        "https://api.example.com",
        detected_sub2api(),
        Some("invite"),
        None,
    );

    assert_eq!(context.site.site_origin, "https://api.example.com");
    assert_eq!(context.site.site_name, "Example");
    assert_eq!(context.site.api_base_url, "https://api.example.com");
    assert_eq!(context.site.backend_kind, discovery::BackendKind::Sub2Api);
}

#[tokio::test]
async fn completed_refresh_wins_when_close_and_refresh_are_ready_together() {
    let outcome =
        await_refresh_preserving_rotation(async { Ok::<_, AppError>("refreshed") }, async {
            "closed"
        })
        .await;

    assert!(matches!(outcome, RefreshWait::Refreshed(Ok("refreshed"))));
}

#[tokio::test]
async fn refresh_started_before_close_is_drained_to_preserve_rotation() {
    let (release_refresh, wait_for_release) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(await_refresh_preserving_rotation(
        async {
            wait_for_release
                .await
                .expect("test releases the refresh response");
            Ok::<_, AppError>("rotated")
        },
        async { "closed" },
    ));

    tokio::task::yield_now().await;
    release_refresh
        .send(())
        .expect("refresh waiter remains alive after close");
    let outcome = tokio::time::timeout(std::time::Duration::from_millis(50), task)
        .await
        .expect("bounded refresh completes")
        .expect("refresh task does not panic");

    assert!(matches!(outcome, RefreshWait::Refreshed(Ok("rotated"))));
}

#[test]
fn persisting_newapi_login_session_stores_tokens_and_native_account_identity() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db);
    let relay_id = with_conn(&state, |conn| {
        creds::save_site_with_backend(
            conn,
            "https://newapi.example",
            "NewAPI",
            "https://newapi.example",
            discovery::BackendKind::NewApi,
        )
    })
    .expect("save site");
    let refreshed = crate::relay::newapi::RefreshedSession {
        access_token: "new-access-token".into(),
        access_expires_at: Some(1_900_000_000),
        session_id: "session-id".into(),
        account: crate::relay::newapi::SelfAccount {
            id: 84,
            username: "newapi-login".into(),
            display_name: "NewAPI Display".into(),
            email: "newapi@example.com".into(),
            group: "default".into(),
            quota: 0,
            used_quota: 0,
        },
        refresh_cookie: "rotated-refresh-cookie".into(),
    };

    let (final_relay_id, account_id) =
        persist_newapi_login_session(&state, relay_id, &refreshed).expect("persist login");
    let persisted = with_conn(&state, |conn| creds::get(conn, final_relay_id))
        .expect("load relay")
        .expect("relay exists");

    assert_eq!(account_id, 84);
    assert_eq!(persisted.auth_token, "new-access-token");
    assert_eq!(
        persisted.refresh_token.as_deref(),
        Some("rotated-refresh-cookie")
    );
    assert_eq!(persisted.token_expires_at, Some(1_900_000_000));
    assert_eq!(persisted.account_id, Some(84));
    assert_eq!(persisted.account_label, "NewAPI Display");
    assert_eq!(persisted.login_identifier, "newapi-login");
}

#[test]
fn persisting_legacy_newapi_session_keeps_refresh_fields_absent() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db);
    let relay_id = with_conn(&state, |conn| {
        creds::save_site_with_backend(
            conn,
            "https://legacy-newapi.example",
            "Legacy NewAPI",
            "https://legacy-newapi.example",
            discovery::BackendKind::NewApi,
        )
    })
    .expect("save site");
    let session = crate::relay::newapi::RefreshedSession {
        access_token: "long-lived-access-token".into(),
        access_expires_at: None,
        session_id: String::new(),
        account: crate::relay::newapi::SelfAccount {
            id: 42,
            username: "legacy-login".into(),
            display_name: "Legacy User".into(),
            email: String::new(),
            group: "default".into(),
            quota: 0,
            used_quota: 0,
        },
        refresh_cookie: String::new(),
    };

    let (final_relay_id, account_id) =
        persist_newapi_login_session(&state, relay_id, &session).expect("persist login");
    let persisted = with_conn(&state, |conn| creds::get(conn, final_relay_id))
        .expect("load relay")
        .expect("relay exists");

    assert_eq!(account_id, 42);
    assert_eq!(persisted.auth_token, "long-lived-access-token");
    assert_eq!(persisted.refresh_token, None);
    assert_eq!(persisted.token_expires_at, None);
    assert_eq!(persisted.account_id, Some(42));
}

#[test]
fn import_result_uses_the_final_relay_id_after_account_merge() {
    let result = ImportResult::authenticated(
        DiscoveredRelaySite {
            site_origin: "https://api.example.com".into(),
            site_name: "Example".into(),
            api_base_url: "https://api.example.com".into(),
            backend_kind: discovery::BackendKind::NewApi,
        },
        11,
    );

    assert_eq!(result.relay_id, 11);
}

#[test]
fn relay_row_serializes_backend_owned_remove_confirmation() {
    let row = RelayRow {
        id: 7,
        site_origin: "https://api.example.com".into(),
        site_name: "Example".into(),
        account_label: String::new(),
        status: RelayRowStatus::NotLoggedIn,
        is_current: false,
        can_query_balance: false,
        can_purchase: true,
        can_view_usage: false,
        can_refresh: false,
        usage_blockers: Vec::new(),
        remove_confirmation: RemoveConfirmation::NeverLoggedIn,
        tiers: Vec::new(),
    };

    let json = serde_json::to_value(row).expect("serialize relay row");
    assert_eq!(json["canPurchase"], true);
    assert_eq!(json["canViewUsage"], false);
    assert_eq!(json["removeConfirmation"], "neverLoggedIn");
    assert_eq!(json["usageBlockers"], serde_json::json!([]));
}

#[test]
fn relay_status_owns_the_global_add_site_prompt_decision() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db);

    assert!(
        relay_status_impl(&state)
            .expect("empty status")
            .should_prompt_add_site
    );

    with_conn(&state, |conn| {
        crate::vendor::creds::save_account(
            conn,
            crate::vendor::Vendor::DeepSeek,
            "token",
            &crate::vendor::VendorAccount {
                account_id: "account-7".into(),
                label: "DeepSeek user".into(),
                login_identifier: "13800000000".into(),
            },
        )?;
        Ok(())
    })
    .expect("save vendor account");

    assert!(
        !relay_status_impl(&state)
            .expect("configured status")
            .should_prompt_add_site
    );
}

#[test]
fn refresh_summary_counts_vendor_config_for_the_current_app() {
    let summary = refresh_summary(
        &AppType::Codex,
        Vec::new(),
        vec![(
            "DeepSeek".into(),
            Ok(super::super::vendor::VendorProvisionSummary {
                provider_id: "managed-deepseek".into(),
                platforms: vec![
                    AppType::Codex.as_str().into(),
                    AppType::Claude.as_str().into(),
                ],
                key_created: true,
                merged_providers: vec!["Imported DeepSeek".into()],
            }),
        )],
    );

    assert_eq!(summary.refreshed_accounts, 1);
    assert_eq!(summary.tiers, 1);
    assert_eq!(summary.other_platform_tiers, 0);
    assert_eq!(summary.keys_created, 1);
    assert_eq!(summary.merged_providers, 1);
    assert!(matches!(summary.notice, RefreshNotice::UpdatedWithKeys));
}

#[test]
fn refresh_result_is_cloneable() {
    let result = RefreshResult {
        summary: RefreshSummary {
            notice: RefreshNotice::None,
            refreshed_accounts: 0,
            tiers: 0,
            keys_created: 0,
            other_platform_tiers: 0,
            merged_providers: 0,
            failures: Vec::new(),
        },
        balances: Vec::new(),
    };

    let cloned = result.clone();
    assert!(matches!(cloned.summary.notice, RefreshNotice::None));
}

fn codex_settings(model: &str, models: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "auth": { "OPENAI_API_KEY": "sk-test" },
        "config": format!(
            "model_provider = \"custom\"\nmodel = {model:?}\n\n[model_providers.custom]\nname = \"Test\"\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        ),
        "modelCatalog": {
            "models": models.iter().map(|model| serde_json::json!({ "model": model })).collect::<Vec<_>>()
        }
    })
}

fn provider_with_id(id: &str) -> Provider {
    Provider {
        id: id.to_string(),
        name: "t".into(),
        settings_config: serde_json::json!({}),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

#[test]
fn codex_model_list_requires_a_real_catalog() {
    let settings = serde_json::json!({
        "config": "model_provider = \"custom\"\nmodel = \"gpt-current\"\n"
    });

    assert!(
        models_from_settings(&settings).is_empty(),
        "旧 provider 只有当前模型时，不能把它冒充成完整支持列表"
    );
}

#[test]
fn selecting_a_codex_model_validates_and_only_updates_the_model_field() {
    let settings = codex_settings("gpt-a", &["gpt-a", "gpt-b"]);

    let selected = select_codex_model(&settings, " gpt-b ").expect("supported model");
    assert_eq!(
        provision::extract_model(&selected).as_deref(),
        Some("gpt-b")
    );
    assert_eq!(selected["modelCatalog"], settings["modelCatalog"]);
    assert_eq!(selected["auth"], settings["auth"]);

    assert!(select_codex_model(&settings, "gpt-unknown").is_err());
}

#[test]
fn refreshing_a_managed_codex_tier_keeps_only_a_still_supported_selection() {
    let defaults = codex_settings("gpt-a", &["gpt-a", "gpt-b"]);
    let previous = codex_settings("gpt-b", &["gpt-a", "gpt-b"]);
    let kept = preserve_supported_codex_model(defaults.clone(), &previous);
    assert_eq!(provision::extract_model(&kept).as_deref(), Some("gpt-b"));

    let removed = codex_settings("gpt-removed", &["gpt-removed"]);
    let reset = preserve_supported_codex_model(defaults, &removed);
    assert_eq!(provision::extract_model(&reset).as_deref(), Some("gpt-a"));
}

/// 形状对齐 [`relay::provision`] 生成侧（`deeplink::build_grokbuild_settings`
/// + `modelCatalog`）：选中模型在 `[model."<default>"]` 表的 `model` 字段。
fn grok_settings(model: &str, models: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "config": format!(
            "[models]\ndefault = \"{model}\"\n\n[model.\"{model}\"]\nmodel = \"{model}\"\nbase_url = \"https://api.example.com\"\nname = \"Test\"\napi_key = \"sk-test\"\napi_backend = \"responses\"\ncontext_window = 500000\n"
        ),
        "modelCatalog": {
            "models": models.iter().map(|model| serde_json::json!({ "model": model })).collect::<Vec<_>>()
        }
    })
}

#[test]
fn selecting_a_grok_model_only_updates_the_model_field() {
    let settings = grok_settings("grok-4.5", &["grok-4.5", "grok-4.6"]);

    let selected = select_grok_model(&settings, "grok-4.6").expect("supported model");
    assert_eq!(
        provision::selected_model(&AppType::GrokBuild, &selected).as_deref(),
        Some("grok-4.6")
    );
    // profile 名（models.default 指向的表）、端点、密钥、目录都不动 ——
    // 它们与模型无关
    let config = selected["config"].as_str().expect("config text");
    assert!(config.contains("[model.\"grok-4.5\"]"), "{config}");
    assert!(config.contains("base_url = \"https://api.example.com\""));
    assert!(config.contains("api_key = \"sk-test\""));
    assert_eq!(selected["modelCatalog"], settings["modelCatalog"]);
}

#[test]
fn refreshing_a_managed_grok_tier_keeps_only_a_still_supported_selection() {
    let defaults = grok_settings("grok-4.5", &["grok-4.5", "grok-4.6"]);
    let previous = grok_settings("grok-4.6", &["grok-4.5", "grok-4.6"]);
    let kept = preserve_supported_grok_model(defaults.clone(), &previous);
    assert_eq!(
        provision::selected_model(&AppType::GrokBuild, &kept).as_deref(),
        Some("grok-4.6")
    );

    let removed = grok_settings("grok-gone", &["grok-gone"]);
    let reset = preserve_supported_grok_model(defaults, &removed);
    assert_eq!(
        provision::selected_model(&AppType::GrokBuild, &reset).as_deref(),
        Some("grok-4.5")
    );
}

/// ⭐ `relay_list_sponsors` 发给前端的**键名**必须是 camelCase。
///
/// 这条守的是一个跨语言的静默失效：`Sponsor` 的 `Deserialize` 用 snake_case
/// （签名覆盖的配置契约，动不了），`Serialize` 用 camelCase（TS 侧惯例）。
/// 两者不一致看起来像疏漏，**很可能被人顺手统一** —— 而统一到 snake_case 时
/// 编译器一声不响，前端拿到的每个字段都是 `undefined` ⇒
/// **首启屏卡片全是空白按钮**（`displayName` 为 undefined、React 什么都不渲染）。
///
/// 断言的是序列化后的键，不是结构体字段名 —— 后者与前端无关。
/// （`remote_config` 那边也有一条同向的闸，两处各守一端：
/// 那条管「结构体的两个方向」，这条管「命令实际吐出去的东西」。）
#[test]
fn list_sponsors_emits_camel_case_keys_for_the_frontend() {
    let sponsor = crate::relay::remote_config::Sponsor {
        site_origin: "https://x.com".into(),
        display_name: "X".into(),
        tagline: "T".into(),
    };
    // 命令的返回类型是 `Vec<Sponsor>`，所以按它实际的序列化形态断言。
    let json = serde_json::to_value(vec![sponsor]).expect("要能序列化");
    let first = json[0].as_object().expect("是个对象");

    for key in ["siteOrigin", "displayName", "tagline"] {
        assert!(
            first.contains_key(key),
            "前端要的键 {key} 不在返回里，实际：{:?}",
            first.keys().collect::<Vec<_>>()
        );
    }
    assert!(
        !first.contains_key("site_origin") && !first.contains_key("display_name"),
        "别把 snake_case 键发给前端（TS 那边按 camelCase 读）"
    );
}

/// ⭐ **`TierInfo` 必须说清自己落在哪个 CLI 上。**
///
/// ## 它守的是什么缺陷（TODO 债 11）
///
/// provision 链路（`refresh_relay_provision`）一次探**全部平台**，`tiers` 收的是
/// 全平台的结果，而 UI 那一行只显示**当前 app** 的档位。于是「这个站没有
/// anthropic 分组」与「拉取失败」在界面上长得一样（都是零档位 +
/// 「该账号在此平台下没有可用分组」）—— 而前者重试一百次也不会有，后者重试有意义。
///
/// 区分它们所需的信息 provision 时**本来就在手上**（每个分组的 `app_type`），
/// 少的只是把它发给前端。没有这个字段，前端拿到一堆 tiers 却分不出哪条是自己的。
///
/// ## 为什么键名是 `appId`
///
/// 前端那边这个概念叫 `AppId`（`lib/api/types.ts`），命令层签名也一直吃
/// `app_id`。发 `appType` 会让同一个东西在两侧各有一个名字。
#[test]
fn tier_info_tells_the_frontend_which_cli_it_landed_on() {
    let tier = TierInfo {
        provider_id: "loongport-0123456789abcdef".into(),
        app_id: AppType::Claude.as_str().to_string(),
        group_name: "pro池".into(),
        display_name: "站 · pro池".into(),
        model: "claude-sonnet-5".into(),
        models: vec!["claude-sonnet-5".into()],
        rate_multiplier: Some(1.0),
        is_current: false,
        can_verify_models: true,
        user_edited: None,
        allow_image_generation: None,
        site_declared_origin: None,
    };

    let json = serde_json::to_value(&tier).expect("要能序列化");
    let obj = json.as_object().expect("是个对象");

    assert_eq!(
        obj.get("appId").and_then(|v| v.as_str()),
        Some("claude"),
        "前端要靠 appId 判断这条档位是不是属于它当前那一屏，实际：{:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        obj.get("canVerifyModels").and_then(|value| value.as_bool()),
        Some(true),
        "模型验证支持资格必须由后端随档位返回"
    );
    assert!(
        !obj.contains_key("app_id"),
        "别把 snake_case 键发给前端（TS 那边按 camelCase 读）"
    );
}

#[test]
fn managed_detection_matches_generated_ids_only() {
    // 正面：provision 生成的 id 必须被认出来。
    let real = provision::provider_id_for("https://bestapi.store", Some(1), 42);
    assert!(is_managed(&provider_with_id(&real)));

    // 反面：用户自己加的 provider 不能被当成托管的（否则会被 provision 覆盖）。
    for id in ["custom-1", "codex-official", "", "LoongPort-1"] {
        assert!(!is_managed(&provider_with_id(id)), "id: {id}");
    }
}

#[test]
fn provision_merge_removes_only_same_app_unmanaged_duplicate() {
    let db = crate::database::Database::memory().expect("内存库");
    let app_type = AppType::Codex;
    let site = "https://relay.example";
    let key = "sk-same";
    let settings = provision::settings_config_for(
        &app_type,
        key,
        "Imported",
        "https://relay.example/v1",
        "model-a",
    )
    .expect("codex 配置");

    let duplicate = Provider {
        id: "cc-switch-duplicate".into(),
        name: "Imported duplicate".into(),
        settings_config: settings.clone(),
        website_url: Some(site.into()),
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    let different_key = Provider {
        id: "cc-switch-different-key".into(),
        name: "Keep different key".into(),
        settings_config: provision::settings_config_for(
            &app_type,
            "sk-other",
            "Other",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置"),
        ..duplicate.clone()
    };
    let managed_duplicate = Provider {
        id: provision::provider_id_for(site, Some(1), 42),
        name: "Managed duplicate".into(),
        meta: Some(managed_meta(&app_type, Some(1), None)),
        ..duplicate.clone()
    };
    db.save_provider(app_type.as_str(), &duplicate)
        .expect("写入重复项");
    db.save_provider(app_type.as_str(), &different_key)
        .expect("写入不同 key");
    db.save_provider(app_type.as_str(), &managed_duplicate)
        .expect("写入托管项");

    let merged =
        provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed_duplicate)
            .expect("收编不该失败");

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "Imported duplicate");
    assert!(db
        .get_provider_by_id("cc-switch-duplicate", app_type.as_str())
        .expect("查询")
        .is_none());
    assert!(db
        .get_provider_by_id("cc-switch-different-key", app_type.as_str())
        .expect("查询")
        .is_some());
    assert!(db
        .get_provider_by_id(&managed_duplicate.id, app_type.as_str())
        .expect("查询")
        .is_some());
}

#[test]
fn provision_merge_reports_when_duplicate_was_current() {
    let db = crate::database::Database::memory().expect("内存库");
    let app_type = AppType::Codex;
    let settings = provision::settings_config_for(
        &app_type,
        "sk-current",
        "Imported",
        "https://relay.example/v1",
        "model-a",
    )
    .expect("codex 配置");
    let duplicate = Provider {
        id: "cc-switch-current".into(),
        name: "Current imported duplicate".into(),
        settings_config: settings,
        website_url: Some("https://relay.example".into()),
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider(app_type.as_str(), &duplicate)
        .expect("写入当前项");
    db.set_current_provider(app_type.as_str(), &duplicate.id)
        .expect("设为当前");

    let managed = Provider {
        id: provision::provider_id_for("https://relay.example", Some(1), 99),
        name: "Managed replacement".into(),
        meta: Some(managed_meta(&app_type, Some(1), None)),
        ..duplicate.clone()
    };
    db.save_provider(app_type.as_str(), &managed)
        .expect("写入托管替代项");

    let merged = provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed)
        .expect("收编不该失败");

    assert_eq!(merged.len(), 1);
    assert!(merged[0].was_current);
    assert_eq!(
        db.get_current_provider(app_type.as_str())
            .expect("读取收编后的当前项")
            .as_deref(),
        Some(managed.id.as_str())
    );
}

#[test]
fn provision_merge_rolls_back_duplicate_deletion_when_current_transfer_fails() {
    let db = crate::database::Database::memory().expect("内存库");
    let app_type = AppType::Codex;
    let settings = provision::settings_config_for(
        &app_type,
        "sk-current",
        "Imported",
        "https://relay.example/v1",
        "model-a",
    )
    .expect("codex 配置");
    let duplicate = Provider {
        id: "cc-switch-current".into(),
        name: "Current imported duplicate".into(),
        settings_config: settings,
        website_url: Some("https://relay.example".into()),
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider(app_type.as_str(), &duplicate)
        .expect("写入当前项");
    db.set_current_provider(app_type.as_str(), &duplicate.id)
        .expect("设为当前");

    let managed = Provider {
        id: provision::provider_id_for("https://relay.example", Some(1), 99),
        name: "Managed replacement".into(),
        meta: Some(managed_meta(&app_type, Some(1), None)),
        ..duplicate.clone()
    };
    db.save_provider(app_type.as_str(), &managed)
        .expect("写入托管替代项");
    {
        let conn = db.conn.lock().expect("lock db");
        conn.execute_batch(&format!(
            "CREATE TRIGGER fail_managed_current
                 BEFORE UPDATE OF is_current ON providers
                 WHEN NEW.id = '{}' AND NEW.is_current = 1
                 BEGIN
                   SELECT RAISE(FAIL, 'injected current transfer failure');
                 END;",
            managed.id
        ))
        .expect("install current-transfer failure");
    }

    let error = provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed)
        .expect_err("current transfer failure must roll back adoption")
        .to_string();

    assert!(error.contains("injected current transfer failure"));
    assert!(db
        .get_provider_by_id(&duplicate.id, app_type.as_str())
        .expect("read duplicate")
        .is_some());
    assert_eq!(
        db.get_current_provider(app_type.as_str())
            .expect("read current after rollback")
            .as_deref(),
        Some(duplicate.id.as_str())
    );
}

#[test]
fn provision_merge_never_uses_an_unmanaged_provider_as_the_owner() {
    let db = crate::database::Database::memory().expect("内存库");
    let app_type = AppType::Codex;
    let settings = provision::settings_config_for(
        &app_type,
        "sk-shared",
        "Imported",
        "https://relay.example/v1",
        "model-a",
    )
    .expect("codex 配置");
    let imported = Provider {
        id: "cc-switch-imported".into(),
        name: "Imported".into(),
        settings_config: settings.clone(),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    let non_managed_candidate = Provider {
        id: "manual-provider".into(),
        name: "Manual".into(),
        settings_config: settings,
        ..imported.clone()
    };
    db.save_provider(app_type.as_str(), &imported)
        .expect("写入导入项");
    db.save_provider(app_type.as_str(), &non_managed_candidate)
        .expect("写入手工项");

    assert!(provider_fingerprint::remove_unmanaged_duplicates(
        &db,
        &app_type,
        &non_managed_candidate,
    )
    .expect("不该失败")
    .is_empty());
    assert!(db
        .get_provider_by_id(&imported.id, app_type.as_str())
        .expect("查询")
        .is_some());
}

#[test]
fn provision_summary_reports_adopted_providers_to_the_frontend() {
    let summary = ProvisionSummary {
        tiers: Vec::new(),
        failures: Vec::new(),
        keys_created: 0,
        merged_providers: vec![MergedProviderInfo {
            name: "Imported duplicate".into(),
            app_id: AppType::Codex.as_str().to_string(),
        }],
    };

    let json = serde_json::to_value(summary).expect("应能序列化");
    assert_eq!(json["mergedProviders"][0]["name"], "Imported duplicate");
    assert_eq!(json["mergedProviders"][0]["appId"], "codex");
}

fn tier(id: &str) -> TierInfo {
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

fn test_app() -> AppType {
    AppType::Codex
}

fn test_newapi_relay(account_id: i64) -> creds::Relay {
    creds::Relay {
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

async fn spawn_discovery_server(
    sub2api_body: Option<serde_json::Value>,
    newapi_body: Option<serde_json::Value>,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::{routing::get, Json, Router};

    let mut app = Router::new();
    if let Some(body) = sub2api_body {
        app = app.route(
            "/api/v1/settings/public",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
            }),
        );
    }
    if let Some(body) = newapi_body {
        app = app.route(
            "/api/status",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
            }),
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind discovery test server");
    let origin = format!("http://{}", listener.local_addr().expect("server address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve discovery app");
    });
    (origin, server)
}

fn newapi_discovery_body() -> serde_json::Value {
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

fn sub2api_discovery_body() -> serde_json::Value {
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

fn saved_relay_app(
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

fn relay_credentials(app: &tauri::App<tauri::test::MockRuntime>, relay_id: i64) -> creds::Relay {
    let state = app.state::<AppState>();
    with_conn(&state, |conn| creds::get(conn, relay_id))
        .expect("read saved relay")
        .expect("saved relay exists")
}

/// 把 `saved_relay_app` 存好的 relay 的 access token 显式置为已过期。
///
/// 必须给一个**过去**的 `token_expires_at`：`saved_relay_app` 存的是 `None`，而
/// `token_looks_valid` 对 `None` 有意乐观降级（返回 true），`usable_relay` 会走
/// token 早退分支，永远到不了要测的续期路径。
fn expire_saved_relay_token(app: &tauri::App<tauri::test::MockRuntime>, relay_id: i64) {
    let state = app.state::<AppState>();
    let conn = state.db.conn.lock().expect("lock memory database");
    conn.execute(
        "UPDATE loongport_relay SET token_expires_at = ?1 WHERE id = ?2",
        rusqlite::params![chrono::Utc::now().timestamp() - 3600, relay_id],
    )
    .expect("expire saved relay token");
}

/// 起一个只回余额相关端点的本地 server（手法照 `relay/backend.rs` 的先例）。
async fn spawn_balance_server(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, task)
}

fn profile_router(balance: serde_json::Value) -> axum::Router {
    use axum::{routing::get, Json, Router};
    Router::new().route(
        "/api/v1/user/profile",
        get(move || async move { Json(balance) }),
    )
}

#[tokio::test]
async fn relay_balance_writes_one_snapshot_on_successful_resolve() {
    let (origin, server) = spawn_balance_server(profile_router(serde_json::json!({
        "code": 0,
        "message": "success",
        "data": {
            "id": 7,
            "username": "Sub User",
            "email": "sub@example.com",
            "balance": 12.5,
            "frozen_balance": 0.0
        }
    })))
    .await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

    let result = relay_balance_impl(app.handle(), relay_id)
        .await
        .expect("成功解析必须 Ok");

    assert!(result.usage.success, "{:?}", result.usage.error);
    let state = app.state::<AppState>();
    let rows = state
        .db
        .list_balance_snapshots(relay_id, None)
        .expect("读快照");
    assert_eq!(rows.len(), 1, "成功路径恰好落一条快照");
    assert_eq!(rows[0].balance_usd, 12.5);
    assert_eq!(rows[0].source, "balance_query");
    assert_eq!(rows[0].relay_id, relay_id);
    server.abort();
}

#[tokio::test]
async fn relay_balance_writes_no_snapshot_when_resolve_fails() {
    use axum::{http::StatusCode, routing::get, Router};
    let router = Router::new().route(
        "/api/v1/user/profile",
        get(|| async { StatusCode::UNAUTHORIZED }),
    );
    let (origin, server) = spawn_balance_server(router).await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

    let result = relay_balance_impl(app.handle(), relay_id)
        .await
        .expect("失败路回 success:false，仍是 Ok");

    assert!(!result.usage.success);
    let state = app.state::<AppState>();
    assert_eq!(
        state
            .db
            .list_balance_snapshots(relay_id, None)
            .expect("读快照")
            .len(),
        0,
        "三路全败不能落快照"
    );
    server.abort();
}

/// ⭐ 快照写入失败绝不能拖垮余额显示（对账是旁路能力，plan §三.2）。
#[tokio::test]
async fn relay_balance_returns_balance_even_when_snapshot_insert_fails() {
    let (origin, server) = spawn_balance_server(profile_router(serde_json::json!({
        "code": 0,
        "message": "success",
        "data": {
            "id": 7,
            "username": "Sub User",
            "email": "sub@example.com",
            "balance": 9.9,
            "frozen_balance": 0.0
        }
    })))
    .await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);
    {
        let state = app.state::<AppState>();
        let conn = state.db.conn.lock().expect("lock memory database");
        conn.execute("DROP TABLE relay_balance_snapshots", [])
            .expect("删表制造写入失败");
    }

    let result = relay_balance_impl(app.handle(), relay_id)
        .await
        .expect("快照写入失败时余额命令仍必须 Ok");
    assert!(result.usage.success);
    assert_eq!(
        result
            .usage
            .data
            .as_ref()
            .and_then(|items| items.first())
            .and_then(|item| item.remaining),
        Some(9.9)
    );
    server.abort();
}

#[tokio::test]
async fn saved_relay_validation_accepts_the_same_detected_backend() {
    let (origin, server) = spawn_discovery_server(None, Some(newapi_discovery_body())).await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

    let relay = usable_relay(app.handle(), relay_id)
        .await
        .expect("same backend remains usable");

    assert_eq!(relay.backend_kind, discovery::BackendKind::NewApi);
    assert_eq!(relay.auth_token, "saved-access-token");
    server.abort();
}

#[tokio::test]
async fn saved_relay_validation_clears_credentials_on_detected_backend_mismatch() {
    let (origin, server) = spawn_discovery_server(Some(sub2api_discovery_body()), None).await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

    let error = usable_relay(app.handle(), relay_id)
        .await
        .expect_err("backend mismatch must stop runtime dispatch");

    assert!(error.to_string().contains("协议"), "{error}");
    let relay = relay_credentials(&app, relay_id);
    assert!(relay.auth_token.is_empty());
    assert!(relay.refresh_token.is_none());
    server.abort();
}

#[tokio::test]
async fn saved_relay_validation_uses_saved_backend_when_probe_is_unsupported() {
    let (origin, server) = spawn_discovery_server(
        Some(serde_json::json!({ "unknown": "sub" })),
        Some(serde_json::json!({ "unknown": "new" })),
    )
    .await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

    let relay = usable_relay(app.handle(), relay_id)
        .await
        .expect("unsupported probe should fall back to the saved backend");

    assert_eq!(relay.backend_kind, discovery::BackendKind::NewApi);
    assert_eq!(relay.auth_token, "saved-access-token");
    assert_eq!(relay.refresh_token.as_deref(), Some("saved-refresh-token"));
    server.abort();
}

#[tokio::test]
async fn saved_relay_validation_preserves_credentials_on_conflicting_protocol() {
    let (origin, server) = spawn_discovery_server(
        Some(sub2api_discovery_body()),
        Some(newapi_discovery_body()),
    )
    .await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

    usable_relay(app.handle(), relay_id)
        .await
        .expect_err("conflicting probe must stop runtime dispatch");

    let relay = relay_credentials(&app, relay_id);
    assert_eq!(relay.auth_token, "saved-access-token");
    assert_eq!(relay.refresh_token.as_deref(), Some("saved-refresh-token"));
    server.abort();
}

#[tokio::test]
async fn saved_relay_validation_preserves_credentials_on_transport_only_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind connection-drop server");
    let origin = format!("http://{}", listener.local_addr().expect("server address"));
    let server = tokio::spawn(async move {
        for _ in 0..discovery::PROBE_CANDIDATES.len() {
            let (stream, _) = listener.accept().await.expect("accept probe request");
            drop(stream);
        }
    });
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

    let error = usable_relay(app.handle(), relay_id)
        .await
        .expect_err("transport failure must stop dispatch");

    assert!(error.to_string().contains("连接"), "{error}");
    let relay = relay_credentials(&app, relay_id);
    assert_eq!(relay.auth_token, "saved-access-token");
    assert_eq!(relay.refresh_token.as_deref(), Some("saved-refresh-token"));
    server.await.expect("connection-drop server completes");
}

/// mock 站点轮换后回的假凭据：沿用 `saved_*` 前缀家族，仅供断言对得上号，
/// 不是任何真实站点的密钥。
const RENEWED_ACCESS: &str = "saved-access-token-renewed";
const RENEWED_ROTATION: &str = "saved-refresh-token-renewed";

/// ⭐ 回归闸（2026-08-17 bestapi.store 线上事故）：登录快照没带回过期时间
/// （`token_expires_at = NULL`）的行，`token_looks_valid` 永远乐观为真，
/// `usable_relay` 的主动续期永不触发；access token 在服务端到 24h 过期后，
/// 探活撞上 401「登录已过期」直接清会话 —— refresh token 一次没用过就被连坐。
/// 钉住：过期类 401 + 手里有 refresh token ⇒ 先静默续期一次、用新凭据重跑原请求。
#[tokio::test]
async fn relay_read_refreshes_once_and_retries_when_token_expires_server_side() {
    use axum::{
        http::{header, HeaderMap, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    let router = Router::new()
        .route(
            "/api/v1/user/profile",
            get(|headers: HeaderMap| async move {
                let stale = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.ends_with("saved-access-token"));
                if stale {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "code": "TOKEN_EXPIRED",
                            "message": "登录已过期，请重新登录"
                        })),
                    );
                }
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": {
                            "id": 7,
                            "username": "Sub User",
                            "email": "sub@example.com",
                            "balance": 12.5,
                            "frozen_balance": 0.0
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/refresh",
            post(|Json(body): Json<serde_json::Value>| async move {
                assert_eq!(body["refresh_token"], "saved-refresh-token", "{body}");
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": {
                            "access_token": RENEWED_ACCESS,
                            "refresh_token": RENEWED_ROTATION,
                            "expires_at": 4_102_444_800_000_i64
                        }
                    })),
                )
            }),
        );
    let (origin, server) = spawn_balance_server(router).await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

    let balance = relay_read_with_refresh_retry(app.handle(), relay_id, |op| async move {
        backend::RuntimeBackend::for_relay(&op).balance().await
    })
    .await
    .expect("服务端 401 过期必须被静默续期救回");

    assert_eq!(balance.balance, 12.5);
    let creds = relay_credentials(&app, relay_id);
    assert_eq!(creds.auth_token, RENEWED_ACCESS);
    assert_eq!(creds.refresh_token.as_deref(), Some(RENEWED_ROTATION));
    assert!(
        creds.token_expires_at.is_some(),
        "续期响应带回的过期时间必须落库 —— 为 NULL 正是这起事故的起点"
    );
    server.abort();
}

/// 续期救不回来时（refresh token 也被服务端拒了），必须把**原错误**交回去：
/// `check_session` 靠错误分类决定清会话，换成续期那条报错会悄悄改变判读。
#[tokio::test]
async fn relay_read_returns_original_error_when_refresh_cannot_rescue() {
    use axum::{
        http::StatusCode,
        routing::{get, post},
        Json, Router,
    };
    let router = Router::new()
        .route(
            "/api/v1/user/profile",
            get(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "code": "TOKEN_EXPIRED",
                        "message": "登录已过期，请重新登录"
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/refresh",
            post(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "code": "REFRESH_TOKEN_INVALID",
                        "message": "refresh token 已失效"
                    })),
                )
            }),
        );
    let (origin, server) = spawn_balance_server(router).await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

    let error = relay_read_with_refresh_retry(app.handle(), relay_id, |op| async move {
        backend::RuntimeBackend::for_relay(&op).balance().await
    })
    .await
    .expect_err("续期失败时原请求的错误必须往外抛");

    assert!(error.to_string().contains("登录已过期"), "{error}");
    assert!(!error.to_string().contains("续期失败"), "{error}");
    let creds = relay_credentials(&app, relay_id);
    assert_eq!(
        creds.auth_token, "saved-access-token",
        "续期失败不得改动库里的凭据"
    );
    server.abort();
}

/// ⭐ 回归闸：充值窗口持有 NewAPI 账号的 refresh 轮换独占权时，`usable_relay`
/// 的静默续期必须被拦下 —— NewAPI 的 refresh cookie 一次性轮换，后台并发续期会把
/// 充值窗口里种着的那颗 cookie 立刻作废（用户充值到一半被踢回登录页）。
///
/// 两个断言互相补充：
/// 1. 持 lease 时：报「充值窗口」错误，且 fake 站点收到 **0** 个 refresh 请求
///    （`newapi::refresh_url` 指向的端点）—— 闸必须挡在发请求之前，不是发完再补救。
/// 2. drop lease 后：同一 relay 的 `usable_relay` 正常走续期（端点收到请求、拿到
///    新 token）—— 证明闸只认 lease，不是无条件挡路。
///
/// 协议细节（refresh 端点路径、cookie 名）全部从 `newapi` owner 派生，本文件
/// 不写字面量 —— `browser_login_dispatch_keeps_protocol_details_out_of_commands`
/// 闸钉着 commands 层不得拥有这些细节。探测阶段会打 `/api/status`（还可能探别的
/// 候选端点吃 404），与断言无关 —— 请求日志里只数 refresh 端点的个数。
#[tokio::test]
async fn active_purchase_session_blocks_newapi_refresh() {
    use axum::{
        extract::Request,
        http::{header, HeaderValue},
        middleware,
        middleware::Next,
        response::IntoResponse,
        routing::get,
        routing::post,
        Json, Router,
    };

    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let every_request = Arc::clone(&requests);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind refresh-block test server");
    let origin = format!("http://{}", listener.local_addr().expect("server address"));
    // refresh 端点路径从 owner 派生：fake 路由和下面的请求计数共用它，两边
    // 永远指向同一个端点（写两份字面量迟早分叉，而且这份文件不许有字面量）。
    let refresh_path = newapi::refresh_url(&origin)
        .expect("newapi refresh url")
        .path()
        .to_string();

    let site = Router::new()
        .route(
            "/api/status",
            get(move || {
                let body = newapi_discovery_body();
                async move { Json(body) }
            }),
        )
        // 不持 lease 的那次 `usable_relay` 要真的续期成功，回一个完整的 NewAPI
        // refresh 信封（parser 要求带轮换后的 Set-Cookie，名字同样取自 owner）。
        .route(
            refresh_path.as_str(),
            post(move || {
                async move {
                    let set_cookie = format!(
                        "{}=rotated-secret; Path=/; HttpOnly",
                        newapi::REFRESH_COOKIE_NAME
                    );
                    (
                        [(
                            header::SET_COOKIE,
                            // HeaderValue 拥有自己的字节：cookie 值是运行期拼出来的
                            // （名字来自 owner 常量），借用拼不出 'static 响应。
                            HeaderValue::from_str(&set_cookie).expect("合法 set-cookie"),
                        )],
                        Json(serde_json::json!({
                            "success": true,
                            "data": {
                                "access_token": "refreshed-access-token",
                                "access_expires_at": 4_102_444_800_i64,
                                "user": {
                                    "id": 7,
                                    "username": "saved-account",
                                    "display_name": "Saved Account",
                                    "email": "saved@example.test",
                                    "group": "default",
                                    "quota": 1,
                                    "used_quota": 0
                                },
                                "session": { "sid": "sid-refreshed" }
                            }
                        })),
                    )
                        .into_response()
                }
            }),
        )
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let requests = Arc::clone(&every_request);
            async move {
                requests.lock().unwrap().push(req.uri().path().to_string());
                next.run(req).await
            }
        }));
    let server = tokio::spawn(async move {
        axum::serve(listener, site)
            .await
            .expect("serve refresh-block app");
    });

    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);
    expire_saved_relay_token(&app, relay_id);

    let refresh_count = || {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.as_str() == refresh_path)
            .count()
    };

    let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
    let lease = coordinator.try_acquire(relay_id).expect("acquire lease");

    let error = usable_relay(app.handle(), relay_id)
        .await
        .expect_err("持 lease 时后台续期必须被拦下");
    assert!(error.to_string().contains("充值窗口"), "{error}");
    assert_eq!(
        refresh_count(),
        0,
        "闸必须挡在发请求之前：fake 站点不该收到任何 refresh 请求"
    );

    drop(lease);
    let relay = usable_relay(app.handle(), relay_id)
        .await
        .expect("lease 释放后同一 relay 必须恢复续期");
    assert_eq!(relay.auth_token, "refreshed-access-token");
    assert_eq!(
        refresh_count(),
        1,
        "不持 lease 时同一 relay 的 usable_relay 会正常尝试续期 —— 闸不是无条件挡路"
    );

    server.abort();
}

/// ⭐ 回归闸：sub2api 充值页必须打开**签名配置的 URL**，不再读站点公开设置的
/// 支付开关去猜 `/purchase` 还是 `/redeem`。
///
/// 三个断言互相补充：
/// 1. 请求日志**不含** `/api/v1/settings/public` —— 路由事实已归签名目录，
///    生产充值路径不该再读站点公开设置。
/// 2. 请求日志恰好是「开窗前续期 + 取账号档案」两个请求 —— 证明 token 寿命续期
///    与登录态注入这些既有行为没有被这次改动顺带丢掉。
/// 3. 窗口打开的 URL **逐字符等于**配置值 —— 配置里故意用了不可推导的路径
///    （`/topup-center?flow=card`），推导逻辑造不出它。
///
/// 用 middleware 记录**所有**请求的 path（含未注册路由的 404）：逐 handler 记录会漏掉
/// 「代码打了但我们没 serve 的路径」，那种漏记正好把要抓的回归放跑。
#[tokio::test]
async fn sub2api_purchase_uses_signed_url() {
    use axum::{
        extract::Request, middleware, middleware::Next, routing::get, routing::post, Json, Router,
    };

    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let every_request = Arc::clone(&requests);

    let app = Router::new()
        // `ensure_token_outlasts_a_payment` 开窗前无条件续期要打的端点。
        .route(
            "/api/v1/auth/refresh",
            post(|| async move {
                Json(serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": {
                        "access_token": "fresh-access-token",
                        "refresh_token": "fresh-refresh-token",
                        "expires_at": 4_102_444_800_000_i64
                    }
                }))
            }),
        )
        // `auth_user_from_profile` 要的账号档案（信封 `data` 里必须有 `id`）。
        .route(
            "/api/v1/user/profile",
            get(|| async move {
                Json(serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": { "id": 7, "email": "saved-account", "username": "Saved Account" }
                }))
            }),
        )
        // ⚠️ 陷阱端点：旧实现靠它读站点公开设置的支付开关猜路由。这里故意把它
        // 配成一个能正常解析的 sub2api 响应 —— 只要充值流程还来问它，下面的
        // 断言当场红。
        .route(
            "/api/v1/settings/public",
            get(|| async move { Json(sub2api_discovery_body()) }),
        )
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let requests = Arc::clone(&every_request);
            async move {
                requests.lock().unwrap().push(req.uri().path().to_string());
                next.run(req).await
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind purchase test server");
    let addr = listener.local_addr().expect("server address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve purchase app");
    });

    // relay 行存 http origin（测试站是本机 http 服务）；签名目录的 `purchase_url`
    // 用同 host:port 的 https 形态 —— `normalize_site_origin` 强制 https 且保留端口，
    // 所以两者恰好同源、`configured_purchase_url` 解析成功。
    let (app, relay_id) =
        saved_relay_app(&format!("http://{addr}"), discovery::BackendKind::Sub2Api);
    let op = relay_credentials(&app, relay_id);

    let configured = format!("https://{addr}/topup-center?flow=card");
    let config = remote_config::RemoteConfig {
        relay_directory: remote_config::RelayDirectoryPolicy {
            blocked_hosts: vec![],
            sites: std::collections::BTreeMap::from([(
                "127.0.0.1".to_string(),
                remote_config::RelayDirectorySite {
                    veridrop_host: None,
                    entry_url: None,
                    purchase_url: Some(configured.clone()),
                    usage_url: None,
                    display_name: None,
                },
            )]),
        },
        ..remote_config::RemoteConfig::default()
    };
    let purchase_url = remote_config::configured_purchase_url(&config, &op.site_origin)
        .expect("签名目录解析不该报错")
        .expect("这个站在目录里配了购买入口");

    let window = purchase::purchase_window(relay_id, &op.site_origin);
    open_sub2api_site_window(app.handle(), op, window, purchase_url)
        .await
        .expect("开充值窗");

    let paths = requests.lock().unwrap().clone();
    assert_eq!(
        paths,
        vec!["/api/v1/auth/refresh", "/api/v1/user/profile"],
        "充值流程只该打「开窗前续期 + 取账号档案」两个请求；\
             出现 /api/v1/settings/public 说明又回去按公开设置猜路由了"
    );

    let window = app
        .get_webview_window(&purchase::window_label(relay_id))
        .expect("充值窗应该开出来了");
    let opened = window
        .url()
        .expect("mock 窗口能读回创建时的 URL")
        .to_string();
    assert_eq!(
        opened, configured,
        "打开的外部 URL 必须恰好是签名配置值，不是推导出的 /purchase 或 /redeem"
    );

    server.abort();
}

// ======================================================================
// NewAPI 充值分派（Task 7）。全部直接驱动 `dispatch_purchase` —— 生产
// `load_cached()` 用生产公钥验签，测试无法（也不该）伪造缓存，这个接缝与
// `open_sub2api_purchase_window` 的「参数化只为可测」是同一惯例。
// 协议字面量一律从 `newapi` owner 派生（backend.rs 的架构闸钉着）。
// ======================================================================

/// 起一个「记录全部请求 path」的哨兵站点：只应答 NewAPI 探测端点的形状
/// （`/api/status`），其余 404 —— 任何路径都会进日志（middleware 不挑路由）。
async fn recording_newapi_sentinel(
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    use axum::{extract::Request, middleware, middleware::Next, routing::get, Json, Router};

    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let every_request = Arc::clone(&requests);
    let app = Router::new()
        .route(
            "/api/status",
            get(|| async move { Json(newapi_discovery_body()) }),
        )
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let requests = Arc::clone(&every_request);
            async move {
                requests.lock().unwrap().push(req.uri().path().to_string());
                next.run(req).await
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind newapi purchase sentinel");
    let addr = listener.local_addr().expect("sentinel address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve newapi purchase sentinel");
    });
    (format!("http://{addr}"), requests, server)
}

/// 等 monitor 任务收场把 lease 还回去（`open` 返回与后台任务 drop lease 之间
/// 有毫秒级竞态，轮询等待而不是睡固定时长）。
async fn until_purchase_lease_released(app: &tauri::App<tauri::test::MockRuntime>, relay_id: i64) {
    let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while coordinator.is_active(relay_id) {
        assert!(
            std::time::Instant::now() < deadline,
            "命令返回后 lease 必须随即释放"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// 把 `saved_relay_app` 存好的 refresh credential 覆写成空白（模拟凭据缺失的行）。
fn blank_saved_refresh_credential(app: &tauri::App<tauri::test::MockRuntime>, relay_id: i64) {
    let state = app.state::<AppState>();
    let conn = state.db.conn.lock().expect("lock memory database");
    conn.execute(
        "UPDATE loongport_relay SET refresh_token = ?1 WHERE id = ?2",
        rusqlite::params!["   ", relay_id],
    )
    .expect("blank refresh credential");
}

#[tokio::test]
async fn newapi_purchase_dispatch_times_out_without_touching_sub2api_endpoints() {
    let (origin, requests, server) = recording_newapi_sentinel().await;
    // relay 行存 https 形态的哨兵 origin（生产行的 origin 在导入时就归一成 https），
    // purchase_url 用同 host:port 的 https 形态 —— `configured_purchase_url` 才解析得出。
    let (app, relay_id) = saved_relay_app(
        &origin.replacen("http://", "https://", 1),
        discovery::BackendKind::NewApi,
    );
    let op = relay_credentials(&app, relay_id);
    assert_eq!(
        op.refresh_token.as_deref(),
        Some("saved-refresh-token"),
        "前提：这行有非空 refresh credential"
    );

    let configured = format!(
        "{}{}",
        origin.replacen("http://", "https://", 1),
        "/console/topup"
    );
    let config = remote_config::RemoteConfig {
        relay_directory: remote_config::RelayDirectoryPolicy {
            blocked_hosts: vec![],
            sites: std::collections::BTreeMap::from([(
                "127.0.0.1".to_string(),
                remote_config::RelayDirectorySite {
                    veridrop_host: None,
                    entry_url: None,
                    purchase_url: Some(configured.clone()),
                    usage_url: None,
                    display_name: None,
                },
            )]),
        },
        ..remote_config::RemoteConfig::default()
    };
    let purchase_url = remote_config::configured_purchase_url(&config, &op.site_origin)
        .expect("签名目录解析不该报错")
        .expect("这个站在目录里配了购买入口");

    // MockRuntime 的 cookies_for_url 恒返回空 ⇒ 走 300ms 启动超时路径（生产 20s）。
    let window = purchase::purchase_window(relay_id, &op.site_origin);
    let error = dispatch_site_window(app.handle(), op, window, purchase_url)
        .await
        .expect_err("mock 下观察不到轮换，必须按超时收场");

    assert!(
        error.to_string().contains("重新登录"),
        "超时错误要有「重新登录」语义：{error}"
    );

    // 协议隔离：NewAPI 的充值分派不得打任何 sub2api 端点（协议字面量属于
    // api.rs / 既有测试形状，这里只对照黑名单）。
    let paths = requests.lock().unwrap().clone();
    for forbidden in [
        "/api/v1/settings/public",
        "/api/v1/user/profile",
        "/api/v1/auth/refresh",
    ] {
        assert!(
            !paths.iter().any(|path| path == forbidden),
            "NewAPI 充值分派不该打 sub2api 端点 {forbidden}：{paths:?}"
        );
    }

    until_purchase_lease_released(&app, relay_id).await;
    // 「窗口已销毁」在 MockRuntime 上不可观察：destroy 只清运行时自己的窗口表，
    // manager 那份（get_webview_window 读的）要等事件循环处理 Destroyed 才清，
    // 而 mock 的 run_iteration 是 no-op、测试也不驱动 run。销毁动作本身钉在
    // `newapi_purchase::open` 的 ready-Err 路径（返回前 destroy）与 monitor 兜底；
    // 这里能观察到的等价不变量是「lease 已还」—— 没有挂着 lease 却无人管理的窗口。
    assert!(
        !app.state::<AppState>()
            .purchase_sessions
            .is_active(relay_id),
        "超时收场后 lease 必须已释放"
    );

    server.abort();
}

#[tokio::test]
async fn newapi_purchase_blank_refresh_credential_is_rejected_before_a_window() {
    let (app, relay_id) = saved_relay_app("https://newapi.example", discovery::BackendKind::NewApi);
    blank_saved_refresh_credential(&app, relay_id);
    let op = relay_credentials(&app, relay_id);

    let window = purchase::purchase_window(relay_id, &op.site_origin);
    let error = dispatch_site_window(
        app.handle(),
        op,
        window,
        url::Url::parse("https://newapi.example/console/topup").unwrap(),
    )
    .await
    .expect_err("空白 refresh credential 必须在建窗前被拒绝");

    assert!(
        error.to_string().contains("重新登录"),
        "错误要指明出路：{error}"
    );
    assert!(
        app.get_webview_window(&purchase::window_label(relay_id))
            .is_none(),
        "被拒绝的调用不得留下窗口"
    );
    assert!(
        !app.state::<AppState>()
            .purchase_sessions
            .is_active(relay_id),
        "失败路径的 lease 必须已释放"
    );
}

#[tokio::test]
async fn newapi_purchase_focuses_an_existing_window_without_http_or_lease() {
    // relay 行故意存 http origin：任何回归（比如有人把续期挪到聚焦检查之前）都会
    // 真的打上这个哨兵，日志就不再是空的。
    let (origin, requests, server) = recording_newapi_sentinel().await;
    let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::NewApi);

    // 预建同 label 窗口，模拟「这一行的充值窗已经开着」。
    let label = purchase::window_label(relay_id);
    tauri::WebviewWindowBuilder::new(
        app.handle(),
        &label,
        tauri::WebviewUrl::External(url::Url::parse("about:blank").unwrap()),
    )
    .build()
    .expect("预建同 label 窗口");

    let op = relay_credentials(&app, relay_id);
    let window = purchase::purchase_window(relay_id, &op.site_origin);
    dispatch_site_window(
        app.handle(),
        op,
        window,
        url::Url::parse(&format!("{origin}/console/topup")).unwrap(),
    )
    .await
    .expect("聚焦现有窗口是 Ok");

    assert!(
        requests.lock().unwrap().is_empty(),
        "同一行第二击不得发任何 HTTP：{:?}",
        requests.lock().unwrap()
    );
    assert!(
        !app.state::<AppState>()
            .purchase_sessions
            .is_active(relay_id),
        "聚焦路径不得取 lease"
    );

    server.abort();
}

#[tokio::test]
async fn newapi_purchase_ignores_another_relays_lease() {
    // relay1（NewAPI）的 lease 被占着，relay2（也是 NewAPI）的充值分派照样能拿到
    // **自己的** lease —— lease 按 relay 键控，一行占用不该拦住另一行。
    //
    // 第二腿特意用 NewApi 而不是 sub2api（review F3）：sub2api 分派根本不碰
    // lease 协调器（跨行隔离对它是平凡成立，走通开窗本身已由 Task 4 的测试覆盖）；
    // NewApi 腿会真的执行 `try_acquire(relay2)`，被 relay1 的 lease 挡住与否在这里
    // 才是可观察的。判据：错误是 mock 下的启动超时（「重新登录」）而不是 lease
    // 占用文案（「正在使用或正在关闭」）—— 后者出现说明 try_acquire 被**别人**
    // 的 lease 挡了（两条文案都含「充值窗口」，判别要认后者的专属措辞）。
    let (app, relay1) = saved_relay_app("https://newapi.example", discovery::BackendKind::NewApi);
    let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
    let held = coordinator
        .try_acquire(relay1)
        .expect("占用 relay1 的 lease");

    // 第二行：另一个 NewAPI 站点账号，自己的 id 与有效 refresh credential。
    let relay2 = {
        let state = app.state::<AppState>();
        let conn = state.db.conn.lock().expect("lock memory database");
        let id = creds::save_site_with_backend(
            &conn,
            "https://newapi2.example",
            "Second relay",
            "https://newapi2.example",
            discovery::BackendKind::NewApi,
        )
        .expect("save second relay");
        creds::save_credentials(
            &conn,
            id,
            creds::AccountIdentity {
                id: 8,
                label: "Second Account",
                login_identifier: "second-account",
            },
            "second-access-token",
            Some("second-refresh-cookie"),
            None,
            creds::SessionEnvironment::default(),
        )
        .expect("save second credentials");
        id
    };

    let op = relay_credentials(&app, relay2);
    let window = purchase::purchase_window(relay2, &op.site_origin);
    let error = dispatch_site_window(
        app.handle(),
        op,
        window,
        url::Url::parse("https://newapi2.example/console/topup").unwrap(),
    )
    .await
    .expect_err("mock 下观察不到轮换，relay2 走自己的超时收场");

    let error = error.to_string();
    assert!(
        error.contains("重新登录"),
        "relay2 拿到了自己的 lease、走进了自己的开窗流程（mock 超时收场）：{error}"
    );
    assert!(
        !error.contains("正在使用或正在关闭"),
        "出现 lease 占用类错误说明 try_acquire 被别人（relay1）的 lease 挡了：{error}"
    );

    assert!(
        coordinator.is_active(relay1),
        "relay1 的 lease 不受 relay2 的分派影响"
    );
    until_purchase_lease_released(&app, relay2).await;

    drop(held);
}

#[tokio::test]
async fn newapi_purchase_reports_when_its_own_lease_is_already_held() {
    let (app, relay_id) = saved_relay_app("https://newapi.example", discovery::BackendKind::NewApi);
    let op = relay_credentials(&app, relay_id);

    let coordinator = Arc::clone(&app.state::<AppState>().purchase_sessions);
    let held = coordinator.try_acquire(relay_id).expect("预占自己的 lease");

    let window = purchase::purchase_window(relay_id, &op.site_origin);
    let error = dispatch_site_window(
        app.handle(),
        op,
        window,
        url::Url::parse("https://newapi.example/console/topup").unwrap(),
    )
    .await
    .expect_err("自己的 lease 被占时必须明确报错");

    assert!(
        error.to_string().contains("充值窗口"),
        "错误要说清是充值窗口占用：{error}"
    );
    assert!(
        app.get_webview_window(&purchase::window_label(relay_id))
            .is_none(),
        "不得开第二个窗口"
    );
    drop(held);
}

#[tokio::test]
async fn newapi_account_mismatch_stops_before_group_or_token_inventory() {
    use axum::{
        routing::{delete, get, post},
        Json, Router,
    };
    use serde_json::json;

    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let account_requests = Arc::clone(&requests);
    let group_requests = Arc::clone(&requests);
    let token_requests = Arc::clone(&requests);
    let create_requests = Arc::clone(&requests);
    let reveal_requests = Arc::clone(&requests);
    let delete_requests = Arc::clone(&requests);
    let app = Router::new()
        .route(
            "/api/user/self",
            get(move || {
                let requests = Arc::clone(&account_requests);
                async move {
                    requests.lock().unwrap().push("account".into());
                    Json(json!({
                        "success": true,
                        "data": {
                            "id": 99,
                            "username": "other-account",
                            "display_name": "Other Account",
                            "email": "other@example.test",
                            "group": "default",
                            "quota": 0,
                            "used_quota": 0
                        }
                    }))
                }
            }),
        )
        .route(
            "/api/user/self/groups",
            get(move || {
                let requests = Arc::clone(&group_requests);
                async move {
                    requests.lock().unwrap().push("groups".into());
                    Json(json!({ "success": true, "data": {} }))
                }
            }),
        )
        .route(
            "/api/token/",
            get(move || {
                let requests = Arc::clone(&token_requests);
                async move {
                    requests.lock().unwrap().push("tokens".into());
                    Json(json!({
                        "success": true,
                        "data": {
                            "page": 1,
                            "page_size": 100,
                            "total": 0,
                            "items": []
                        }
                    }))
                }
            })
            .post(move || {
                let requests = Arc::clone(&create_requests);
                async move {
                    requests.lock().unwrap().push("create".into());
                    Json(json!({ "success": true }))
                }
            }),
        )
        .route(
            "/api/token/{id}/key",
            post(move || {
                let requests = Arc::clone(&reveal_requests);
                async move {
                    requests.lock().unwrap().push("reveal".into());
                    Json(json!({ "success": true, "data": { "key": "unexpected" } }))
                }
            }),
        )
        .route(
            "/api/token/{id}",
            delete(move || {
                let requests = Arc::clone(&delete_requests);
                async move {
                    requests.lock().unwrap().push("delete".into());
                    Json(json!({ "success": true }))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind account-mismatch server");
    let origin = format!("http://{}", listener.local_addr().expect("server address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve test app");
    });
    let op = creds::Relay {
        site_origin: origin,
        ..test_newapi_relay(7)
    };

    let error = match provision_backend(&op, None).await {
        Ok(_) => panic!("persisted account mismatch must stop provisioning"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("账号不一致"), "{error}");
    assert_eq!(
            requests.lock().unwrap().as_slice(),
            ["account"],
            "account preflight must be the only remote request; no group/token inventory or mutation may run"
        );
    server.abort();
}

fn test_newapi_group(
    identity: &str,
    api_key: &str,
) -> crate::relay::newapi_provision::ReconciledGroup {
    crate::relay::newapi_provision::ReconciledGroup {
        identity: crate::relay::newapi::GroupIdentity(identity.into()),
        name: identity.into(),
        rate_multiplier: Some(1.25),
        description: format!("{identity} description"),
        api_key: api_key.into(),
        token_was_created: false,
    }
}

fn newapi_models() -> Vec<String> {
    provision::normalize_model_names(vec![
        "gemini-2.5-pro".into(),
        "claude-haiku-4-5".into(),
        "gpt-5.4".into(),
        "claude-sonnet-4-5".into(),
        "gpt-5.4".into(),
    ])
}

#[test]
fn newapi_model_catalog_requires_at_least_one_normalized_model() {
    assert!(normalize_newapi_model_catalog(None).is_none());
    assert!(normalize_newapi_model_catalog(Some(vec!["  ".into(), "\n".into()])).is_none());
    assert_eq!(
        normalize_newapi_model_catalog(Some(vec![
            " gpt-5.4 ".into(),
            "gemini-2.5-pro".into(),
            "gpt-5.4".into(),
        ])),
        Some(vec!["gemini-2.5-pro".into(), "gpt-5.4".into()])
    );
}

fn newapi_batch(
    op: &creds::Relay,
    groups: &[crate::relay::newapi_provision::ReconciledGroup],
) -> ManagedProvisionBatch {
    let account_id = op.account_id.expect("test relay has account id");
    // 与 provision_backend 同一条纪律：keep 槽位跟着分类走（混合目录 = 三个聊天栏）。
    let mut observed_keep = std::collections::HashSet::new();
    let candidates = groups
        .iter()
        .flat_map(|group| {
            let models = newapi_models();
            newapi_keep_insert(
                &mut observed_keep,
                &op.site_origin,
                account_id,
                &group.identity,
                Some(&models),
            );
            newapi_candidates_for_group(
                &op.site_origin,
                account_id,
                group,
                &models,
                // 测试钉内置表：选型断言不随本机真实远端缓存漂移。
                &provision::ModelSelectionTables::builtin(),
            )
        })
        .collect();
    ManagedProvisionBatch {
        account_id: Some(account_id),
        site_declaration: None,
        candidates,
        observed_keep,
        failures: Vec::new(),
        keys_created: 0,
    }
}

/// **纯生图分组的 new-api 扇出只出生图候选**（2026-09-05 某 new-api 站点纯生图
/// 分组的实测形状：`gpt-image-2 + nano-banana-2`）。
///
/// 旧行为把每个分组无条件扇出到 claude/codex/gemini：生图模型被写成聊天模型
/// （`ANTHROPIC_MODEL=nano-banana-2`、`GEMINI_MODEL=gpt-image-2`），切过去调用必
/// 404 —— 生图栏则永远零档位（「此账号在当前平台没有可用分组」）。
#[test]
fn newapi_pure_image_group_lands_only_in_the_image_column_and_migrates_legacy_tiers() {
    let op = test_newapi_relay(7);
    let group = test_newapi_group("图", "sk-image");
    let image_models =
        provision::normalize_model_names(vec!["nano-banana-2".into(), "gpt-image-2".into()]);
    let account_id = 7;

    let candidates = newapi_candidates_for_group(
        &op.site_origin,
        account_id,
        &group,
        &image_models,
        &provision::ModelSelectionTables::builtin(),
    );
    assert_eq!(candidates.len(), 1, "纯生图分组不该再扇出到聊天栏");
    assert_eq!(candidates[0].app_type, AppType::CodexImage);
    // 默认模型 = gpt-image 家族优先（跨家族并存时表里靠前的家族胜出）。
    assert_eq!(candidates[0].model, "gpt-image-2");

    // 先按旧行为落三栏（等价于升级前 provision 过的存量），再按新分类
    // provision 一次：三个聊天栏的旧投影必须被清掉、生图栏出现新档位。
    let db = Arc::new(crate::database::Database::memory().expect("memory db"));
    let state = AppState::new(db.clone());
    persist_provision_batch(&state, &op, newapi_batch(&op, std::slice::from_ref(&group)))
        .expect("seed legacy three-column projections");
    let provider_id =
        provision::newapi_provider_id_for(&op.site_origin, account_id, &group.identity.0);
    for app_type in newapi_app_types() {
        assert!(db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .expect("read legacy projection")
            .is_some());
    }

    let mut keep = std::collections::HashSet::new();
    newapi_keep_insert(
        &mut keep,
        &op.site_origin,
        account_id,
        &group.identity,
        Some(&image_models),
    );
    let migrated = ManagedProvisionBatch {
        account_id: Some(account_id),
        site_declaration: None,
        candidates,
        observed_keep: keep,
        failures: Vec::new(),
        keys_created: 0,
    };
    let summary = persist_provision_batch(&state, &op, migrated).expect("migrate to image column");
    assert_eq!(summary.tiers.len(), 1);
    assert_eq!(summary.tiers[0].app_id, AppType::CodexImage.as_str());
    for app_type in newapi_app_types() {
        assert!(
            db.get_provider_by_id(&provider_id, app_type.as_str())
                .expect("read pruned projection")
                .is_none(),
            "{} 的旧投影没被清掉",
            app_type.as_str()
        );
    }
    // 生图栏新档位：codex 形状（生图 MCP 的读取契约）+ 家族优先选出的模型。
    let image_provider = db
        .get_provider_by_id(&provider_id, AppType::CodexImage.as_str())
        .expect("read image projection")
        .expect("image projection exists");
    assert_eq!(
        provision::extract_model(&image_provider.settings_config).as_deref(),
        Some("gpt-image-2")
    );
    assert_eq!(
        provision::extract_api_key(&image_provider.settings_config, &AppType::CodexImage)
            .as_deref(),
        Some("sk-image")
    );
}

#[test]
fn newapi_group_expands_to_three_app_configs_with_one_provider_id() {
    let op = test_newapi_relay(7);
    let group = test_newapi_group(" vip/\u{4e2d}\u{6587} \u{1f680} ", "sk-shared");
    let batch = newapi_batch(&op, std::slice::from_ref(&group));

    assert_eq!(batch.candidates.len(), 3);
    assert_eq!(batch.observed_keep.len(), 3);
    let provider_ids = batch
        .candidates
        .iter()
        .map(|candidate| candidate.provider_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(provider_ids.len(), 1);
    assert_eq!(
        batch
            .candidates
            .iter()
            .map(|candidate| candidate.app_type.as_str())
            .collect::<Vec<_>>(),
        vec!["claude", "codex", "gemini"]
    );

    let db = Arc::new(crate::database::Database::memory().expect("memory db"));
    let state = AppState::new(db.clone());
    let summary = persist_provision_batch(&state, &op, batch).expect("persist projections");

    assert_eq!(summary.tiers.len(), 3);
    for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        let provider = db
            .get_provider_by_id(summary.tiers[0].provider_id.as_str(), app_type.as_str())
            .expect("read provider")
            .expect("projection exists");
        assert_eq!(
            provision::extract_api_key(&provider.settings_config, &app_type).as_deref(),
            Some("sk-shared")
        );
        assert_eq!(
            provider.website_url.as_deref(),
            Some(op.site_origin.as_str())
        );
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.loongport_account_id),
            Some(7)
        );
    }
}

#[test]
fn newapi_refresh_preserves_edited_config_but_recomputes_unedited_defaults() {
    let op = test_newapi_relay(7);
    let first_group = test_newapi_group("vip", "sk-first");
    let db = Arc::new(crate::database::Database::memory().expect("memory db"));
    let state = AppState::new(db.clone());
    let first = persist_provision_batch(&state, &op, newapi_batch(&op, &[first_group]))
        .expect("initial provision");
    let provider_id = first.tiers[0].provider_id.clone();

    let mut edited = db
        .get_provider_by_id(&provider_id, AppType::Codex.as_str())
        .expect("read edited provider")
        .expect("edited provider exists");
    edited.settings_config = provision::settings_config_for(
        &AppType::Codex,
        "sk-first",
        "Custom Name",
        "https://custom.example/v1",
        "gpt-custom",
    )
    .expect("custom codex config");
    let mut expected_edited = edited.settings_config.clone();
    assert!(provision::patch_api_key(
        &mut expected_edited,
        &AppType::Codex,
        "sk-second"
    ));
    db.save_provider(AppType::Codex.as_str(), &edited)
        .expect("save edited provider");
    db.set_user_edited(AppType::Codex.as_str(), &provider_id, true)
        .expect("mark edited");

    let mut unedited = db
        .get_provider_by_id(&provider_id, AppType::Gemini.as_str())
        .expect("read unedited provider")
        .expect("unedited provider exists");
    unedited.settings_config["env"]["GEMINI_MODEL"] =
        serde_json::Value::String("gemini-stale".into());
    db.save_provider(AppType::Gemini.as_str(), &unedited)
        .expect("save stale unedited provider");

    let second_group = test_newapi_group("vip", "sk-second");
    let second_batch = newapi_batch(&op, &[second_group]);
    persist_provision_batch(&state, &op, second_batch).expect("refresh provision");

    let edited_after = db
        .get_provider_by_id(&provider_id, AppType::Codex.as_str())
        .expect("read refreshed edited provider")
        .expect("refreshed edited provider exists");
    assert_eq!(edited_after.settings_config, expected_edited);
    let unedited_after = db
        .get_provider_by_id(&provider_id, AppType::Gemini.as_str())
        .expect("read refreshed default provider")
        .expect("refreshed default provider exists");
    assert_eq!(
        provision::extract_api_key(&unedited_after.settings_config, &AppType::Gemini).as_deref(),
        Some("sk-second")
    );
    assert_eq!(
        unedited_after
            .settings_config
            .pointer("/env/GEMINI_MODEL")
            .and_then(serde_json::Value::as_str),
        Some("gemini-2.5-pro")
    );
}

#[test]
fn newapi_unclassified_keep_retains_failed_group_and_prunes_only_the_current_account() {
    let account_seven = test_newapi_relay(7);
    let account_eight = test_newapi_relay(8);
    let db = Arc::new(crate::database::Database::memory().expect("memory db"));
    let state = AppState::new(db.clone());

    persist_provision_batch(
        &state,
        &account_seven,
        newapi_batch(
            &account_seven,
            &[
                test_newapi_group("observed", "sk-seven-observed"),
                test_newapi_group("removed", "sk-seven-removed"),
            ],
        ),
    )
    .expect("seed account seven");
    persist_provision_batch(
        &state,
        &account_eight,
        newapi_batch(
            &account_eight,
            &[
                test_newapi_group("observed", "sk-eight-observed"),
                test_newapi_group("removed", "sk-eight-removed"),
            ],
        ),
    )
    .expect("seed account eight");

    let observed = crate::relay::newapi::GroupIdentity("observed".into());
    let retained_id = provision::newapi_provider_id_for(&account_seven.site_origin, 7, &observed.0);
    let removed_id = provision::newapi_provider_id_for(&account_seven.site_origin, 7, "removed");
    // 对账没走完（拿到 observed 清单但没拿到 sk）：分类未知，保全四个槽位。
    let mut failure_keep = std::collections::HashSet::new();
    newapi_keep_insert(
        &mut failure_keep,
        &account_seven.site_origin,
        7,
        &observed,
        None,
    );
    let failure_batch = ManagedProvisionBatch {
        account_id: Some(7),
        site_declaration: None,
        candidates: Vec::new(),
        observed_keep: failure_keep,
        failures: vec![FailureInfo {
            group_name: "observed".into(),
            reason: "reveal: temporary failure".into(),
        }],
        keys_created: 0,
    };
    let summary = persist_provision_batch(&state, &account_seven, failure_batch)
        .expect("retained existing providers keep the refresh partial-successful");

    assert!(summary.tiers.is_empty());
    assert_eq!(summary.failures.len(), 1);
    for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        assert!(db
            .get_provider_by_id(&retained_id, app_type.as_str())
            .expect("read retained provider")
            .is_some());
        assert!(db
            .get_provider_by_id(&removed_id, app_type.as_str())
            .expect("read removed provider")
            .is_none());

        let other_account_id =
            provision::newapi_provider_id_for(&account_eight.site_origin, 8, "removed");
        assert!(db
            .get_provider_by_id(&other_account_id, app_type.as_str())
            .expect("read other account provider")
            .is_some());
    }
}

#[test]
fn newapi_provider_write_failure_keeps_successful_apps_and_reports_the_failure() {
    let op = test_newapi_relay(7);
    let db = Arc::new(crate::database::Database::memory().expect("memory db"));
    {
        let conn = db.conn.lock().expect("lock memory db");
        conn.execute_batch(
            "CREATE TRIGGER fail_newapi_claude_write
                 BEFORE INSERT ON providers
                 WHEN NEW.app_type = 'claude'
                 BEGIN
                   SELECT RAISE(FAIL, 'injected claude write failure');
                 END;",
        )
        .expect("install selective write failure");
    }
    let state = AppState::new(db.clone());

    let summary = persist_provision_batch(
        &state,
        &op,
        newapi_batch(&op, &[test_newapi_group("partial", "sk-partial")]),
    )
    .expect("two successful app projections keep the batch successful");

    assert_eq!(
        summary
            .tiers
            .iter()
            .map(|tier| tier.app_id.as_str())
            .collect::<Vec<_>>(),
        vec!["codex", "gemini"]
    );
    assert_eq!(summary.failures.len(), 1);
    assert_eq!(summary.failures[0].group_name, "partial");
    assert!(summary.failures[0].reason.contains("claude"));
    assert!(summary.failures[0]
        .reason
        .contains("injected claude write failure"));
}

/// 构造一条带归属的档位。`account` 为 `None` 表示升级前生成的旧档位。
fn owned(id: &str, site: Option<&str>, account: Option<i64>) -> OwnedTier {
    OwnedTier {
        tier: tier(id),
        site_origin: site.map(str::to_string),
        account_id: account,
    }
}

/// `tiers_of_site` 的归属参数在归属测试里恒定，包一层省得每处重复。
/// 它内部造一个空内存库当 state（`tiers_of_site` 要读「已手工维护」标记；
/// 这些归属测试不关心标记，空库读出来全是 false 即可）。
fn tiers_of(tiers: &[OwnedTier], site: &str, account: Option<i64>) -> Vec<TierInfo> {
    let state = AppState::new(std::sync::Arc::new(
        crate::database::Database::memory().expect("内存库"),
    ));
    tiers_of_site(&state, tiers, site, account, &test_app()).expect("tiers_of_site 不该失败")
}

/// ⭐ **`tiers_of_site` 的 `user_edited` 来自存库标记，不是内容比对。**
///
/// 旧实现靠比对 settings_config 与默认值算出「用户改过没有」；现在改为读
/// `providers.user_edited`（编辑页置位、恢复默认复位）。这条钉住：分组时
/// `user_edited` 如实反映库里标记，而不是原样透传 `None`。
#[test]
fn grouping_reads_the_user_edited_flag_from_the_database() -> Result<(), Box<dyn std::error::Error>>
{
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("内存库"));
    let state = AppState::new(db.clone());
    // 先造两条 provider 行（get_user_edited 读的是 providers 表，不是空表）。
    {
        let conn = crate::database::lock_conn!(db.conn);
        conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config) \
                 VALUES ('t-default','codex','t-default','{}'), ('t-edited','codex','t-edited','{}')",
                [],
            )
            .expect("插行");
    }
    // 库里只给 t-edited 置位；t-default 不置。
    db.set_user_edited(AppType::Codex.as_str(), "t-edited", true)
        .expect("置位");

    let tiers = vec![
        owned("t-default", Some(site), Some(1)),
        owned("t-edited", Some(site), Some(1)),
    ];
    let got = tiers_of_site(&state, &tiers, site, Some(1), &test_app()).expect("分组不该失败");
    let flags: Vec<_> = got.iter().map(|t| t.user_edited).collect();
    assert_eq!(
        flags,
        vec![Some(false), Some(true)],
        "user_edited 该读库里标记（t-default 没置位=false，t-edited 置位=true）"
    );
    Ok(())
}

#[test]
fn tiers_are_grouped_by_site_origin_not_by_guessing() {
    let a = "https://bestapi.store";
    let b = "https://other.dev";
    let tiers = vec![
        owned("t-a1", Some(a), Some(1)),
        owned("t-b1", Some(b), Some(1)),
        owned("t-a2", Some(a), Some(1)),
        // 没有 website_url 的历史数据：不归任何站。
        owned("t-orphan", None, Some(1)),
    ];

    assert_eq!(
        tiers_of(&tiers, a, Some(1))
            .iter()
            .map(|t| t.provider_id.clone())
            .collect::<Vec<_>>(),
        vec!["t-a1", "t-a2"],
        "同站的档位要按原顺序全带上（顺序 = provision 时的 sort_index，倍率低的在前）"
    );
    assert_eq!(tiers_of(&tiers, b, Some(1)).len(), 1);

    // 孤儿档位不能被塞给任何站 —— 塞错了用户会以为在 A 站买的档位属于 B 站。
    let all: usize = [a, b]
        .iter()
        .map(|s| tiers_of(&tiers, s, Some(1)).len())
        .sum();
    assert_eq!(all, 3, "4 条里那条没有 website_url 的必须落空");
}

/// ⭐ **同一个站上的两个账号不能看到对方的档位。**
///
/// 实测踩到的类：归属原本只判 `website_url`（站点），于是同站每一行都显示该站的
/// **全部**档位 —— 用户看到的档位数与他实际拥有的不符，点进去用的还是别人的 sk
/// （连账单都算到别人头上）。
#[test]
fn tiers_are_split_between_two_accounts_on_the_same_site() {
    let site = "https://bestapi.store";
    let tiers = vec![
        owned("t-acct7", Some(site), Some(7)),
        owned("t-acct9", Some(site), Some(9)),
        // 升级前生成的：没记账号 ⇒ 只按站点归，两个账号都看得到（见函数文档）。
        owned("t-legacy", Some(site), None),
    ];

    let seven: Vec<_> = tiers_of(&tiers, site, Some(7))
        .iter()
        .map(|t| t.provider_id.clone())
        .collect();
    assert_eq!(
        seven,
        vec!["t-acct7", "t-legacy"],
        "账号 7 只该看到自己的 + 没记归属的旧档位，**不该看到账号 9 的**"
    );

    let nine: Vec<_> = tiers_of(&tiers, site, Some(9))
        .iter()
        .map(|t| t.provider_id.clone())
        .collect();
    assert_eq!(nine, vec!["t-acct9", "t-legacy"]);

    // 还没登录的行（没有 account_id）：有主的档位都不是它的。
    let anon: Vec<_> = tiers_of(&tiers, site, None)
        .iter()
        .map(|t| t.provider_id.clone())
        .collect();
    assert_eq!(anon, vec!["t-legacy"], "未登录的行不该认领任何有主的档位");
}

#[test]
fn site_matching_is_exact_not_prefix() {
    // 前缀匹配会让 https://api.store 命中 https://api.store.evil.com。
    let tiers = vec![owned("t1", Some("https://api.store"), Some(1))];
    assert_eq!(tiers_of(&tiers, "https://api.store", Some(1)).len(), 1);
    assert!(tiers_of(&tiers, "https://api.sto", Some(1)).is_empty());
    assert!(tiers_of(&tiers, "https://api.store.evil.com", Some(1)).is_empty());
}

#[test]
fn chatgpt_quit_is_codex_only() {
    // 用户同意 + codex ⇒ 退。
    assert!(should_quit_chatgpt(true, &AppType::Codex));
    // 用户同意但切的是别的平台 ⇒ **不退**。ChatGPT 桌面版只读 ~/.codex，
    // 切 claude/gemini 档位去关它纯属扰民（关掉用户正开着的、与本次切换无关的对话）。
    assert!(!should_quit_chatgpt(true, &AppType::Claude));
    assert!(!should_quit_chatgpt(true, &AppType::Gemini));
    // 用户没同意 ⇒ 一律不退，哪怕是 codex。
    assert!(!should_quit_chatgpt(false, &AppType::Codex));
}

#[test]
fn switch_confirmation_is_decided_before_mutating_the_target() {
    assert!(should_request_switch_confirmation(
        &AppType::Codex,
        None,
        true
    ));
    assert!(!should_request_switch_confirmation(
        &AppType::Claude,
        None,
        true
    ));
    assert!(!should_request_switch_confirmation(
        &AppType::Codex,
        Some(false),
        true
    ));
}

#[test]
fn managed_meta_pins_api_format_for_codex_and_leaves_others_empty() {
    // codex：不写 apiFormat 会落到 ProxyChat profile —— 那是唯一会 spawn codex
    // 子进程的分支。
    assert_eq!(
        managed_meta(&AppType::Codex, Some(1), None)
            .api_format
            .as_deref(),
        Some("openai_responses")
    );

    // 其它 CLI：`api_format` **只被 codex_config.rs 消费**，给它们填值不会有人读，
    // 反而让人以为那里有语义。
    for app_type in [AppType::Claude, AppType::Gemini] {
        assert_eq!(
            managed_meta(&app_type, Some(1), None).api_format,
            None,
            "{app_type:?} 不该有 api_format —— 只有 codex 会读它"
        );
    }
}

#[test]
fn default_site_is_the_placeholder_from_the_requirement() {
    assert_eq!(DEFAULT_SITE, "790053500.com");
}

/// ⭐ 钉住「默认站在 aff **内置表**里有码」—— 这与它上一版的规则**正好相反**。
///
/// 默认站曾是维护者自己的站，那时它**有意不在** aff 表里（服务端拒绝自己邀请自己）。
/// 换成 `790053500.com` 之后那条理由不再适用，有码才是对的 —— 但
/// [`crate::relay::aff`] 的测试里仍留着「维护者自己的站不该有码」那条，
/// 很容易有人按类比把默认站也从表里划掉，而那**不报任何错**，
/// 只是每一次「留空点确定」都白丢一笔返利。
///
/// ⚠️ **它守的是内置那一层，不是运行时的最终取值**（codex review 纠正）：
/// 实际取码走 [`crate::relay::remote_config::resolve_aff_code`] 的两层回落，
/// 远端配置命中就用远端的，且**远端给空串 = 撤销、不回落到内置**。
/// 所以本条断言不能、也不该保证「线上一定带码」—— 那取决于维护者当天发的配置。
#[test]
fn the_default_site_has_a_builtin_affiliate_code() {
    assert!(
        crate::relay::aff::aff_code_for(&format!("https://{DEFAULT_SITE}")).is_some(),
        "{DEFAULT_SITE} 是默认站且不是维护者自己的站，必须在 aff 内置表里"
    );
}

/// 造一条 provider。`site` 进 `website_url`（归属依据），`id` 决定它是否被认作托管项。
fn seeded(id: &str, name: &str, site: Option<&str>) -> Provider {
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
    }
}

/// 带账号归属的那种（provision 从此都写它，见 `managed_meta`）。
fn seeded_owned(id: &str, name: &str, site: Option<&str>, account_id: i64) -> Provider {
    Provider {
        meta: Some(managed_meta(&AppType::Codex, Some(account_id), None)),
        ..seeded(id, name, site)
    }
}

/// ⭐ **A 账号 provision 不能删掉同站 B 账号的档位。**
///
/// 这是本轮实测追出来的一类：归属原本只判 `website_url`（站点），而 `keep` 只装
/// **这一次** provision（= 一个账号）生成的 id ⇒ A 刷新一次就把 B 的全部档位
/// 当成「不再存在」删光。同一个缺陷在 `remove_site_impl`（删一个账号）下更彻底：
/// 它传空 `keep`，等于清掉该站所有账号的档位。
#[test]
fn pruning_one_account_leaves_another_accounts_tiers_on_the_same_site() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

    // 账号 7 的两条：一条这次仍在（keep 里），一条已失效。
    let a_kept = provision::provider_id_for(site, Some(7), 1);
    let a_stale = provision::provider_id_for(site, Some(7), 2);
    // 账号 9 的一条：**这次压根没查它**（不同账号、不同分组集合）。
    let b_tier = provision::provider_id_for(site, Some(9), 1);

    for p in [
        seeded_owned(&a_kept, "A·留", Some(site), 7),
        seeded_owned(&a_stale, "A·废", Some(site), 7),
        seeded_owned(&b_tier, "B·别动", Some(site), 9),
    ] {
        db.save_provider("codex", &p).expect("seed");
    }

    let state = AppState::new(db.clone());
    let keep: std::collections::HashSet<(String, String)> = [("codex".to_string(), a_kept.clone())]
        .into_iter()
        .collect();

    // 以账号 7 的身份清理。
    let removed = prune_stale_tiers(&state, site, Some(7), &keep).expect("prune");
    assert_eq!(removed, 1, "只该删账号 7 那条失效的");

    let ids = db.get_provider_ids("codex").expect("list");
    assert!(ids.contains(&a_kept), "账号 7 这次生成的要留着");
    assert!(!ids.contains(&a_stale), "账号 7 失效的那条该删");
    assert!(
        ids.contains(&b_tier),
        "⭐ 账号 9 的档位**必须留着** —— 它不在这次的 keep 里只是因为压根没查它"
    );
}

/// 这道闸守 `prune_stale_tiers` 的三个判据。
///
/// 它是**唯一会删用户数据的 relay 代码路径**，判据放宽一点就会误删用户手工配置的
/// provider（不可挽回）；收紧一点则清不掉脏记录（就是用户撞见的「claude 下还有
/// codex 分组，点刷新也不消失」）。所以正反两面都要钉住。
#[test]
fn prune_only_touches_this_sites_managed_tiers() {
    let site = "https://bestapi.store";
    let other_site = "https://other.dev";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

    // 这次 provision 生成的（该留）。
    let kept_id = provision::provider_id_for(site, Some(1), 1);
    // 同一个站的托管项，但这次没生成（该删 —— 分组已被中转站删掉 / 旧版本写错的）。
    let stale_id = provision::provider_id_for(site, Some(1), 2);
    // **别的站**的托管项：这次压根没查它的分组，凭「这次没生成」删它是错的。
    let other_site_id = provision::provider_id_for(other_site, Some(1), 3);

    for (app, p) in [
        ("codex", seeded(&kept_id, "留下", Some(site))),
        ("codex", seeded(&stale_id, "该删", Some(site))),
        ("codex", seeded(&other_site_id, "别的站", Some(other_site))),
        // 用户手工加的：id 不是我们生成的形状 ⇒ 一律不碰，哪怕 website_url 是同一个站。
        ("codex", seeded("my-own-provider", "用户自己的", Some(site))),
        // 托管项但没有 website_url（历史数据）⇒ 归属不明，不删（宁可漏删不可错删）。
        (
            "codex",
            seeded(
                &provision::provider_id_for(site, Some(1), 9),
                "无归属",
                None,
            ),
        ),
        // **另一个 app_type 下的脏记录** —— 正是用户撞见的那种（openai 分组被
        // 旧代码写进了 claude 下）。必须也被清掉，所以不能只扫参数指定的那个 app。
        ("claude", seeded(&stale_id, "串台到 claude", Some(site))),
    ] {
        db.save_provider(app, &p).expect("seed");
    }

    let state = AppState::new(db.clone());
    // 这次只在 codex 下生成了 kept_id。
    let keep: std::collections::HashSet<(String, String)> =
        [("codex".to_string(), kept_id.clone())]
            .into_iter()
            .collect();

    let removed = prune_stale_tiers(&state, site, Some(1), &keep).expect("prune");
    assert_eq!(removed, 2, "该删的是 codex 与 claude 下那两条 stale");

    let codex_ids = db.get_provider_ids("codex").expect("list codex");
    assert!(codex_ids.contains(&kept_id), "这次生成的必须留着");
    assert!(!codex_ids.contains(&stale_id), "同站的过期档位必须删掉");
    assert!(
        codex_ids.contains(&other_site_id),
        "别的站的档位不能删 —— 这次没查它的分组"
    );
    assert!(
        codex_ids.contains("my-own-provider"),
        "用户手工配的 provider 绝不能删"
    );
    assert!(
        codex_ids.contains(&provision::provider_id_for(site, Some(1), 9)),
        "没有 website_url 的托管项归属不明，不该删"
    );

    let claude_ids = db.get_provider_ids("claude").expect("list claude");
    assert!(
        !claude_ids.contains(&stale_id),
        "串到别的 app_type 下的脏记录也要清 —— 只扫一个 app 就漏了它"
    );
}

/// ⭐ 用户实测那个 bug 的**精确复现**：同一个 id 在一个 app 下合法、在另一个下是脏的。
///
/// ## 为什么上面那条测试放过了它
///
/// 那条构造的串台记录在**两个 app 下都该删**（`keep` 里压根没有它）。
/// 而真实情形是：`pro池` 这个分组的 platform 是 openai ⇒ 它在 **codex 下合法**，
/// 但旧版本的 bug 把它也写进了 **claude** ⇒ claude 下那条是脏的。
///
/// 而 `provider_id = sha256(site_origin + group_id)`，**不含 app_type** ⇒
/// 两条记录的 id **完全相同**（实测 `loongport-8c669ca0b007e7ea`）。
/// 于是「keep 只放 id」时：那个 id 因为 codex 下合法而进了 keep，
/// claude 下那条脏记录就被当成「该保留」⇒ **点多少次刷新都不消失**。
///
/// 这正是用户反复报的那个现象。判据必须是 **(app_type, id) 组合**。
#[test]
fn a_group_valid_in_one_app_does_not_protect_its_twin_in_another_app() {
    let site = "https://790053500.com";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

    // 同一个分组（group_id = 1）⇒ 两个 app 下**同一个 id**。
    let shared_id = provision::provider_id_for(site, Some(1), 1);
    db.save_provider("codex", &seeded(&shared_id, "pro池", Some(site)))
        .expect("seed codex");
    db.save_provider("claude", &seeded(&shared_id, "pro池", Some(site)))
        .expect("seed claude");

    let state = AppState::new(db.clone());
    // 这次 provision 只把它落到 codex（因为它的 platform 是 openai）。
    let keep: std::collections::HashSet<(String, String)> =
        [("codex".to_string(), shared_id.clone())]
            .into_iter()
            .collect();

    let removed = prune_stale_tiers(&state, site, Some(1), &keep).expect("prune");

    assert_eq!(removed, 1, "claude 下那条脏记录必须被删掉");
    assert!(
        db.get_provider_ids("codex")
            .expect("codex")
            .contains(&shared_id),
        "codex 下那条是这次生成的，必须留着"
    );
    assert!(
        !db.get_provider_ids("claude")
            .expect("claude")
            .contains(&shared_id),
        "claude 下那条必须被删 —— 它与 codex 下那条 id 相同，\
             但『在 codex 下合法』不该保护它"
    );
}

/// 当前项也删。
///
/// `ProviderService::delete` 拒绝删当前项（防用户误删正在用的配置），但走到 prune
/// 这一步说明**服务端已经没有这个分组了**，它的 sk 是死的 —— 留着当「当前项」只会
/// 让 CLI 拿失效密钥去发请求。用户重新选一个可用的即可。
#[test]
fn prune_deletes_the_current_tier_too() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let stale_id = provision::provider_id_for(site, Some(1), 7);

    db.save_provider("codex", &seeded(&stale_id, "过期的当前项", Some(site)))
        .expect("seed");
    db.set_current_provider("codex", &stale_id)
        .expect("set current");

    let state = AppState::new(db.clone());
    let removed =
        prune_stale_tiers(&state, site, Some(1), &std::collections::HashSet::new()).expect("prune");

    assert_eq!(removed, 1, "当前项也该被删掉");
    assert!(
        db.get_provider_by_id(&stale_id, "codex")
            .expect("query")
            .is_none(),
        "过期的当前项必须真的从库里消失"
    );
}

/// ⚠️ **「恢复默认配置」必须按档位自己的归属找中转站，不能用全局「当前站」。**
///
/// 这条钉的是 review 抓出的那个 P0：原来那行是 `creds::load()`，返回的是
/// `ORDER BY is_current DESC LIMIT 1` —— 全局当前站。而分组页把所有中转站并列，
/// 用户展开 B 站点它的档位时，会拿到 **A 站的 `api_base_url`** ⇒ 那个档位被写成
/// 「B 的 sk + A 的端点」⇒ 每次调用都 401，而界面显示恢复成功。
///
/// **单站用户完全碰不到**（那时当前站就是唯一的站），所以手工测试测不出来 ——
/// 这正是它需要一条测试的原因。
///
/// 会红的改法：把归属判据换回 `creds::load()` / 任何「全局当前」的东西。
type DiagCompletion = (
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

struct ResetVerifier {
    senders: Mutex<HashMap<TargetKey, DiagCompletion>>,
}

impl ResetVerifier {
    fn new() -> Self {
        Self {
            senders: Mutex::new(HashMap::new()),
        }
    }

    fn complete(&self, target: &TargetKey, report: VerificationReport) -> bool {
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

fn verification_report(target: TargetKey, verdict: Verdict) -> VerificationReport {
    VerificationReport {
        target,
        verdict,
        evidence_level: EvidenceLevel::ProtocolBehavior,
        facts: Vec::new(),
        diagnostics: Vec::new(),
        rules_version: RULES_VERSION,
        checked_at: 1_786_214_400,
    }
}

fn reset_state(valid_key: bool) -> (AppState, Arc<ResetVerifier>, String, String, TargetKey) {
    let site = "https://reset.example";
    let db = Arc::new(crate::database::Database::memory().expect("init db"));
    let verifier = Arc::new(ResetVerifier::new());
    let mut state = AppState::new(db.clone());
    state.model_verification = Arc::new(ModelVerificationCoordinator::with_verifier(
        db.clone(),
        verifier.clone(),
    ));
    let row_id = with_conn(&state, |conn| {
        creds::save_site(conn, site, "Reset", "https://reset.example/v1")
    })
    .expect("save site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "reset@example.com",
                login_identifier: "reset@example.com",
            },
            "token",
            None,
            None,
            creds::SessionEnvironment::default(),
        )
    })
    .expect("save credentials");

    let provider_id = provision::provider_id_for(site, Some(7), 1);
    let other_provider_id = provision::provider_id_for(site, Some(7), 2);
    let settings_config = if valid_key {
        provision::settings_config_for(
            &AppType::Codex,
            "sk-reset",
            "Reset tier",
            "https://reset.example/v1",
            DEFAULT_MODEL,
        )
        .expect("codex config")
    } else {
        serde_json::json!({"model_provider":"custom"})
    };
    let provider = Provider {
        settings_config,
        ..seeded_owned(&provider_id, "Reset tier", Some(site), 7)
    };
    db.save_provider("codex", &provider).expect("save provider");
    db.save_provider(
        "codex",
        &Provider {
            settings_config: provision::settings_config_for(
                &AppType::Codex,
                "sk-other",
                "Other tier",
                "https://reset.example/v1",
                DEFAULT_MODEL,
            )
            .expect("other config"),
            ..seeded_owned(&other_provider_id, "Other tier", Some(site), 7)
        },
    )
    .expect("save other provider");
    db.set_user_edited("codex", &provider_id, true)
        .expect("mark edited");

    let running = TargetKey::new(&provider_id, "codex", "gpt-running");
    for report in [
        verification_report(
            TargetKey::new(&provider_id, "codex", "gpt-a"),
            Verdict::Suspicious,
        ),
        verification_report(
            TargetKey::new(&provider_id, "codex", "gpt-b"),
            Verdict::Anomaly,
        ),
        verification_report(
            TargetKey::new(&other_provider_id, "codex", "gpt-other"),
            Verdict::Trusted,
        ),
    ] {
        crate::relay::model_verification::store::upsert_active(&db, &report)
            .expect("seed verification report");
    }

    (state, verifier, provider_id, other_provider_id, running)
}

#[tokio::test]
async fn reset_tier_config_validation_failure_cancels_run_but_preserves_all_reports() {
    let (state, verifier, provider_id, other_provider_id, running) = reset_state(false);
    state
        .model_verification
        .start(running.clone())
        .await
        .expect("start run");

    let error = reset_tier_config_in_state(&state, &provider_id, AppType::Codex)
        .expect_err("missing key must reject reset");

    assert!(error.to_string().contains("密钥"));
    assert_eq!(
        state
            .model_verification
            .list_results(&[provider_id.clone(), other_provider_id.clone()])
            .expect("list reports")
            .len(),
        3
    );
    let _ = verifier.complete(
        &running,
        verification_report(running.clone(), Verdict::Trusted),
    );
    tokio::task::yield_now().await;
    assert_eq!(
        state
            .model_verification
            .list_results(&[provider_id, other_provider_id])
            .expect("reports after late completion")
            .len(),
        3
    );
}

#[tokio::test]
async fn reset_tier_config_save_failure_cancels_run_but_preserves_all_reports() {
    let (state, verifier, provider_id, other_provider_id, running) = reset_state(true);
    state
        .model_verification
        .start(running.clone())
        .await
        .expect("start run");
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_reset BEFORE UPDATE ON providers
                 BEGIN SELECT RAISE(FAIL, 'reject reset'); END;",
        )
        .expect("install failure trigger");

    let error = reset_tier_config_in_state(&state, &provider_id, AppType::Codex)
        .expect_err("provider save must fail");

    assert!(matches!(error, AppError::Database(_)));
    assert_eq!(
        state
            .model_verification
            .list_results(&[provider_id.clone(), other_provider_id.clone()])
            .expect("list reports")
            .len(),
        3
    );
    let _ = verifier.complete(
        &running,
        verification_report(running.clone(), Verdict::Trusted),
    );
    tokio::task::yield_now().await;
    assert_eq!(
        state
            .model_verification
            .list_results(&[provider_id, other_provider_id])
            .expect("reports after late completion")
            .len(),
        3
    );
}

#[tokio::test]
async fn reset_tier_config_success_clears_only_target_scope_and_rejects_late_completion() {
    let (state, verifier, provider_id, other_provider_id, running) = reset_state(true);
    state
        .model_verification
        .start(running.clone())
        .await
        .expect("start run");

    reset_tier_config_in_state(&state, &provider_id, AppType::Codex).expect("reset succeeds");

    let rows = state
        .model_verification
        .list_results(&[provider_id.clone(), other_provider_id.clone()])
        .expect("list reports");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target.provider_id, other_provider_id);
    assert!(!state
        .db
        .get_user_edited("codex", &provider_id)
        .expect("edited flag"));
    let _ = verifier.complete(
        &running,
        verification_report(running.clone(), Verdict::Trusted),
    );
    tokio::task::yield_now().await;
    assert!(state
        .model_verification
        .list_results(&[provider_id])
        .expect("target reports")
        .is_empty());
}

/// ⭐ 恢复默认必须保住**每一个**带目录平台的 `modelCatalog`。
///
/// 回归背景：PR #237 给 grok 补目录时只改了 persist 侧的平台名单、漏了 reset 侧
/// ——「恢复默认」把 Claude / Gemini / Grok 的模型芯片清空到下次 provision。
/// 名单唯源是 [`provision::model_catalog_apps`]，这条测试按平台全量遍历：
/// 以后名单加平台，新平台自动被覆盖，不会再出现「persist 改了 reset 没跟」。
#[test]
fn reset_tier_config_keeps_the_model_catalog_for_every_catalog_app() {
    for (app_type, model_names) in [
        (AppType::Claude, vec!["claude-opus-5", "claude-sonnet-5"]),
        (AppType::Codex, vec!["gpt-5.6-codex", "gpt-5.6-mini"]),
        (AppType::Gemini, vec!["gemini-3-pro", "gemini-3-flash"]),
        (AppType::GrokBuild, vec!["grok-4.6", "grok-4.5"]),
    ] {
        let models: Vec<String> = model_names.iter().map(|s| s.to_string()).collect();
        let site = "https://catalog.example";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "Catalog", "https://catalog.example/v1")
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "catalog@example.com",
                    login_identifier: "catalog@example.com",
                },
                "token",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let settings = provision::settings_config_with_models(
            &app_type,
            "sk-catalog",
            "Catalog·Pro",
            "https://catalog.example/v1",
            &models[0],
            Some(&models),
        )
        .expect("settings with catalog");
        // 前提：该平台的生成器真把目录写进了 settings（Gemini 只收 gemini-* 家族，
        // 所以每个平台用自家家族的模型名）。
        let before = models_from_settings(&settings);
        assert_eq!(
            before.len(),
            models.len(),
            "{} 的生成器没把目录写进 settings —— 测试前提不成立",
            app_type.as_str()
        );

        db.save_provider(
            app_type.as_str(),
            &Provider {
                settings_config: settings,
                ..seeded_owned(&provider_id, "Catalog·Pro", Some(site), 7)
            },
        )
        .expect("save provider");

        reset_tier_config_in_state(&state, &provider_id, app_type.clone()).expect("reset succeeds");

        let after = state
            .db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .expect("read back")
            .expect("provider 还在")
            .settings_config;
        assert_eq!(
            models_from_settings(&after),
            before,
            "{} 恢复默认后 modelCatalog 必须原样保留",
            app_type.as_str()
        );
    }
}

/// 备份是「删 auth.json」之前的唯一后路，所以它必须真的把内容拷出来。
///
/// ⚠️ **测试绝不能碰真实的 `~/.codex/auth.json`** —— 那里面是用户的 OAuth
/// refresh token，跑一次测试把开发者自己的 ChatGPT 登录搞掉是不可接受的副作用
/// （`chatgpt_app.rs:349` 那条注释钉的是同一件事）。所以这里不调
/// `get_codex_auth_path()`，而是自己造一个临时文件喂给 `backup_codex_auth`，
/// 并用 `CC_SWITCH_TEST_HOME` 把备份目标也关进临时目录。
#[test]
#[serial_test::serial]
fn backup_copies_auth_json_before_it_gets_deleted() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let original_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
    std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

    let auth_path = temp.path().join("auth.json");
    let payload = r#"{"tokens":{"refresh_token":"secret"}}"#;
    std::fs::write(&auth_path, payload).expect("write fake auth.json");

    let backup = backup_codex_auth(&auth_path)
        .expect("备份不该失败")
        .expect("有源文件时必须返回备份路径");

    let backup_path = std::path::Path::new(&backup);
    assert_eq!(
        std::fs::read_to_string(backup_path).expect("read backup"),
        payload,
        "备份内容必须与原文件逐字节一致 —— 它是用户唯一的还原来源"
    );
    assert!(
        auth_path.exists(),
        "备份是**拷贝**不是移动：这一步失败时调用方要能原地中止，源文件必须还在"
    );
    assert!(
        backup_path.starts_with(temp.path()),
        "备份必须落在 CC_SWITCH_TEST_HOME 下，绝不能写到真实的 ~/.cc-switch"
    );
    let name = backup_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert!(
        name.starts_with("codex-auth-") && name.ends_with(".json"),
        "文件名要能让人一眼看出这是什么、什么时候备的，实际是 {name}"
    );

    match original_test_home {
        Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
        None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
    }
}

/// 没有 `auth.json` 是正常状态（从没登录过 ChatGPT），**不是错误**。
///
/// 判成错误的后果：整条「切回官方登录」在这类用户身上直接失败，
/// 而他们恰恰是最该能用它的人（想清掉 LoongPort 写的路由、自己去登录）。
#[test]
#[serial_test::serial]
fn missing_auth_json_is_not_an_error() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let original_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
    std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

    let absent = temp.path().join("auth.json");
    assert!(!absent.exists(), "前提：这个文件本来就不存在");

    assert!(
        backup_codex_auth(&absent)
            .expect("不存在不该报错")
            .is_none(),
        "没有源文件时返回 None（表示「没什么可备份」），而不是 Err"
    );
    assert!(
        !temp.path().join(".loongport").join("backups").exists(),
        "没东西要备份时不该顺手建出一个空的 backups 目录"
    );

    match original_test_home {
        Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
        None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
    }
}

/// ⭐ **命令层必须真的调 `refresh_live_for_current_tiers`** —— 两处都不能漏。
///
/// ## 为什么这条测试读源码而不是调函数
///
/// provision 入口仍吃 `&tauri::AppHandle`；reset 的数据库与协调器路径已经下沉到
/// `reset_tier_config_in_state` 并由真实行为测试覆盖，但“当前项刷新 live 文件”会触碰
/// 用户配置，单元测试不能安全执行。第二路 review 实测证明了这条接线盲区的代价：
/// 把那两处调用注释掉，2578 条测试**全绿**——
/// 那条集成测试（`loongport_codex_live.rs`）自己调服务层，所以它测的是服务层，
/// 不是「命令层有没有调服务层」。
///
/// 源码断言是这里唯一能把那一步钉住的手段（与仓里 `vendorSwitchGuardContract`
/// 那条同一个理由与形态）。它守的不是实现细节，而是**这条链路还接着吗** ——
/// 断了的症状是静默的：界面提示刷新成功，而 CLI 一直用旧密钥。
#[test]
fn refresh_live_for_current_tiers_is_wired_into_both_commands() {
    // 两条路各在各的领域模块里（provision 管线 / 行维护），各扫各的文件。
    let src = include_str!("provision.rs");

    // 取 `refresh_relay_provision` 到 `prune_stale_tiers` 调用之间那段（provision 那条路）。
    let provision = {
        let start = src
            .find("async fn refresh_relay_provision")
            .expect("refresh_relay_provision 还在吗");
        let end = src[start..]
            .find("let removed = prune_stale_tiers")
            .expect("provision 末尾那段清理还在吗");
        &src[start..start + end]
    };
    assert!(
        provision.contains("refresh_live_for_current_tiers(state, &refresh_live)"),
        "⭐ provision 链路不再刷新当前档位的 live config —— \
             sk 被撤销重建后，CLI 会一直用旧密钥，而用户点不动那个档位（UI 认为它已是当前项）"
    );

    // 取真正执行重置的 state helper 那段（在 rows.rs）。
    let rows_src = include_str!("rows.rs");
    let reset = {
        let start = rows_src
            .find("fn reset_tier_config_in_state")
            .expect("reset_tier_config_in_state 还在吗");
        let end = rows_src[start..]
            .find("\n/// 保存中转站行的手工顺序")
            .expect("reset 之后那个命令还在吗");
        &rows_src[start..start + end]
    };
    assert!(
        reset.contains("refresh_live_for_current_tiers("),
        "⭐ `reset_tier_config_impl` 不再刷新 live config —— \
             那会让「恢复默认配置」这个按钮对当前项**整体无效**（改坏的配置就在 live 文件里）"
    );
}

/// ⭐ **默认路径下，删账号不许毁掉「别的平台」正在用的档位** —— 前端那道判据挡不住这一类。
///
/// 这是 review 抓出的缺陷现场，复现路径：
///
/// 1. `list_relays_impl` 吃 `app_type` ⇒ `RelayRow.tiers` 只含**当前 tab** 的档位；
/// 2. 如果删除资格只按当前 tab 的档位判断，claude tab 可能看不到 codex 的当前项；
/// 3. 而这个账号在 **codex** 下的档位正是 codex 的当前项 ⇒ 删下去把它清了，
///    `~/.codex/config.toml` 却还指着它。
///
/// 所以闸必须在后端、必须扫全部 app。**会红的改法**：把
/// `apps_using_this_accounts_tiers` 从只扫 `AppType::all()` 改成只扫某一个 app。
#[test]
fn removing_an_account_is_refused_while_another_app_still_uses_its_tier() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id = with_conn(&state, |conn| {
        creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
    })
    .expect("save site");

    // 登录这一行 —— **必须有 `account_id`**：没有它的行派生不出 provider id、
    // 名下不可能有档位，守卫对那种行有意不拦（见
    // `an_untagged_row_is_not_blocked_by_another_accounts_current_tier`）。
    let row_id = with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "me@example.com",
                login_identifier: "me@example.com",
            },
            "tok",
            None,
            None,
            creds::SessionEnvironment::default(),
        )
    })
    .expect("save credentials");

    // 这个账号在 codex 下的档位，且**它就是 codex 的当前项**。
    let codex_tier = provision::provider_id_for(site, Some(7), 1);
    db.save_provider(
        "codex",
        &seeded_owned(&codex_tier, "BestApi · Pro", Some(site), 7),
    )
    .expect("seed codex tier");
    db.set_current_provider("codex", &codex_tier)
        .expect("set codex current");

    // 用户此刻停在 claude tab 上（那边这一行没有当前项）—— 前端会放行，后端必须拦。
    let err = remove_site_impl(&state, row_id, false, None)
        .expect_err("⭐ 名下有档位是别的平台的当前项时，删除必须失败");
    let msg = err.to_string();
    assert!(
        msg.contains("codex"),
        "文案必须点名是哪个平台 —— 用户要去那里切走，实际：{msg}"
    );
    assert!(
        msg.contains("BestApi · Pro"),
        "文案必须点名是哪个档位，实际：{msg}"
    );

    // 全有或全无：拦下之后**一条都不能少**，账号行也必须还在。
    assert!(
        db.get_provider_by_id(&codex_tier, "codex")
            .expect("query")
            .is_some(),
        "被拦下时那条档位必须完好 —— 半删会留下用户处置不了的孤儿记录"
    );
    assert!(
        with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("query row")
            .is_some(),
        "档位没删掉，账号行也不该删"
    );
}

/// 反面：没有任何平台在用它时，删除照常进行（连带清掉档位）。
///
/// 这条与上一条成对 —— 只有上一条的话，把闸写成「无条件拒绝」也能过。
#[test]
fn removing_an_account_still_works_when_no_app_uses_its_tiers() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id = with_conn(&state, |conn| {
        creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
    })
    .expect("save site");

    let tier = provision::provider_id_for(site, None, 1);
    db.save_provider("codex", &seeded(&tier, "BestApi · Pro", Some(site)))
        .expect("seed");
    // **不设 current** —— 别的 provider 是当前项，或压根没有当前项。

    remove_site_impl(&state, row_id, false, None).expect("没人在用它时删除该成功");

    assert!(
        db.get_provider_by_id(&tier, "codex")
            .expect("query")
            .is_none(),
        "档位该被连带清掉"
    );
    assert!(
        with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("query row")
            .is_none(),
        "账号行该被删掉"
    );
}

/// 第三条出路：用户在前端弹窗里看着「Codex 正在用 xxx」按了确认（`force`）⇒
/// 连在用的档位一起删干净。
///
/// 这条与第一条成对 —— 没有它的话，把闸写成「在用就无条件拒绝（连 force 也拦）」
/// 也能过前两条。钉住的语义：
///
/// - force 放行后**全有或全无**：档位、账号行都得没了（不留孤儿记录）；
/// - 被删的是 codex 的**当前项** —— 这条测试的内存库里**没有官方 seed**
///   （`init_default_official_providers` 只在真应用启动时跑），切官方必然失败
///   ⇒ 它同时钉住降级路径：**安置失败不阻断删除**，current 悬空自愈。
///   （正向「切回官方」那条没法单测 —— `ProviderService::switch` 会写真实
///   live 配置文件，codex/claude 没有测试沙箱；映射由
///   `official_seed_id_maps_text_apps_and_denies_codex_image` 把守。）
#[test]
fn forced_removal_deletes_even_while_another_app_uses_its_tier() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id = with_conn(&state, |conn| {
        creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
    })
    .expect("save site");

    let row_id = with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "me@example.com",
                login_identifier: "me@example.com",
            },
            "tok",
            None,
            None,
            creds::SessionEnvironment::default(),
        )
    })
    .expect("save credentials");

    let codex_tier = provision::provider_id_for(site, Some(7), 1);
    db.save_provider(
        "codex",
        &seeded_owned(&codex_tier, "BestApi · Pro", Some(site), 7),
    )
    .expect("seed codex tier");
    db.set_current_provider("codex", &codex_tier)
        .expect("set codex current");

    remove_site_impl(&state, row_id, true, None).expect("force 是用户知情后的选择，该放行");

    assert!(
        db.get_provider_by_id(&codex_tier, "codex")
            .expect("query")
            .is_none(),
        "force 该连当前项档位一起删掉"
    );
    assert!(
        with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("query row")
            .is_none(),
        "账号行该被删掉"
    );
}

/// 闸的归属判据必须与 `prune_stale_tiers` 是**同一份** —— 否则守卫与删除各认一套：
/// 守卫说「这条不是你的、不拦」，删除说「这条是你的、删了」⇒ 恰好绕过守卫。
///
/// 这条钉的是「别人的当前项不该拦住我」这一半（宽松方向的误判）。
#[test]
fn the_guard_ignores_another_accounts_current_tier_on_the_same_site() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

    // 账号 9 的档位是 codex 的当前项。
    let b_tier = provision::provider_id_for(site, Some(9), 1);
    db.save_provider("codex", &seeded_owned(&b_tier, "B 的档位", Some(site), 9))
        .expect("seed");
    db.set_current_provider("codex", &b_tier)
        .expect("set current");

    let state = AppState::new(db.clone());

    // 以账号 7 的身份问「我名下有在用的吗」—— 答案必须是「没有」。
    assert!(
        apps_using_this_accounts_tiers(&state, site, Some(7)).is_empty(),
        "同站另一个账号的当前项不该拦住我删自己的账号"
    );
    // 而账号 9 自己问，必须撞上。
    assert_eq!(
        apps_using_this_accounts_tiers(&state, site, Some(9)).len(),
        1,
        "账号 9 名下那条正是当前项，必须被认出来"
    );
}

/// ⭐ **还没登录的行（`account_id` 为 `None`）不该被别人的档位拦住**。
///
/// 第二路 review 抓出的：`belongs_to_account` 对 `None` 返回 `true`（"不按账号过滤"），
/// 那对**删除**方向是对的（同站没记归属的旧档位该跟着清），但守卫方向反过来就成了
/// 「把别人正在用的档位算成你的」。
///
/// 这种行真实可达：`clear_credentials` 会把 `account_id` 置 `NULL`（站点换了后端
/// 协议时走这条），而唯一索引把 `NULL` 视为互不相等 ⇒ 它与已登录的行并存。
/// 症状是用户删一个**空行**时被告知「你名下还有档位正在使用中：B 的档位（codex）」，
/// 而唯一出路是去 codex 把 B 切走。
///
/// 会红的改法：去掉 `apps_using_this_accounts_tiers` 里那个 `account_id.is_some()`。
#[test]
fn an_untagged_row_is_not_blocked_by_another_accounts_current_tier() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

    // 账号 9 的档位是 codex 的当前项。
    let b_tier = provision::provider_id_for(site, Some(9), 1);
    db.save_provider("codex", &seeded_owned(&b_tier, "B 的档位", Some(site), 9))
        .expect("seed");
    db.set_current_provider("codex", &b_tier)
        .expect("set current");

    let state = AppState::new(db.clone());

    assert!(
        apps_using_this_accounts_tiers(&state, site, None).is_empty(),
        "⭐ 还没登录的行认不出归属 ⇒ 不该拦。它派生不出 provider id，\
             名下本来就不可能有档位，漏拦没有代价；而误拦会让用户删不掉一个空行"
    );

    // 而删除方向的语义不变：`prune_stale_tiers` 传 `None` 时仍会清同站没记归属的档位。
    // 这条只是确认上面那个改动没顺手改掉 `belongs_to_account` 本身。
    let legacy = provision::provider_id_for(site, None, 5);
    db.save_provider("codex", &seeded(&legacy, "旧数据", Some(site)))
        .expect("seed legacy");
    let legacy_provider = db
        .get_provider_by_id(&legacy, "codex")
        .expect("query")
        .expect("在");
    assert!(
        belongs_to_account(&legacy_provider, site, None),
        "删除方向对 `None` 仍是「算是我的」—— 那是旧数据能被清掉的前提"
    );
}

/// ⭐ **还没登录的 relay 行，不能把同站别人账号的档位记到自己头上。**
///
/// Task 3 review 抓出的：把 `relay_balance_inputs` 的内联判据收敛到
/// `belongs_to_account` 时，`(relay.account_id, 档位账号)` 的 `(None, Some)` 那格
/// 从「不认」翻成了「认」—— 未登录行会收走别人档位的 sk、对账会把别人的成本
/// 算进这一行。现在余额 / 对账走严格版 [`belongs_to_relay`]（还原原内联语义
/// `(None, Some(_)) => false`），清理 / 守卫路径仍走宽松版 [`belongs_to_account`]。
///
/// 会红的改法：`relay_balance_inputs` 改回 `belongs_to_account`。
#[test]
fn an_unlogged_relay_row_is_not_attributed_another_accounts_tier() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

    // 账号 9 的档位，带真实 sk（否则「没收走」的断言没有判别力）。
    let b_tier = provision::provider_id_for(site, Some(9), 1);
    let mut b_provider = seeded_owned(&b_tier, "B 的档位", Some(site), 9);
    b_provider.settings_config = serde_json::json!({ "auth": { "OPENAI_API_KEY": "sk-b" } });
    db.save_provider("codex", &b_provider).expect("seed B");

    // 同站一条没记账号的旧档位（升级前生成），也没有 sk —— 只用于钉住
    // `(None, None) => true` 这格没被顺手改掉。
    let legacy = provision::provider_id_for(site, None, 5);
    db.save_provider("codex", &seeded(&legacy, "旧数据", Some(site)))
        .expect("seed legacy");

    let state = AppState::new(db.clone());
    let mut unlogged = purchase_capability_relay(creds::BackendKind::Sub2Api);
    unlogged.site_origin = site.to_string();
    unlogged.account_id = None;

    let (_, keys) = relay_balance_inputs(&state, &unlogged);
    assert!(
        keys.is_empty(),
        "⭐ 未登录的行认不出归属 ⇒ 同站别人账号的档位（哪怕有 sk）不该被收走：{keys:?}"
    );

    // 对照组：账号 9 自己的行必须能拿到那把 sk —— 证明上面不是「本来就收不到」。
    let mut owner_row = unlogged.clone();
    owner_row.account_id = Some(9);
    let (_, keys) = relay_balance_inputs(&state, &owner_row);
    assert_eq!(
        keys,
        vec!["sk-b".to_string()],
        "档位自己的账号必须收得到 sk"
    );

    // 两个判据函数在关键那格的分歧是**有意的**，钉住防止将来被「顺手统一」：
    // 删除方向（belongs_to_account）对 None 宽松（旧数据要能清），
    // 归属方向（belongs_to_relay）对 None 严格（别人的不能认领）。
    let b_in_db = db
        .get_provider_by_id(&b_tier, "codex")
        .expect("query")
        .expect("在");
    assert!(
        belongs_to_account(&b_in_db, site, None),
        "删除方向对 `None` 仍宽松 —— 别改"
    );
    assert!(
        !belongs_to_relay(&b_in_db, site, None),
        "归属方向对 `(None, Some)` 必须严格 —— 别人的档位不能记到未登录行头上"
    );
    let legacy_in_db = db
        .get_provider_by_id(&legacy, "codex")
        .expect("query")
        .expect("在");
    assert!(
        belongs_to_relay(&legacy_in_db, site, None),
        "同站没记归属的旧档位仍是「可能是我的」（(None, None) => true）"
    );
}

/// ⭐ **登录态失效之后，那一行仍然带着它的档位、昵称和「已过期」这个状态。**
///
/// 修之前 `check_session` 走的是 `clear_credentials`，它把 `account_id` 一起抹掉，
/// 于是三件事同时静默出错（都不报任何错）：
///
/// 1. `tiers_of_site` 对「行没有 account_id、档位有」判为不属于它
///    ⇒ **返回空 tiers**，界面退化成「没有可用分组 + 获取密钥」；
/// 2. `session_expired()` 要求 `account_id.is_some()` ⇒ 变成 `false`
///    ⇒ 界面说「还没登录」，而用户明明登录过；
/// 3. `account_label` 被清空 ⇒ 昵称没了。
///
/// 而 sk 一把都没失效。用户看到的是「密钥没了」，然后去重建一遍。
///
/// 会红的改法：把 `check_session` 里的 `clear_session` 换回 `clear_credentials`。
#[test]
fn an_expired_session_keeps_its_tiers_label_and_usable_status() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());

    let row_id = with_conn(&state, |conn| {
        creds::save_site(conn, site, "BestAPI", "https://bestapi.store")
    })
    .expect("save site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "我的号",
                login_identifier: "me@x.com",
            },
            "tok",
            None,
            Some(1),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("save credentials");

    let tier_id = provision::provider_id_for(site, Some(7), 1);
    let settings_config = provision::settings_config_for(
        &AppType::Codex,
        "sk-valid",
        "Pro池",
        "https://bestapi.store/v1",
        "gpt-5.6-sol",
    )
    .expect("settings");
    db.save_provider(
        "codex",
        &Provider {
            settings_config,
            ..seeded_owned(&tier_id, "Pro池", Some(site), 7)
        },
    )
    .expect("seed tier");

    with_conn(&state, |conn| creds::clear_session(conn, row_id)).expect("clear session");

    let rows = list_relays_impl(&state, AppType::Codex).expect("list relays");
    let row = rows.iter().find(|r| r.id == row_id).expect("行还在");

    assert!(
        matches!(row.status, RelayRowStatus::SessionExpiredUsable),
        "登录过 + 没 token + 没 refresh ⇒ 必须报「登录已过期」，而不是「还没登录」"
    );
    assert_eq!(row.account_label, "我的号", "昵称不该跟着会话一起没");
    assert_eq!(
        row.tiers.len(),
        1,
        "⭐ 分组与 sk 与网页登录态无关，不该从界面消失"
    );
    assert_eq!(row.tiers[0].provider_id, tier_id);
}

#[test]
fn a_relay_with_a_managed_key_can_query_balance_without_a_session() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id =
        with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
    let provider_id = provision::provider_id_for(site, None, 1);
    let settings = provision::settings_config_for(
        &AppType::Codex,
        "sk-test",
        "Pro池",
        "https://bestapi.store/v1",
        "gpt-5.6-sol",
    )
    .expect("settings");
    db.save_provider(
        "codex",
        &Provider {
            id: provider_id,
            name: "Pro池".into(),
            settings_config: settings,
            website_url: Some(site.into()),
            category: Some("aggregator".into()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        },
    )
    .expect("provider");

    let row = list_relays_impl(&state, AppType::Codex)
        .expect("list")
        .into_iter()
        .find(|row| row.id == row_id)
        .expect("row");
    assert!(matches!(row.status, RelayRowStatus::NotLoggedIn));
    assert!(row.can_query_balance);
    assert!(!row.can_refresh);
    assert!(
        relay_refresh_targets(&state, &AppType::Codex)
            .expect("refresh targets")
            .iter()
            .any(|(id, _, can_refresh)| *id == row_id && !can_refresh),
        "顶部全量刷新也要包含只能用 SK 查余额的账号"
    );
}

#[test]
fn a_refreshable_session_is_not_reported_as_not_logged_in() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db);
    let row_id =
        with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "我的号",
                login_identifier: "me@x.com",
            },
            "expired-token",
            Some("refresh-token"),
            Some(1),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("credentials");

    let row = list_relays_impl(&state, AppType::Codex)
        .expect("list")
        .into_iter()
        .find(|row| row.id == row_id)
        .expect("row");

    assert!(
        !matches!(row.status, RelayRowStatus::NotLoggedIn),
        "refresh token 可自动续期时，后端不能要求用户重新登录"
    );
    assert!(row.can_refresh);
}

#[test]
fn session_expired_usable_requires_an_extractable_managed_key() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id =
        with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "我的号",
                login_identifier: "me@x.com",
            },
            "token",
            None,
            Some(1),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("credentials");

    let tier_id = provision::provider_id_for(site, Some(7), 1);
    db.save_provider(
        "codex",
        &Provider {
            id: tier_id,
            name: "坏配置".into(),
            settings_config: serde_json::json!({}),
            website_url: Some(site.into()),
            category: Some("aggregator".into()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: Some(managed_meta(&AppType::Codex, Some(7), None)),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        },
    )
    .expect("provider");
    with_conn(&state, |conn| creds::clear_session(conn, row_id)).expect("clear session");

    let row = list_relays_impl(&state, AppType::Codex)
        .expect("list")
        .into_iter()
        .find(|row| row.id == row_id)
        .expect("row");

    assert!(matches!(row.status, RelayRowStatus::SessionExpired));
    assert!(!row.can_query_balance);
}

/// ⭐ **倍率必须活过 provision → 库 → `listRelays` 这一整条**。
///
/// 它是这次改动的核心：倍率从「每次渲染现拉」改成「provision 写一次、之后只读本地」。
/// 链路上任何一环断掉，症状都是**界面永远显示「倍率未知」**，而没有报错 ——
/// 只有这条端到端的断言守得住。
///
/// 会红的改法：`persist_provision_batch` 里不写 `set_tier_rate_multiplier`，
/// 或 `list_tiers_impl` 把 `rate_multiplier` 改回写死 `None`。
#[test]
fn a_provisioned_rate_survives_into_list_relays() {
    let site = "https://bestapi.store";
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());

    let row_id =
        with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "我的号",
                login_identifier: "me@x.com",
            },
            "tok",
            None,
            Some(i64::MAX),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("credentials");
    let op = with_conn(&state, |conn| creds::get(conn, row_id))
        .expect("load")
        .expect("exists");

    let provider_id = provision::provider_id_for(site, Some(7), 1);
    let batch = ManagedProvisionBatch {
        account_id: Some(7),
        site_declaration: None,
        candidates: vec![ManagedProvisionCandidate {
            provider_id: provider_id.clone(),
            app_type: AppType::Codex,
            group_id: "1".into(),
            group_name: "Pro池".into(),
            rate_multiplier: Some(0.15),
            api_key: "sk-test".into(),
            model: "gpt-5.6-sol".into(),
            models: None,
            roles: None,
            allow_image_generation: Some(false),
            api_base_url: site.into(),
        }],
        observed_keep: Default::default(),
        failures: Vec::new(),
        keys_created: 0,
    };
    persist_provision_batch(&state, &op, batch).expect("persist");

    let rows = list_relays_impl(&state, AppType::Codex).expect("list relays");
    let tier = rows
        .iter()
        .find(|r| r.id == row_id)
        .expect("行在")
        .tiers
        .first()
        .expect("档位在");
    assert_eq!(
        tier.rate_multiplier,
        Some(0.15),
        "⭐ 倍率必须从本地库读回来 —— 它不再靠任何网络请求补齐"
    );
}

/// 「恢复内置默认」把应用过声明的档位退回去：settings 重建为
/// `settings_config_for` 形状（sk/端点/模型保留）、标注清空。
/// 与 `first_import_applies_site_declaration_segment` 构成一对往返。
#[test]
fn reset_site_config_restores_builtin_defaults() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let site = "https://api.example.com";

    // 直接造一条「应用过声明」的托管档位（不跑 persist，那段已有专测）。
    let provider_id = provision::provider_id_for(site, Some(7), 1);
    let mut settings = provision::settings_config_for(
        &AppType::Codex,
        "sk-test",
        "Example·Pro池",
        "https://api.example.com/v1",
        "gpt-5.6-sol",
    )
    .expect("defaults");
    let declared = crate::relay::site_config::parse_site_config(
            r#"{
                "schema_version": 1,
                "site_origin": "https://api.example.com",
                "platforms": { "openai": { "model": "gpt-5.6-codex", "model_reasoning_effort": "minimal" } }
            }"#,
        )
        .expect("declaration");
    crate::relay::site_config::apply_segment_to_app(
        &AppType::Codex,
        declared
            .segment_for(platform_map::Platform::OpenAI)
            .unwrap(),
        &mut settings,
    )
    .expect("apply");
    let provider = crate::provider::Provider {
        id: provider_id.clone(),
        name: "Example·Pro池".into(),
        settings_config: settings,
        website_url: Some(site.into()),
        category: Some("aggregator".into()),
        created_at: Some(chrono::Utc::now().timestamp_millis()),
        sort_index: Some(0),
        notes: None,
        meta: Some(crate::provider::ProviderMeta {
            site_declared_origin: Some(site.into()),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    state.db.save_provider("codex", &provider).expect("save");

    // 重建要素提取 + 重建（command 体内联逻辑的等价直调，不起 tauri runtime）。
    let (api_key, base_url, model) =
        rebuild_inputs_from_settings(&AppType::Codex, &provider.settings_config)
            .expect("rebuild inputs");
    assert_eq!(api_key, "sk-test");
    assert_eq!(base_url, "https://api.example.com/v1");
    // 模型提取自声明覆盖后的值——重建以现状为基线，不回滚站长的模型选择
    assert_eq!(model, "gpt-5.6-codex");
    let defaults = provision::settings_config_for(
        &AppType::Codex,
        &api_key,
        "Example·Pro池",
        &base_url,
        &model,
    )
    .expect("rebuild");
    let toml_value: toml::Value = toml::from_str(defaults["config"].as_str().unwrap()).unwrap();
    // 声明的参数键已退掉（reasoning 回到内置 high），sk/端点保留
    assert_eq!(toml_value["model_reasoning_effort"].as_str(), Some("high"));
    assert_eq!(toml_value["model"].as_str(), Some("gpt-5.6-codex"));
    assert_eq!(
        toml_value["model_providers"]["custom"]["base_url"].as_str(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(defaults["auth"]["OPENAI_API_KEY"], "sk-test");
}

/// 站点声明（relay/site_config.rs）随首次导入自动应用：段覆盖内置默认的调用
/// 参数、deny 键进不来、meta 落「站点推荐配置」标注。spec 的 M2 硬门槛。
#[test]
fn first_import_applies_site_declaration_segment() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let site = "https://api.example.com";
    let row_id = with_conn(&state, |conn| {
        creds::save_site_with_backend(conn, site, "Example", site, discovery::BackendKind::Sub2Api)
    })
    .expect("save site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "我的号",
                login_identifier: "me@x.com",
            },
            "tok",
            None,
            Some(i64::MAX),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("credentials");
    let op = with_conn(&state, |conn| creds::get(conn, row_id))
        .expect("load")
        .expect("exists");

    let declaration = crate::relay::site_config::parse_site_config(
        r#"{
                "schema_version": 1,
                "site_origin": "https://api.example.com",
                "platforms": {
                    "openai": {
                        "model": "gpt-5.6-codex",
                        "model_reasoning_effort": "minimal",
                        "model_context_window": 272000,
                        "mcp_servers": { "evil": {} }
                    }
                }
            }"#,
    )
    .expect("declaration");

    let provider_id = provision::provider_id_for(site, Some(7), 1);
    let batch = ManagedProvisionBatch {
        account_id: Some(7),
        site_declaration: Some(declaration),
        candidates: vec![ManagedProvisionCandidate {
            provider_id: provider_id.clone(),
            app_type: AppType::Codex,
            group_id: "1".into(),
            group_name: "Pro池".into(),
            rate_multiplier: Some(0.15),
            api_key: "sk-test".into(),
            model: "gpt-5.6-sol".into(),
            models: None,
            roles: None,
            allow_image_generation: Some(false),
            api_base_url: site.into(),
        }],
        observed_keep: Default::default(),
        failures: Vec::new(),
        keys_created: 0,
    };
    persist_provision_batch(&state, &op, batch).expect("persist");

    let provider = state
        .db
        .get_provider_by_id(&provider_id, "codex")
        .expect("read")
        .expect("provider exists");
    let config_text = provider.settings_config["config"].as_str().expect("toml");
    let parsed: toml::Value = toml::from_str(config_text).expect("valid toml");
    // 站长声明优先：模型与推理档位都来自声明段
    assert_eq!(parsed["model"].as_str(), Some("gpt-5.6-codex"));
    assert_eq!(
        parsed["model_reasoning_effort"].as_str(),
        Some("minimal"),
        "声明段覆盖内置写死的 high"
    );
    assert_eq!(parsed["model_context_window"].as_integer(), Some(272000));
    // deny 键进不来；端点与 sk 保持建档值
    assert!(parsed.get("mcp_servers").is_none());
    assert_eq!(
        parsed["model_providers"]["custom"]["base_url"].as_str(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(
        provider.settings_config["auth"]["OPENAI_API_KEY"],
        "sk-test"
    );
    // 来源标注（UI 的「站点推荐配置」徽标 + 回退入口数据）
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|m| m.site_declared_origin.as_deref()),
        Some("https://api.example.com")
    );
}

#[test]
fn session_probe_clears_only_confirmed_auth_failures() {
    assert!(should_clear_credentials_after_probe_error(
        &AppError::Config(
            "newapi self 失败: 登录态已失效（HTTP 401），请重新登录中转站账号".into()
        )
    ));
    assert!(should_clear_credentials_after_probe_error(
        &AppError::Config("登录已过期，请重新登录".into())
    ));
    assert!(!should_clear_credentials_after_probe_error(
        &AppError::Config("newapi self 请求失败: HTTP 500".into())
    ));
    assert!(!should_clear_credentials_after_probe_error(
        &AppError::Config("newapi self 请求失败: 连不上服务器（boom）".into())
    ));
}

#[test]
fn persisting_a_newapi_refresh_updates_rotated_cookie_and_account_identity() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id = with_conn(&state, |conn| {
        creds::save_site_with_backend(
            conn,
            "https://newapi.example",
            "NewAPI",
            "https://newapi.example",
            discovery::BackendKind::NewApi,
        )
    })
    .expect("save site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "Old Label",
                login_identifier: "old-login",
            },
            "stale-access",
            Some("old-refresh"),
            Some(1),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("save credentials");

    let current = with_conn(&state, |conn| creds::get(conn, row_id))
        .expect("load relay")
        .expect("relay exists");
    let renewed = persist_refreshed_session(
        &state,
        &current,
        &backend::RefreshedSession {
            auth_token: "new-access".into(),
            refresh_credential: Some("rotated-refresh".into()),
            token_expires_at: Some(1_900_000_000),
            account: Some(backend::RuntimeAccount {
                id: 7,
                label: "NewAPI Display".into(),
                login_identifier: "newapi-login".into(),
            }),
        },
    )
    .expect("persist refresh");

    assert_eq!(renewed.auth_token, "new-access");
    assert_eq!(renewed.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(renewed.account_label, "NewAPI Display");
    assert_eq!(renewed.login_identifier, "newapi-login");

    let persisted = with_conn(&state, |conn| creds::get(conn, row_id))
        .expect("reload relay")
        .expect("relay exists");
    assert_eq!(persisted.auth_token, "new-access");
    assert_eq!(persisted.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(persisted.token_expires_at, Some(1_900_000_000));
    assert_eq!(persisted.account_label, "NewAPI Display");
    assert_eq!(persisted.login_identifier, "newapi-login");
}

#[test]
fn identity_refresh_failure_keeps_a_refreshed_session_usable() {
    let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db.clone());
    let row_id = with_conn(&state, |conn| {
        creds::save_site_with_backend(
            conn,
            "https://newapi.example",
            "NewAPI",
            "https://newapi.example",
            discovery::BackendKind::NewApi,
        )
    })
    .expect("save site");
    with_conn(&state, |conn| {
        creds::save_credentials(
            conn,
            row_id,
            creds::AccountIdentity {
                id: 7,
                label: "Old Label",
                login_identifier: "old-login",
            },
            "stale-access",
            Some("old-refresh"),
            Some(1),
            creds::SessionEnvironment::default(),
        )
    })
    .expect("save credentials");

    let current = with_conn(&state, |conn| creds::get(conn, row_id))
        .expect("load relay")
        .expect("relay exists");
    let renewed = persist_refreshed_session_with_identity_writer(
        &state,
        &current,
        &backend::RefreshedSession {
            auth_token: "new-access".into(),
            refresh_credential: Some("rotated-refresh".into()),
            token_expires_at: Some(1_900_000_000),
            account: Some(backend::RuntimeAccount {
                id: 7,
                label: "NewAPI Display".into(),
                login_identifier: "newapi-login".into(),
            }),
        },
        |_state, _relay_id, _account| Err(AppError::Database("identity write failed".into())),
    )
    .expect("token refresh should stay usable");

    assert_eq!(renewed.auth_token, "new-access");
    assert_eq!(renewed.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(renewed.account_label, "Old Label");
    assert_eq!(renewed.login_identifier, "old-login");

    let persisted = with_conn(&state, |conn| creds::get(conn, row_id))
        .expect("reload relay")
        .expect("relay exists");
    assert_eq!(persisted.auth_token, "new-access");
    assert_eq!(persisted.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(persisted.token_expires_at, Some(1_900_000_000));
    assert_eq!(persisted.account_label, "Old Label");
    assert_eq!(persisted.login_identifier, "old-login");
}

#[tokio::test]
async fn pricing_refresh_skips_fresh_rows_and_continues_after_one_failure() {
    let mut fresh = test_newapi_relay(1);
    fresh.pricing_synced_at = Some(100);
    let failed = test_newapi_relay(2);
    let succeeded = test_newapi_relay(3);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let attempts_for_refresh = Arc::clone(&attempts);

    let summary = refresh_due_relay_pricing_rows(
        vec![fresh, failed, succeeded],
        159,
        std::time::Duration::from_secs(60),
        move |relay| {
            let attempts = Arc::clone(&attempts_for_refresh);
            async move {
                attempts.lock().unwrap().push(relay.id);
                if relay.id == 2 {
                    Err(AppError::Message("expected failure".into()))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await;

    assert_eq!(summary.attempted, 2);
    assert_eq!(summary.succeeded, 1);
    assert_eq!(summary.failed.len(), 1);
    assert_eq!(summary.failed[0].0, 2);
    let mut attempted_ids = attempts.lock().unwrap().clone();
    attempted_ids.sort_unstable();
    assert_eq!(attempted_ids, vec![2, 3]);
}

fn pricing_timestamp_state(initial: Option<i64>) -> (AppState, i64) {
    let db = Arc::new(crate::database::Database::memory().expect("init db"));
    let state = AppState::new(db);
    let relay_id = with_conn(&state, |conn| {
        creds::save_site_with_backend(
            conn,
            "https://pricing.example",
            "Pricing",
            "https://pricing.example/v1",
            discovery::BackendKind::Sub2Api,
        )
    })
    .unwrap();
    if let Some(initial) = initial {
        with_conn(&state, |conn| {
            creds::mark_pricing_synced(conn, relay_id, initial)
        })
        .unwrap();
    }
    (state, relay_id)
}

#[test]
fn successful_full_refresh_marks_pricing_fresh() {
    let (state, relay_id) = pricing_timestamp_state(None);

    mark_pricing_after_success(&state, relay_id, 456, Ok(())).unwrap();

    let relay = with_conn(&state, |conn| creds::get(conn, relay_id))
        .unwrap()
        .unwrap();
    assert_eq!(relay.pricing_synced_at, Some(456));
}

#[test]
fn failed_full_refresh_keeps_the_previous_pricing_time() {
    let (state, relay_id) = pricing_timestamp_state(Some(123));

    let result: Result<(), AppError> = mark_pricing_after_success(
        &state,
        relay_id,
        456,
        Err(AppError::Message("expected failure".into())),
    );

    assert!(result.is_err());
    let relay = with_conn(&state, |conn| creds::get(conn, relay_id))
        .unwrap()
        .unwrap();
    assert_eq!(relay.pricing_synced_at, Some(123));
}
