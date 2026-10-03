//! Preparatory ZCode account adapters. No live Tauri command is registered.

pub(crate) mod checkpoint;
pub mod core;
pub mod native;
pub(crate) mod transaction;

#[cfg(test)]
mod roundtrip_tests;
