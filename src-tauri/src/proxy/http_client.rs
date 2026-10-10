//! 全局 HTTP 客户端模块
//!
//! 提供支持全局代理配置的 HTTP 客户端。
//! 所有需要发送 HTTP 请求的模块都应使用此模块提供的客户端。

use once_cell::sync::OnceCell;
use reqwest::Client;
use std::env;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::Duration;

/// 全局 HTTP 客户端实例
static GLOBAL_CLIENT: OnceCell<RwLock<HttpClients>> = OnceCell::new();

// Both policies share the existing proxy/TLS configuration owner and update lock.
#[derive(Clone)]
struct HttpClients {
    ordinary: Client,
    authenticated: Client,
}

fn build_clients(proxy_url: Option<&str>) -> Result<HttpClients, String> {
    Ok(HttpClients {
        ordinary: build_client(proxy_url)?,
        authenticated: build_authenticated_client(proxy_url)?,
    })
}

/// 当前代理 URL（用于日志和状态查询）
static CURRENT_PROXY_URL: OnceCell<RwLock<Option<String>>> = OnceCell::new();

/// CC Switch 代理服务器当前监听的端口
static CC_SWITCH_PROXY_PORT: OnceCell<RwLock<u16>> = OnceCell::new();

/// 设置 CC Switch 代理服务器的监听端口
///
/// 应在代理服务器启动时调用，以便系统代理检测能正确识别自己的端口
pub fn set_proxy_port(port: u16) {
    if let Some(lock) = CC_SWITCH_PROXY_PORT.get() {
        if let Ok(mut current_port) = lock.write() {
            *current_port = port;
            log::debug!("[GlobalProxy] Updated CC Switch proxy port to {port}");
        }
    } else {
        let _ = CC_SWITCH_PROXY_PORT.set(RwLock::new(port));
        log::debug!("[GlobalProxy] Initialized CC Switch proxy port to {port}");
    }
}

/// 获取 CC Switch 代理服务器的监听端口
fn get_proxy_port() -> u16 {
    CC_SWITCH_PROXY_PORT
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|port| *port)
        .unwrap_or(15721) // 默认端口作为回退
}

/// 初始化全局 HTTP 客户端
///
/// 应在应用启动时调用一次。
///
/// # Arguments
/// * `proxy_url` - 代理 URL，如 `http://127.0.0.1:7890` 或 `socks5://127.0.0.1:1080`
///   传入 None 或空字符串表示直连
pub fn init(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    let client = build_clients(effective_url)?;

    // 尝试初始化全局客户端，如果已存在则记录警告并使用 apply_proxy 更新
    if GLOBAL_CLIENT.set(RwLock::new(client.clone())).is_err() {
        log::warn!(
            "[GlobalProxy] [GP-003] Already initialized, updating instead: {}",
            effective_url
                .map(mask_url)
                .unwrap_or_else(|| "direct connection".to_string())
        );
        // 已初始化，改用 apply_proxy 更新
        return apply_proxy(proxy_url);
    }

    // 初始化代理 URL 记录
    let _ = CURRENT_PROXY_URL.set(RwLock::new(effective_url.map(|s| s.to_string())));

    log::info!(
        "[GlobalProxy] Initialized: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 验证代理配置（不应用）
///
/// 只验证代理 URL 是否有效，不实际更新全局客户端。
/// 用于在持久化之前验证配置的有效性。
///
/// # Arguments
/// * `proxy_url` - 代理 URL，None 或空字符串表示直连
///
/// # Returns
/// 验证成功返回 Ok(())，失败返回错误信息
pub fn validate_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    // 只调用 build_client 来验证，但不应用
    build_clients(effective_url)?;
    Ok(())
}

/// 应用代理配置（假设已验证）
///
/// 直接应用代理配置到全局客户端，不做额外验证。
/// 应在 validate_proxy 成功后调用。
///
/// # Arguments
/// * `proxy_url` - 代理 URL，None 或空字符串表示直连
pub fn apply_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    let new_client = build_clients(effective_url)?;

    // 更新客户端
    if let Some(lock) = GLOBAL_CLIENT.get() {
        let mut client = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-001] Failed to acquire write lock: {e}");
            "Failed to update proxy: lock poisoned".to_string()
        })?;
        *client = new_client;
    } else {
        // 如果还没初始化，则初始化
        return init(proxy_url);
    }

    // 更新代理 URL 记录
    if let Some(lock) = CURRENT_PROXY_URL.get() {
        let mut url = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-002] Failed to acquire URL write lock: {e}");
            "Failed to update proxy URL record: lock poisoned".to_string()
        })?;
        *url = effective_url.map(|s| s.to_string());
    }

    log::info!(
        "[GlobalProxy] Applied: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 更新代理配置（热更新）
///
/// 可在运行时调用以更改代理设置，无需重启应用。
/// 注意：此函数同时验证和应用，如果需要先验证后持久化再应用，
/// 请使用 validate_proxy + apply_proxy 组合。
///
/// # Arguments
/// * `proxy_url` - 新的代理 URL，None 或空字符串表示直连
#[allow(dead_code)]
pub fn update_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    let new_client = build_clients(effective_url)?;

    // 更新客户端
    if let Some(lock) = GLOBAL_CLIENT.get() {
        let mut client = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-001] Failed to acquire write lock: {e}");
            "Failed to update proxy: lock poisoned".to_string()
        })?;
        *client = new_client;
    } else {
        // 如果还没初始化，则初始化
        return init(proxy_url);
    }

    // 更新代理 URL 记录
    if let Some(lock) = CURRENT_PROXY_URL.get() {
        let mut url = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-002] Failed to acquire URL write lock: {e}");
            "Failed to update proxy URL record: lock poisoned".to_string()
        })?;
        *url = effective_url.map(|s| s.to_string());
    }

    log::info!(
        "[GlobalProxy] Updated: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 获取全局 HTTP 客户端
///
/// 返回配置了代理的客户端（如果已配置代理），否则返回跟随系统代理的客户端。
pub fn get() -> Client {
    GLOBAL_CLIENT
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|c| c.ordinary.clone())
        .unwrap_or_else(|| {
            log::warn!("[GlobalProxy] [GP-004] Client not initialized, using fallback");
            build_client(None).unwrap_or_default()
        })
}

/// Client for requests carrying credentials, including provider-specific headers.
/// Redirects may retain credentials only within the original scheme/host/port.
/// Never fall back to an unrestricted client after a configuration/lock failure.
pub fn get_authenticated() -> Result<Client, String> {
    authenticated_client_from(GLOBAL_CLIENT.get())
}

fn authenticated_client_from(lock: Option<&RwLock<HttpClients>>) -> Result<Client, String> {
    match lock {
        Some(lock) => lock
            .read()
            .map(|clients| clients.authenticated.clone())
            .map_err(|_| "Failed to get authenticated HTTP client: lock poisoned".to_string()),
        None => build_authenticated_client(None),
    }
}

/// 获取当前代理 URL
///
/// 返回当前配置的代理 URL，None 表示直连。
pub fn get_current_proxy_url() -> Option<String> {
    CURRENT_PROXY_URL
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|url| url.clone())
}

/// 检查是否正在使用代理
#[allow(dead_code)]
pub fn is_proxy_enabled() -> bool {
    get_current_proxy_url().is_some()
}

/// 构建 HTTP 客户端
fn build_client(proxy_url: Option<&str>) -> Result<Client, String> {
    build_client_with_redirect(proxy_url, reqwest::redirect::Policy::default())
}

/// reqwest turns URL userinfo into Basic Authorization even without auth headers.
pub(crate) fn url_has_credentials(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| !url.username().is_empty() || url.password().is_some())
}

fn same_origin(left: &url::Url, right: &url::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().is_some()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn build_authenticated_client(proxy_url: Option<&str>) -> Result<Client, String> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt
            .previous()
            .first()
            .is_some_and(|origin| same_origin(origin, attempt.url()))
        {
            // A custom policy does not inherit reqwest's default ten-hop bound.
            reqwest::redirect::Policy::limited(10).redirect(attempt)
        } else {
            attempt.error("authenticated request cannot redirect across origins")
        }
    });
    build_client_with_redirect(proxy_url, policy)
}

fn build_client_with_redirect(
    proxy_url: Option<&str>,
    policy: reqwest::redirect::Policy,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .redirect(policy)
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60))
        // 禁用 reqwest 自动解压：防止 reqwest 覆盖客户端原始 accept-encoding header。
        // 响应解压由 response_processor 根据 content-encoding 手动处理。
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd();

    // 有代理地址则使用代理，否则跟随系统代理
    if let Some(url) = proxy_url {
        // 先验证 URL 格式和 scheme
        let parsed = url::Url::parse(url)
            .map_err(|e| format!("Invalid proxy URL '{}': {}", mask_url(url), e))?;

        let scheme = parsed.scheme();
        if !["http", "https", "socks5", "socks5h"].contains(&scheme) {
            return Err(format!(
                "Invalid proxy scheme '{}' in URL '{}'. Supported: http, https, socks5, socks5h",
                scheme,
                mask_url(url)
            ));
        }

        let proxy = reqwest::Proxy::all(url)
            .map_err(|e| format!("Invalid proxy URL '{}': {}", mask_url(url), e))?;
        builder = builder.proxy(proxy);
        log::debug!("[GlobalProxy] Proxy configured: {}", mask_url(url));
    } else {
        // 未设置全局代理时，让 reqwest 自动检测系统代理（环境变量）
        // 若系统代理指向本机，禁用系统代理避免自环
        if system_proxy_points_to_loopback() {
            builder = builder.no_proxy();
            log::warn!(
                "[GlobalProxy] System proxy points to localhost, bypassing to avoid recursion"
            );
        } else {
            log::debug!("[GlobalProxy] Following system proxy (no explicit proxy configured)");
        }
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

fn system_proxy_points_to_loopback() -> bool {
    const KEYS: [&str; 6] = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];

    KEYS.iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .any(|value| proxy_points_to_loopback(&value))
}

fn proxy_points_to_loopback(value: &str) -> bool {
    fn host_is_loopback(host: &str) -> bool {
        if host.eq_ignore_ascii_case("localhost") {
            return true;
        }
        host.parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
    }

    // 检查是否指向 CC Switch 自己的代理端口
    // 只有指向自己的代理才需要跳过，避免递归
    fn is_cc_switch_proxy_port(port: Option<u16>) -> bool {
        let cc_switch_port = get_proxy_port();
        port == Some(cc_switch_port)
    }

    if let Ok(parsed) = url::Url::parse(value) {
        if let Some(host) = parsed.host_str() {
            // 只有当主机是 loopback 且端口是 CC Switch 的端口时才返回 true
            return host_is_loopback(host) && is_cc_switch_proxy_port(parsed.port());
        }
        return false;
    }

    let with_scheme = format!("http://{value}");
    if let Ok(parsed) = url::Url::parse(&with_scheme) {
        if let Some(host) = parsed.host_str() {
            return host_is_loopback(host) && is_cc_switch_proxy_port(parsed.port());
        }
    }

    false
}

/// 隐藏 URL 中的敏感信息（用于日志）
pub fn mask_url(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        // 隐藏用户名和密码，保留 scheme、host 和端口
        let host = parsed.host_str().unwrap_or("?");
        match parsed.port() {
            Some(port) => format!("{}://{}:{}", parsed.scheme(), host, port),
            None => format!("{}://{}", parsed.scheme(), host),
        }
    } else {
        // URL 解析失败，返回部分内容。截断点回退到最近的字符边界，
        // 避免在多字节 UTF-8 字符中间切割导致 panic。
        if url.len() > 20 {
            let cut = (0..=20)
                .rev()
                .find(|&i| url.is_char_boundary(i))
                .unwrap_or(0);
            format!("{}...", &url[..cut])
        } else {
            url.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn authenticated_redirect_ordinary_client_retains_cross_origin_redirects() {
        use crate::proxy::redirect_test_support::MockServer;
        let source = MockServer::spawn().await;
        let target = MockServer::spawn().await;
        source.redirect("/download", 302, &format!("{}/final", target.base_url));
        let result = build_client(None)
            .unwrap()
            .get(format!("{}/download", source.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), reqwest::StatusCode::OK);
        assert_eq!(source.received().len(), 1);
        assert_eq!(target.received().len(), 1);
    }

    #[test]
    fn authenticated_redirect_origin_includes_scheme_host_and_effective_port() {
        for (left, right, expected) in [
            ("https://EXAMPLE.test/a", "https://example.test:443/b", true),
            ("http://example.test/a", "http://example.test:80/b", true),
            (
                "https://example.test:443/a",
                "http://example.test:443/b",
                false,
            ),
            (
                "http://example.test:80/a",
                "https://example.test:80/b",
                false,
            ),
            ("https://example.test/a", "https://other.test/a", false),
            (
                "https://example.test/a",
                "https://example.test:444/a",
                false,
            ),
        ] {
            assert_eq!(
                same_origin(
                    &url::Url::parse(left).unwrap(),
                    &url::Url::parse(right).unwrap()
                ),
                expected,
                "{left} -> {right}"
            );
        }
        assert!(url_has_credentials("https://user:password@example.test"));
        assert!(url_has_credentials("https://:password@example.test"));
        assert!(!url_has_credentials("https://example.test"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn authenticated_redirect_uninitialized_fallback_is_guarded() {
        use crate::proxy::redirect_test_support::MockServer;
        let source = MockServer::spawn().await;
        let target = MockServer::spawn().await;
        source.redirect("/start", 302, &format!("{}/final", target.base_url));
        let result = authenticated_client_from(None)
            .unwrap()
            .get(format!("{}/start", source.base_url))
            .header("x-private-token", "fake-fallback-canary")
            .send()
            .await;
        assert!(result.unwrap_err().is_redirect());
        assert_eq!(source.received().len(), 1);
        assert!(target.received().is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn authenticated_redirect_poisoned_lock_fails_closed() {
        let lock = RwLock::new(build_clients(None).unwrap());
        let _ = std::panic::catch_unwind(|| {
            let _write = lock.write().unwrap();
            panic!("synthetic lock poisoning");
        });
        assert!(authenticated_client_from(Some(&lock)).is_err());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn authenticated_redirect_pair_uses_proxy_on_init_apply_and_update() {
        if crate::proxy::redirect_test_support::run_in_isolated_process(
            "proxy::http_client::tests::authenticated_redirect_pair_uses_proxy_on_init_apply_and_update",
        ) {
            return;
        }
        use crate::proxy::redirect_test_support::MockServer;
        struct Restore(Option<HttpClients>, Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(clients) = self.0.take() {
                    *GLOBAL_CLIENT.get().unwrap().write().unwrap() = clients;
                }
                if let Some(lock) = CURRENT_PROXY_URL.get() {
                    *lock.write().unwrap() = self.1.take();
                }
            }
        }
        let _restore = Restore(
            GLOBAL_CLIENT
                .get()
                .map(|lock| lock.read().unwrap().clone())
                .or_else(|| Some(build_clients(None).unwrap())),
            get_current_proxy_url(),
        );
        let proxy = MockServer::spawn().await;
        for apply in [init, apply_proxy, update_proxy] {
            apply(Some(&proxy.base_url)).unwrap();
            for client in [get(), get_authenticated().unwrap()] {
                client
                    .get("http://authenticated-redirect.invalid/resource")
                    .header("x-private-token", "fake-proxy-canary")
                    .send()
                    .await
                    .unwrap();
            }
            assert_eq!(
                get_current_proxy_url().as_deref(),
                Some(proxy.base_url.as_str())
            );
        }
        assert_eq!(proxy.received().len(), 6);
        assert!(proxy
            .received()
            .iter()
            .all(|request| request.headers["x-private-token"] == "fake-proxy-canary"));
        assert!(apply_proxy(Some("invalid-scheme://127.0.0.1:1")).is_err());
        get_authenticated()
            .unwrap()
            .get("http://authenticated-redirect.invalid/still-configured")
            .send()
            .await
            .unwrap();
        assert_eq!(
            proxy.received().len(),
            7,
            "invalid update must leave the configured pair intact"
        );
        assert!(build_clients(Some("socks5://127.0.0.1:1080")).is_ok());
        assert!(build_clients(Some("https://127.0.0.1:7890")).is_ok());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn authenticated_redirect_pair_preserves_system_proxy_and_loop_avoidance() {
        if crate::proxy::redirect_test_support::run_in_isolated_process(
            "proxy::http_client::tests::authenticated_redirect_pair_preserves_system_proxy_and_loop_avoidance",
        ) {
            return;
        }
        use crate::proxy::redirect_test_support::MockServer;
        const KEYS: [&str; 8] = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ];
        struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>, u16);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    match value {
                        Some(value) => env::set_var(key, value),
                        None => env::remove_var(key),
                    }
                }
                set_proxy_port(self.1);
            }
        }
        let _restore = Restore(
            KEYS.iter().map(|&key| (key, env::var_os(key))).collect(),
            get_proxy_port(),
        );
        for key in KEYS {
            env::remove_var(key);
        }
        let proxy = MockServer::spawn().await;
        let target = MockServer::spawn().await;
        env::set_var("HTTP_PROXY", &proxy.base_url);
        let port = url::Url::parse(&proxy.base_url).unwrap().port().unwrap();
        set_proxy_port(port);
        let pair = build_clients(None).unwrap();
        for client in [pair.ordinary, pair.authenticated] {
            client
                .get(format!("{}/direct", target.base_url))
                .send()
                .await
                .unwrap();
        }
        assert_eq!(target.received().len(), 2);
        assert!(proxy.received().is_empty(), "must bypass own proxy port");
        set_proxy_port(if port == 1 { 2 } else { 1 });
        let pair = build_clients(None).unwrap();
        for client in [pair.ordinary, pair.authenticated] {
            client
                .get("http://authenticated-redirect.invalid/system-proxy")
                .send()
                .await
                .unwrap();
        }
        assert_eq!(
            proxy.received().len(),
            2,
            "both policies must retain external system proxy"
        );
    }

    #[test]
    fn test_mask_url() {
        assert_eq!(mask_url("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert_eq!(
            mask_url("http://user:pass@127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            mask_url("socks5://admin:secret@proxy.example.com:1080"),
            "socks5://proxy.example.com:1080"
        );
        // 无端口的 URL 不应显示 ":?"
        assert_eq!(
            mask_url("http://proxy.example.com"),
            "http://proxy.example.com"
        );
        assert_eq!(
            mask_url("https://user:pass@proxy.example.com"),
            "https://proxy.example.com"
        );
    }

    #[test]
    fn test_mask_url_does_not_panic_on_multibyte_boundary() {
        // 一个无法被 Url::parse 解析、且在字节 20 处正好切在多字节字符中间的字符串。
        let bad = "这是一个无效的代理地址不能解析";
        assert!(bad.len() > 20 && !bad.is_char_boundary(20));
        let masked = mask_url(bad);
        assert!(masked.ends_with("..."));
    }

    #[test]
    fn test_build_client_direct() {
        let result = build_client(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_with_http_proxy() {
        let result = build_client(Some("http://127.0.0.1:7890"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_with_socks5_proxy() {
        let result = build_client(Some("socks5://127.0.0.1:1080"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_invalid_url() {
        // reqwest::Proxy::all 对某些无效 URL 不会立即报错
        // 使用明确无效的 scheme 来触发错误
        let result = build_client(Some("invalid-scheme://127.0.0.1:7890"));
        assert!(result.is_err(), "Should reject invalid proxy scheme");
    }

    #[test]
    #[serial_test::serial]
    fn test_proxy_points_to_loopback() {
        if crate::proxy::redirect_test_support::run_in_isolated_process(
            "proxy::http_client::tests::test_proxy_points_to_loopback",
        ) {
            return;
        }
        // 设置 CC Switch 代理端口为 15721（默认值）
        set_proxy_port(15721);

        // 只有指向 CC Switch 自己端口的 loopback 地址才返回 true
        assert!(proxy_points_to_loopback("http://127.0.0.1:15721"));
        assert!(proxy_points_to_loopback("socks5://localhost:15721"));
        assert!(proxy_points_to_loopback("127.0.0.1:15721"));

        // 其他 loopback 端口不应该被跳过（允许使用其他本地代理工具）
        assert!(!proxy_points_to_loopback("http://127.0.0.1:7890"));
        assert!(!proxy_points_to_loopback("socks5://localhost:1080"));

        // 非 loopback 地址不应该被跳过
        assert!(!proxy_points_to_loopback("http://192.168.1.10:7890"));
        assert!(!proxy_points_to_loopback("http://192.168.1.10:15721"));
    }

    #[test]
    #[serial_test::serial]
    fn test_system_proxy_points_to_loopback() {
        if crate::proxy::redirect_test_support::run_in_isolated_process(
            "proxy::http_client::tests::test_system_proxy_points_to_loopback",
        ) {
            return;
        }
        let _guard = env_lock().lock().unwrap();

        // 设置 CC Switch 代理端口
        set_proxy_port(15721);

        let keys = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ];

        for key in &keys {
            std::env::remove_var(key);
        }

        // 指向 CC Switch 端口的代理应该被跳过
        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:15721");
        assert!(system_proxy_points_to_loopback());

        // 指向其他端口的本地代理不应该被跳过
        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:7890");
        assert!(!system_proxy_points_to_loopback());

        // 非 loopback 地址不应该被跳过
        std::env::set_var("HTTP_PROXY", "http://10.0.0.2:7890");
        assert!(!system_proxy_points_to_loopback());

        for key in &keys {
            std::env::remove_var(key);
        }
    }
}
