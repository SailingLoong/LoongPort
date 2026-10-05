//! macOS adapter for a source-bound app explicitly launched by LoongPort.
//! No name-wide signals, force quit, real-user fallback, or credential IO.
use super::admission::{BlockedReason, ContextProbe, WriterState};
use super::native_context::{
    native_pids, native_process_identity, ContextSelection, NativeContextProbe,
};
use super::native_lifecycle::{self, BoundInstance, Driver, Failure, Instance};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use objc2_app_kit::NSRunningApplication;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use zeroize::Zeroizing;

fn registry() -> &'static Mutex<BTreeMap<String, BoundInstance>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, BoundInstance>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}
struct Source {
    id: String,
    home: String,
    username: String,
    uid: u32,
    base: PathBuf,
    executable: PathBuf,
}
fn present_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
fn source(probe: &NativeContextProbe, selection: &ContextSelection) -> Result<Source, Failure> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Bootstrap {
        #[serde(default, deserialize_with = "present_string")]
        data_base_dir: Option<String>,
    }
    let observed = probe.observe().map_err(|_| Failure::SourceChanged)?;
    let settings = Zeroizing::new(observed.settings);
    let bootstrap: Bootstrap =
        serde_json::from_slice(&settings).map_err(|_| Failure::SourceChanged)?;
    let base = bootstrap
        .data_base_dir
        .as_deref()
        .map(super::desktop_text::js_trim)
        .unwrap_or(&observed.home);
    let base = PathBuf::from(base);
    if !base.is_absolute()
        || base.join(".zcode/v2") != selection.data_root
        || std::fs::canonicalize(&base).ok().as_ref() != Some(&base)
    {
        return Err(Failure::SourceChanged);
    }
    let uid = unsafe { libc::geteuid() };
    let binding = serde_json::to_vec(&(
        "zcode-controlled-source-v1",
        &selection.install_path,
        &selection.data_root,
        observed.root_identity,
        observed.install.artifact_sha256,
        &observed.home,
        &observed.username,
        uid,
        &base,
    ))
    .map_err(|_| Failure::SourceChanged)?;
    Ok(Source {
        id: URL_SAFE_NO_PAD.encode(Sha256::digest(binding)),
        home: observed.home,
        username: observed.username,
        uid,
        base,
        executable: selection.install_path.join("Contents/MacOS/ZCode"),
    })
}
struct MacDriver<'a> {
    probe: &'a NativeContextProbe,
    selection: &'a ContextSelection,
    source: Source,
}
impl MacDriver<'_> {
    fn app(&self, pid: i32) -> Result<objc2::rc::Retained<NSRunningApplication>, Failure> {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .ok_or(Failure::InstanceChanged)?;
        let bundle = app
            .bundleURL()
            .and_then(|url| url.path())
            .map(|path| PathBuf::from(path.to_string()))
            .ok_or(Failure::InstanceChanged)?;
        let executable = app
            .executableURL()
            .and_then(|url| url.path())
            .map(|path| PathBuf::from(path.to_string()))
            .ok_or(Failure::InstanceChanged)?;
        if bundle != self.selection.install_path
            || executable != self.source.executable
            || app.bundleIdentifier().map(|id| id.to_string()).as_deref() != Some("dev.zcode.app")
        {
            return Err(Failure::InstanceChanged);
        }
        Ok(app)
    }
}
impl Driver for MacDriver<'_> {
    fn source(&mut self) -> Result<String, Failure> {
        source(self.probe, self.selection).map(|source| source.id)
    }
    fn writers(&mut self) -> Result<WriterState, Failure> {
        self.probe
            .observe()
            .map(|o| o.writers)
            .map_err(|_| Failure::SourceChanged)
    }
    fn instance(&mut self, pid: i32) -> Result<Option<Instance>, Failure> {
        if !native_pids(self.source.uid)
            .map_err(|_| Failure::InstanceChanged)?
            .contains(&pid)
        {
            return Ok(None);
        }
        let (executable, start) =
            native_process_identity(pid, self.source.uid).map_err(|_| Failure::InstanceChanged)?;
        if executable != self.source.executable {
            return Err(Failure::InstanceChanged);
        }
        self.app(pid)?;
        Ok(Some(Instance {
            pid,
            uid: self.source.uid,
            start,
            executable,
            bundle_id: "dev.zcode.app".into(),
        }))
    }
    fn quit(&mut self, instance: &Instance) -> Result<bool, Failure> {
        let app = self.app(instance.pid)?;
        if self.instance(instance.pid)?.as_ref() != Some(instance)
            || self.source()? != self.source.id
        {
            return Err(Failure::InstanceChanged);
        }
        Ok(app.terminate())
    }
    fn launch(&mut self) -> Result<Instance, Failure> {
        if self.source()? != self.source.id || self.writers()? != WriterState::Stopped {
            return Err(Failure::RestartFailed);
        }
        let mut child = Command::new(&self.source.executable)
            .env_clear()
            .env("HOME", &self.source.home)
            .env("USER", &self.source.username)
            .env("LOGNAME", &self.source.username)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("ZCODE_DATA_BASE_DIR", &self.source.base)
            .current_dir(&self.selection.install_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Failure::RestartFailed)?;
        let pid = i32::try_from(child.id()).map_err(|_| Failure::RestartFailed)?;
        // Reap only this child, never terminate it or a shared process.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        for _ in 0..40 {
            if let Ok(Some(instance)) = self.instance(pid) {
                return Ok(instance);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(Failure::RestartFailed)
    }
    fn wait_step(&mut self) {
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn error(failure: Failure) -> BlockedReason {
    match failure {
        Failure::SourceChanged => BlockedReason::ContextChanged,
        Failure::Unbound | Failure::QuitRefused | Failure::QuitTimeout => BlockedReason::AppRunning,
        _ => BlockedReason::WriterStateUnknown,
    }
}
fn driver<'a>(
    probe: &'a NativeContextProbe,
    selection: &'a ContextSelection,
) -> Result<MacDriver<'a>, BlockedReason> {
    Ok(MacDriver {
        probe,
        selection,
        source: source(probe, selection).map_err(error)?,
    })
}
pub(super) fn open(
    probe: &NativeContextProbe,
    selection: &ContextSelection,
) -> Result<(), BlockedReason> {
    let mut driver = driver(probe, selection)?;
    let id = driver.source.id.clone();
    let bound = native_lifecycle::restart(&mut driver, &id, None).map_err(error)?;
    let mut registry = registry()
        .lock()
        .map_err(|_| BlockedReason::ContextChanged)?;
    // Only one app can be launched from a globally stopped writer snapshot.
    registry.clear();
    registry.insert(id, bound);
    Ok(())
}
pub(super) fn stop(
    probe: &NativeContextProbe,
    selection: &ContextSelection,
) -> Result<bool, BlockedReason> {
    let mut driver = driver(probe, selection)?;
    if driver.writers().map_err(error)? == WriterState::Stopped {
        return Ok(false);
    }
    let id = driver.source.id.clone();
    let bound = registry()
        .lock()
        .map_err(|_| BlockedReason::ContextChanged)?
        .get(&id)
        .cloned();
    native_lifecycle::stop(&mut driver, &id, bound.as_ref()).map_err(error)?;
    Ok(true)
}
pub(super) fn restart(
    probe: &NativeContextProbe,
    selection: &ContextSelection,
) -> Result<(), BlockedReason> {
    let mut driver = driver(probe, selection)?;
    let id = driver.source.id.clone();
    let old = registry()
        .lock()
        .map_err(|_| BlockedReason::ContextChanged)?
        .get(&id)
        .cloned();
    let bound = native_lifecycle::restart(&mut driver, &id, old.as_ref()).map_err(error)?;
    let mut registry = registry()
        .lock()
        .map_err(|_| BlockedReason::ContextChanged)?;
    registry.clear();
    registry.insert(id, bound);
    Ok(())
}
