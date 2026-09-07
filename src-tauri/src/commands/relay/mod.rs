//! LoongPort 中转站的 Tauri 命令层。
//!
//! 中转站命令：
//!
//! | 命令 | 干什么 |
//! |---|---|
//! | [`relay_status`] | 首启该弹哪个弹窗、当前是什么状态 |
//! | [`relay_import_site`] | 发现协议；必要时让用户在可见 WebView 完成网页验证，并在同一会话登录 |
//! | [`relay_login`] | 开登录 WebView，等凭据回来 |
//! | [`relay_refresh`] | 探活 + 重拉分组（每组备好 sk → 写成 provider） |
//! | [`relay_switch_tier`] | 选分组 → 退 ChatGPT → 切换 → 重开 |
//!
//! ## 为什么切换编排在 Rust 侧而不是前端
//!
//! 「退出 ChatGPT → 切换 → 重开」如果写在前端的按钮回调里，那么**托盘快切、deeplink 导入、
//! 项目快照**这三条路径都会绕过它（它们在 Rust 侧直接调 `ProviderService::switch`），用户
//! 从托盘切完就会发现 codex 还连着旧分组。放在这一层是让「切换分组」只有一个入口。
//!
//! ⚠️ 编排在这一层**不等于**别处进不来 —— 那要靠 [`crate::relay::managed`] 的守卫。
//! 已收口的是托盘（列表里剔掉托管项）与 `switch_provider` / `update_provider` /
//! `delete_provider` 三条通用命令。
//!
//! **项目快照走的是「提示而不是替他退」**（`services::profile::apply`，2026-08-04）：
//! 它切 codex 供应商后会往 warnings 里加一句「请重启 ChatGPT」，而**不**替用户退掉那个
//! app —— 应用快照是「一次动作切一批 app」，用户点的是「切到这个项目」，把它读成
//! 「同意关掉我正开着的 ChatGPT」是过度解释（`switch_provider` 的文档把这条定死了：
//! `None` = 不碰 ChatGPT）。那句 warning 经 `profiles.applyWarnings` 的 toast 到达用户。
//!
//! **deeplink 导入仍直接调 `ProviderService::switch`**（要构造一条带 `enabled=true` 的
//! deep link 才碰得到，优先级低于上面几条）。

use futures::future::join_all;
use serde::Serialize;
use std::{
    future::Future,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::events::{emit_provider_switched, PURCHASE_CLOSED};
use crate::provider::Provider;
use crate::relay::provision::models_from_settings;
// ⚠️ 这里**有意不导入** `balance` / `imagegen` / `login` / `provision` / `site_config`
// 这五个 crate::relay 子模块 —— 本目录有同名领域模块，`use super::*` 会让子模块名
// 遮蔽它们。需要那五个模块的文件各自 `use crate::relay::<名>;`。
use crate::relay::provision::DEFAULT_MODEL;
use crate::relay::{
    backend, browser_bridge, chatgpt_app, creds, discovery, imagegen_mcp,
    model_verification::target as verification_target, newapi, newapi_provision, newapi_purchase,
    platform_map, pricing, provider_fingerprint, purchase, reconcile, remote_config, sub2api,
};
use crate::services::ProviderService;
use crate::store::AppState;

/// 默认中转站域名。域名输入框的底纹词，用户直接点确定就用它。
///
/// ⚠️ **它不再是维护者自己的站**（2026-08-04 从 `bestapi.store` 改过来 —— 那个站
/// 没有精力持续运维，默认值不该指向一个自己都不盯着的站）。那个巧合曾让三处文档把
/// 「默认站」与「维护者自己的站」写成一件事（本文档、[`crate::relay::aff`] 的
/// `aff_code_for` 与它那条「维护者自己的站有意缺席」的测试）—— 别再绑回去。
///
/// ⇒ 换这个值时**要重新确认它在 [`crate::relay::aff`] / [`crate::relay::promo`]
/// 两张内置表里各该有什么**（两张表各自按 host 查，彼此独立，但都与「谁是默认站」
/// 有关：默认站是最常被走到的那条路）。当前这个站在 aff 内置表里、不在 promo 内置表里，
/// 前者**本模块 `tests` 里有一条闸钉着**（跟着这个常量一起改，它会当场告诉你）。
//
// ⚠️ 有意**不写那条测试的名字**：rustdoc 的 intra-doc link 链不进 `#[cfg(test)]`，
// 写成反引号裸名字就没有任何东西能验它 —— 2026-08-04 同一次改名里连漏两处指针
// （两路 review 各抓一次）。指「本模块 tests 里」而不指名字，改名就不会让它悬空。
const DEFAULT_SITE: &str = "790053500.com";

/// 这条 provider 是不是 LoongPort 管的。
///
/// 判据本身在 [`crate::relay::managed`]（唯一来源，托盘与命令层守卫也用它）；这里只是
/// 把「按 `&Provider` 判」这个便利形状留在本地，别在这儿重写前缀。
fn is_managed(p: &Provider) -> bool {
    crate::relay::is_managed(&p.id)
}

/// 托管 provider 的 meta。
///
/// **`apiFormat` 必须显式写 `openai_responses`**：不写它会落到 `ProxyChat` profile，而那是
/// 唯一会去 spawn `codex debug models --bundled` 子进程的分支。sub2api 的 openai 网关原生走
/// Responses，写对了就永远走内嵌模板、不起子进程。
/// 托管档位的 `meta`。
///
/// `account_id` 是**归属依据**，不是可选的装饰：同一个站可以挂多个账号，而
/// `website_url` 只记站点 ⇒ 少了它，清理 / 重建 / 删站三处都会误伤同站另一个账号的
/// 档位（见 [`crate::provider::ProviderMeta::loongport_account_id`] 的文档）。
fn managed_meta(
    app_type: &AppType,
    account_id: Option<i64>,
    group: Option<crate::provider::LoongportGroupIdentity>,
) -> crate::provider::ProviderMeta {
    crate::provider::ProviderMeta {
        // `api_format` **只被 `codex_config.rs` 消费**（`CodexCatalogToolProfile::from_api_format`），
        // 对 claude / gemini 无意义 —— 给它们填值不会有人读，反而让人以为那里有语义。
        api_format: match app_type {
            AppType::Codex => Some("openai_responses".to_string()),
            _ => None,
        },
        loongport_account_id: account_id,
        loongport_group: group,
        ..Default::default()
    }
}

fn with_conn<T>(
    state: &AppState,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let conn = state
        .db
        .conn
        .lock()
        .map_err(|e| AppError::Database(format!("获取数据库连接失败: {e}")))?;
    f(&conn)
}

// ─────────────────────── 领域模块与再导出 ───────────────────────
// 拆分说明（2026-09-07）：本模块曾是 1.1 万行的单文件命令层。现按领域切成子模块；
// 拆分是纯位置移动，不改任何行为 —— 外部路径（`crate::commands::relay_*` 命令、
// `super::relay::<helper>`、托盘/deeplink/maintenance 的 pub(crate) 接缝）全部经由
// 下面的 glob 再导出保持不变。子模块共享本文件的 use 块（各自 `use super::*`），
// 生产代码各归各域；跨域测试仍集中在 tests.rs（命令层的集成式测试）。
mod balance;
mod directory;
mod imagegen;
mod login;
mod official;
mod provision;
mod rows;
mod session;
mod site_config;
mod switch;
mod windows;

pub use balance::*;
pub use directory::*;
pub use imagegen::*;
pub use login::*;
pub use official::*;
pub use provision::*;
pub use rows::*;
pub use session::*;
pub use site_config::*;
pub use switch::*;
pub use windows::*;

#[cfg(test)]
mod test_support;
