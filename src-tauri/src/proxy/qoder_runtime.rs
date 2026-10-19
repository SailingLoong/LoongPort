//! Admission for the fixed application-owned external Qoder runtime.
use super::qoder_host::HostConfig;
use std::path::PathBuf;
#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeError {
    NodeUnavailable,
    NodeVersionUnsupported,
    SdkMissing,
    SdkVersionUnsupported,
    SdkPathInvalid,
    SdkLoadFailed,
    SdkRuntimeMissing,
}
pub async fn resolve_runtime(
    node: PathBuf,
    worker: PathBuf,
    root: PathBuf,
) -> Result<HostConfig, RuntimeError> {
    use std::{process::Stdio, time::Duration};
    use tokio::process::Command;
    if !node.is_absolute() {
        return Err(RuntimeError::NodeUnavailable);
    }
    let node = node
        .canonicalize()
        .map_err(|_| RuntimeError::NodeUnavailable)?;
    let mut version = Command::new(&node);
    version
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(3), version.output())
        .await
        .map_err(|_| RuntimeError::NodeUnavailable)?
        .map_err(|_| RuntimeError::NodeUnavailable)?;
    let major = std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|s| s.trim().strip_prefix('v'))
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse::<u32>().ok());
    if !output.status.success() || !matches!(major,Some(n) if n>=22) {
        return Err(RuntimeError::NodeVersionUnsupported);
    }
    let mut probe = Command::new(&node);
    probe
        .arg(&worker)
        .arg("--check-runtime")
        .arg("--sdk-root")
        .arg(&root)
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        probe.env("SystemRoot", system_root);
    }
    let output = tokio::time::timeout(Duration::from_secs(10), probe.output())
        .await
        .map_err(|_| RuntimeError::SdkLoadFailed)?
        .map_err(|_| RuntimeError::SdkLoadFailed)?;
    let facts: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| RuntimeError::SdkLoadFailed)?;
    if !output.status.success() || facts["ok"] != true || facts["sdkVersion"] != "1.0.53" {
        return Err(match facts["code"].as_str() {
            Some("QODER_SDK_MISSING") => RuntimeError::SdkMissing,
            Some("QODER_SDK_PATH_INVALID") => RuntimeError::SdkPathInvalid,
            Some("QODER_SDK_VERSION_UNSUPPORTED") => RuntimeError::SdkVersionUnsupported,
            Some("QODER_SDK_RUNTIME_MISSING") => RuntimeError::SdkRuntimeMissing,
            _ => RuntimeError::SdkLoadFailed,
        });
    }
    Ok(HostConfig {
        node,
        worker,
        sdk_root: Some(root.canonicalize().map_err(|_| RuntimeError::SdkMissing)?),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn node() -> PathBuf {
        if let Some(path) = std::env::var_os("QODER_HOST_NODE") {
            return PathBuf::from(path).canonicalize().unwrap();
        }
        let binary = if cfg!(windows) { "node.exe" } else { "node" };
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(binary))
            .find(|p| p.is_file())
            .expect("Node is required for runtime integration tests")
            .canonicalize()
            .unwrap()
    }
    fn worker() -> PathBuf {
        PathBuf::from(
            std::env::var_os("QODER_HOST_PRODUCT_ROOT").unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .as_os_str()
                    .to_owned()
            }),
        )
        .join("scripts/qoder/worker.mjs")
    }
    #[tokio::test]
    async fn relative_node_is_not_admitted() {
        assert_eq!(
            resolve_runtime("node".into(), worker(), "/unused".into())
                .await
                .err(),
            Some(RuntimeError::NodeUnavailable)
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn old_node_version_fails_before_loading_sdk() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let n = d.path().join("node");
        std::fs::write(&n, "#!/bin/sh\nprintf 'v20.0.0\\n'\n").unwrap();
        std::fs::set_permissions(&n, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            resolve_runtime(n, worker(), d.path().into()).await.err(),
            Some(RuntimeError::NodeVersionUnsupported)
        );
    }
    #[tokio::test]
    #[ignore = "requires Node>=22 for SDK admission"]
    async fn missing_sdk_has_specific_admission_error() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_runtime(node(), worker(), d.path().into())
                .await
                .err(),
            Some(RuntimeError::SdkMissing)
        );
    }
    #[tokio::test]
    #[ignore = "requires official external SDK/runtime installation and an absolute QODER_HOST_NODE"]
    async fn actual_sdk_probe_uses_absolute_node_and_fixed_root() {
        let root = PathBuf::from(std::env::var_os("LOONGPORT_QODER_SDK_TEST_ROOT").unwrap());
        let host = resolve_runtime(node(), worker(), root.clone())
            .await
            .unwrap();
        assert!(host.node.is_absolute());
        assert_eq!(host.sdk_root, Some(root.canonicalize().unwrap()));
    }
}
