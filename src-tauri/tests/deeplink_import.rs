use std::sync::Arc;

use cc_switch_lib::{import_provider_from_deeplink, parse_deeplink_url, AppState, Database};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, reset_test_fs, test_mutex};

#[test]
fn deeplink_import_claude_provider_persists_to_db() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let url = "loongport://v1/import?resource=provider&app=claude&name=DeepLink%20Claude&homepage=https%3A%2F%2Fexample.com&endpoint=https%3A%2F%2Fapi.example.com%2Fv1&apiKey=sk-test-claude-key&model=claude-sonnet-4&icon=claude";
    let request = parse_deeplink_url(url).expect("parse deeplink url");

    let db = Arc::new(Database::memory().expect("create memory db"));
    let state = AppState::new(db.clone());

    let provider_id = import_provider_from_deeplink(&state, request.clone())
        .expect("import provider from deeplink");

    // Verify DB state
    let providers = db.get_all_providers("claude").expect("get providers");
    let provider = providers
        .get(&provider_id)
        .expect("provider created via deeplink");

    assert_eq!(provider.name, request.name.clone().unwrap());
    assert_eq!(provider.website_url.as_deref(), request.homepage.as_deref());
    assert_eq!(provider.icon.as_deref(), Some("claude"));
    let auth_token = provider
        .settings_config
        .pointer("/env/ANTHROPIC_AUTH_TOKEN")
        .and_then(|v| v.as_str());
    let base_url = provider
        .settings_config
        .pointer("/env/ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str());
    assert_eq!(auth_token, request.api_key.as_deref());
    assert_eq!(base_url, request.endpoint.as_deref());
}

#[test]
fn deeplink_import_codex_provider_builds_auth_and_config() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let url = "loongport://v1/import?resource=provider&app=codex&name=DeepLink%20Codex&homepage=https%3A%2F%2Fopenai.example&endpoint=https%3A%2F%2Fapi.openai.example%2Fv1&apiKey=sk-test-codex-key&model=gpt-4o&icon=openai";
    let request = parse_deeplink_url(url).expect("parse deeplink url");

    let db = Arc::new(Database::memory().expect("create memory db"));
    let state = AppState::new(db.clone());

    let provider_id = import_provider_from_deeplink(&state, request.clone())
        .expect("import provider from deeplink");

    let providers = db.get_all_providers("codex").expect("get providers");
    let provider = providers
        .get(&provider_id)
        .expect("provider created via deeplink");

    assert_eq!(provider.name, request.name.clone().unwrap());
    assert_eq!(provider.website_url.as_deref(), request.homepage.as_deref());
    assert_eq!(provider.icon.as_deref(), Some("openai"));
    let auth_value = provider
        .settings_config
        .pointer("/auth/OPENAI_API_KEY")
        .and_then(|v| v.as_str());
    let config_text = provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert_eq!(auth_value, request.api_key.as_deref());
    assert!(
        config_text.contains(request.endpoint.as_deref().unwrap()),
        "config.toml content should contain endpoint"
    );
    assert!(
        config_text.contains("model = \"gpt-4o\""),
        "config.toml content should contain model setting"
    );
}

/// ⭐ **Deep Link 不能造出「伪托管」provider** —— 那会是一条不可见、不可删的记录。
///
/// ## 这条测试守的缺陷（review 抓出）
///
/// provider id 是 `<用户给的名字>-<时间戳>`，而「是不是 LoongPort 托管的」只看 id
/// 前缀 `loongport-`。所以名字取 `loongport` 时生成的 id 恰好命中那个前缀 ⇒
/// 这条**普通** provider 会被：
///
/// - provider 列表按前缀过滤掉（`ProviderList.tsx`）⇒ 看不见；
/// - `update_provider` / `delete_provider` 的 `reject_if_managed` 拦下 ⇒ 删不掉；
/// - 而它没有运营商归属，所以运营商区不显示它、`prune_stale_tiers` 也不清它。
///
/// 合起来是一条**永久留在库里、完全不可管理**的记录。
///
/// ⚠️ 名字 `loongport`（不带连字符）自己并不命中前缀，**是拼上 `-<时间戳>` 之后才命中的**
/// —— 所以判据必须落在拼好的 id 上。会红的改法：把生成端那道检查改成只查名字。
#[test]
fn deeplink_cannot_forge_a_managed_provider_id() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    // 三种都试：不带连字符的、带连字符的、以及大小写混写（id 会被转小写）。
    for name in ["loongport", "loongport-pro", "LoongPort"] {
        let url = format!(
            "loongport://v1/import?resource=provider&app=claude&name={name}\
             &homepage=https%3A%2F%2Fexample.com\
             &endpoint=https%3A%2F%2Fapi.example.com%2Fv1&apiKey=sk-x&model=claude-sonnet-4"
        );
        let request = parse_deeplink_url(&url).expect("parse deeplink url");

        let db = Arc::new(Database::memory().expect("create memory db"));
        let state = AppState::new(db.clone());
        let provider_id =
            import_provider_from_deeplink(&state, request).expect("import should succeed");

        // 前缀字面量：`operator` 模块是 crate 私有的（有意如此，它不是对外 API），
        // 所以这里比对字符串本身。它与 Rust 侧 `MANAGED_ID_PREFIX`、前端
        // `MANAGED_PROVIDER_ID_PREFIX` 三处一致由 `managed.rs` 那条
        // `prefix_matches_the_frontend_copy` 测试钉着。
        assert!(
            !provider_id.starts_with("loongport-"),
            "⭐ 名字 {name:?} 生成的 id {provider_id:?} 命中了托管前缀 —— \
             那条记录会变成不可见且不可删的孤儿"
        );
        // 而且它必须真的被建出来了（不是靠拒绝导入来「避免」这个问题 ——
        // 用户给自己的 provider 起什么名字是他的自由）。
        assert!(
            db.get_provider_by_id(&provider_id, "claude")
                .expect("query")
                .is_some(),
            "provider 仍应正常导入，只是 id 换了个安全的开头"
        );
    }
}
