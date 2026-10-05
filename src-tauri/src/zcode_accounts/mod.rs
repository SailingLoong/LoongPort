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

/// Absolute virtual paths for pure fixtures; never creates or opens these paths.
#[cfg(test)]
fn synthetic_test_path(relative: &str) -> std::path::PathBuf {
    let root = if cfg!(windows) {
        r"C:\synthetic"
    } else {
        "/synthetic"
    };
    let path = std::path::Path::new(root).join(relative);
    assert!(path.is_absolute());
    path
}
