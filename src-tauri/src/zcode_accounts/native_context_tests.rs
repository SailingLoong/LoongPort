use super::*;
use crate::zcode_accounts::admission::VerifiedContext;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

struct Fixture {
    _temp: tempfile::TempDir,
    selection: ContextSelection,
    user: OsUser,
    manifest: InstalledContract,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let install = base.join("ZCode.app");
        let home = base.join("home");
        let data = base.join("data/.zcode/v2");
        fs::create_dir_all(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(home.join(".zcode/v2")).unwrap();
        let mut manifest = installed_contract();
        for (index, artifact) in manifest.artifacts.iter_mut().enumerate() {
            let path = install.join(artifact.relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = format!("synthetic public artifact {index}").into_bytes();
            fs::write(&path, &bytes).unwrap();
            artifact.sha256 = Sha256::digest(&bytes).into();
            artifact.exact_size = Some(bytes.len() as u64);
        }
        fs::write(
            home.join(".zcode/v2/setting.json"),
            serde_json::to_vec(&serde_json::json!({
                "dataBaseDir":base.join("data"), "providerFamilyDomain":"zai",
                "providerFamilyConnectionSelections":{"zai":{"kind":"individual-coding-plan"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let user = OsUser {
            home: home.to_str().unwrap().into(),
            username: "synthetic-user".into(),
            uid: unsafe { libc::geteuid() },
        };
        Self {
            _temp: temp,
            selection: ContextSelection {
                install_path: install,
                data_root: data,
                key_mode: KeyMode::Standard,
            },
            user,
            manifest,
        }
    }
    fn settings(&self) -> PathBuf {
        Path::new(&self.user.home).join(".zcode/v2/setting.json")
    }
    fn probe(&self) -> NativeContextProbe {
        let probe = NativeContextProbe::from_parts(
            self.selection.clone(),
            self.user.clone(),
            &self.manifest,
        )
        .unwrap();
        *probe.test_facts.lock().unwrap() = Some(Ok(SystemFacts {
            user: self.user.clone(),
            writers: WriterState::Stopped,
        }));
        probe
    }
    fn contracts(&self) -> Vec<ContractEntry> {
        vec![ContractEntry {
            fingerprint: self.manifest.fingerprint(),
            native_gate_passed: true,
        }]
    }
}

#[test]
fn renderer_selection_contains_only_untrusted_paths_and_key_mode() {
    let good = r#"{"installPath":"/Applications/ZCode.app","dataRoot":"/synthetic/.zcode/v2","keyMode":"standard"}"#;
    assert!(serde_json::from_str::<ContextSelection>(good).is_ok());
    for field in [
        "secret",
        "home",
        "username",
        "verified",
        "nativeGatePassed",
        "family",
        "standardDesktopLaunch",
    ] {
        let mut value: serde_json::Value = serde_json::from_str(good).unwrap();
        value[field] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<ContextSelection>(value).is_err(),
            "{field}"
        );
    }
    assert!(
        serde_json::from_str::<ContextSelection>(&good.replace("standard", "fallback")).is_err()
    );
}
#[test]
fn manifest_is_exact_and_native_gate_is_macos_only() {
    let contracts = supported_contracts();
    assert_eq!(contracts.len(), 1);
    assert_eq!(contracts[0].fingerprint.version, "3.14.4");
    assert_eq!(contracts[0].fingerprint.build, "3.14.4.7912");
    assert_eq!(contracts[0].native_gate_passed, cfg!(target_os = "macos"));
    // Independent SHA256 calculation over the documented fixed framing.
    let digest = contracts[0]
        .fingerprint
        .artifact_sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        digest,
        "fb679708ff47c0c3b96d80013e1706bc91ca4fe51017b8c8abe182b4af01a489"
    );
    let manifest = installed_contract();
    assert_eq!(manifest.artifacts[0].exact_size, Some(326_914_570));
    assert_eq!(
        contracts[0].fingerprint.artifact_sha256,
        manifest.fingerprint().artifact_sha256
    );
    // Bind admission to the actual product manifest, using only synthetic paths
    // and settings. The artifact reader has separate exact-hash fixture tests.
    let fixture = Fixture::new();
    let mut observation = fixture.probe().observe().unwrap();
    observation.install = manifest.fingerprint();
    let result = VerifiedContext::assess(observation.clone(), &contracts);
    if cfg!(target_os = "macos") {
        assert!(result.is_ok());
    } else {
        assert!(matches!(result, Err(BlockedReason::NativeGatePending)));
    }
    observation.install.artifact_sha256[0] ^= 1;
    assert!(matches!(
        VerifiedContext::assess(observation, &contracts),
        Err(BlockedReason::UnsupportedBuild)
    ));
}
#[test]
fn exact_fixture_artifacts_and_standard_os_identity_admit_without_credentials() {
    let fixture = Fixture::new();
    // A credential directory cannot be decoded as a file. A successful observation
    // and admission demonstrate that this passive path never opens credentials.
    fs::create_dir(fixture.selection.data_root.join("credentials.json")).unwrap();
    let probe = fixture.probe();
    let observation = probe.observe().unwrap();
    assert_eq!(observation.home, fixture.user.home);
    assert_eq!(observation.username, "synthetic-user");
    assert_eq!(observation.credential_root, fixture.selection.data_root);
    assert_eq!(observation.settings_file, fixture.settings());
    assert_eq!(
        observation.root_identity,
        [
            fs::metadata(&fixture.selection.data_root).unwrap().dev(),
            fs::metadata(&fixture.selection.data_root).unwrap().ino()
        ]
    );
    let context = VerifiedContext::assess(observation, &fixture.contracts()).unwrap();
    context
        .recheck(probe.observe().unwrap(), &fixture.contracts())
        .unwrap();
}
#[test]
fn custom_and_unknown_modes_stop_before_files_are_opened() {
    for (mode, reason) in [
        (KeyMode::Custom, BlockedReason::CustomKeyContext),
        (KeyMode::Unknown, BlockedReason::KeyContextUnknown),
    ] {
        let mut fixture = Fixture::new();
        fixture.selection.key_mode = mode;
        fixture.selection.install_path = PathBuf::from("/does-not-exist/ZCode.app");
        assert!(
            matches!(NativeContextProbe::from_parts(fixture.selection,fixture.user,&fixture.manifest),Err(actual) if actual==reason)
        );
    }
}
#[test]
fn malformed_or_changed_artifacts_never_match_a_contract() {
    for index in 0..3 {
        let fixture = Fixture::new();
        let path = fixture
            .selection
            .install_path
            .join(fixture.manifest.artifacts[index].relative_path);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            NativeContextProbe::from_parts(fixture.selection, fixture.user, &fixture.manifest),
            Err(BlockedReason::UnsupportedBuild)
        ));
    }
}
#[test]
fn artifact_replacement_same_bytes_and_in_place_edit_invalidate_cached_hash() {
    for in_place in [false, true] {
        let fixture = Fixture::new();
        let path = fixture
            .selection
            .install_path
            .join(fixture.manifest.artifacts[0].relative_path);
        // Filesystems may coalesce writes within one clock tick. Authenticate a
        // known old mtime so an in-place write must expose a new modification.
        let original_modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let set_original_modified = |path: &Path| {
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(original_modified))
                .unwrap();
        };
        set_original_modified(&path);
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            original_modified
        );
        let probe = fixture.probe();
        probe.observe().unwrap();
        let bytes = fs::read(&path).unwrap();
        let stamp = |path: &Path| {
            let meta = fs::metadata(path).unwrap();
            (
                meta.dev(),
                meta.ino(),
                meta.len(),
                meta.mtime(),
                meta.mtime_nsec(),
                meta.ctime(),
                meta.ctime_nsec(),
                meta.mode(),
                meta.uid(),
                meta.nlink(),
            )
        };
        let before = stamp(&path);
        if in_place {
            fs::write(&path, &bytes).unwrap();
        } else {
            let moved = path.with_extension("old");
            fs::rename(&path, moved).unwrap();
            fs::write(&path, &bytes).unwrap();
            set_original_modified(&path);
        }
        let after = stamp(&path);
        if in_place {
            assert_eq!((before.0, before.1, before.2), (after.0, after.1, after.2));
            assert_ne!((before.3, before.4), (after.3, after.4));
        } else {
            assert_ne!((before.0, before.1), (after.0, after.1));
            assert_eq!((before.2, before.3, before.4), (after.2, after.3, after.4));
        }
        let outcome = probe.observe().map(|_| ());
        eprintln!("in_place={in_place}, before={before:?}, after={after:?}, outcome={outcome:?}");
        assert!(
            matches!(outcome, Err(BlockedReason::ContextChanged)),
            "in_place={in_place}, before={before:?}, after={after:?}, outcome={outcome:?}"
        );
    }
}
#[test]
fn symlinked_artifact_and_selected_root_are_rejected() {
    let fixture = Fixture::new();
    let path = fixture
        .selection
        .install_path
        .join(fixture.manifest.artifacts[0].relative_path);
    let original = path.with_extension("original");
    fs::rename(&path, &original).unwrap();
    symlink(&original, &path).unwrap();
    assert!(
        NativeContextProbe::from_parts(fixture.selection, fixture.user, &fixture.manifest).is_err()
    );
    let mut fixture = Fixture::new();
    let alias = fixture.selection.data_root.with_extension("alias");
    symlink(&fixture.selection.data_root, &alias).unwrap();
    fixture.selection.data_root = alias;
    assert!(matches!(
        NativeContextProbe::from_parts(fixture.selection, fixture.user, &fixture.manifest),
        Err(BlockedReason::RootUnverified)
    ));
}
#[test]
fn root_swap_and_os_identity_change_invalidate_context() {
    let fixture = Fixture::new();
    let probe = fixture.probe();
    let moved = fixture.selection.data_root.with_extension("old");
    fs::rename(&fixture.selection.data_root, moved).unwrap();
    fs::create_dir(&fixture.selection.data_root).unwrap();
    fs::set_permissions(
        &fixture.selection.data_root,
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(matches!(
        probe.observe(),
        Err(BlockedReason::ContextChanged)
    ));
    let fixture = Fixture::new();
    let probe = fixture.probe();
    let mut user = fixture.user.clone();
    user.username.push('x');
    *probe.test_facts.lock().unwrap() = Some(Ok(SystemFacts {
        user,
        writers: WriterState::Stopped,
    }));
    assert!(matches!(
        probe.observe(),
        Err(BlockedReason::ContextChanged)
    ));
}
#[test]
fn settings_changes_are_reread_and_bound_by_admission() {
    let fixture = Fixture::new();
    let probe = fixture.probe();
    let context = VerifiedContext::assess(probe.observe().unwrap(), &fixture.contracts()).unwrap();
    let mut settings = fs::read(fixture.settings()).unwrap();
    settings.push(b' ');
    fs::write(fixture.settings(), settings).unwrap();
    assert_eq!(
        context.recheck(probe.observe().unwrap(), &fixture.contracts()),
        Err(BlockedReason::ContextChanged)
    );
}
#[test]
fn settings_are_bounded_regular_files_and_use_standard_home_only() {
    for mode in 0..3 {
        let fixture = Fixture::new();
        let probe = fixture.probe();
        let path = fixture.settings();
        match mode {
            0 => fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap(),
            1 => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            _ => {
                let moved = path.with_extension("old");
                fs::rename(&path, &moved).unwrap();
                symlink(moved, &path).unwrap();
            }
        }
        assert!(matches!(
            probe.observe(),
            Err(BlockedReason::SettingsInvalid)
        ));
    }
}
#[test]
fn existing_strict_settings_parser_rejects_wrong_root_duplicate_family_team_and_legacy() {
    for (bytes, reason) in [
        (
            r#"{"dataBaseDir":"/wrong","providerFamilyDomain":"zai"}"#,
            BlockedReason::RootUnverified,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyDomain":"bigmodel"}"#,
            BlockedReason::SettingsInvalid,
        ),
        (
            r#"{"providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"team-coding-plan"}}}"#,
            BlockedReason::TeamUnsupported,
        ),
        (
            r#"{"providerFamilyDomain":"zai","modelProviderFamilyModes":{}}"#,
            BlockedReason::LegacySelection,
        ),
    ] {
        let mut fixture = Fixture::new();
        fixture.selection.data_root = Path::new(&fixture.user.home).join(".zcode/v2");
        fs::set_permissions(
            &fixture.selection.data_root,
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(fixture.settings(), bytes).unwrap();
        let probe = fixture.probe();
        assert!(
            matches!(VerifiedContext::assess(probe.observe().unwrap(),&fixture.contracts()),Err(actual) if actual==reason)
        );
    }
}
#[test]
fn process_classifier_is_conservative_for_known_and_shared_executables() {
    let install = Path::new("/Applications/ZCode.app");
    for path in [
        "/Applications/ZCode.app/Contents/MacOS/ZCode",
        "/Applications/ZCode.app/Contents/Frameworks/ZCode Helper.app/Contents/MacOS/ZCode Helper",
        "/usr/local/bin/zcode",
        "/usr/local/bin/zcode-host",
    ] {
        assert!(
            classify_executable(install, Path::new(path)) == WriterState::Running,
            "{path}"
        );
    }
    for path in [
        "/opt/bin/node",
        "/opt/bin/nodejs",
        "/opt/bin/bun",
        "/opt/bin/deno",
        "/opt/bin/electron",
    ] {
        assert!(
            classify_executable(install, Path::new(path)) == WriterState::Unknown,
            "{path}"
        );
    }
    assert!(classify_executable(install, Path::new("/usr/bin/ls")) == WriterState::Stopped);
}
#[test]
fn runtime_writer_and_enumeration_failures_remain_blocking() {
    for state in [WriterState::Running, WriterState::Unknown] {
        let fixture = Fixture::new();
        let probe = fixture.probe();
        *probe.test_facts.lock().unwrap() = Some(Ok(SystemFacts {
            user: fixture.user.clone(),
            writers: state,
        }));
        let result = VerifiedContext::assess(probe.observe().unwrap(), &fixture.contracts());
        let expected = if state == WriterState::Running {
            BlockedReason::AppRunning
        } else {
            BlockedReason::WriterStateUnknown
        };
        assert!(matches!(result,Err(actual) if actual==expected));
    }
    let fixture = Fixture::new();
    let probe = fixture.probe();
    *probe.test_facts.lock().unwrap() = Some(Err(BlockedReason::WriterStateUnknown));
    assert!(matches!(
        probe.observe(),
        Err(BlockedReason::WriterStateUnknown)
    ));
}
#[cfg(not(target_os = "macos"))]
#[test]
fn production_constructor_on_other_platforms_is_unsupported() {
    let fixture = Fixture::new();
    assert!(matches!(
        NativeContextProbe::new(fixture.selection),
        Err(BlockedReason::UnsupportedPlatform)
    ));
}

#[test]
fn subsequent_observations_do_not_stream_artifact_files_again() {
    use std::io::{Seek, SeekFrom};
    let fixture = Fixture::new();
    let probe = fixture.probe();
    let mut held = probe.artifacts[0].file.try_clone().unwrap();
    // A cloned descriptor shares the read offset. Only metadata is needed after
    // the first whole-file authentication; even a zero offset stays untouched.
    held.seek(SeekFrom::Start(0)).unwrap();
    probe.observe().unwrap();
    probe.observe().unwrap();
    assert_eq!(held.stream_position().unwrap(), 0);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "invoked only by the native syscall parent with a synthetic HOME"]
fn native_process_fixture_child() {
    use std::io::{Read, Write};
    let user = native_user().unwrap();
    assert_eq!(user.uid, unsafe { libc::geteuid() });
    assert!(!user.username.is_empty());
    assert!(Path::new(&user.home).is_absolute());
    assert_ne!(user.home, "/synthetic/zcode-probe-unused-home");
    println!("ZCODE_NATIVE_PROBE_READY");
    std::io::stdout().flush().unwrap();
    let mut one = [0u8; 1];
    std::io::stdin().read_exact(&mut one).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_native_syscalls_resolve_os_user_and_synthetic_child_identity() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let executable = fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let mut child = ChildGuard(
        Command::new(&executable)
            .args([
                "--exact",
                "zcode_accounts::native_context::tests::native_process_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .env("HOME", "/synthetic/zcode-probe-unused-home")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(child.0.stdout.take().unwrap());
    let mut ready = false;
    for _ in 0..32 {
        let mut line = String::new();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.contains("ZCODE_NATIVE_PROBE_READY") {
            ready = true;
            break;
        }
    }
    assert!(
        ready,
        "synthetic child must complete backend OS-user lookup"
    );
    let pid = child.0.id() as i32;
    let uid = unsafe { libc::geteuid() };
    assert!(native_pids(uid).unwrap().contains(&pid));
    let actual = native_process_path(pid, uid).unwrap();
    assert_eq!(fs::canonicalize(&actual).unwrap(), executable);
    assert!(native_process_path(pid, uid.wrapping_add(1)).is_err());
    assert!(classify_executable(executable.parent().unwrap(), &actual) == WriterState::Running);
    child.0.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.0.wait().unwrap().success());
    assert!(native_process_path(pid, uid).is_err());
    assert!(!native_pids(uid).unwrap().contains(&pid));
}

#[test]
fn hardlinked_settings_are_not_read_as_public_metadata() {
    let fixture = Fixture::new();
    let probe = fixture.probe();
    fs::hard_link(
        fixture.settings(),
        fixture.selection.data_root.join("credential-alias"),
    )
    .unwrap();
    assert!(matches!(
        probe.observe(),
        Err(BlockedReason::SettingsInvalid)
    ));
}

#[test]
fn local_mount_flag_is_required_for_pid_based_directory_lock_contract() {
    assert!(is_local_mount(0x0000_1000));
    assert!(is_local_mount(0x0000_1000 | 0x8));
    assert!(!is_local_mount(0));
    assert!(!is_local_mount(0x8));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_selected_temporary_root_is_a_local_mount() {
    let fixture = Fixture::new();
    let root = File::open(fixture.selection.data_root).unwrap();
    assert!(native_local_root(&root).is_ok());
}

#[test]
fn selected_official_root_may_be_readable_without_permission_changes() {
    let fixture = Fixture::new();
    fs::set_permissions(
        &fixture.selection.data_root,
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let probe = fixture.probe();
    VerifiedContext::assess(probe.observe().unwrap(), &fixture.contracts()).unwrap();
    assert_eq!(
        fs::metadata(&fixture.selection.data_root).unwrap().mode() & 0o777,
        0o755
    );
}
