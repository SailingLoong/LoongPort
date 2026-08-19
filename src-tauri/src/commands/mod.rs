#![allow(non_snake_case)]

mod auth;
// 自动模式（系统按策略挑托管档位）：选路在 proxy 层，这里是开关/策略命令层。
pub(crate) mod auto_mode;
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
mod model_verification;
mod omo;
mod onboarding;
mod openclaw;
mod pi;
mod plugin;
mod profile;
mod prompt;
mod provider;
mod proxy;
// 中转站扣费对账：`commands/relay.rs` 已经 8819+ 行，对账命令单独放，不再往里堆。
mod reconcile;
mod relay;
mod session_manager;
mod settings;
// 「点 Star 领注册礼」的机制层：星数取数 / gh 代点 / 邀请 payload。
// 策略在 onboarding（新人首启）与前端（红点入口），机制两端共用所以单独收拢。
pub mod skill;
mod star_reward;
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
pub use auto_mode::*;
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
pub use model_verification::*;
pub use omo::*;
// 新人引导：与 relay 主流程解耦的引导调度（判据 + 一次性标志 + 官方站注册窗）。
pub use onboarding::*;
pub use openclaw::*;
pub(crate) use pi::*;
pub use plugin::*;
pub use profile::*;
pub use prompt::*;
pub use provider::*;
pub use proxy::*;
pub use reconcile::*;
pub use relay::*;
pub use session_manager::*;
pub use settings::*;
pub use skill::*;
pub use star_reward::*;
pub use stream_check::*;
pub use subscription::*;
pub use vendor::*;
pub use xai_oauth::*;

pub use lightweight::*;
pub use s3_sync::*;
pub use usage::*;
pub use webdav_sync::*;
pub use workspace::*;
