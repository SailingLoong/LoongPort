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
//! ## 为什么 `bestapi.store` 在本表里、却有意不在 aff 表里
//!
//! 那两张表的排除理由完全不同，别看到「同一个站」就以为矛盾：
//!
//! - **aff 表排除它**：服务端拒绝自己邀请自己（`affiliate_service.go:300`），
//!   `inviterSummary.UserID == userID` ⇒ `ErrAffiliateCodeInvalid`。
//! - **promo 表包含它**：优惠码不是邀请关系，是**站主给新用户的赠额活动** ——
//!   站主给自己站的新用户发赠额天经地义，服务端**没有任何「邀请人 == 被邀请人」
//!   那类身份比对**。`promo_service.go:91` 的 `ApplyPromoCode` 查两样：
//!   码本身有效（status / 过期 / 用量上限，`promo_code.go:37` 的 `CanUse`），
//!   以及**这个用户没用过这个码**（`promo_service.go:117`，
//!   命中则 `ErrPromoCodeAlreadyUsed`）。两者都与「谁是站主」无关。
//!
//! ## 为什么只有这一个站（2026-08-04 维护者拍板）
//!
//! `LOONGPORT` 是维护者在**自己的站** `bestapi.store` 后台建的码。
//! 别的站的优惠码得由那些站主自己建，我们无从知晓。
//!
//! ⚠️ **有意不做「所有站都试着填 LOONGPORT」** —— 别的站没建这个码，
//! 注册页的实时校验（`RegisterView.vue:599` 的 `validatePromoCodeDebounced`）
//! 会给一个红框 + 「优惠码无效」⇒ 用户以为自己或我们出错了。
//! 一个错误的红框比不填糟得多。
//!
//! ## 码的格式：我们只搬运，不校验
//!
//! 与 [`super::aff`] 同一条理由 —— 规则在服务端且可能随版本变。
//! 但**录表时录错**是我们自己的问题，所以下面有一条编译期之外、录入那一刻的闸。

/// 站点 host → 注册优惠码。
///
/// key 是**归一后的 host**（小写、去 `www.`、不带端口），与 [`super::aff::AFF_CODES`]
/// 同一套归一规则 —— 两处共用 [`super::aff::lookup_host`]，不各写一份。
const PROMO_CODES: &[(&str, &str)] = &[
    // 维护者自己的站。⚠️ 与 aff 表**有意相反**（那张表排除它），理由见模块文档。
    ("bestapi.store", "LOONGPORT"),
];

/// 查这个站有没有注册优惠码。
///
/// `None` = 表里没有 ⇒ 调用方**什么都不做**（不预填那个框）。
/// 绝大多数站都会走这条路。
pub fn promo_code_for(site_origin: &str) -> Option<&'static str> {
    let host = super::aff::lookup_host(site_origin);
    PROMO_CODES
        .iter()
        .find(|(table_host, _)| *table_host == host)
        .map(|(_, code)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_code_for_the_maintainers_own_site() {
        assert_eq!(promo_code_for("https://bestapi.store"), Some("LOONGPORT"));
    }

    /// ⭐ **这条钉住「promo 表包含 bestapi.store 而 aff 表排除它」不是矛盾。**
    ///
    /// 两张表的排除理由不同（见模块文档）。会有人看到这个「不一致」就来
    /// 「修正」其中一张 —— 改哪张都是错的：给 aff 表补上它会让服务端日志多一条
    /// `ErrAffiliateCodeInvalid`；从 promo 表删掉它会让用户白丢赠额。
    #[test]
    fn the_two_tables_deliberately_disagree_about_the_maintainers_site() {
        let origin = "https://bestapi.store";
        assert_eq!(
            promo_code_for(origin),
            Some("LOONGPORT"),
            "优惠码是站主给新用户的赠额，自己的站当然要给"
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
        // 复用 `aff::lookup_host` ⇒ 端口 / `www.` / 大小写的归一行为**必须一致**。
        // 各写一份迟早会漂成「aff 查到了 promo 没查到」的静默失效。
        for origin in [
            "https://www.bestapi.store",
            "https://BestApi.store",
            "https://bestapi.store:443",
            "bestapi.store",
        ] {
            assert_eq!(promo_code_for(origin), Some("LOONGPORT"), "{origin}");
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
    fn table_hosts_are_already_normalized() {
        // 表里的 key 若带 scheme / 端口 / `www.` / 大写，就永远查不到 ——
        // 因为查表前 `lookup_host` 已经把输入归一了。这是个静默失效，必须有闸。
        for (host, _) in PROMO_CODES {
            assert_eq!(
                &super::super::aff::lookup_host(host),
                host,
                "表里的 key 没归一: {host}"
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
