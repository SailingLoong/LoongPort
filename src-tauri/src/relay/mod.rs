//! LoongPort 中转站接入层。
//!
//! 与 cc-switch 的关系：本模块是 fork 新增的，把「一个中转站账号」变成 CLI 可用的
//! provider 记录。
//!
//! ## 目标形态是多中转站 × 多 CLI，当前实现只覆盖一格
//!
//! | 维度 | 已实现 | 缺口在哪 |
//! |---|---|---|
//! | 中转站 | sub2api | [`api`] 的 DTO 与端点是 sub2api 形状。接 new-api 要在这一层分化出 provider trait |
//! | CLI | codex | **只缺配置写入形状**：[`provision::settings_config_for`] 生成的是 codex 的 TOML |
//! | 平台 | macOS | [`login`] 的凭据回传与 [`chatgpt_app`] 只在 macOS 实测过 |
//!
//! ⚠️ **别把「当前只实现了 codex」读成「设计上只支持 codex」** —— 骨架已按多维度建好：
//!
//! - Key 命名契约**四段含 platform**（[`provision`]）—— 跨平台不会撞号
//! - [`platform_map`] 六个平台**全覆盖**，且有编译期基数闸
//! - 命令层签名**都吃 `app_id`**（`commands::relay`），非 codex 明确报错而非静默走错分支
//! - [`creds`] 的 `login_identifier` 是**中立命名**（不叫 `account_email`），
//!   正是为 new-api 那种用 username 登录的中转站留的 —— 列名进了 schema，改它是迁移不是重构
//!
//! 所以扩展时改的是「加一份实现」，不是「拆掉一个假设」。
//!
//! 模块划分：
//!
//! - [`api`]：中转站的窄 DTO + HTTP 客户端（探测 / 分组 / Key / 余额 / 倍率）。**当前是 sub2api 形状**
//! - [`balance`]：一行余额的有序回落链（cc-switch → sub2api sk → JWT），两类行共用
//! - [`creds`]：凭据的内存结构与持久化（`loongport_credential` 表）
//! - [`aff`]：站点 host → 我们的注册邀请码（编译期常量表）
//! - [`identity`]：站点身份的唯一源 —— 注册域（apex）与取数地址两个归一，身份判断全走它
//! - [`login`]：登录 WebView（加载中转站真实登录页，从 localStorage 取凭据回传）
//! - [`purchase`]：充值 WebView（**与 `login` 方向相反** —— 把已有登录态注入进充值页）
//! - [`platform_map`]：sub2api 的 `platform` ↔ cc-switch 的 `AppType` 映射表（唯一一处映射数据）
//! - [`provision`]：分组 → sk → codex provider 的展开
//! - [`reconcile`]：扣费对账的余额快照表（`relay_balance_snapshots`，唯一采样点）
//! - [`remote_config`]：远端配置（赞助商 + 邀请码，Ed25519 验签、三层回落）
//! - [`stats`]：匿名使用统计（只报站点 host 与个数，默认开、可关）
//! - [`managed`]：「这条 provider 是不是托管的」的唯一判据 + 各入口的守卫
//! - [`chatgpt_app`]：ChatGPT 桌面版（bundle id `com.openai.codex`）的退出与重开
//!
//! ## 与 V1 LoongPort 的差异（有意简化，不是遗漏）
//!
//! V1 的 `relay/` 有 22 个文件 11881 行，因为它要同时满足：三个 CLI（claude/codex/
//! gemini）、多中转站、云同步边界、failover 队列、Windows 一等公民。V2 全部收窄到
//! 「codex × sub2api × macOS」，所以：
//!
//! - **凭据回传走 `on_navigation` 拦一次自定义 scheme 跳转**，不做 V1 那套 `document.title`
//!   分片协议（LP1 握手 + stop-and-wait + FNV 校验 + 重传，1155 行）。那套是为
//!   Windows WebView2 的 4096 字符标题上限设计的；URL 长度上限远高于它，macOS 单平台
//!   下一次跳转就能把 4 个键送完。**要加 Windows 时这里可能要回退到 V1 那套**，见
//!   [`login`] 的模块文档。
//! - **Key 命名契约是四段** `LoongPort/<device-id>/<platform>/<group-id>`，与 V1 同构。
//!   （曾砍成三段，理由是「V2 只有 openai，那段恒定即冗余」；多平台之后该理由不成立 ——
//!   分组 id 只在平台内唯一，跨平台会撞号。2026-08-02 改回四段，见 [`provision`]。）
//! - **不做云同步边界**（V1 的 `sync_guard.rs` 1297 行）：V2 不接 WebDAV/S3。
//! - **不做 failover 队列维护**（V1 `expand.rs` 的一半）：V2 第一版不开本地代理。

pub mod aff;
pub mod api;
pub mod backend;
pub mod balance;
pub mod browser_bridge;
pub mod cc_switch_import;
pub mod chatgpt_app;
pub mod creds;
pub mod discovery;
pub mod identity;
pub mod imagegen_mcp;
pub mod leaderboard;
pub mod login;
pub mod managed;
pub mod newapi;
pub mod newapi_provision;
pub mod newapi_purchase;
pub mod onboarding;
// Phase 1 defines this crate-internal contract before Phase 2 consumes it.
#[allow(dead_code)]
pub mod model_verification;
pub mod platform_map;
pub mod pricing;
pub mod promo;
pub(crate) mod provider_fingerprint;
pub mod provision;
pub mod purchase;
pub mod purchase_session;
pub mod reconcile;
pub mod remote_config;
pub mod site_config;
pub mod site_probe;
pub mod stats;
pub mod transit;

pub use managed::{is_managed, reject_if_managed};
