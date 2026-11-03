//! 拒绝网页发来的请求，让开着的浏览器标签页花不了本地代理里的用户凭据。
//!
//! 代理会把请求携带的任意凭据换成已存供应商的凭据，因此任何能连上这个端口的东西
//! 都能用它们。CLI 客户端从不发 `Origin`，而浏览器对每个 POST（包括跳过预检的
//! `no-cors` 请求）和每个 WebSocket 握手都会带上它——拒绝它就挡住了页面跨站调用
//! 代理。DNS rebinding 页面是同源的、能读 GET 响应，但它必须用自己的域名够到我们，
//! 所以回环监听时只接受公共 DNS 指不到的 Host。
//!
//! 只读头部：请求本体原样通过。

use super::server::ProxyState;
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::net::IpAddr;

/// Claude Desktop 的命名空间已有自己的 gateway token 校验（页面拿不到 token），
/// 该路径不受头部规则约束。
const TOKEN_PROTECTED_PREFIX: &str = "/claude-desktop/";

pub(super) async fn reject_browser_requests(
    State(state): State<ProxyState>,
    request: Request,
    next: Next,
) -> Response {
    let listen_address = state.config.read().await.listen_address.clone();
    match check_request(
        request.uri().path(),
        request.headers(),
        &listen_address,
        None,
    ) {
        Ok(()) => next.run(request).await,
        Err(reason) => {
            log::warn!(
                "[ProxyServer] rejected {} {}: {reason}",
                request.method(),
                request.uri().path()
            );
            (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": {
                        "type": "forbidden",
                        "message": format!("LoongPort local proxy: {reason}"),
                    }
                })),
            )
                .into_response()
        }
    }
}

fn check_request(
    path: &str,
    headers: &HeaderMap,
    listen_address: &str,
    client_host: Option<&str>,
) -> Result<(), &'static str> {
    if !path.starts_with(TOKEN_PROTECTED_PREFIX) && headers.contains_key(header::ORIGIN) {
        return Err("requests from web pages are not accepted");
    }
    if is_loopback_listener(listen_address) {
        if let Some(host) = headers.get(header::HOST) {
            let host = host.to_str().ok().and_then(host_name);
            if !host.is_some_and(|host| is_local_host(&host, client_host)) {
                return Err("unexpected Host header");
            }
        }
    }
    Ok(())
}

/// 绑在局域网地址上的监听可以以该网络使用的任意名字访问，
/// 所以 Host 规则只在代理仅监听本机时生效。
fn is_loopback_listener(listen_address: &str) -> bool {
    let address = listen_address.trim().trim_matches(['[', ']']);
    address.eq_ignore_ascii_case("localhost")
        // `to_canonical`：`::ffff:127.0.0.1` 也绑在回环上，但 IPv6 的 is_loopback 不认它
        || address
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// Host 头的主机部分：小写、去端口、去方括号与结尾点号。
fn host_name(value: &str) -> Option<String> {
    let authority: http::uri::Authority = value.trim().parse().ok()?;
    let host = authority
        .host()
        .trim_matches(['[', ']'])
        .trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// rebinding 页面用不了的名字：IP 字面量（rebinding 需要域名）、`localhost` 与
/// 保留私用后缀 `.localhost` / `.local` / `.internal`（末者覆盖
/// `host.docker.internal`），以及用户配置的客户端地址。
fn is_local_host(host: &str, client_host: Option<&str>) -> bool {
    host.parse::<IpAddr>().is_ok()
        || host == "localhost"
        || [".localhost", ".local", ".internal"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
        || client_host.is_some_and(|client| {
            client
                .trim()
                .trim_matches(['[', ']'])
                .trim_end_matches('.')
                .eq_ignore_ascii_case(host)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn cli_requests_pass() {
        for host in ["127.0.0.1:15721", "localhost:15721", "[::1]:15721"] {
            let h = headers(&[("host", host)]);
            assert_eq!(check_request("/v1/messages", &h, "127.0.0.1", None), Ok(()));
        }
        // 完全没有 Host（HTTP/1.0 客户端）不是浏览器
        assert_eq!(
            check_request("/v1/messages", &HeaderMap::new(), "127.0.0.1", None),
            Ok(())
        );
    }

    #[test]
    fn any_origin_is_rejected_outside_claude_desktop() {
        for origin in ["https://evil.example", "null", "tauri://localhost"] {
            let h = headers(&[("host", "127.0.0.1:15721"), ("origin", origin)]);
            for path in ["/v1/messages", "/v1/responses", "/v1beta/models", "/status"] {
                assert!(check_request(path, &h, "127.0.0.1", None).is_err());
            }
            // 局域网监听同样覆盖：Origin 规则不依赖 Host
            assert!(check_request("/v1/messages", &h, "0.0.0.0", None).is_err());
        }
    }

    #[test]
    fn claude_desktop_keeps_its_token_check_only() {
        let h = headers(&[("host", "127.0.0.1:15721"), ("origin", "app://claude")]);
        assert_eq!(
            check_request("/claude-desktop/v1/messages", &h, "127.0.0.1", None),
            Ok(())
        );
        // rebinding 在那里同样适用：Host 规则不豁免
        let h = headers(&[("host", "rebind.evil.example:15721")]);
        assert!(check_request("/claude-desktop/v1/models", &h, "127.0.0.1", None).is_err());
    }

    #[test]
    fn rebinding_hosts_are_rejected_on_loopback_listener() {
        for host in [
            "rebind.evil.example:15721",
            "evil.example.",
            "localhost.evil.example",
        ] {
            let h = headers(&[("host", host)]);
            assert!(
                check_request("/status", &h, "127.0.0.1", None).is_err(),
                "{host}"
            );
            assert!(check_request("/status", &h, "::1", None).is_err(), "{host}");
            assert!(
                check_request("/status", &h, "[::ffff:127.0.0.1]", None).is_err(),
                "{host}"
            );
        }
    }

    #[test]
    fn local_names_and_configured_client_host_pass() {
        for host in [
            "host.docker.internal:15721",
            "my-mac.local:15721",
            "app.localhost:15721",
            "LOCALHOST.:15721",
            "172.25.144.1:15721",
        ] {
            let h = headers(&[("host", host)]);
            assert_eq!(
                check_request("/v1/messages", &h, "127.0.0.1", None),
                Ok(()),
                "{host}"
            );
        }
        let h = headers(&[("host", "Devbox.Example.com:15721")]);
        assert_eq!(
            check_request(
                "/v1/messages",
                &h,
                "127.0.0.1",
                Some(" devbox.example.com ")
            ),
            Ok(())
        );
    }
}
