//! Account commands share the existing owner; native access requires the platform manifest gate.

pub(crate) mod admission;
#[cfg(feature = "gui")]
pub(crate) mod api;
#[cfg(any(feature = "gui", test))]
pub(crate) mod bundle;
#[cfg(any(feature = "gui", test))]
pub(crate) mod bundle_export;
#[cfg(any(feature = "gui", test))]
pub(crate) mod bundle_import;
#[cfg(any(feature = "gui", test))]
pub(crate) mod bundle_limits;
#[cfg(any(feature = "gui", test))]
pub(crate) mod capture_reviews;
pub(crate) mod checkpoint;
#[cfg(any(feature = "gui", test))]
pub(crate) mod connection_check;
pub mod core;
pub(crate) mod desktop_text;
#[cfg(any(target_os = "macos", all(test, unix)))]
pub(crate) mod discovery_paths;
#[cfg(any(feature = "gui", test))]
pub(crate) mod import_reviews;
pub(crate) mod key_intent;
#[cfg(all(feature = "gui", target_os = "macos"))]
pub(crate) mod latest_version;
pub(crate) mod library_context;
pub mod native;
pub(crate) mod native_context;
#[cfg(any(all(feature = "gui", target_os = "macos"), test))]
pub(crate) mod native_lifecycle;
#[cfg(all(feature = "gui", target_os = "macos"))]
pub(crate) mod native_process_control;
#[cfg(any(feature = "gui", test))]
pub(crate) mod oauth;
#[cfg(any(feature = "gui", test))]
pub(crate) mod oauth_account;
#[cfg(feature = "gui")]
pub(crate) mod oauth_runtime;
#[cfg(any(feature = "gui", test))]
pub(crate) mod oauth_service;
#[cfg(any(feature = "gui", test))]
pub(crate) mod official;
#[cfg(any(feature = "gui", test))]
pub(crate) mod official_http;
pub(crate) mod operation_log;
pub(crate) mod recovery;
#[cfg(feature = "gui")]
pub(crate) mod runtime;
#[cfg(any(feature = "gui", test))]
pub(crate) mod session_checks;
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
