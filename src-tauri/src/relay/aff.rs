//! 注册邀请码：站点 host → 我们的 aff 码。
//!
//! ## 编译期常量表，不做远端拉取
//!
//! 上游 cc-switch 就是这么做的（`claudeDesktopProviderPresets.ts` 里 20+ 条 preset
//! 各硬编码一个 `?aff=` / `?ref=` 链接，随 MIT 仓库明文公开），业界惯例如此 ⇒ 尺子1
//! 命中即停。
//!
//! **明确不做「拉远端 JSON / git 上的映射文件」**：那多一个网络依赖、一个失败模式
//! （拉不到怎么办）、一个信任边界（远端文件能改我们发出去的链接），换来的只是
//! 「改码不用发版」—— 而邀请码几乎不变。
//!
//! ## ⚠️ 这张表随发布包分发出去（维护者已知情并拍板：要编进去）
//!
//! 编译进二进制的字符串 `strings` 一下就能看到 ⇒ 那几个**不是维护者自己的站**的码
//! 一旦发布就等于公开「LoongPort 与这几家有返利关系」。
//!
//! 这通常是可接受的：aff 码不是密钥，泄露它的后果只是「别人也能用这个码注册」，
//! 而那对码的持有者是**收益**不是损失。2026-08-03 维护者明确拍板要编进去。
//!
//! ## 数据来源与边界
//!
//! 来自维护者的一份私有表。⚠️ **那份表含邀请主号邮箱与返利比例，是私有商业数据
//! —— 只把 host → code 两列搬进来**，其余留在表里。
//!
//! 两条**有意不进表**的（进了也是死数据）：
//!
//! - **`bestapi.store` 是维护者自己的站**，没有邀请码。服务端也会拒
//!   （`affiliate_service.go:300`：`inviterSummary.UserID == userID` ⇒
//!   `ErrAffiliateCodeInvalid`），而且那个错误**被 swallow 了**
//!   （`auth_service.go:243-247` 只记日志、注册照样成功）⇒ 是「服务端日志里一条错误」
//!   而不是用户可见的失败。**别去修一个不存在的用户可见错误。**
//! - **newapi 的站**（表里那个「可乐」）：它的注册路径是 `/sign-up?aff=` 而不是
//!   sub2api 的 `/register?aff=`，且它的码含小写而 sub2api 会 `ToUpper` ——
//!   **不同系统的码空间不同**。我们的 `probe_site` 只认 sub2api 站，非 sub2api 压根
//!   加不进来 ⇒ 那条进表永远命中不到。等真支持 newapi 那轮再说。
//!
//! ## 码的格式：我们只搬运，不校验
//!
//! 服务端**有**格式规则（`isValidAffiliateCodeFormat`，`affiliate_service.go:48-58`：
//! 4-32 位、只允许 `A-Z` / `0-9` / `_` / `-`，且校验前先
//! `strings.ToUpper(strings.TrimSpace(..))`）。
//!
//! **但客户端仍然不做格式校验** —— 理由不是「形状不确定」，而是**规则在服务端且可能随
//! 版本变**（`AffiliateCodeMaxLength` 是个常量，上游改它我们不会知道）。
//! 客户端唯一该做的是 trim（防录表时手滑带空格），剩下让服务端判。

/// 站点 host → 我们的 aff 码。
///
/// key 是**归一后的 host**（小写、去 `www.` 前缀、**不带端口**），不是完整 URL ——
/// 见 [`aff_code_for`] 的文档。
///
/// 码按**原样大写**录入（服务端会 `ToUpper`，但表里就写成最终形态更少一层心智负担）。
const AFF_CODES: &[(&str, &str)] = &[
    ("api.aijws.com", "RJZUAA8XX6W7"),
    ("790053500.com", "FQSPPFUYXSSS"),
    ("wawapii.com", "4PAUD8SSZXG7"),
    ("999555999.com", "XNTZVS78F7WY"),
    // ⚠️ bestapi.store 有意不在表里 —— 那是维护者自己的站，见模块文档。
];

/// 把 `site_origin` 归一成查表用的 host。
///
/// 三件事：**去 scheme、去端口、去 `www.` 前缀、转小写**。
///
/// ## 为什么按 host 查而不按完整 `site_origin`
///
/// - **端口必须丢掉**：`normalize_site_origin`（`api.rs`）**会保留端口**，
///   所以存的可能是 `https://x.com:8443`。同一个站换个端口不该变成另一家中转站。
/// - **`www.` 必须归一**：同一个站可能有 `https://x.com` 与 `https://www.x.com`
///   两种 origin（维护者那份表里写的就是 `www.bestapi.store`，而我们存的是不带
///   `www.` 的）。不归一会多一类「表里明明有却查不到」的静默失效。
///
/// ⚠️ **有意不做更聪明的匹配**：不做子域通配、不做模糊。那会把不相关的站匹进去，
/// 后果是**给错的站带上我们的码** —— 比查不到糟得多。
pub(super) fn lookup_host(site_origin: &str) -> String {
    let without_scheme = site_origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(site_origin);
    // 端口与路径都切掉（`site_origin` 正常不带路径，但多切一次没坏处）。
    let host = without_scheme
        .split(['/', ':'])
        .next()
        .unwrap_or(without_scheme)
        .to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// 查这个站有没有我们的 aff 码。
///
/// `None` = 表里没有 ⇒ 调用方**什么都不做**（不写那个键、不改 URL）。
/// 绝大多数站都会走这条路 —— 但**默认站在表里、有码**（2026-08-04 起它不再是
/// 维护者自己的站，那之前这句话是反的）。
///
/// ⚠️ 这是**内置那一层**，不是最终取值：调用方走
/// [`super::remote_config::resolve_aff_code`] 的两层回落，远端配置能覆盖也能撤销它。
pub fn aff_code_for(site_origin: &str) -> Option<&'static str> {
    let host = lookup_host(site_origin);
    AFF_CODES
        .iter()
        .find(|(table_host, _)| *table_host == host)
        .map(|(_, code)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_code_for_a_site_in_the_table() {
        assert_eq!(aff_code_for("https://wawapii.com"), Some("4PAUD8SSZXG7"));
    }

    #[test]
    fn a_site_not_in_the_table_yields_none_rather_than_a_wrong_code() {
        // 绝大多数站走这条路。返回 None 意味着「不带码」，而不是「带一个默认码」——
        // 后者会把我们的码贴到不相关的站上。
        assert_eq!(aff_code_for("https://some-other-relay.com"), None);
    }

    #[test]
    fn the_maintainers_own_site_is_deliberately_absent() {
        // ⭐ 这条是**有意的缺席**，不是遗漏：服务端会拒自己邀请自己
        // （`affiliate_service.go:300`），补上是给服务端日志里塞一条错误。
        //
        // ⚠️ 这个理由**只适用于维护者自己的站**，别推广到「默认站」——
        // 2026-08-04 之前两者恰好是同一个站，那个巧合已经不成立了：默认站现在**必须**
        // 在表里有码，`commands::relay` 的 `DEFAULT_SITE` 那里有闸钉着。
        // （指到那个常量而不是那条测试的名字：测试名改了没有任何东西能验，
        //   而常量名是真代码，rustdoc 与编译器都管得到。）
        for origin in [
            "https://bestapi.store",
            "https://www.bestapi.store",
            "https://bestapi.store:443",
        ] {
            assert_eq!(aff_code_for(origin), None, "{origin} 不该有码");
        }
    }

    #[test]
    fn port_is_ignored_so_one_site_is_not_two_relays() {
        // `normalize_site_origin` 会保留端口 ⇒ 存的可能带 `:8443`。
        // 不丢端口的话同一个站换端口就查不到码了。
        assert_eq!(
            aff_code_for("https://wawapii.com:8443"),
            Some("4PAUD8SSZXG7")
        );
    }

    #[test]
    fn www_and_case_are_normalized() {
        // 维护者那份表里有的条目写的是 `www.` 形式，而我们存的 origin 通常不带 ——
        // 不归一会变成「表里明明有却查不到」的静默失效。
        for origin in [
            "https://www.wawapii.com",
            "https://WawaPii.com",
            "https://WWW.WAWAPII.COM",
        ] {
            assert_eq!(aff_code_for(origin), Some("4PAUD8SSZXG7"), "{origin}");
        }
    }

    #[test]
    fn a_bare_host_without_scheme_still_resolves() {
        // 调用方**应该**传归一过的 site_origin，但别为这个假设崩掉。
        assert_eq!(aff_code_for("wawapii.com"), Some("4PAUD8SSZXG7"));
    }

    #[test]
    fn subdomains_do_not_inherit_the_parent_codes() {
        // ⚠️ 这条钉住「不做聪明匹配」：`api.aijws.com` 在表里，
        // 但 `aijws.com` 与 `evil.aijws.com` **都不该**命中它。
        // 给错的站带码比查不到糟得多。
        assert_eq!(aff_code_for("https://aijws.com"), None);
        assert_eq!(aff_code_for("https://evil.aijws.com"), None);
        // 反过来：表里那个精确 host 要能查到。
        assert_eq!(aff_code_for("https://api.aijws.com"), Some("RJZUAA8XX6W7"));
    }

    #[test]
    fn every_code_satisfies_the_servers_format_rules() {
        // 服务端 `isValidAffiliateCodeFormat`：4-32 位、只允许 A-Z / 0-9 / _ / -。
        // 我们不在运行时校验（规则在服务端、可能随版本变），但**录表时录错**
        // 是我们自己的问题 —— 这条闸在编译期之外、录入那一刻就拦住它。
        for (host, code) in AFF_CODES {
            assert!(
                (4..=32).contains(&code.len()),
                "{host} 的码长度不合规: {code}"
            );
            assert!(
                code.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '-'),
                "{host} 的码含服务端不接受的字符（注意小写会被 ToUpper，别录小写）: {code}"
            );
            // 顺手拦住录表时的空格手滑。
            assert_eq!(code.trim(), *code, "{host} 的码带空格");
        }
    }

    #[test]
    fn table_hosts_are_already_normalized() {
        // 表里的 key 若带 scheme / 端口 / `www.` / 大写，就永远查不到 ——
        // 因为查表前 `lookup_host` 已经把输入归一了。这是个静默失效，必须有闸。
        for (host, _) in AFF_CODES {
            assert_eq!(&lookup_host(host), host, "表里的 key 没归一: {host}");
            assert!(!host.contains("://"), "key 不该带 scheme: {host}");
            assert!(!host.contains(':'), "key 不该带端口: {host}");
        }
    }

    #[test]
    fn no_duplicate_hosts() {
        // 重复 key 时 `find` 只会命中第一条，第二条成了静默死数据。
        let mut hosts: Vec<&str> = AFF_CODES.iter().map(|(h, _)| *h).collect();
        let before = hosts.len();
        hosts.sort_unstable();
        hosts.dedup();
        assert_eq!(hosts.len(), before, "表里有重复的 host");
    }
}
