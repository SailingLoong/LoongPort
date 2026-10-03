//! Preparatory ZCode account adapters. No live Tauri command is registered.

pub(crate) mod admission;
pub(crate) mod checkpoint;
pub mod core;
pub mod native;
pub(crate) mod recovery;
#[cfg(feature = "gui")]
pub(crate) mod runtime;
pub(crate) mod transaction;

#[cfg(test)]
mod roundtrip_tests;
