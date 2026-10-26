//! Observed desktop context for official ZCode requests. Never creates device identities.
use reqwest::header::{HeaderMap, HeaderValue};
use std::{io::Read, path::Path};

#[derive(Clone, Default)]
pub(super) struct SourceContext {
    language: Option<String>,
    timezone: Option<String>,
    os_version: Option<String>,
    device_mid: Option<String>,
}

fn printable(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 512 && value.bytes().all(|b| (32..=126).contains(&b)))
        .then(|| value.to_owned())
}

impl SourceContext {
    pub(super) fn with_device_mid(mut self, selected: Option<&str>) -> Self {
        self.device_mid = selected.map(String::from);
        self
    }
    pub(super) fn observe(root: Option<&Path>) -> Self {
        Self {
            language: sys_locale::get_locale().and_then(|s| printable(&s)),
            timezone: std::fs::read_link("/etc/localtime")
                .ok()
                .and_then(|p| p.to_str().and_then(timezone_from_path)),
            os_version: os_version(),
            device_mid: root.and_then(existing_device_mid),
        }
    }

    pub(super) fn headers(&self, version: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let mut insert = |name: &'static str, value: &str| {
            if let Some(value) = printable(value).and_then(|s| HeaderValue::from_str(&s).ok()) {
                headers.insert(name, value);
            }
        };
        insert("http-referer", "https://zcode.z.ai");
        insert("x-title", "LoongPort");
        // This transport targets the fixed production API origin.
        insert("x-release-channel", "production");
        insert(
            "user-agent",
            concat!("LoongPort/", env!("CARGO_PKG_VERSION")),
        );
        if let Some(version) = version {
            insert("x-zcode-app-version", version);
        }
        insert("x-platform", &platform());
        insert("x-os-category", std::env::consts::OS);
        insert(
            "x-client-language",
            self.language.as_deref().unwrap_or("unknown"),
        );
        insert(
            "x-client-timezone",
            self.timezone.as_deref().unwrap_or("unknown"),
        );
        if let Some(version) = self.os_version.as_deref() {
            insert("x-os-version", version);
        }
        apply_device_mid(&mut headers, self.device_mid.as_deref());
        headers
    }

    pub(super) fn marketing_language(&self) -> &'static str {
        if self
            .language
            .as_deref()
            .is_some_and(|s| s.starts_with("zh"))
        {
            "zh-CN"
        } else {
            "en-US"
        }
    }
}

pub(super) fn apply_device_mid(headers: &mut HeaderMap, value: Option<&str>) {
    headers.remove("x-device-mid");
    if let Some(value) = value
        .and_then(printable)
        .and_then(|s| HeaderValue::from_str(&s).ok())
    {
        let mut value = value;
        value.set_sensitive(true);
        headers.insert("x-device-mid", value);
    }
}

pub(super) fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

fn timezone_from_path(path: &str) -> Option<String> {
    printable(path.split_once("/zoneinfo/")?.1)
}

pub(super) fn existing_device_mid(root: &Path) -> Option<String> {
    // This is the official existing telemetry identity, shared across saved accounts.
    // Missing or malformed state must never trigger UUID generation or file writes.
    let file = std::fs::File::open(root.join("telemetry-state.json")).ok()?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let mid = value.get("deviceMid")?.as_str()?.trim();
    uuid::Uuid::parse_str(mid).ok()?;
    printable(mid)
}

#[cfg(unix)]
fn os_version() -> Option<String> {
    // Same OS fact as Node's os.version(); no shell or generated machine ID.
    let mut info = std::mem::MaybeUninit::<libc::utsname>::uninit();
    if unsafe { libc::uname(info.as_mut_ptr()) } != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    printable(
        unsafe { std::ffi::CStr::from_ptr(info.version.as_ptr()) }
            .to_str()
            .ok()?,
    )
}
#[cfg(not(unix))]
fn os_version() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_identity_is_reused_without_modifying_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("telemetry-state.json");
        let bytes = br#"{"deviceMid":"11111111-2222-4333-8444-555555555555","lastDailyActiveDate":"synthetic"}"#;
        std::fs::write(&path, bytes).unwrap();
        let a = SourceContext::observe(Some(root.path())).headers(Some("99.1.2"));
        let b = SourceContext::observe(Some(root.path())).headers(Some("99.1.2"));
        assert_eq!(a["x-device-mid"], b["x-device-mid"]);
        assert_eq!(a["x-device-mid"], "11111111-2222-4333-8444-555555555555");
        assert!(a["x-device-mid"].is_sensitive());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn device_selection_none_removes_an_existing_native_header() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("telemetry-state.json"),
            br#"{"deviceMid":"11111111-2222-4333-8444-555555555555"}"#,
        )
        .unwrap();
        assert!(!SourceContext::observe(Some(root.path()))
            .with_device_mid(None)
            .headers(None)
            .contains_key("x-device-mid"));
    }
    #[test]
    fn imported_device_overrides_headers_without_modifying_the_native_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("telemetry-state.json");
        let bytes = br#"{"deviceMid":"11111111-2222-4333-8444-555555555555"}"#;
        std::fs::write(&path, bytes).unwrap();
        let imported = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let context = SourceContext::observe(Some(root.path()));
        assert_eq!(
            context.clone().headers(None)["x-device-mid"],
            "11111111-2222-4333-8444-555555555555"
        );
        let headers = context
            .with_device_mid(Some(imported))
            .headers(Some("99.1.2"));
        assert_eq!(headers["x-device-mid"], imported);
        assert!(headers["x-device-mid"].is_sensitive());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn missing_invalid_and_oversized_state_never_create_an_identity() {
        let root = tempfile::tempdir().unwrap();
        assert!(existing_device_mid(root.path()).is_none());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        for bytes in [
            br#"{"deviceMid":"not-a-device"}"#.to_vec(),
            vec![b' '; 16 * 1024 + 1],
        ] {
            std::fs::write(root.path().join("telemetry-state.json"), &bytes).unwrap();
            assert!(existing_device_mid(root.path()).is_none());
        }
    }
    #[test]
    fn source_context_rejects_header_injection_and_uses_iana_timezone() {
        assert!(printable("value\r\nx-device-mid: forged").is_none());
        assert_eq!(
            timezone_from_path("/var/db/timezone/zoneinfo/Asia/Shanghai").as_deref(),
            Some("Asia/Shanghai")
        );
        assert!(timezone_from_path("/unrelated/path").is_none());
    }
}
