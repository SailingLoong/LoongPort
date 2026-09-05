//! 注册优惠码：站点 host → 优惠码（注册即得赠额）。
//!
//! ## 与 [`super::aff`] 是**两个不同的服务端字段**，别合并
//!
//! sub2api 的注册表单有三个独立字段（`auth_handler.go:50-58` 的 `RegisterRequest`）：
//!
//! | 字段 | 是什么 | 我们管不管 |
//! |---|---|---|
//! | `aff_code` | 邀请返利码，**注册人的上级拿返利** | 管，见 [`super::aff`] |
//! | `promo_code` | 注册优惠码，**注册人自己得赠额** | 管，就是本模块 |
//! | `invitation_code` | 邀请制站点的准入码 | 不管（我们对接的站不是邀请制） |
//!
//! 两个码**互不排斥**，可以同时带（服务端分别处理：`auth_service.go:243` 处理 aff、
//! `auth_service.go:259` 处理 promo）。所以这张表与 aff 那张表**各自独立**，
//! 一个站可以只有其中一个、也可以两个都有。
//!
//! ## `bestapi.store` 为什么两张表都不在
//!
//! 两张表的排除理由不同，别看到「同一个站」就想「修正」其中一张：
//!
//! - **aff 表排除它**：服务端拒绝自己邀请自己（`affiliate_service.go:300`），
//!   `inviterSummary.UserID == userID` ⇒ `ErrAffiliateCodeInvalid`。
//! - **promo 表曾包含它**（优惠码是站主给新用户的赠额活动，与服务端身份比对无关），
//!   但那个码 `LOONGPORT` 已于 **2026-08-16 在服务端删除**：内置条目随之清空，
//!   对已发出去的客户端同日由远端配置下发
//!   `promo_codes: {"bestapi.store": ""}` 撤销（`resolve_code` 的
//!   「远端空串 = 撤销、不回落内置」语义）。两层一起收口，注册页不再预填。
//!
//! ⚠️ **有意不做「所有站都试着填优惠码」** —— 别的站没建码，
//! 注册页的实时校验（`RegisterView.vue:599` 的 `validatePromoCodeDebounced`）
//! 会给一个红框 + 「优惠码无效」⇒ 用户以为自己或我们出错了。
//! 一个错误的红框比不填糟得多。将来要再上码：这里加条目，或直接走远端
//! `promo_codes` 表（不用发版）。
//!
//! ## 码的格式：我们只搬运，不校验
//!
//! 与 [`super::aff`] 同一条理由 —— 规则在服务端且可能随版本变。
//! 但**录表时录错**是我们自己的问题，所以下面有一条编译期之外、录入那一刻的闸。

/// 站点 → 注册优惠码。
///
/// key 是**注册域（apex）**，与 [`super::aff::AFF_CODES`] 同一套身份归一 ——
/// 两处共用 [`super::identity::site_domain`]，不各写一份。
const PROMO_CODES: &[(&str, &str)] = &[
    // 2026-08-16 起为空：唯一的码 LOONGPORT 已在服务端删除（见模块文档）。
];

/// 查这个站有没有注册优惠码。
///
/// `None` = 表里没有 ⇒ 调用方**什么都不做**（不预填那个框）。
/// 绝大多数站都会走这条路。
pub fn promo_code_for(site_origin: &str) -> Option<&'static str> {
    let domain = super::identity::site_domain(site_origin);
    PROMO_CODES
        .iter()
        .find(|(table_domain, _)| *table_domain == domain)
        .map(|(_, code)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-16 起：唯一的码已在服务端删除，内置表清空 —— 任何站都不预填。
    #[test]
    fn the_builtin_table_is_empty_since_the_code_was_deleted_serverside() {
        assert_eq!(promo_code_for("https://bestapi.store"), None);
        assert!(PROMO_CODES.is_empty(), "再加码请连着更新这条与模块文档");
    }

    /// ⭐ **这条钉住「两张表都不含 bestapi.store」各自的理由，防止有人反向「补齐」。**
    ///
    /// 排除理由不同（见模块文档）：aff 是服务端拒绝自己邀请自己；
    /// promo 是码已删除。给 aff 表补上它会让服务端日志多一条
    /// `ErrAffiliateCodeInvalid`；给 promo 表补回旧码会让注册页弹「优惠码无效」。
    #[test]
    fn neither_table_carries_the_maintainers_site_anymore() {
        let origin = "https://bestapi.store";
        assert_eq!(
            promo_code_for(origin),
            None,
            "LOONGPORT 已在服务端删除（2026-08-16），填回去就是红框"
        );
        assert_eq!(
            super::super::aff::aff_code_for(origin),
            None,
            "返利码是邀请关系，服务端拒绝自己邀请自己"
        );
    }

    #[test]
    fn a_site_not_in_the_table_yields_none_rather_than_a_wrong_code() {
        // ⚠️ 这条是本模块最重要的约束：别的站**没建** LOONGPORT 这个码，
        // 填上去会让注册页弹「优惠码无效」的红框 —— 用户以为出错了。
        for origin in [
            "https://wawapii.com",
            "https://api.aijws.com",
            "https://some-other-relay.com",
        ] {
            assert_eq!(promo_code_for(origin), None, "{origin} 不该有码");
        }
    }

    #[test]
    fn host_normalization_is_shared_with_the_aff_table() {
        // 复用 `identity::site_domain` ⇒ 端口 / `www.` / 子域 / 大小写的归一行为**必须一致**。
        // 各写一份迟早会漂成「aff 查到了 promo 没查到」的静默失效。
        // 表清空后这条同时守「bestapi.store 的任何变体都不再把码带回来」。
        for origin in [
            "https://www.bestapi.store",
            "https://BestApi.store",
            "https://bestapi.store:443",
            "bestapi.store",
        ] {
            assert_eq!(promo_code_for(origin), None, "{origin}");
        }
    }

    #[test]
    fn subdomains_do_not_inherit_the_code() {
        // 与 aff 表同一条：给错的站带码比查不到糟得多。
        assert_eq!(promo_code_for("https://evil.bestapi.store"), None);
    }

    #[test]
    fn every_code_satisfies_the_servers_format_rules() {
        // 服务端 `promo_service.go` 按码字符串直接查库，没有格式白名单
        // （不同于 aff 的 `isValidAffiliateCodeFormat`）。但**录表时录错**
        // 仍然是我们自己的问题：带空格的码查不到、小写的码可能查不到。
        for (host, code) in PROMO_CODES {
            assert!(!code.is_empty(), "{host} 的码是空的");
            assert_eq!(code.trim(), *code, "{host} 的码带空格");
        }
    }

    #[test]
    fn table_keys_are_already_registrable_domains() {
        // 表里的 key 若带 scheme / 端口 / 子域前缀，就永远查不到 ——
        // 因为查表前 `site_domain` 已经把输入归到注册域了。这是个静默失效，必须有闸。
        for (domain, _) in PROMO_CODES {
            assert_eq!(
                &crate::relay::identity::site_domain(domain),
                domain,
                "表里的 key 不是注册域: {domain}"
            );
        }
    }

    #[test]
    fn no_duplicate_hosts() {
        // 重复 key 时 `find` 只会命中第一条，第二条成了静默死数据。
        let mut hosts: Vec<&str> = PROMO_CODES.iter().map(|(h, _)| *h).collect();
        let before = hosts.len();
        hosts.sort_unstable();
        hosts.dedup();
        assert_eq!(hosts.len(), before, "表里有重复的 host");
    }
}
