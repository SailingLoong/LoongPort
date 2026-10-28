//! Explicit read-only known-location observation; never native switch authorization.
use serde::Serialize;
use std::path::{Path, PathBuf};
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreviewError {
    UnsafeSource,
    SourceChanged,
    UnreadableSource,
    InvalidDocument,
    UnsupportedContext,
    #[cfg(not(any(unix, windows)))]
    UnsupportedPlatform,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AuthObservation {
    Missing,
    RecognizedPersonalFormat,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    source_verification: &'static str,
    auth_file: AuthObservation,
    logout_marker_present: bool,
    realm: Option<&'static str>,
    activation_available: bool,
}
#[derive(Clone, Copy)]
enum HostOs {
    #[cfg(any(test, not(any(target_os = "windows", target_os = "macos"))))]
    Linux,
    #[cfg(any(test, target_os = "macos"))]
    Macos,
    #[cfg(any(test, target_os = "windows"))]
    Windows,
}
const AUTH_LIMIT: u64 = 4 * 1024 * 1024;
const MARKER_LIMIT: u64 = 64 * 1024;

#[cfg(feature = "gui")]
pub(crate) fn preview_current() -> Result<Preview, PreviewError> {
    let home = dirs::home_dir().ok_or(PreviewError::UnsafeSource)?;
    #[cfg(target_os = "windows")]
    let os = HostOs::Windows;
    #[cfg(target_os = "macos")]
    let os = HostOs::Macos;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let os = HostOs::Linux;
    observe_at(&home, os, || {})
}
fn auth_path(home: &Path, os: HostOs) -> PathBuf {
    home.join(match os {
        #[cfg(any(test, not(any(target_os = "windows", target_os = "macos"))))]
        HostOs::Linux => ".local/share/CodeBuddyExtension/Data/Public/auth",
        #[cfg(any(test, target_os = "macos"))]
        HostOs::Macos => "Library/Application Support/CodeBuddyExtension/Data/Public/auth",
        #[cfg(any(test, target_os = "windows"))]
        HostOs::Windows => "AppData/Local/CodeBuddyExtension/Data/Public/auth",
    })
    .join("workbuddy-desktop.info")
}
fn marker_path(auth: &Path) -> PathBuf {
    let mut name = auth.as_os_str().to_os_string();
    name.push(".logged-out");
    PathBuf::from(name)
}
#[derive(PartialEq, Eq)]
struct Identity {
    volume: u64,
    index: u64,
}
fn identity(file: &std::fs::File) -> Result<Identity, PreviewError> {
    let metadata = file
        .metadata()
        .map_err(|_| PreviewError::UnreadableSource)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity {
            volume: metadata.dev(),
            index: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(PreviewError::UnsafeSource);
        }
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: live owned file handle and initialized writable structure for this call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(PreviewError::UnreadableSource);
        }
        Ok(Identity {
            volume: u64::from(information.dwVolumeSerialNumber),
            index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        Err(PreviewError::UnsupportedPlatform)
    }
}
fn parents(path: &Path) -> Result<Vec<Identity>, PreviewError> {
    let mut result = Vec::new();
    for parent in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if !parent.is_absolute() {
            continue;
        }
        let meta = match std::fs::symlink_metadata(parent) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(PreviewError::UnreadableSource),
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(PreviewError::UnsafeSource);
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::{fs::MetadataExt, fs::OpenOptionsExt};
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            };
            if meta.file_attributes() & 0x400 != 0 {
                return Err(PreviewError::UnsafeSource);
            }
            options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options
            .open(parent)
            .map_err(|_| PreviewError::UnsafeSource)?;
        if !file
            .metadata()
            .map_err(|_| PreviewError::UnreadableSource)?
            .is_dir()
        {
            return Err(PreviewError::UnsafeSource);
        }
        result.push(identity(&file)?);
    }
    Ok(result)
}
#[derive(PartialEq, Eq)]
struct Sample {
    identity: Identity,
    modified: std::time::SystemTime,
    bytes: zeroize::Zeroizing<Vec<u8>>,
}
fn read_file(path: &Path, limit: u64) -> Result<Option<Sample>, PreviewError> {
    use std::io::Read;
    let Some(mut file) = crate::config_file_io::open_regular_file(path, limit)
        .map_err(|_| PreviewError::UnsafeSource)?
    else {
        return Ok(None);
    };
    let before = identity(&file)?;
    let meta = file
        .metadata()
        .map_err(|_| PreviewError::UnreadableSource)?;
    let modified = meta
        .modified()
        .map_err(|_| PreviewError::UnreadableSource)?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PreviewError::UnreadableSource)?;
    let after = file
        .metadata()
        .map_err(|_| PreviewError::UnreadableSource)?;
    if bytes.len() as u64 > limit {
        return Err(PreviewError::UnsafeSource);
    }
    if identity(&file)? != before
        || after.len() != meta.len()
        || after
            .modified()
            .map_err(|_| PreviewError::UnreadableSource)?
            != modified
    {
        return Err(PreviewError::SourceChanged);
    }
    Ok(Some(Sample {
        identity: before,
        modified,
        bytes,
    }))
}
#[derive(PartialEq, Eq)]
struct Pair {
    parents: Vec<Identity>,
    auth: Option<Sample>,
    marker: Option<Sample>,
}
fn read_pair(auth: &Path) -> Result<Pair, PreviewError> {
    let parent = auth.parent().ok_or(PreviewError::UnsafeSource)?;
    let before = parents(parent)?;
    let auth_sample = read_file(auth, AUTH_LIMIT)?;
    let marker = read_file(&marker_path(auth), MARKER_LIMIT)?;
    if parents(parent)? != before {
        return Err(PreviewError::SourceChanged);
    }
    Ok(Pair {
        parents: before,
        auth: auth_sample,
        marker,
    })
}
fn observe_at(home: &Path, os: HostOs, after_read: impl FnOnce()) -> Result<Preview, PreviewError> {
    if !home.is_absolute()
        || home.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(PreviewError::UnsafeSource);
    }
    let path = auth_path(home, os);
    let first = read_pair(&path)?;
    after_read();
    let realm = first
        .auth
        .as_ref()
        .map(|auth| {
            super::native::observe(&auth.bytes).map_err(|error| match error {
                super::native::NativeError::UnsupportedContext => PreviewError::UnsupportedContext,
                _ => PreviewError::InvalidDocument,
            })
        })
        .transpose()?;
    // An observed-stable pair, not a client lock or atomic two-file snapshot.
    if read_pair(&path)? != first {
        return Err(PreviewError::SourceChanged);
    }
    Ok(Preview {
        source_verification: "knownLocationOnly",
        auth_file: if realm.is_some() {
            AuthObservation::RecognizedPersonalFormat
        } else {
            AuthObservation::Missing
        },
        logout_marker_present: first.marker.is_some(),
        realm,
        activation_available: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
    }
    fn expected(home: &Path, os: HostOs) -> PathBuf {
        home.join(match os {
            HostOs::Linux => ".local/share/CodeBuddyExtension/Data/Public/auth",
            HostOs::Macos => "Library/Application Support/CodeBuddyExtension/Data/Public/auth",
            HostOs::Windows => "AppData/Local/CodeBuddyExtension/Data/Public/auth",
        })
        .join("workbuddy-desktop.info")
    }
    fn document(realm: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"account":{"uid":"PRIVATE-UID","type":"personal","email":"private@example.invalid"},"auth":{"domain":realm,"accessToken":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":"PRIVATE-TOKEN"}}},"accounts":[],"allAccounts":[]})).unwrap()
    }
    fn install(home: &Path, os: HostOs, bytes: &[u8]) -> PathBuf {
        let p = expected(home, os);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
    #[test]
    fn known_location_observation_is_redacted_and_never_authorizes_activation() {
        for os in [HostOs::Linux, HostOs::Macos, HostOs::Windows] {
            for realm in ["www.workbuddy.cn", "www.codebuddy.cn"] {
                let home = fixture();
                let bytes = document(realm);
                let p = install(home.path(), os, &bytes);
                let result = observe_at(home.path(), os, || {}).unwrap();
                assert_eq!(result.realm, Some(realm));
                assert_eq!(result.auth_file, AuthObservation::RecognizedPersonalFormat);
                assert!(!result.activation_available);
                assert!(!result.logout_marker_present);
                let json = serde_json::to_string(&result).unwrap();
                assert!(json.contains("knownLocationOnly"));
                for private in [
                    "PRIVATE-UID",
                    "PRIVATE-TOKEN",
                    "private@example.invalid",
                    home.path().to_str().unwrap(),
                    "accessToken",
                    "envelope",
                ] {
                    assert!(!json.contains(private));
                }
                assert_eq!(std::fs::read(p).unwrap(), bytes);
            }
        }
    }
    #[test]
    fn missing_known_location_never_creates_a_directory() {
        let home = fixture();
        let result = observe_at(home.path(), HostOs::Linux, || {}).unwrap();
        assert_eq!(result.auth_file, AuthObservation::Missing);
        assert!(result.realm.is_none());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
    #[test]
    fn marker_is_only_an_observation_and_other_files_remain_untouched() {
        let home = fixture();
        let path = install(home.path(), HostOs::Linux, &document("www.workbuddy.cn"));
        let marker = path.with_file_name("workbuddy-desktop.info.logged-out");
        std::fs::write(&marker, b"PRIVATE-MARKER").unwrap();
        for (name, data) in [
            ("workbuddy-desktop-ai.info", "PRIVATE-AI"),
            ("keyblob", "PRIVATE-KEY"),
            ("chat.sqlite", "PRIVATE-CHAT"),
            ("settings.json", "PRIVATE-SETTINGS"),
        ] {
            std::fs::write(path.parent().unwrap().join(name), data).unwrap();
        }
        let before: std::collections::BTreeMap<_, _> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let b = std::fs::read(&p).unwrap();
                (p, b)
            })
            .collect();
        let result = observe_at(home.path(), HostOs::Linux, || {}).unwrap();
        assert!(result.logout_marker_present);
        assert!(!result.activation_available);
        let text = serde_json::to_string(&result).unwrap();
        assert!(!text.contains("PRIVATE"));
        assert!(!text.contains("loggedIn"));
        for (p, b) in before {
            assert_eq!(std::fs::read(p).unwrap(), b);
        }
    }
    #[test]
    fn ai_file_is_not_a_fallback_for_missing_cn_file() {
        let home = fixture();
        let path = expected(home.path(), HostOs::Linux);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path.with_file_name("workbuddy-desktop-ai.info"),
            document("www.workbuddy.ai"),
        )
        .unwrap();
        assert_eq!(
            observe_at(home.path(), HostOs::Linux, || {})
                .unwrap()
                .auth_file,
            AuthObservation::Missing
        );
    }
    #[test]
    fn original_strict_schema_rejections_are_preserved_without_error_data() {
        let home = fixture();
        let path = install(home.path(), HostOs::Linux, &document("www.workbuddy.cn"));
        let base: serde_json::Value =
            serde_json::from_slice(&document("www.workbuddy.cn")).unwrap();
        let mut bad = Vec::new();
        for realm in [
            "www.workbuddy.ai",
            "WWW.WORKBUDDY.CN",
            "https://www.workbuddy.cn",
            "www.workbuddy.cn.evil",
        ] {
            let mut x = base.clone();
            x["auth"]["domain"] = realm.into();
            bad.push(serde_json::to_vec(&x).unwrap());
        }
        for (key, value) in [
            ("type", serde_json::json!("enterprise")),
            ("isCurrentOneIdEnterprise", serde_json::json!(true)),
            ("enterpriseId", serde_json::json!("PRIVATE-ENTERPRISE")),
        ] {
            let mut x = base.clone();
            x["account"][key] = value;
            bad.push(serde_json::to_vec(&x).unwrap());
        }
        let mut conflict = base.clone();
        conflict["uid"] = "different".into();
        bad.push(serde_json::to_vec(&conflict).unwrap());
        bad.push(br#"{"account":{},"account":{"uid":"PRIVATE"}}"#.to_vec());
        bad.push(b"PRIVATE-MALFORMED-JSON".to_vec());
        for bytes in bad {
            std::fs::write(&path, &bytes).unwrap();
            let error = observe_at(home.path(), HostOs::Linux, || {}).unwrap_err();
            let text = serde_json::to_string(&error).unwrap();
            assert!(!text.contains("PRIVATE"));
            assert!(!text.contains(home.path().to_str().unwrap()));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn unsafe_directory_and_oversize_auth_or_marker_are_refused() {
        let home = fixture();
        let path = install(home.path(), HostOs::Linux, &document("www.workbuddy.cn"));
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(AUTH_LIMIT + 1).unwrap();
        drop(file);
        assert_eq!(
            observe_at(home.path(), HostOs::Linux, || {}).unwrap_err(),
            PreviewError::UnsafeSource
        );
        std::fs::write(&path, document("www.workbuddy.cn")).unwrap();
        let marker = marker_path(&path);
        let file = std::fs::File::create(&marker).unwrap();
        file.set_len(MARKER_LIMIT + 1).unwrap();
        drop(file);
        assert!(observe_at(home.path(), HostOs::Linux, || {}).is_err());
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        assert!(observe_at(home.path(), HostOs::Linux, || {}).is_err());
        // PathBuf::join normalizes away `..` for Windows verbatim bases
        // (as returned by canonicalize). Append literal OS text instead so the
        // input actually exercises observe_at's ParentDir refusal on every OS.
        let mut traversal = home.path().as_os_str().to_os_string();
        traversal.push(std::path::MAIN_SEPARATOR_STR);
        traversal.push("..");
        let traversal = PathBuf::from(traversal);
        assert!(traversal
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)));
        let after_read = std::cell::Cell::new(false);
        assert_eq!(
            observe_at(&traversal, HostOs::Linux, || after_read.set(true)).unwrap_err(),
            PreviewError::UnsafeSource
        );
        assert!(!after_read.get());
    }
    #[cfg(windows)]
    #[test]
    fn verbatim_parent_fixture_preserves_the_component_that_join_normalizes() {
        // Pure path regression: no discovery or reads of this synthetic location.
        let base = Path::new(r"\\?\C:\synthetic-native-preview\child");
        assert!(!base
            .join("..")
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)));
        let mut literal = base.as_os_str().to_os_string();
        literal.push(r"\..");
        let literal = PathBuf::from(literal);
        assert!(literal
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)));
        assert_eq!(
            observe_at(&literal, HostOs::Windows, || panic!(
                "unsafe path reached IO"
            ))
            .unwrap_err(),
            PreviewError::UnsafeSource
        );
    }
    #[test]
    fn auth_marker_and_parent_changes_block_the_result() {
        for kind in ["auth", "marker", "replacement", "parent"] {
            let home = fixture();
            let bytes = document("www.workbuddy.cn");
            let path = install(home.path(), HostOs::Linux, &bytes);
            let result = observe_at(home.path(), HostOs::Linux, || match kind {
                "auth" => {
                    std::fs::write(&path, document("www.codebuddy.cn")).unwrap();
                }
                "marker" => {
                    std::fs::write(marker_path(&path), b"logged-out").unwrap();
                }
                "replacement" => {
                    std::fs::rename(&path, path.with_extension("previous")).unwrap();
                    std::fs::write(&path, &bytes).unwrap();
                }
                _ => {
                    let parent = path.parent().unwrap();
                    std::fs::rename(parent, parent.with_file_name("previous-auth")).unwrap();
                    std::fs::create_dir(parent).unwrap();
                    std::fs::write(&path, &bytes).unwrap();
                }
            });
            assert_eq!(result.unwrap_err(), PreviewError::SourceChanged);
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlink_auth_marker_and_parent_refuse_without_following_targets() {
        use std::os::unix::fs::symlink;
        for kind in ["auth", "marker", "parent"] {
            let home = fixture();
            let target = fixture();
            let bytes = document("www.workbuddy.cn");
            let path = install(home.path(), HostOs::Linux, &bytes);
            let secret = target.path().join("PRIVATE-TARGET");
            std::fs::write(&secret, &bytes).unwrap();
            match kind {
                "auth" => {
                    std::fs::remove_file(&path).unwrap();
                    symlink(&secret, &path).unwrap();
                }
                "marker" => symlink(&secret, marker_path(&path)).unwrap(),
                _ => {
                    let parent = path.parent().unwrap();
                    std::fs::remove_file(&path).unwrap();
                    std::fs::remove_dir(parent).unwrap();
                    symlink(target.path(), parent).unwrap();
                }
            }
            assert!(observe_at(home.path(), HostOs::Linux, || {}).is_err());
            assert_eq!(std::fs::read(&secret).unwrap(), bytes);
        }
    }
}
