//! Relay-site integration.
//!
//! Protocol clients own HTTP contracts; discovery and backend dispatch select the protocol.
//! `provision` and `newapi_provision` claim keys and produce tiers for command-layer persistence.
//! `model_selection` owns deterministic policy; `provider_config` owns client configuration.
//! `remote_config` owns signed configuration snapshots and policy loading.
//! Credentials, managed identities, catalogs and subscription windows each have a single owner.

pub mod aff;
pub mod backend;
pub mod balance;
pub mod browser_bridge;
pub mod browser_connect;
pub mod cc_switch_import;
pub mod chatgpt_app;
pub mod creds;
pub mod directory;
pub mod discovery;
pub mod identity;
pub mod imagegen;
pub mod imagegen_mcp;
pub mod login;
pub mod managed;
pub(crate) mod model_catalog;
pub mod model_selection;
pub mod newapi;
pub mod newapi_provision;
#[cfg(feature = "gui")]
pub mod newapi_purchase;
pub mod onboarding;
pub mod provider_config;
pub mod sub2api;
pub mod tier_windows;
// Phase 1 defines this crate-internal contract before Phase 2 consumes it.
#[allow(dead_code)]
pub mod model_verification;
pub mod platform_map;
pub mod plaza;
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
