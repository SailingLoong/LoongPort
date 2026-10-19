//! Fixed Qoder Desktop 0.4.8 / macOS ARM64 device seam. No download, fallback,
//! arbitrary path, credential discovery, or cached/copied device identity.
use super::Dependency;
use zeroize::Zeroizing;

pub(super) struct DeviceIdentity {
    pub(super) token: Zeroizing<String>,
    pub(super) code: Zeroizing<String>,
    pub(super) kind: Zeroizing<String>,
}
pub(super) trait DeviceSource: Send + Sync {
    fn availability(&self) -> Result<(), Dependency> {
        Ok(())
    }
    fn resolve(&self, subject: &str) -> Result<DeviceIdentity, Dependency>;
}
pub(super) struct InstalledApp;
impl DeviceSource for InstalledApp {
    fn availability(&self) -> Result<(), Dependency> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            macos::Native.verify_installation().map(|_| ())
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            Err(Dependency::OfficialDeviceUnsupported)
        }
    }
    fn resolve(&self, subject: &str) -> Result<DeviceIdentity, Dependency> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            resolve_with(&macos::Native, subject)
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = subject;
            Err(Dependency::OfficialDeviceUnsupported)
        }
    }
}
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), all(test, unix)))]
#[path = "device/install.rs"]
mod install;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "device/macos.rs"]
mod macos;

#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
const MAX_OUTPUT: usize = 1024 * 1024;
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
trait Platform {
    type Installation;
    // Production Child owns a suspended direct child and kills/reaps on Drop.
    type Child;
    fn verify_installation(&self) -> Result<Self::Installation, Dependency>;
    fn start_suspended(&self, installation: &Self::Installation)
        -> Result<Self::Child, Dependency>;
    fn verify_running_image(
        &self,
        installation: &Self::Installation,
        child: &mut Self::Child,
    ) -> Result<(), Dependency>;
    fn resume_and_read(
        &self,
        child: &mut Self::Child,
        input: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, Dependency>;
}
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
fn resolve_with(platform: &impl Platform, subject: &str) -> Result<DeviceIdentity, Dependency> {
    if subject.is_empty() || subject.len() > 256 || !subject.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Dependency::OfficialDeviceFailed);
    }
    let installation = platform.verify_installation()?;
    let mut child = platform.start_suspended(&installation)?;
    platform.verify_running_image(&installation, &mut child)?;
    let mut input = Zeroizing::new(
        serde_json::to_vec(&serde_json::json!({"account": subject}))
            .map_err(|_| Dependency::OfficialDeviceFailed)?,
    );
    input.push(b'\n');
    let output = platform.resume_and_read(&mut child, &input)?;
    parse_output(&output)
}
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
fn parse_output(output: &[u8]) -> Result<DeviceIdentity, Dependency> {
    use zeroize::Zeroize;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        machine_token: String,
        machine_code: String,
        machine_type: String,
    }
    impl Drop for Raw {
        fn drop(&mut self) {
            self.machine_token.zeroize();
            self.machine_code.zeroize();
            self.machine_type.zeroize();
        }
    }
    if output.is_empty() || output.len() > MAX_OUTPUT {
        return Err(Dependency::OfficialDeviceFailed);
    }
    let mut raw: Raw =
        serde_json::from_slice(output).map_err(|_| Dependency::OfficialDeviceFailed)?;
    if [&raw.machine_token, &raw.machine_code, &raw.machine_type]
        .iter()
        .any(|v| v.is_empty() || v.len() > 4096 || !v.bytes().all(|b| b.is_ascii_graphic()))
    {
        return Err(Dependency::OfficialDeviceFailed);
    }
    Ok(DeviceIdentity {
        token: Zeroizing::new(std::mem::take(&mut raw.machine_token)),
        code: Zeroizing::new(std::mem::take(&mut raw.machine_code)),
        kind: Zeroizing::new(std::mem::take(&mut raw.machine_type)),
    })
}

#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
fn status_enabled(output: &[u8]) -> bool {
    std::str::from_utf8(output).is_ok_and(|s| s.trim() == "assessments enabled")
}
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
fn accepts_policy(verdict: Option<bool>, source: Option<&str>, overridden: bool) -> bool {
    verdict == Some(true)
        && !overridden
        && matches!(source, Some("Developer ID" | "Notarized Developer ID"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Fake {
        events: Arc<Mutex<Vec<&'static str>>>,
        reject: Option<&'static str>,
        output: Vec<u8>,
    }
    struct Child(Arc<Mutex<Vec<&'static str>>>);
    impl Drop for Child {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("kill/reap");
        }
    }
    impl Fake {
        fn step(&self, step: &'static str) -> Result<(), Dependency> {
            self.events.lock().unwrap().push(step);
            if self.reject == Some(step) {
                Err(Dependency::OfficialDeviceUnverified)
            } else {
                Ok(())
            }
        }
        fn new(reject: Option<&'static str>) -> Self {
            Self { events: Arc::default(), reject, output: br#"{"machineToken":"synthetic-token","machineCode":"synthetic-code","machineType":"synthetic-type"}"#.to_vec() }
        }
    }
    impl Platform for Fake {
        type Installation = ();
        type Child = Child;
        fn verify_installation(&self) -> Result<(), Dependency> {
            self.step("install/policy")
        }
        fn start_suspended(&self, _: &()) -> Result<Child, Dependency> {
            self.step("start/suspended")?;
            Ok(Child(self.events.clone()))
        }
        fn verify_running_image(&self, _: &(), _: &mut Child) -> Result<(), Dependency> {
            self.step("dynamic/cdhash")
        }
        fn resume_and_read(
            &self,
            _: &mut Child,
            input: &[u8],
        ) -> Result<Zeroizing<Vec<u8>>, Dependency> {
            self.step("resume/input")?;
            assert_eq!(input, b"{\"account\":\"synthetic-uid\"}\n");
            Ok(Zeroizing::new(self.output.clone()))
        }
    }
    #[test]
    fn verified_child_identity_precedes_resume_and_subject_input() {
        let p = Fake::new(None);
        let d = resolve_with(&p, "synthetic-uid").unwrap();
        assert_eq!(d.token.as_str(), "synthetic-token");
        assert_eq!(
            *p.events.lock().unwrap(),
            [
                "install/policy",
                "start/suspended",
                "dynamic/cdhash",
                "resume/input",
                "kill/reap"
            ]
        );
    }
    #[test]
    fn absent_bad_hash_or_native_policy_refusal_never_starts() {
        let p = Fake::new(Some("install/policy"));
        assert!(resolve_with(&p, "synthetic-uid").is_err());
        assert_eq!(*p.events.lock().unwrap(), ["install/policy"]);
    }
    #[test]
    fn replaced_or_wrong_cdhash_child_is_killed_without_input_or_resume() {
        let p = Fake::new(Some("dynamic/cdhash"));
        assert!(resolve_with(&p, "synthetic-uid").is_err());
        assert_eq!(
            *p.events.lock().unwrap(),
            [
                "install/policy",
                "start/suspended",
                "dynamic/cdhash",
                "kill/reap"
            ]
        );
    }
    #[test]
    fn bad_subject_never_starts_a_component() {
        for uid in ["", "uid\n", "uid with space"] {
            let p = Fake::new(None);
            assert!(resolve_with(&p, uid).is_err());
            assert!(p.events.lock().unwrap().is_empty());
        }
    }
    #[test]
    fn global_disabled_and_unknown_status_are_not_execution_permission() {
        assert!(status_enabled(b"assessments enabled\n"));
        for output in [
            b"assessments disabled".as_slice(),
            b"",
            b"enabled",
            b"assessments enabled\nunknown",
            b"\xff",
        ] {
            assert!(!status_enabled(output));
        }
    }
    #[test]
    fn native_policy_requires_known_positive_non_overridden_verdict() {
        assert!(accepts_policy(
            Some(true),
            Some("Notarized Developer ID"),
            false
        ));
        for (verdict, source, overridden) in [
            (None, Some("Developer ID"), false),
            (Some(false), Some("Developer ID"), false),
            (Some(true), None, false),
            (Some(true), Some("unknown"), false),
            (Some(true), Some("Developer ID"), true),
            (Some(true), Some("security disabled"), false),
        ] {
            assert!(!accepts_policy(verdict, source, overridden));
        }
    }
    #[test]
    fn timeout_or_read_failure_reaps_child_and_never_returns_identity() {
        let p = Fake::new(Some("resume/input"));
        assert!(resolve_with(&p, "synthetic-uid").is_err());
        assert_eq!(p.events.lock().unwrap().last(), Some(&"kill/reap"));
    }
    #[test]
    fn required_device_fields_allow_bounded_auxiliary_metadata() {
        // The fixed community consumer picks three keys rather than asserting
        // that the official helper has no additional diagnostics/metadata.
        let output = br#"{"machineToken":"synthetic-token","machineCode":"synthetic-code","machineType":"synthetic-type","account":"synthetic-uid","vmInfo":{"synthetic":true},"accountOutcome":true,"accountReturnCode":0}"#;
        assert!(parse_output(output).is_ok());
    }
    #[test]
    fn output_is_bounded_typed_and_header_safe() {
        for output in [
            b"not json".to_vec(),
            vec![b' '; MAX_OUTPUT + 1],
            br#"{"machineToken":"x\r\ny","machineCode":"x","machineType":"x"}"#.to_vec(),
            br#"{"machineToken":"","machineCode":"x","machineType":"x"}"#.to_vec(),
            br#"{"machineToken":1,"machineCode":"x","machineType":"x"}"#.to_vec(),
            br#"{"machineToken":"x","machineCode":"x","machineType":"x"} garbage"#.to_vec(),
        ] {
            assert!(parse_output(&output).is_err());
        }
    }
}
