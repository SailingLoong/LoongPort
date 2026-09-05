//! 站点身份的唯一源：两个语义不同的归一，别混用。
//!
//! - [`site_domain`]：站点的**身份**（注册域 / apex，Public Suffix List 判定）。
//!   子域（`www.` / `api.` / ……）只是同一站点自己挂的前缀，不参与身份 ——
//!   站点的面板、API、文档常分挂不同子域，按全 host 认身份会把同站拆成两家，
//!   比对静默失配（2026-09-05 实测：实测数据上传因 `api.` 前缀整桶丢弃）。
//!   维护者拍板：**注册域是中转站归属的唯一标识**。
//! - [`request_host`]：**取数地址**（去 scheme / 端口 / 路径、小写、剥 `www.`）。
//!   拼真实 URL 用 —— well-known 抓取、外站链接、探针名单要打的是真实主机，
//!   它的产出不是身份。
//!
//! 所有「这家是不是那家」的判断走 [`site_domain`]；所有「往哪台主机发请求」
//! 走 [`request_host`]。两个函数只此一份，别处（含前端）不得再抄实现。

/// 取数地址：去 scheme / 端口 / 路径，小写，剥 `www.` 前缀。
pub(crate) fn request_host(origin: &str) -> String {
    let without_scheme = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    // 端口与路径都切掉（`site_origin` 正常不带路径，但多切一次没坏处）。
    let host = without_scheme
        .split(['/', ':'])
        .next()
        .unwrap_or(without_scheme)
        .to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// 站点身份：注册域（apex）。
///
/// PSL 判不出的输入（裸公共后缀、`localhost`、空串）原样回落为
/// [`request_host`] 的产出 —— 身份函数必须全定义、确定性，宁可保守也不 panic。
/// IP 是特判：PSL 的兜底规则会把 `127.0.0.1` 错算成 `0.1`，IP 本身就是身份。
pub(crate) fn site_domain(origin: &str) -> String {
    let host = request_host(origin);
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    match psl::domain_str(&host) {
        Some(domain) => domain.to_string(),
        None => host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_host_strips_scheme_port_path_www_and_case() {
        for (input, want) in [
            ("https://Panel.Example/login", "panel.example"),
            ("https://panel.example:8443", "panel.example"),
            ("https://WWW.panel.example", "panel.example"),
            ("panel.example", "panel.example"),
        ] {
            assert_eq!(request_host(input), want, "{input}");
        }
    }

    #[test]
    fn site_domain_is_the_registrable_domain_not_the_host() {
        // 子域是站点自己的前缀，不参与身份 —— 这是 2026-09-05 拍板的口径。
        for (input, want) in [
            ("https://api.panel.example/v1", "panel.example"),
            ("https://panel.example", "panel.example"),
            ("https://cdn.www.panel.example", "panel.example"),
            ("https://api.panel.example:8443", "panel.example"),
        ] {
            assert_eq!(site_domain(input), want, "{input}");
        }
    }

    #[test]
    fn multi_label_public_suffixes_do_not_over_strip() {
        // 用 PSL 而不是「取最后两段」：co.uk 是公共后缀，注册域是三段。
        assert_eq!(site_domain("https://shop.example.co.uk"), "example.co.uk");
        // 连字符是标签内容不是分隔：api-top 整段是一个标签。
        assert_eq!(site_domain("https://api-top.example"), "api-top.example");
    }

    #[test]
    fn unregistrable_inputs_fall_back_to_the_request_host() {
        // IP、localhost、空串：PSL 判不出注册域，原样回落，不 panic。
        assert_eq!(site_domain("https://127.0.0.1:8080"), "127.0.0.1");
        assert_eq!(site_domain("https://localhost:3000"), "localhost");
        assert_eq!(site_domain(""), "");
    }
}
