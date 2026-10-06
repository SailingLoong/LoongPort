//! Public update metadata only; never grants installed-build admission.
use serde::Deserialize;
pub(super) const MAX_MANIFEST_BYTES: usize = 64 * 1024;
#[derive(Deserialize)]
struct Manifest {
    version: String,
}
pub(super) fn version(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest: Manifest = serde_yaml::from_slice(bytes).ok()?;
    if manifest.version.len() > 32 {
        return None;
    }
    let value = semver::Version::parse(&manifest.version).ok()?;
    Some(value.to_string())
}
/// Target 3.14.4's public ManifestUpdateProvider: stable channel=1, no device_mid.
pub(super) fn manifest_url(architecture: &str) -> Option<String> {
    let platform = match architecture {
        "aarch64" => "darwin-aarch64",
        "x86_64" => "darwin-x86_64",
        _ => return None,
    };
    Some(format!(
        "https://zcode.z.ai/api/v1/releases/electron/manifest?platform={platform}&channel=1"
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_version_is_projected_from_public_manifest() {
        assert_eq!(version(b"version: 3.14.4\nfiles:\n  - url: https://example.invalid/unused.zip\nreleaseNotes: ignored"), Some("3.14.4".into()));
        assert_eq!(
            version(b"version: 3.14.5-beta.1"),
            Some("3.14.5-beta.1".into())
        );
    }
    #[test]
    fn malformed_duplicate_numeric_missing_and_oversize_are_rejected() {
        for bytes in [
            b"version: 3.14.4\nversion: 9.9.9".as_slice(),
            b"version: 42".as_slice(),
            b"files: []".as_slice(),
            b"version: unknown".as_slice(),
            b"version: [3.14.4]".as_slice(),
        ] {
            assert!(version(bytes).is_none());
        }
        assert!(version(&vec![b' '; MAX_MANIFEST_BYTES + 1]).is_none());
    }
    #[test]
    fn platform_is_bounded_and_endpoint_does_not_send_device_identifiers() {
        assert!(manifest_url("aarch64")
            .unwrap()
            .contains("platform=darwin-aarch64&channel=1"));
        assert!(manifest_url("x86_64")
            .unwrap()
            .contains("platform=darwin-x86_64&channel=1"));
        assert!(manifest_url("unknown").is_none());
        assert!(!manifest_url("aarch64").unwrap().contains("device_mid"));
    }
}
