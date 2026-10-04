//! Account commands share the existing owner; native access requires the platform manifest gate.

pub(crate) mod admission;
#[cfg(feature = "gui")]
pub(crate) mod api;
pub(crate) mod checkpoint;
pub mod core;
pub mod native;
pub(crate) mod native_context;
pub(crate) mod recovery;
#[cfg(feature = "gui")]
pub(crate) mod runtime;
pub(crate) mod transaction;

#[cfg(test)]
mod roundtrip_tests;
