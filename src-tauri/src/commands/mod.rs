#![allow(non_snake_case)]

mod auth;
mod balance;
mod codex_oauth;
mod coding_plan;
mod config;
mod copilot;
mod deeplink;
mod env;
mod failover;
mod global_proxy;
mod hermes;
mod import_export;
mod mcp;
mod misc;
mod model_fetch;
mod omo;
mod openclaw;
mod plugin;
mod profile;
mod prompt;
mod provider;
mod proxy;
mod relay;
mod session_manager;
mod settings;
pub mod skill;
mod stream_check;
mod subscription;
// `pub(crate)`：`relay::cc_switch_import` 的导入流程在导入后要调
// `run_post_import_sync`（见 `import_export.rs` 的 `import_config_from_file` 同款后置）。
pub(crate) mod sync_support;
// ⚠️ **不能写成 `pub mod vendor;`**：`crate::vendor`（契约层）已经占了这个模块名，
// 而 lib.rs 有一句 `pub use commands::*` ⇒ 两个 `vendor` 撞在同一个命名空间里，
// rustc 报 `hidden_glob_reexports`，而 `-D warnings` 把它当错误。
// 与本目录其它模块同一形状：模块私有、符号 glob 导出。
mod vendor;
mod xai_oauth;

mod lightweight;
mod s3_sync;
mod usage;
mod webdav_sync;
mod workspace;

pub use auth::*;
pub use balance::*;
pub use codex_oauth::*;
pub use coding_plan::*;
pub use config::*;
pub use copilot::*;
pub use deeplink::*;
pub use env::*;
pub use failover::*;
pub use global_proxy::*;
pub use hermes::*;
pub use import_export::*;
pub use mcp::*;
pub use misc::*;
pub use model_fetch::*;
pub use omo::*;
pub use openclaw::*;
pub use plugin::*;
pub use profile::*;
pub use prompt::*;
pub use provider::*;
pub use proxy::*;
pub use relay::*;
pub use session_manager::*;
pub use settings::*;
pub use skill::*;
pub use stream_check::*;
pub use subscription::*;
pub use vendor::*;
pub use xai_oauth::*;

pub use lightweight::*;
pub use s3_sync::*;
pub use usage::*;
pub use webdav_sync::*;
pub use workspace::*;
