//! Admission for the application-owned Node HTTP protocol worker; no SDK install.
use super::qoder_host::HostConfig;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeError {
    NodeUnavailable,
    NodeVersionUnsupported,
    WorkerUnavailable,
    WorkerProtocolUnsupported,
    Cleanup,
}

async fn probe(
    node: &Path,
    args: &[&std::ffi::OsStr],
    timeout: Duration,
) -> Result<Vec<u8>, RuntimeError> {
    let mut command = Command::new(node);
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command
        .spawn()
        .map_err(|_| RuntimeError::WorkerUnavailable)?;
    let Some(stdout) = child.stdout.take() else {
        return match tokio::time::timeout(Duration::from_secs(2), child.kill()).await {
            Ok(Ok(())) => Err(RuntimeError::WorkerUnavailable),
            _ => Err(RuntimeError::Cleanup),
        };
    };
    let work = async {
        let mut bytes = Vec::new();
        stdout
            .take(4097)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ())?;
        if bytes.len() > 4096 {
            return Err(());
        }
        let status = child.wait().await.map_err(|_| ())?;
        if !status.success() {
            return Err(());
        }
        Ok(bytes)
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(bytes)) => Ok(bytes),
        _ => match tokio::time::timeout(Duration::from_secs(2), child.kill()).await {
            Ok(Ok(())) => Err(RuntimeError::WorkerUnavailable),
            _ => Err(RuntimeError::Cleanup),
        },
    }
}

pub async fn resolve_runtime(node: PathBuf, worker: PathBuf) -> Result<HostConfig, RuntimeError> {
    if !node.is_absolute() {
        return Err(RuntimeError::NodeUnavailable);
    }
    if !worker.is_absolute() {
        return Err(RuntimeError::WorkerUnavailable);
    }
    let node = node
        .canonicalize()
        .map_err(|_| RuntimeError::NodeUnavailable)?;
    let worker = worker
        .canonicalize()
        .map_err(|_| RuntimeError::WorkerUnavailable)?;
    let output = probe(&node, &["--version".as_ref()], Duration::from_secs(3)).await?;
    let major = std::str::from_utf8(&output)
        .ok()
        .and_then(|s| s.trim().strip_prefix('v'))
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse::<u32>().ok());
    if !matches!(major, Some(n) if n >= 22) {
        return Err(RuntimeError::NodeVersionUnsupported);
    }
    let output = probe(
        &node,
        &[worker.as_os_str(), "--check-runtime".as_ref()],
        Duration::from_secs(10),
    )
    .await?;
    let facts: serde_json::Value =
        serde_json::from_slice(&output).map_err(|_| RuntimeError::WorkerProtocolUnsupported)?;
    if facts["ok"] != true || facts["transport"] != "qoder-http" || facts["protocolVersion"] != 1 {
        return Err(RuntimeError::WorkerProtocolUnsupported);
    }
    Ok(HostConfig { node, worker })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn relative_runtime_paths_are_rejected() {
        assert_eq!(
            resolve_runtime("node".into(), "worker.mjs".into())
                .await
                .err(),
            Some(RuntimeError::NodeUnavailable)
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn old_node_fails_without_loading_worker() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let node = root.path().join("node");
        let worker = root.path().join("worker.mjs");
        std::fs::write(&node, "#!/bin/sh\nprintf 'v20.0.0\\n'\n").unwrap();
        std::fs::write(&worker, "throw new Error('must not load')").unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            resolve_runtime(node, worker).await.err(),
            Some(RuntimeError::NodeVersionUnsupported)
        );
    }
}
