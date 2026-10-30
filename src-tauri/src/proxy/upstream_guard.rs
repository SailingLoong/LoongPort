//! Reject provider URLs that would send a request back to this proxy listener.
//! Adapted from cc-switch cd9c721086fbee36a1d9b1d986a8f3e14ac69147.
//! LoongPort bounds same-port DNS checks and resolves aliases after a shortcut miss.

use super::ProxyError;
use std::future::Future;
use std::net::{IpAddr, UdpSocket};

pub(super) async fn reject_self_forwarding(
    target: &str,
    listen_address: &str,
    listen_port: u16,
) -> Result<(), ProxyError> {
    #[cfg(test)]
    super::server::loopback_tests::guard_phase("guard_start");
    let Ok(url) = url::Url::parse(target) else {
        return Ok(());
    };
    if listen_port == 0 || url.port_or_known_default() != Some(listen_port) {
        return Ok(());
    }
    let Ok(listener) = listen_address.trim_matches(['[', ']']).parse::<IpAddr>() else {
        return Ok(());
    };
    #[cfg(test)]
    super::server::loopback_tests::guard_phase("guard_url_parsed");
    let targets_listener = match url.host() {
        Some(url::Host::Ipv4(ip)) => is_listener_address(ip.into(), listener),
        Some(url::Host::Ipv6(ip)) => is_listener_address(ip.into(), listener),
        Some(url::Host::Domain(host)) => {
            domain_targets_listener(host, listener, tokio::net::lookup_host((host, listen_port)))
                .await?
        }
        None => false,
    };
    #[cfg(test)]
    super::server::loopback_tests::guard_phase("guard_decided");
    if targets_listener {
        // Do not include the URL: provider paths and queries can contain API keys.
        return Err(ProxyError::ConfigError(
            "Upstream URL points to LoongPort's own proxy listener. Set the provider base URL to the real upstream service.".to_string(),
        ));
    }
    Ok(())
}

// The resolver future is shared by production and deterministic DNS fixtures.
async fn domain_targets_listener<T>(
    host: &str,
    listener: IpAddr,
    lookup: impl Future<Output = std::io::Result<T>>,
) -> Result<bool, ProxyError>
where
    T: Iterator<Item = std::net::SocketAddr>,
{
    let host = host.trim_end_matches('.');
    if (host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost"))
        && (is_listener_address(std::net::Ipv4Addr::LOCALHOST.into(), listener)
            || is_listener_address(std::net::Ipv6Addr::LOCALHOST.into(), listener))
    {
        return Ok(true);
    }
    #[cfg(test)]
    super::server::loopback_tests::guard_phase("dns_lookup_start");
    let resolved = resolve_alias(lookup).await;
    #[cfg(test)]
    super::server::loopback_tests::guard_phase("dns_lookup_finished");
    Ok(resolved?.any(|address| is_listener_address(address.ip(), listener)))
}

// Keep the resolver boundary shared with the deterministic slow-DNS regression.
async fn resolve_alias<T>(
    lookup: impl Future<Output = std::io::Result<T>>,
) -> Result<T, ProxyError> {
    tokio::time::timeout(std::time::Duration::from_millis(500), lookup)
        .await
        .map_err(|_| dns_check_error())?
        .map_err(|_| dns_check_error())
}

fn dns_check_error() -> ProxyError {
    ProxyError::ConfigError("Cannot verify upstream address against the proxy listener. Check the provider hostname and DNS configuration.".to_string())
}

fn is_listener_address(target: IpAddr, listener: IpAddr) -> bool {
    fn normalize(ip: IpAddr) -> IpAddr {
        match ip {
            IpAddr::V6(ip) => ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip.into()),
            ip => ip,
        }
    }
    let target = normalize(target);
    let listener = normalize(listener);
    if listener.is_ipv4() && target.is_ipv6()
        || listener.is_ipv6() && target.is_ipv4() && !listener.is_unspecified()
    {
        return false;
    }
    if target == listener {
        return true;
    }
    if listener.is_unspecified() {
        // A wildcard listener also accepts connections to local interface IPs.
        // Binding an ephemeral UDP socket checks address ownership without sending
        // traffic or adding a dependency for enumerating network interfaces.
        return target.is_loopback()
            || target.is_unspecified()
            || UdpSocket::bind((target, 0)).is_ok();
    }
    target.is_unspecified() && listener.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_own_listener_including_custom_ports_and_url_aliases() {
        for (target, address, port) in [
            ("http://127.0.0.1:3688/v1/responses", "127.0.0.1", 3688),
            ("http://localhost:15721/v1", "127.0.0.1", 15721),
            ("http://LOCALHOST.:15721/v1", "127.0.0.1", 15721),
            ("http://proxy.localhost:15721/v1", "127.0.0.1", 15721),
            ("http://127.1:15721/v1", "127.0.0.1", 15721),
            ("http://[::ffff:127.0.0.1]:15721/v1", "127.0.0.1", 15721),
            ("http://[::1]:15721/v1", "::1", 15721),
            ("http://localhost:15721/v1", "::1", 15721),
            ("http://127.0.0.1:15721/v1", "0.0.0.0", 15721),
            ("http://[::1]:15721/v1", "::", 15721),
            ("http://192.0.2.10:15721/v1", "192.0.2.10", 15721),
            ("http://0.0.0.0:15721/v1", "127.0.0.1", 15721),
            ("http://127.0.0.1/v1", "127.0.0.1", 80),
        ] {
            assert!(
                matches!(
                    reject_self_forwarding(target, address, port).await,
                    Err(ProxyError::ConfigError(_))
                ),
                "{target} via {address}:{port}",
            );
        }
    }

    #[tokio::test]
    async fn allows_other_services_and_does_not_disclose_upstream_secrets() {
        for target in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:1234/v1",
            "http://[::1]:11434/v1",
            "http://[::1]:15721/v1",
            "http://192.0.2.10:15721/v1",
            "http://127.0.0.2:15721/v1",
            "https://api.example.com/v1",
        ] {
            assert!(
                reject_self_forwarding(target, "127.0.0.1", 15721)
                    .await
                    .is_ok(),
                "{target}"
            );
        }
        for listener in ["0.0.0.0", "::", "192.0.2.20"] {
            assert!(
                reject_self_forwarding("http://192.0.2.10:15721/v1", listener, 15721)
                    .await
                    .is_ok()
            );
        }
        assert!(
            reject_self_forwarding("http://unresolvable.invalid:11434/v1", "127.0.0.1", 15721)
                .await
                .is_ok()
        );
        assert!(
            reject_self_forwarding("http://localhost:15721/v1", "127.0.0.1", 0)
                .await
                .is_ok()
        );
        let error = reject_self_forwarding(
            "http://user:secret@127.0.0.1:15721/private-key/v1?key=private-token",
            "127.0.0.1",
            15721,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("own proxy listener"));
        for secret in ["secret", "private-key", "private-token"] {
            assert!(!error.contains(secret));
        }
    }
    #[tokio::test]
    async fn slow_dns_is_bounded_and_error_is_sanitized() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            resolve_alias(std::future::pending::<std::io::Result<()>>()),
        )
        .await
        .expect("listener guard must bound DNS lookup time");
        assert!(matches!(result, Err(ProxyError::ConfigError(_))));
        let error = resolve_alias(std::future::ready(Err::<(), _>(std::io::Error::other(
            "private-hostname private-query",
        ))))
        .await
        .unwrap_err()
        .to_string();
        assert!(!error.contains("private-hostname"));
        assert!(!error.contains("private-query"));
    }
    #[tokio::test]
    async fn localhost_shortcut_miss_still_checks_resolved_addresses() {
        let listener = "127.0.0.2".parse().unwrap();
        let addresses = vec!["127.0.0.2:15721".parse().unwrap()];
        assert!(domain_targets_listener(
            "localhost.",
            listener,
            std::future::ready(Ok(addresses.into_iter()))
        )
        .await
        .unwrap());
        let addresses = vec!["127.0.0.3:15721".parse().unwrap()];
        assert!(!domain_targets_listener(
            "proxy.localhost",
            listener,
            std::future::ready(Ok(addresses.into_iter()))
        )
        .await
        .unwrap());
    }
}
