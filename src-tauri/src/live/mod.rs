//! Dormant upstream ownership and format-patching primitives. Runtime writers
//! remain on their existing owners until live/mode recovery integration.

pub mod engine;
pub mod floor;
pub mod patch;
pub mod residue;

#[cfg(test)]
mod loongport_tests;

#[cfg(test)]
mod loongport_patch_tests;
