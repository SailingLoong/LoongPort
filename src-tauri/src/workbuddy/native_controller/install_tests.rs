//! Real files and original controller/owner; public build bytes and processes are synthetic.
use super::super::{
    install::{open_at, verify_during, Artifact, Origin},
    process::{Process, ProcessBound, Processes, Scope},
};
use super::*;
use crate::workbuddy::model::Domain;

#[path = "capture_tests.rs"]
mod capture_tests;

const ARTIFACTS: &[Artifact] = &[
    Artifact {
        path: "Contents/Info.plist",
        size: 22,
        sha256: "ff16c29b4d46378ba15eabb2d9d1c136172c87b00bd45b83b3a393d0c92fc936",
    },
    Artifact {
        path: "Contents/Resources/app.asar",
        size: 14,
        sha256: "75cea97598d83ee7e10c6f85a86d89f994367080a40f83e9840b58868857fe5b",
    },
    Artifact {
        path: "Contents/MacOS/Electron",
        size: 25,
        sha256: "297385dec7ba4f55644d25294e11c69c7254aab09101595c87f73924de896ba3",
    },
    Artifact {
        path: "Contents/Frameworks/Electron Framework.framework/Versions/A/Electron Framework",
        size: 30,
        sha256: "f14ba4271a953cc19137314da42370c5b8473392130e90a7f6313f75f94695b5",
    },
    Artifact {
        path: "Contents/Frameworks/Electron Framework.framework/Versions/A/Resources/Info.plist",
        size: 25,
        sha256: "c7b355419b102af8b7cb64e416bf7d012cbcf8e33258acd1b1cc9981a6fe07c2",
    },
    Artifact {
        path: "Contents/Resources/app.asar.unpacked/cli/product.json",
        size: 45,
        sha256: "357c45f54001866b61bc69a7f4b4be8492a5e656763acd345270c107bb8283d1",
    },
];
struct InstalledFixture {
    f: Fixture,
    bundle: PathBuf,
    home: PathBuf,
}
impl InstalledFixture {
    async fn new() -> Self {
        let mut f = Fixture::new().await;
        let home = f._root.path().join("os-home");
        let auth = home.join("Library/Application Support/CodeBuddyExtension/Data/Public/auth");
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        std::fs::rename(&f.client, &auth).unwrap();
        f.client = auth;
        let bundle = f._root.path().join("WorkBuddy.app");
        let bytes: &[&[u8]] = &[
            b"synthetic bundle plist",
            b"synthetic asar",
            b"synthetic main executable",
            b"synthetic framework executable",
            b"synthetic framework plist",
            br#"{"authentication":{"id":"workbuddy-desktop"}}"#,
        ];
        for (artifact, bytes) in ARTIFACTS.iter().zip(bytes) {
            let path = bundle.join(artifact.path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let framework = bundle.join("Contents/Frameworks/Electron Framework.framework");
        for (path, target) in [
            ("Versions/Current", "A"),
            ("Electron Framework", "Versions/Current/Electron Framework"),
            ("Libraries", "Versions/Current/Libraries"),
            ("Helpers", "Versions/Current/Helpers"),
            ("Resources", "Versions/Current/Resources"),
        ] {
            std::os::unix::fs::symlink(target, framework.join(path)).unwrap();
        }
        Self { f, bundle, home }
    }
    fn origin(&self) -> Result<Origin, Failure> {
        // No home discovery, credential reads or client execution in this fixture.
        open_at(
            &self.bundle,
            &self.home,
            unsafe { libc::geteuid() },
            Domain::WorkBuddy,
            ARTIFACTS,
        )
    }
    async fn bind_target(&mut self, source: &Origin) {
        let original = std::fs::read(self.f.auth()).unwrap();
        let marker = self.f.client.join("workbuddy-desktop.info.logged-out");
        let old_marker = std::fs::read(&marker).unwrap();
        self.f.context = source.context().clone();
        let binding = self.f.binding();
        store::save(&self.f.session, &binding, vec![]).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(self.f.auth(), image("B")).unwrap();
        let observed = Observation::read(&self.f.auth(), &self.f.context).unwrap();
        self.f.receipt = self
            .f
            .controller(&binding)
            .catalog
            .save(&observed, None, "B")
            .await
            .unwrap();
        std::fs::write(self.f.auth(), original).unwrap();
        std::fs::write(marker, old_marker).unwrap();
    }
    fn controlled(&self, origin: Origin) -> ProcessBound<Origin, Quiet> {
        ProcessBound::new(
            origin,
            Scope::at(&self.bundle, &self.bundle.join("Contents/MacOS/Electron")).unwrap(),
            Quiet,
        )
    }
}
struct Quiet;
impl super::super::process::sealed::Sealed for Quiet {}
impl Processes for Quiet {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure> {
        Ok(vec![Process {
            id: std::process::id(),
            start_identity: [100, 1],
            executable: std::env::current_exe().unwrap().canonicalize().unwrap(),
        }])
    }
}

#[tokio::test]
async fn each_operation_keeps_content_rechecks_within_explicit_budget() {
    use super::super::install::take_content_reads;
    let mut f = InstalledFixture::new().await;
    take_content_reads();
    let origin = f.origin().unwrap();
    f.bind_target(&origin).await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let selected = c.select(&f.f.key()).await.unwrap();
    let mut source = f.controlled(origin);
    // Match the installed factory's verification before the controller starts.
    source.verify_stopped().unwrap();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let prepare = take_content_reads();
    for _ in 0..3 {
        source.verify_stopped().unwrap();
    }
    let extra = take_content_reads();
    assert_eq!(
        c.apply(&plan, &mut source, &mut Files).await,
        Ok(Outcome::Applied)
    );
    let apply = take_content_reads();
    c.recover(&mut source, &mut Files).await.unwrap();
    let recover = take_content_reads();
    println!("actual content bytes prepare={prepare:?} apply={apply:?} recover={recover:?}");
    for (operation, reads, passes) in [
        ("prepare", prepare, 12),
        ("apply", apply, 12),
        ("recover", recover, 6),
        ("three extra checkpoints", extra, 6),
    ] {
        println!("actual artifact content bytes for {operation}: {reads:?}");
        assert_eq!(reads.len(), ARTIFACTS.len());
        for artifact in ARTIFACTS {
            assert_eq!(
                reads[&f.bundle.join(artifact.path)],
                artifact.size * passes,
                "{operation} changed content-check budget for {}",
                artifact.path
            );
        }
    }
}

#[tokio::test]
async fn content_change_during_the_only_hash_refuses_before_an_origin_is_returned() {
    let f = InstalledFixture::new().await;
    let original = std::fs::read(f.f.auth()).unwrap();
    let changed = f.bundle.join(ARTIFACTS[1].path);
    let _hook = super::super::install::on_content_read(move |path| {
        if path == changed {
            let mut bytes = std::fs::read(path).unwrap();
            bytes[0] ^= 1;
            std::fs::write(path, bytes).unwrap();
        }
    });
    assert!(f.origin().is_err());
    assert_eq!(std::fs::read(f.f.auth()).unwrap(), original);
    assert!(!f
        .f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
}

fn mutate_install(bundle: &Path, kind: &str) {
    if kind == "link" {
        let link = bundle.join("Contents/Frameworks/Electron Framework.framework/Versions/Current");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("B", link).unwrap();
    } else {
        let path = bundle.join(ARTIFACTS[1].path);
        let mut bytes = std::fs::read(&path).unwrap();
        if kind == "replace" {
            std::fs::rename(&path, path.with_extension("previous")).unwrap();
        } else {
            bytes[0] ^= 1;
        }
        std::fs::write(path, bytes).unwrap();
    }
}

#[tokio::test]
async fn installation_change_while_waiting_for_owner_never_refreshes_or_writes() {
    use super::super::install::take_content_reads;
    for kind in ["same-length", "replace", "link"] {
        let mut f = InstalledFixture::new().await;
        take_content_reads();
        let origin = f.origin().unwrap();
        f.bind_target(&origin).await;
        let _collision = super::super::install::SimulatedTimestampCollision::new();
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let selected = c.select(&f.f.key()).await.unwrap();
        let mut source = f.controlled(origin);
        let auth = std::fs::read(f.f.auth()).unwrap();
        let accounts = std::fs::read(f.f.session.root().join(store::FILE)).unwrap();
        let held = crate::services::sync_protocol::sync_mutex().lock().await;
        let mut io = Files;
        let mut waiting = Box::pin(c.prepare(&selected, &mut source, &mut io));
        assert!(futures::poll!(&mut waiting).is_pending());
        mutate_install(&f.bundle, kind);
        drop(held);
        assert!(waiting.await.is_err(), "accepted queued {kind}");
        assert_eq!(std::fs::read(f.f.auth()).unwrap(), auth);
        assert_eq!(
            std::fs::read(f.f.session.root().join(store::FILE)).unwrap(),
            accounts
        );
        assert!(!f
            .f
            .session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
            .exists());
        let reads = take_content_reads();
        for artifact in ARTIFACTS {
            let count = reads[&f.bundle.join(artifact.path)];
            assert!(
                (artifact.size * 4..=artifact.size * 5).contains(&count),
                "unexpected queued-check read budget: {kind}"
            );
        }
    }
}

struct MutateAfterProcessObservation {
    bundle: PathBuf,
    kind: &'static str,
}
struct FailSecondScan {
    count: usize,
    main: PathBuf,
}
impl super::super::process::sealed::Sealed for FailSecondScan {}
impl Processes for FailSecondScan {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure> {
        self.count += 1;
        if self.count == 2 {
            Ok(vec![Process {
                id: 101,
                start_identity: [100, 1],
                executable: self.main.clone(),
            }])
        } else {
            Quiet.snapshot()
        }
    }
}
#[tokio::test]
async fn failed_final_pid_check_invalidates_refreshed_baseline_until_new_operation() {
    let f = InstalledFixture::new().await;
    let origin = f.origin().unwrap();
    let main = f.bundle.join("Contents/MacOS/Electron");
    let mut source = ProcessBound::new(
        origin,
        Scope::at(&f.bundle, &main).unwrap(),
        FailSecondScan { count: 0, main },
    );
    assert_eq!(source.begin_operation(), Err(Failure::SourceChanged));
    assert_eq!(source.verify_unchanged(), Err(Failure::SourceChanged));
    assert_eq!(source.begin_operation(), Ok(()));
}
impl super::super::process::sealed::Sealed for MutateAfterProcessObservation {}
impl Processes for MutateAfterProcessObservation {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure> {
        let rows = Quiet.snapshot()?;
        mutate_install(&self.bundle, self.kind);
        Ok(rows)
    }
}
#[tokio::test]
async fn change_after_final_process_observation_cannot_refresh_the_build_baseline() {
    for kind in ["same-length", "replace", "link"] {
        let mut f = InstalledFixture::new().await;
        let origin = f.origin().unwrap();
        f.bind_target(&origin).await;
        let _collision = super::super::install::SimulatedTimestampCollision::new();
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let selected = c.select(&f.f.key()).await.unwrap();
        let mut source = ProcessBound::new(
            origin,
            Scope::at(&f.bundle, &f.bundle.join("Contents/MacOS/Electron")).unwrap(),
            MutateAfterProcessObservation {
                bundle: f.bundle.clone(),
                kind,
            },
        );
        let before = std::fs::read(f.f.auth()).unwrap();
        assert!(c.prepare(&selected, &mut source, &mut Files).await.is_err());
        assert_eq!(std::fs::read(f.f.auth()).unwrap(), before);
        assert!(!f
            .f
            .session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
            .exists());
    }
}
async fn change_earlier_source_while_later_artifacts_are_verified(link: bool) {
    let f = InstalledFixture::new().await;
    let mut origin = f.origin().unwrap();
    let auth = std::fs::read(f.f.auth()).unwrap();
    let result = verify_during(&mut origin, &mut |index| {
        if index != 1 {
            return;
        }
        if link {
            let path = f
                .bundle
                .join("Contents/Frameworks/Electron Framework.framework/Versions/Current");
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink("B", path).unwrap();
        } else {
            let path = f.bundle.join("Contents/Resources/app.asar");
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[0] ^= 1;
            std::fs::write(path, bytes).unwrap();
        }
    });
    assert_eq!(result, Err(Failure::SourceChanged));
    assert_eq!(std::fs::read(f.f.auth()).unwrap(), auth);
    assert!(!f
        .f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
}
#[tokio::test]
async fn earlier_artifact_change_during_later_hashes_refuses_the_same_admission() {
    change_earlier_source_while_later_artifacts_are_verified(false).await;
}
#[tokio::test]
async fn earlier_artifact_changed_during_later_hash_with_colliding_revision_is_rejected() {
    let f = InstalledFixture::new().await;
    let mut origin = f.origin().unwrap();
    let before = std::fs::read(f.f.auth()).unwrap();
    let _collision = super::super::install::SimulatedTimestampCollision::new();
    let result = verify_during(&mut origin, &mut |index| {
        if index == 1 {
            let path = f.bundle.join(ARTIFACTS[0].path);
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[0] ^= 1;
            std::fs::write(path, bytes).unwrap();
        }
    });
    assert_eq!(result, Err(Failure::SourceChanged));
    assert_eq!(std::fs::read(f.f.auth()).unwrap(), before);
}
#[tokio::test]
async fn earlier_link_change_during_later_hashes_refuses_the_same_admission() {
    change_earlier_source_while_later_artifacts_are_verified(true).await;
}
#[tokio::test]
async fn installed_source_survives_auth_replacement_without_account_identity_in_its_context() {
    let f = InstalledFixture::new().await;
    let mut origin = f.origin().unwrap();
    let context = origin.context().clone();
    std::fs::rename(f.f.auth(), f.f.client.join("previous-auth")).unwrap();
    std::fs::write(f.f.auth(), image("different-personal-account")).unwrap();
    origin.verify_stopped().unwrap();
    assert!(context == *f.origin().unwrap().context());
}
#[tokio::test]
async fn installed_source_uses_the_official_shared_auth_root_instead_of_product_config_storage() {
    let f = InstalledFixture::new().await;
    let config = f.home.join(".workbuddy");
    std::fs::create_dir(&config).unwrap();
    let bait = config.join("workbuddy-desktop.info");
    std::fs::write(&bait, b"unrelated product configuration sentinel").unwrap();
    let origin = f.origin().unwrap();
    assert_eq!(origin.root(), f.f.client);
    assert_eq!(
        std::fs::read(bait).unwrap(),
        b"unrelated product configuration sentinel"
    );
}
#[tokio::test]
async fn changed_artifact_is_rejected_even_when_revision_timestamps_collide() {
    let f = InstalledFixture::new().await;
    let mut origin = f.origin().unwrap();
    let path = f.bundle.join(ARTIFACTS[1].path);
    let before = std::fs::read(&path).unwrap();
    let mut changed = before.clone();
    changed[0] ^= 1;
    std::fs::write(&path, &changed).unwrap();
    assert_ne!(before, changed);
    assert_eq!(before.len(), changed.len());
    let _collision = super::super::install::SimulatedTimestampCollision::new();
    assert!(
        origin.verify_stopped().is_err(),
        "changed bytes with equal reported revision were admitted"
    );
}
#[tokio::test]
async fn installed_source_rejects_changed_build_and_framework_alias_before_native_writes() {
    use std::os::unix::fs::MetadataExt;

    fn revision(m: &std::fs::Metadata) -> (u64, u64, u32, u64, i64, i64, i64, i64) {
        (
            m.dev(),
            m.ino(),
            m.uid(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    }
    for kind in [
        "asar",
        "unpacked-config",
        "main",
        "framework",
        "framework-alias",
        "bundle",
        "auth-root",
    ] {
        let f = InstalledFixture::new().await;
        let mut origin = f.origin().unwrap();
        let auth = std::fs::read(f.f.auth()).unwrap();
        let mut diagnostic = None;
        match kind {
            "framework-alias" => {
                let p = f
                    .bundle
                    .join("Contents/Frameworks/Electron Framework.framework/Versions/Current");
                std::fs::remove_file(&p).unwrap();
                std::os::unix::fs::symlink("B", p).unwrap();
            }
            "bundle" => {
                std::fs::rename(&f.bundle, f.bundle.with_extension("previous")).unwrap();
                std::fs::create_dir(&f.bundle).unwrap();
            }
            "auth-root" => {
                std::fs::rename(&f.f.client, f.f.client.with_extension("previous")).unwrap();
                std::fs::create_dir(&f.f.client).unwrap();
                std::fs::write(f.f.auth(), &auth).unwrap();
            }
            _ => {
                let i = match kind {
                    "asar" => 1,
                    "main" => 2,
                    "framework" => 3,
                    _ => 5,
                };
                let p = f.bundle.join(ARTIFACTS[i].path);
                let mut bytes = std::fs::read(&p).unwrap();
                let before = revision(&std::fs::metadata(&p).unwrap());
                let original = bytes.clone();
                bytes[0] ^= 1;
                std::fs::write(&p, bytes).unwrap();
                let after = revision(&std::fs::metadata(&p).unwrap());
                let changed = std::fs::read(&p).unwrap() != original;
                diagnostic = Some((before, after, changed));
            }
        }
        assert!(origin.verify_stopped().is_err(), "admitted {kind}; synthetic revision (dev, ino, uid, size, mtime, mtime_nsec, ctime, ctime_nsec), before/after/content_changed: {diagnostic:?}");
        assert_eq!(std::fs::read(f.f.auth()).unwrap(), auth);
        assert!(!f
            .f
            .session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
            .exists());
    }
}
#[tokio::test]
async fn installed_source_rejects_unrecognized_build_before_returning_an_origin() {
    let f = InstalledFixture::new().await;
    let path = f
        .bundle
        .join("Contents/Resources/app.asar.unpacked/cli/product.json");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] ^= 1;
    std::fs::write(path, bytes).unwrap();
    assert!(f.origin().is_err());
}
#[tokio::test]
async fn installed_source_construction_does_not_require_parseable_or_present_auth() {
    for kind in ["missing", "invalid", "logged-out"] {
        let f = InstalledFixture::new().await;
        match kind {
            "missing" => std::fs::remove_file(f.f.auth()).unwrap(),
            "invalid" => std::fs::write(f.f.auth(), b"{broken synthetic auth").unwrap(),
            _ => {}
        }
        f.origin().unwrap().verify_stopped().unwrap();
    }
}
#[tokio::test]
async fn installed_source_rejects_foreign_owner_and_auth_aliases_without_following_data() {
    let f = InstalledFixture::new().await;
    assert!(open_at(
        &f.bundle,
        &f.home,
        unsafe { libc::geteuid() } ^ 1,
        Domain::WorkBuddy,
        ARTIFACTS
    )
    .is_err());
    let auth = std::fs::read(f.f.auth()).unwrap();
    std::fs::rename(f.f.auth(), f.f.client.join("target-auth")).unwrap();
    std::os::unix::fs::symlink("target-auth", f.f.auth()).unwrap();
    assert!(f.origin().is_err());
    assert_eq!(std::fs::read(f.f.client.join("target-auth")).unwrap(), auth);
}
#[tokio::test]
async fn installed_source_composes_with_original_prepare_apply_and_account_cas() {
    let mut f = InstalledFixture::new().await;
    let origin = f.origin().unwrap();
    f.bind_target(&origin).await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let selected = c.select(&f.f.key()).await.unwrap();
    let mut source = f.controlled(origin);
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    assert_eq!(
        c.apply(&plan, &mut source, &mut Files).await.unwrap(),
        Outcome::Applied
    );
    let after: Value = serde_json::from_slice(&std::fs::read(f.f.auth()).unwrap()).unwrap();
    assert_eq!(after["account"]["uid"], "B");
    assert_eq!(after["preserve"]["original"], "A");
    assert!(!f
        .f
        .client
        .join("workbuddy-desktop.info.logged-out")
        .exists());
}
#[tokio::test]
async fn installed_prepare_keeps_original_auth_and_marker_preimage_checks() {
    for marker in [false, true] {
        let mut f = InstalledFixture::new().await;
        let origin = f.origin().unwrap();
        f.bind_target(&origin).await;
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let selected = c.select(&f.f.key()).await.unwrap();
        let mut source = f.controlled(origin);
        let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
        let path = if marker {
            f.f.client.join("workbuddy-desktop.info.logged-out")
        } else {
            f.f.auth()
        };
        let changed = if marker {
            b"external-marker".to_vec()
        } else {
            image("external-account")
        };
        std::fs::write(&path, &changed).unwrap();
        let journal =
            f.f.session
                .root()
                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
        let clear = std::fs::read(&journal).unwrap();
        assert_eq!(
            c.apply(&plan, &mut source, &mut Files).await,
            Err(Failure::SourceChanged)
        );
        assert_eq!(std::fs::read(path).unwrap(), changed);
        assert_eq!(std::fs::read(journal).unwrap(), clear);
    }
}
#[tokio::test]
async fn installed_recovery_preserves_unknown_auth_then_restores_original_known_image() {
    for kind in ["missing", "invalid"] {
        let mut f = InstalledFixture::new().await;
        let origin = f.origin().unwrap();
        f.bind_target(&origin).await;
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let selected = c.select(&f.f.key()).await.unwrap();
        let mut source = f.controlled(origin);
        let before = std::fs::read(f.f.auth()).unwrap();
        let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
        let mut stopped = f.f.port();
        let mut io = Interrupted {
            files: Files,
            stopped: stopped.stopped.clone(),
            written: false,
        };
        // Interrupt the original write through its stopped fixture. The actual
        // installation source is reconstructed independently for recovery.
        assert_eq!(
            c.apply(&plan, &mut stopped, &mut io).await,
            Err(Failure::RecoveryRequired)
        );
        drop(plan);
        drop(source);
        let known = std::fs::read(f.f.auth()).unwrap();
        let journal =
            f.f.session
                .root()
                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
        let pending = std::fs::read(&journal).unwrap();
        if kind == "missing" {
            std::fs::remove_file(f.f.auth()).unwrap();
        } else {
            std::fs::write(f.f.auth(), b"{unknown synthetic image").unwrap();
        }
        let mut recovered = f.controlled(f.origin().unwrap());
        assert_eq!(
            c.recover(&mut recovered, &mut Files).await,
            Err(Failure::RecoveryRequired)
        );
        assert_eq!(std::fs::read(&journal).unwrap(), pending);
        std::fs::write(f.f.auth(), known).unwrap();
        assert_eq!(
            c.recover(&mut recovered, &mut Files).await.unwrap(),
            Outcome::Restored
        );
        assert_eq!(std::fs::read(f.f.auth()).unwrap(), before);
    }
}
#[tokio::test]
async fn cancelled_prepare_waiting_for_owner_never_initializes_a_journal() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let held = crate::services::sync_protocol::sync_mutex().lock().await;
    let before = std::fs::read(f.auth()).unwrap();
    let mut io = Files;
    let mut waiting = Box::pin(c.prepare(&selected, &mut source, &mut io));
    assert!(matches!(
        futures::poll!(&mut waiting),
        std::task::Poll::Pending
    ));
    drop(waiting);
    drop(held);
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
}
#[tokio::test]
async fn dropping_a_prepared_plan_does_not_publish_a_pending_native_write() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let before = std::fs::read(f.auth()).unwrap();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let journal = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    let clear = std::fs::read(&journal).unwrap();
    drop(plan);
    drop(source);
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert_eq!(std::fs::read(journal).unwrap(), clear);
    assert_eq!(
        crate::workbuddy::native_owner::inspect(f.session.root(), &f.session.read().unwrap())
            .unwrap(),
        crate::workbuddy::native_owner::Disposition::Clear
    );
}
