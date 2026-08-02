//! LoongPort 运营商接入层（V2 极简版）。
//!
//! 与 cc-switch 的关系：本模块是 fork 新增的，把「一个 sub2api 账号」变成 codex 可用的
//! provider 记录。**只服务 codex 一个 CLI、只对接 sub2api 一种运营商**。
//!
//! 模块划分：
//!
//! - [`api`]：sub2api 的窄 DTO + HTTP 客户端（探测 / 分组 / Key / 余额）
//! - [`creds`]：凭据的内存结构与持久化（`loongport_credential` 表）
//! - [`login`]：登录 WebView（加载运营商真实登录页，从 localStorage 取凭据回传）
//! - [`provision`]：分组 → sk → codex provider 的展开
//! - [`managed`]：「这条 provider 是不是托管的」的唯一判据 + 各入口的守卫
//! - [`chatgpt_app`]：ChatGPT 桌面版（bundle id `com.openai.codex`）的退出与重开
//!
//! ## 与 V1 LoongPort 的差异（有意简化，不是遗漏）
//!
//! V1 的 `operator/` 有 22 个文件 11881 行，因为它要同时满足：三个 CLI（claude/codex/
//! gemini）、多运营商、云同步边界、failover 队列、Windows 一等公民。V2 全部收窄到
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

pub mod api;
pub mod chatgpt_app;
pub mod creds;
pub mod login;
pub mod managed;
pub mod provision;

pub use managed::{filter_unmanaged, is_managed, reject_if_managed};
