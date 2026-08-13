//! 官网直连账号（vendor）层。与 [`crate::relay`]（中转站）**平级并列**。
//!
//! ## 为什么不复用 relay 的 Client
//!
//! 差异不在 HTTP 形状，在语义：中转站一个账号给**多个分组**（多档位、有倍率）、
//! key 列表明文可认领、站点域名要用户输入并探测；官网一个账号就**一个 endpoint**、
//! 无倍率、**明文只在创建那一刻给一次**、域名是编译期常量。
//! 硬合会造出一堆语义不成立的方法（`list_groups` 返回什么？倍率填什么？）。
//!
//! ## 为什么是 enum 而不是 trait
//!
//! `async fn` in trait 返回 RPITIT ⇒ **不是 dyn-compatible** ⇒ `Box<dyn _>` 不成立，
//! 而「按 vendor_id 取一个实现」正需要它；`async-trait` 不是本仓的直接依赖。
//! enum 静态分派零新依赖、编译期穷尽，与 [`crate::relay::platform_map`] 的风格一致。
//! 加第二家厂商 = 加一个变体 + 编译器把所有没覆盖的 match 点报出来。

pub mod creds;
pub mod deepseek;
pub mod provision;

/// 支持的官网厂商。加一家就加一个变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    DeepSeek,
}

impl Vendor {
    /// 稳定标识，进数据库与 provider id 的哈希。⚠️ 改它是迁移不是重构。
    pub fn vendor_id(&self) -> &'static str {
        match self {
            Vendor::DeepSeek => "deepseek",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Vendor::DeepSeek => "DeepSeek",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "deepseek" => Some(Vendor::DeepSeek),
            _ => None,
        }
    }
}

/// 一把已存在的 key。**有意不含明文字段** —— 列表接口拿不到明文，
/// 让类型把这个事实钉住。删除靠这里的三元组定位（不是靠名字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorKey {
    pub name: String,
    /// 脱敏值（含 `*`）。删除请求里叫 `redacted_key`。
    pub redacted_key: String,
    /// Unix 秒。
    pub created_at: i64,
    pub tracking_id: String,
}

/// 登录后确认的账号身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorAccount {
    /// 厂商侧用户 id。**是 String 不是 i64** —— DeepSeek 给的是 UUID。
    pub account_id: String,
    /// 给人看的名字。
    pub label: String,
    /// 重登时预填进登录框的值（DeepSeek 是手机号）。
    pub login_identifier: String,
}

/// 结构化错误。命令层与 UI 按它分派，**不许靠字符串匹配**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorError {
    /// 登录态失效（DeepSeek 的 `code: 40002`）。⚠️ 只清 token，**不清 api_key**。
    AuthExpired,
    /// key 数量到上限（DeepSeek 是 100 把，`biz_code: 1`）。
    KeyLimitReached,
    /// 本该拿到明文却拿到脱敏值。见 [`deepseek::validate_plaintext_key`]。
    RedactedValueReturned,
    Transient(String),
}

impl From<VendorError> for crate::error::AppError {
    fn from(e: VendorError) -> Self {
        crate::error::AppError::Config(match e {
            VendorError::AuthExpired => "登录已过期，请重新登录".to_string(),
            VendorError::KeyLimitReached => {
                "账号内 API key 已达 100 上限，请到官网删除一些".to_string()
            }
            VendorError::RedactedValueReturned => {
                "官网返回的密钥是脱敏值而非明文，已中止".to_string()
            }
            VendorError::Transient(m) => m,
        })
    }
}

/// 本客户端建的 key 的名字。
///
/// ```text
/// LoongPort专用/a<account-id>
/// ```
///
/// ⚠️ **中文「专用」二字是维护者定的字面量**，用户要在官网列表里一眼认出来。
///
/// ## 为什么按**账号**而不按机器（2026-08-04 改，维护者实测推翻原设计）
///
/// 初版第二段是 `device_id`，理由是「多台机器各认自己那把，否则 A 机器改了 Key、
/// B 机器的配置就悄悄失效」。**那个理由站不住** —— 维护者在 relay 侧实测证伪：
///
/// `provision` **从不改动已有 Key**（认领到就直接用），能换掉 sk 的只有「用户去
/// 网页端手工删了重建」，而那种情况下不论按机器还是按账号，其它机器都一样要重新
/// provision。用 device_id 换来的不是安全，只是**每台机器各堆一份**。
///
/// 代价是真的：他一个 sub2api 账号下堆了 11 把、只有 3 把在用。
/// DeepSeek 这边上限 100 把，三台 Mac 各建一套同样是白耗。
///
/// 前缀 `a` 让人一眼看出哪段是账号（对齐 relay 侧
/// `LoongPort/a<account-id>/<platform>/<group-id>` 的写法）。
///
/// ## 与 relay 的差异：无 platform / group 段
///
/// DeepSeek 的一把 sk **六个平台通吃**（同一把同时能请求 `/v1`、`/anthropic`、
/// 根路径），所以没有「每平台一把」的概念 —— 两段就够。
///
/// ⚠️ 这个名字进了服务端、跨端可见，改它等于所有已建 key 认领不回来。
/// 本次改动**有意接受那个代价**（孤儿是一次性的，而按机器命名是每加一台永久 +1）。
pub fn key_name_for(account_id: &str) -> String {
    format!("LoongPort专用/a{account_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key 名字按**账号**而不按机器 —— 那样三台机器共用一把
    /// （理由见 `key_name_for` 的文档：按机器命名的理由已被实测推翻）。
    #[test]
    fn key_name_is_scoped_to_the_account_not_the_machine() {
        let name = key_name_for("11eb18b1-2784-43ba-8324-16c5eef7f72c");
        assert_eq!(
            name, "LoongPort专用/a11eb18b1-2784-43ba-8324-16c5eef7f72c",
            "两段：中文前缀 + a<account-id>"
        );
        // 同一个账号在任何机器上算出的名字都一样 —— 这就是「共用一把」的机制。
        assert_eq!(name, key_name_for("11eb18b1-2784-43ba-8324-16c5eef7f72c"));
        // 不同账号必须不同（否则会删到别人那把）。
        assert_ne!(name, key_name_for("other-account"));
    }

    #[test]
    fn vendor_id_round_trips() {
        assert_eq!(Vendor::from_id("deepseek"), Some(Vendor::DeepSeek));
        assert_eq!(Vendor::from_id("kimi"), None);
        assert_eq!(Vendor::DeepSeek.vendor_id(), "deepseek");
    }
}
