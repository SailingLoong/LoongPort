//! Compile actual codec source with existing dependencies; no native IO owner.
#[path = "../../src-tauri/src/zcode_accounts/bundle.rs"]
mod bundle;
#[path = "../../src-tauri/src/zcode_accounts/bundle_limits.rs"]
mod bundle_limits;
#[path = "../../src-tauri/src/zcode_accounts/core.rs"]
pub mod core;
#[path = "../../src-tauri/src/zcode_accounts/desktop_text.rs"]
mod desktop_text;
#[path = "../../src-tauri/src/zcode_accounts/native.rs"]
pub mod native;
