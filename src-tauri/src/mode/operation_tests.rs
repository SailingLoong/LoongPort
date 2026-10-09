use super::*;
use crate::live::engine::lock_app;
use crate::live::patch::json::JsonPatch;
use crate::live::patch::KeyPath;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::Path;

use std::sync::RwLock;

struct Fixture {
    key: RwLock<VaultContext>,
    _dir: tempfile::TempDir,
    store: DeviceStore,
    a: PathBuf,
    b: PathBuf,
    app: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let a = dir.path().join("client/a.json");
        let b = dir.path().join("client/b.json");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::write(&a, "{\n  \"user\": 1,\n  \"key\": \"old\"\n}").unwrap();
        fs::write(&b, "{\n  \"key\": \"old\"\n}").unwrap();
        // 每个测试用自己的应用名，写锁互不影响。
        let app = format!("op-test-{}", dir.path().display());
        Self {
            key: RwLock::new(VaultContext::generate().unwrap()),
            store: DeviceStore::at(dir.path().join("device")),
            a,
            b,
            app,
            _dir: dir,
        }
    }

    fn files(&self) -> Vec<LiveFile> {
        vec![LiveFile::shared(&self.a), LiveFile::shared(&self.b)]
    }

    fn read(&self, path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn temp_files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.a.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().contains(".tmp."))
            .collect()
    }
}

fn set_key(value: &str) -> JsonPatch {
    JsonPatch {
        set: vec![(KeyPath::new(&["key"]), json!(value))],
        ..JsonPatch::default()
    }
}

fn switch(fx: &Fixture, pointer: &RefCell<Option<String>>) -> Result<OperationReport, AppError> {
    let patch = set_key("new");
    let guard = lock_app(&fx.app);
    run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[
            FileChange {
                file: LiveFile::shared(&fx.a),
                patch: &patch,
            },
            FileChange {
                file: LiveFile::shared(&fx.b),
                patch: &patch,
            },
        ],
        PendingTarget::pointer(Some("B".into())),
        &|target| {
            *pointer.borrow_mut() = target.pointer.clone();
            Ok(())
        },
    )
}

fn recover_now(fx: &Fixture, pointer: &RefCell<Option<String>>) -> Option<RecoveryOutcome> {
    failpoint::crash_at(None);
    let guard = lock_app(&fx.app);
    recover(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &|target| {
            *pointer.borrow_mut() = target.pointer.clone();
            Ok(())
        },
    )
    .unwrap()
}

fn assert_old(fx: &Fixture) {
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "old"}));
    assert_eq!(fx.read(&fx.b), json!({"key": "old"}));
}

fn assert_new(fx: &Fixture) {
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "new"}));
    assert_eq!(fx.read(&fx.b), json!({"key": "new"}));
}

/// 改 a、删 b，在 `stage` 之后的某一步崩溃。
fn write_a_delete_b(fx: &Fixture, crash: &str) -> RefCell<Option<String>> {
    let pointer = RefCell::new(None);
    let patch = set_key("new");
    let delete = crate::live::patch::WholeFile::Delete;
    failpoint::crash_at(Some(crash));
    let guard = lock_app(&fx.app);
    let result = run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[
            FileChange {
                file: LiveFile::shared(&fx.a),
                patch: &patch,
            },
            FileChange {
                file: LiveFile::shared(&fx.b),
                patch: &delete,
            },
        ],
        PendingTarget::pointer(Some("B".into())),
        &|target| {
            *pointer.borrow_mut() = target.pointer.clone();
            Ok(())
        },
    );
    failpoint::crash_at(None);
    drop(guard);
    assert!(result.is_err(), "crash injected at {crash}");
    pointer
}

#[test]
fn deleting_a_file_is_part_of_the_operation_and_rolls_forward() {
    let fx = Fixture::new();
    let pointer = write_a_delete_b(&fx, "pending");
    assert_eq!(recover_now(&fx, &pointer), Some(RecoveryOutcome::Discarded));
    assert_old(&fx);

    for crash in ["published:0", "published:1", "target"] {
        let fx = Fixture::new();
        let pointer = write_a_delete_b(&fx, crash);
        let pending = state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
            .unwrap()
            .expect("pending");
        assert_eq!(pending.files[1].planned, None, "{crash}: deletion recorded");
        assert_eq!(pending.files[1].staged, None, "{crash}: nothing staged");

        assert_eq!(
            recover_now(&fx, &pointer),
            Some(RecoveryOutcome::RolledForward),
            "{crash}"
        );
        assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "new"}), "{crash}");
        assert!(!fx.b.exists(), "{crash}: b deleted");
        assert_eq!(*pointer.borrow(), Some("B".into()), "{crash}");
        assert!(fx.temp_files().is_empty(), "{crash}");
    }
}

#[test]
fn deleting_a_missing_file_is_a_noop() {
    let fx = Fixture::new();
    fs::remove_file(&fx.b).unwrap();
    let guard = lock_app(&fx.app);
    let delete = crate::live::patch::WholeFile::Delete;
    let report = run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[FileChange {
            file: LiveFile::shared(&fx.b),
            patch: &delete,
        }],
        PendingTarget::default(),
        &|_| Ok(()),
    )
    .unwrap();
    assert!(report.changed.is_empty());
    assert!(!fx.b.exists());
}

#[test]
fn a_clean_run_writes_every_file_then_the_target() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let report = switch(&fx, &pointer).unwrap();
    assert_new(&fx);
    assert_eq!(report.changed, vec![fx.a.clone(), fx.b.clone()]);
    assert_eq!(*pointer.borrow(), Some("B".into()));
    assert_eq!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app).unwrap(),
        None
    );
    assert!(fx.temp_files().is_empty());
}

#[test]
fn a_crash_before_the_intent_is_recorded_changes_nothing() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("staged"));
    switch(&fx, &pointer).expect_err("crash");
    assert_eq!(recover_now(&fx, &pointer), None);
    assert_old(&fx);
    assert_eq!(*pointer.borrow(), None);
}

#[test]
fn a_crash_before_publishing_is_discarded() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    assert!(
        !fx.temp_files().is_empty(),
        "staged files survive the crash"
    );

    assert_eq!(recover_now(&fx, &pointer), Some(RecoveryOutcome::Discarded));
    assert_old(&fx);
    assert_eq!(*pointer.borrow(), None);
    assert!(fx.temp_files().is_empty());
    assert_eq!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app).unwrap(),
        None
    );
}

#[test]
fn a_crash_halfway_through_publishing_rolls_forward() {
    for point in ["published:0", "published:1", "target"] {
        let fx = Fixture::new();
        let pointer = RefCell::new(None);
        failpoint::crash_at(Some(point));
        switch(&fx, &pointer).expect_err("crash");
        *pointer.borrow_mut() = None;

        assert_eq!(
            recover_now(&fx, &pointer),
            Some(RecoveryOutcome::RolledForward),
            "{point}"
        );
        assert_new(&fx);
        assert_eq!(*pointer.borrow(), Some("B".into()), "{point}");
        assert!(fx.temp_files().is_empty(), "{point}");
        assert_eq!(
            state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app).unwrap(),
            None
        );
    }
}

#[test]
fn the_next_operation_finishes_a_crashed_one_first() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);

    let report = switch(&fx, &pointer).unwrap();
    assert_eq!(report.recovered, Some(RecoveryOutcome::RolledForward));
    assert_new(&fx);
}

#[test]
fn a_file_changed_after_publishing_started_retains_verification_required_intent() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("crash");
    fs::write(&fx.b, "{\"key\": \"user edit\"}").unwrap();

    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::VerificationRequired {
            paths: vec![fx.b.clone()]
        })
    );
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "new"}));
    assert_eq!(fx.read(&fx.b), json!({"key": "user edit"}));
    assert_eq!(
        *pointer.borrow(),
        None,
        "unverified files must not commit the target"
    );
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
}

#[test]
fn a_file_changed_before_anything_was_published_discards_the_operation() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    fs::write(&fx.b, "{\"key\": \"user edit\"}").unwrap();

    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::Abandoned {
            paths: vec![fx.b.clone()]
        })
    );
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "old"}));
    assert_eq!(fx.read(&fx.b), json!({"key": "user edit"}));
    assert_eq!(*pointer.borrow(), None, "target is not committed");
    assert!(fx.temp_files().is_empty());
    assert_eq!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app).unwrap(),
        None
    );
}

/// 「已开始发布」记不下来（这里模拟状态文件写失败）就不发布：什么都没改，意图也清掉。
#[test]
fn nothing_is_published_when_the_publish_marker_cannot_be_recorded() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("mark"));
    switch(&fx, &pointer).expect_err("marker not recorded");
    failpoint::crash_at(None);

    assert_old(&fx);
    assert_eq!(*pointer.borrow(), None);
    assert!(fx.temp_files().is_empty());
    assert_eq!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app).unwrap(),
        None
    );
}

/// 「已开始发布」在换进第一个文件之前就记下了：这之后崩溃、第一个文件又被外部改掉，
/// 恢复时照样前滚，改掉的文件不动，其余文件的暂存内容照样发布，不会被当成「还没开始」
/// 丢掉。
#[test]
fn a_file_changed_after_the_publish_marker_still_rolls_forward() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("marked"));
    switch(&fx, &pointer).expect_err("crash");
    assert!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
            .unwrap()
            .unwrap()
            .published
    );
    fs::write(&fx.a, "{\"user\": 1, \"key\": \"client refresh\"}").unwrap();

    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::VerificationRequired {
            paths: vec![fx.a.clone()]
        })
    );
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "client refresh"}));
    assert_eq!(fx.read(&fx.b), json!({"key": "new"}));
    assert_eq!(*pointer.borrow(), None);
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
}

/// 放弃的时候不能连带丢掉还没发布的文件：Codex 删掉 `auth.json` 之后，登录只在暂存
/// 的临时文件里。
#[test]
fn files_still_waiting_to_be_published_are_finished_even_if_another_file_changed() {
    let fx = Fixture::new();
    let stash = fx.a.parent().unwrap().join("stash.json");
    fs::create_dir_all(stash.parent().unwrap()).unwrap();
    fs::write(&stash, "old stash").unwrap();
    let pointer = RefCell::new(None);
    let delete = crate::live::patch::WholeFile::Delete;
    let patch = set_key("new");
    let write_stash = crate::live::patch::WholeFile::Write(b"login".to_vec());
    failpoint::crash_at(Some("published:0"));
    let guard = lock_app(&fx.app);
    let result = run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[
            FileChange {
                file: LiveFile::private(&fx.a),
                patch: &delete,
            },
            FileChange {
                file: LiveFile::shared(&fx.b),
                patch: &patch,
            },
            FileChange {
                file: LiveFile::private(&stash),
                patch: &write_stash,
            },
        ],
        PendingTarget::pointer(Some("B".into())),
        &|_| Ok(()),
    );
    failpoint::crash_at(None);
    drop(guard);
    result.expect_err("crash");
    assert!(!fx.a.exists(), "the login already left a");
    fs::write(&fx.b, "{\"key\": \"client edit\"}").unwrap();

    assert_eq!(
        recover(
            &fx.store,
            &fx.key.read().unwrap(),
            &lock_app(&fx.app),
            &[
                LiveFile::private(&fx.a),
                LiveFile::shared(&fx.b),
                LiveFile::private(&stash)
            ],
            &|target| {
                *pointer.borrow_mut() = target.pointer.clone();
                Ok(())
            }
        )
        .unwrap(),
        Some(RecoveryOutcome::VerificationRequired {
            paths: vec![fx.b.clone()]
        })
    );
    assert_eq!(fs::read(&stash).unwrap(), b"login");
    assert_eq!(fx.read(&fx.b), json!({"key": "client edit"}));
    assert_eq!(*pointer.borrow(), None);
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
}

#[test]
fn a_missing_staged_file_is_skipped_and_the_rest_rolls_forward() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("crash");
    for tmp in fx.temp_files() {
        fs::remove_file(tmp).unwrap();
    }

    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::VerificationRequired {
            paths: vec![fx.b.clone()]
        })
    );
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "new"}));
    assert_eq!(fx.read(&fx.b), json!({"key": "old"}));
    assert_eq!(*pointer.borrow(), None);
}

#[test]
fn a_write_that_finds_an_unfinished_operation_finishes_it_and_asks_to_retry() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let commit = |target: &PendingTarget| {
        *pointer.borrow_mut() = target.pointer.clone();
        Ok(())
    };
    let guard = lock_app(&fx.app);
    recover_before_write(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &commit,
    )
    .expect("nothing pending");
    drop(guard);

    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    let guard = lock_app(&fx.app);
    let err = recover_before_write(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &commit,
    )
    .expect_err("state moved");
    assert!(
        matches!(
            err,
            AppError::Localized {
                key: "live.recovered_before_write",
                ..
            }
        ),
        "{err}"
    );
    assert_new(&fx);
    assert_eq!(*pointer.borrow(), Some("B".into()));
    recover_before_write(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &commit,
    )
    .expect("finished now");

    // 丢弃的操作什么都没改，照常往下写。
    drop(guard);
    fs::write(&fx.a, "{\"user\": 1, \"key\": \"old\"}").unwrap();
    fs::write(&fx.b, "{\"key\": \"old\"}").unwrap();
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    let guard = lock_app(&fx.app);
    recover_before_write(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &commit,
    )
    .expect("discarded");
}

/// macOS 上用不可变标志让替换失败（目标被占用、只读时的样子）。
#[cfg(target_os = "macos")]
struct Immutable(PathBuf);

#[cfg(target_os = "macos")]
impl Immutable {
    fn set(path: &Path, on: bool) {
        let status = std::process::Command::new("/usr/bin/chflags")
            .arg(if on { "uchg" } else { "nouchg" })
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[cfg(target_os = "macos")]
impl Drop for Immutable {
    fn drop(&mut self) {
        Self::set(&self.0, false);
    }
}

#[cfg(target_os = "macos")]
fn switch_with_locked_file(fx: &Fixture, index: usize) -> RefCell<Option<String>> {
    let path = if index == 0 {
        fx.a.clone()
    } else {
        fx.b.clone()
    };
    let _unlock = Immutable(path.clone());
    let pointer = RefCell::new(None);
    failpoint::on_before_publish(Some(Box::new(move |at, _| {
        if at == index {
            Immutable::set(&path, true);
        }
    })));
    let result = switch(fx, &pointer);
    failpoint::on_before_publish(None);
    result.expect_err("the file cannot be replaced");
    pointer
}

#[cfg(target_os = "macos")]
#[test]
fn a_publish_that_fails_midway_keeps_its_staged_file_and_rolls_forward_later() {
    let fx = Fixture::new();
    let pointer = switch_with_locked_file(&fx, 1);
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "new"}));
    assert_eq!(fx.temp_files().len(), 1, "b's staged file is kept");
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());

    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::RolledForward)
    );
    assert_new(&fx);
    assert_eq!(*pointer.borrow(), Some("B".into()));
    assert!(fx.temp_files().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn a_publish_that_fails_on_the_first_file_changes_nothing() {
    let fx = Fixture::new();
    let pointer = switch_with_locked_file(&fx, 0);
    assert_old(&fx);
    assert_eq!(*pointer.borrow(), None);
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
    assert!(!fx.temp_files().is_empty());
    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::RolledForward)
    );
    assert_new(&fx);
}

#[test]
fn a_concurrent_edit_is_merged_by_replanning() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let b = fx.b.clone();
    let mut fired = false;
    failpoint::on_before_publish(Some(Box::new(move |index, _| {
        if index == 1 && !fired {
            fired = true;
            fs::write(&b, "{\n  \"key\": \"old\",\n  \"added\": true\n}").unwrap();
        }
    })));
    let result = switch(&fx, &pointer);
    failpoint::on_before_publish(None);

    result.unwrap();
    assert_eq!(fx.read(&fx.b), json!({"key": "new", "added": true}));
    assert_eq!(*pointer.borrow(), Some("B".into()));
    assert!(fx.temp_files().is_empty());
}

#[test]
fn a_file_that_keeps_changing_before_anything_is_published_changes_nothing() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let a = fx.a.clone();
    let mut round = 0;
    failpoint::on_before_publish(Some(Box::new(move |index, _| {
        if index == 0 {
            round += 1;
            fs::write(&a, format!("{{\"user\": {round}, \"key\": \"old\"}}")).unwrap();
        }
    })));
    let result = switch(&fx, &pointer);
    failpoint::on_before_publish(None);

    assert!(matches!(result, Err(AppError::Conflict(_))), "{result:?}");
    assert_eq!(fx.read(&fx.b), json!({"key": "old"}));
    assert_eq!(*pointer.borrow(), None);
    assert_eq!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app).unwrap(),
        None
    );
    assert!(fx.temp_files().is_empty());
}

#[test]
fn a_broken_file_stops_the_whole_operation_up_front() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    fs::write(&fx.b, "{ broken").unwrap();
    let err = switch(&fx, &pointer).expect_err("refused");
    assert!(err.to_string().contains("b.json"), "{err}");
    assert_eq!(fx.read(&fx.a), json!({"user": 1, "key": "old"}));
    assert_eq!(fs::read_to_string(&fx.b).unwrap(), "{ broken");
    assert_eq!(*pointer.borrow(), None);
    assert!(fx.temp_files().is_empty());
}

#[test]
fn every_file_is_backed_up_once_before_its_first_write() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    switch(&fx, &pointer).unwrap();
    let backups: Vec<Vec<u8>> = fs::read_dir(fx.store.first_write_backup_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| !path.to_string_lossy().ends_with(".source"))
        .map(|path| {
            // Device identities use portable '/' spelling even when read_dir
            // returns a native Windows path. The registry still validates the
            // backup filename and full digest; do not normalize its namespace.
            let filename = path.file_name().unwrap().to_str().unwrap();
            let file = crate::secrets::owned_file::DeviceFile::registered(format!(
                "{}/{filename}",
                crate::secrets::owned_file::DEVICE_BACKUP_DIR,
            ))
            .unwrap();
            assert_eq!(fx.store.path_for(&file), path);
            fx.store
                .read_device(&fx.key.read().unwrap(), &file)
                .unwrap()
                .unwrap()
                .to_vec()
        })
        .collect();
    assert_eq!(backups.len(), 2);
    assert!(backups.contains(&b"{\n  \"user\": 1,\n  \"key\": \"old\"\n}".to_vec()));
}

#[test]
fn recovery_refuses_unknown_privacy_operation_contract_and_unadmitted_paths_without_cleanup() {
    for variant in [
        "privacy",
        "operation",
        "contract",
        "path",
        "staging",
        "duplicate",
        "unknown",
    ] {
        let fx = Fixture::new();
        let pointer = RefCell::new(None);
        failpoint::crash_at(Some("pending"));
        switch(&fx, &pointer).expect_err("crash");
        failpoint::crash_at(None);
        let vault = fx.key.read().unwrap();
        let mut pending = state::pending(&fx.store, &vault, &fx.app).unwrap().unwrap();
        let innocent = fx.a.parent().unwrap().join("innocent.txt");
        fs::write(&innocent, b"preserve me").unwrap();
        match variant {
            "privacy" => pending.files[0].private = None,
            "operation" => pending.op = "future-operation".into(),
            "contract" => {
                pending.target.state = Some(state::ModeState {
                    contract: Some(state::Contract {
                        version: 999,
                        key: "a".repeat(64),
                        exclusive: Default::default(),
                        extra: Default::default(),
                    }),
                    ..Default::default()
                })
            }
            "path" => pending.files[0].path = innocent.clone(),
            "staging" => pending.files[0].staged = Some(innocent.clone()),
            "duplicate" => pending.files.push(pending.files[0].clone()),
            "unknown" => {
                pending.extra.insert("future".into(), json!(true));
            }
            _ => unreachable!(),
        }
        // Simulate an authenticated journal from an unsupported/incompatible writer;
        // admission must not rely on what this version's serializer can construct.
        let mut stored = state::load(&fx.store, &vault).unwrap();
        stored.apps.get_mut(&fx.app).unwrap().pending = Some(pending);
        let file = crate::secrets::owned_file::DeviceFile::registered("live-state.json").unwrap();
        fx.store
            .write_device(&vault, &file, &serde_json::to_vec(&stored).unwrap())
            .unwrap();
        let before = fs::read(fx.store.state_path()).unwrap();
        let staged = fx.temp_files();
        let guard = lock_app(&fx.app);
        let result = recover(&fx.store, &vault, &guard, &fx.files(), &|_| {
            panic!("target must not commit")
        });
        assert!(result.is_err(), "{variant}: {result:?}");
        assert_eq!(
            fs::read(fx.store.state_path()).unwrap(),
            before,
            "{variant}"
        );
        assert_eq!(fx.temp_files(), staged, "{variant}");
        assert_eq!(fs::read(&innocent).unwrap(), b"preserve me", "{variant}");
        assert_old(&fx);
    }
}

#[test]
fn changed_staging_is_never_published_or_deleted_during_recovery() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    let pending = state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .unwrap();
    let stage = pending.files[0].staged.as_ref().unwrap();
    fs::write(stage, b"external replacement").unwrap();
    let guard = lock_app(&fx.app);
    assert!(recover(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &|_| panic!("must not commit")
    )
    .is_err());
    assert_eq!(fs::read(stage).unwrap(), b"external replacement");
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
    assert_old(&fx);
}

#[test]
fn blocked_recovery_rejects_next_operation_and_retains_original_intent() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    fs::write(
        &fx.a,
        br#"{"key":"newer same-account token", "generation":2}"#,
    )
    .unwrap();
    let previous = state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .unwrap();
    switch(&fx, &pointer).expect_err("blocked");
    assert_eq!(fx.read(&fx.a)["generation"], 2);
    assert_eq!(fx.read(&fx.a)["key"], "newer same-account token");
    assert_eq!(*pointer.borrow(), None);
    assert_eq!(
        state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
            .unwrap()
            .unwrap(),
        previous
    );
}

#[test]
fn target_only_partial_commit_is_replayed_from_durable_intent() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let guard = lock_app(&fx.app);
    let vault = fx.key.read().unwrap();
    run(
        &fx.store,
        &vault,
        &guard,
        state::op::SWITCH,
        &[],
        PendingTarget::pointer(Some("B".into())),
        &|target| {
            *pointer.borrow_mut() = target.pointer.clone();
            Err(AppError::Message(
                "injected failure after first target owner".into(),
            ))
        },
    )
    .expect_err("partial target");
    assert_eq!(*pointer.borrow(), Some("B".into()));
    assert!(
        state::pending(&fx.store, &vault, &fx.app)
            .unwrap()
            .unwrap()
            .published
    );
    assert_eq!(
        recover(&fx.store, &vault, &guard, &[], &|target| {
            assert_eq!(target.pointer.as_deref(), Some("B"));
            Ok(())
        })
        .unwrap(),
        Some(RecoveryOutcome::RolledForward)
    );
    assert!(state::pending(&fx.store, &vault, &fx.app)
        .unwrap()
        .is_none());
    assert_old(&fx);
}

#[test]
fn guarded_credential_write_does_not_replan_over_a_newer_generation() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let old = fs::read(&fx.a).unwrap();
    let guarded = crate::live::patch::Guarded {
        expected_pre: digest(Some(&old)),
        then: crate::live::patch::WholeFile::Write(
            br#"{"token":"old-snapshot", "generation":1}"#.to_vec(),
        ),
    };
    let a = fx.a.clone();
    failpoint::on_before_publish(Some(Box::new(move |_, _| {
        fs::write(&a, br#"{"token":"refreshed", "generation":2}"#).unwrap();
    })));
    let guard = lock_app(&fx.app);
    let result = run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[FileChange {
            file: LiveFile::private(&fx.a),
            patch: &guarded,
        }],
        PendingTarget::pointer(Some("B".into())),
        &|target| {
            *pointer.borrow_mut() = target.pointer.clone();
            Ok(())
        },
    );
    failpoint::on_before_publish(None);
    assert!(matches!(result, Err(AppError::Conflict(_))));
    assert_eq!(fx.read(&fx.a)["generation"], 2);
    assert_eq!(fx.read(&fx.a)["token"], "refreshed");
    assert_eq!(*pointer.borrow(), None);
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_none());
}

#[cfg(unix)]
#[test]
fn recovery_rejects_symlinked_staging_without_following_or_removing_it() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    let pending = state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .unwrap();
    let stage = pending.files[0].staged.as_ref().unwrap();
    fs::remove_file(stage).unwrap();
    std::os::unix::fs::symlink(&fx.b, stage).unwrap();
    let guard = lock_app(&fx.app);
    assert!(recover(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &|_| panic!("must not commit")
    )
    .is_err());
    assert!(fs::symlink_metadata(stage)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_old(&fx);
}

#[test]
fn file_change_during_target_commit_retains_pending_for_verification() {
    let fx = Fixture::new();
    let patch = set_key("new");
    let guard = lock_app(&fx.app);
    let result = run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[FileChange {
            file: LiveFile::shared(&fx.a),
            patch: &patch,
        }],
        PendingTarget::pointer(Some("B".into())),
        &|_| {
            fs::write(&fx.a, br#"{"key":"refreshed during target commit"}"#).unwrap();
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(fx.read(&fx.a)["key"], "refreshed during target commit");
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
}

#[test]
fn recovery_rechecks_preimage_before_each_publication() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    // The persistent marker and first file prove a publication began. Refreshing
    // the remaining credential must not be confused with the old preimage.
    let b = fx.b.clone();
    failpoint::on_before_publish(Some(Box::new(move |index, _| {
        if index == 1 {
            fs::write(&b, br#"{"key":"fresh", "generation":3}"#).unwrap();
        }
    })));
    let result = recover_now(&fx, &pointer);
    failpoint::on_before_publish(None);
    assert!(matches!(
        result,
        Some(RecoveryOutcome::VerificationRequired { .. })
    ));
    assert_eq!(fx.read(&fx.b)["generation"], 3);
    assert_eq!(*pointer.borrow(), None);
}

#[test]
fn directory_sync_failure_after_first_rename_retains_recoverable_intent() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("publish:durability"));
    switch(&fx, &pointer).expect_err("directory durability failure");
    failpoint::crash_at(None);
    assert_eq!(fx.read(&fx.a)["key"], "new", "rename already happened");
    assert_eq!(fx.read(&fx.b)["key"], "old");
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
    assert_eq!(
        recover_now(&fx, &pointer),
        Some(RecoveryOutcome::RolledForward)
    );
    assert_new(&fx);
    assert_eq!(*pointer.borrow(), Some("B".into()));
}

#[test]
fn unpublished_cleanup_revalidates_staging_and_preserves_intent_on_mismatch() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(None);
    let vault = fx.key.read().unwrap();
    let pending = state::pending(&fx.store, &vault, &fx.app).unwrap().unwrap();
    let stage = pending.files[0].staged.as_ref().unwrap();
    fs::write(stage, b"unrelated replacement").unwrap();
    drop_unpublished(&fx.store, &vault, &lock_app(&fx.app), &pending);
    assert_eq!(fs::read(stage).unwrap(), b"unrelated replacement");
    assert!(state::pending(&fx.store, &vault, &fx.app)
        .unwrap()
        .is_some());
    assert_old(&fx);
}

#[test]
fn initial_noop_file_remains_in_the_readback_contract() {
    let fx = Fixture::new();
    fs::write(&fx.a, "{\n  \"key\": \"new\"\n}").unwrap();
    let patch = set_key("new");
    let guard = lock_app(&fx.app);
    let result = run(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        state::op::SWITCH,
        &[
            FileChange {
                file: LiveFile::shared(&fx.a),
                patch: &patch,
            },
            FileChange {
                file: LiveFile::shared(&fx.b),
                patch: &patch,
            },
        ],
        PendingTarget::pointer(Some("B".into())),
        &|_| {
            fs::write(&fx.a, br#"{"key":"newer credential"}"#).unwrap();
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(fx.read(&fx.a)["key"], "newer credential");
    let pending = state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .unwrap();
    assert_eq!(pending.files.len(), 2);
    assert_eq!(pending.files[0].pre, pending.files[0].planned);
    assert_eq!(pending.files[0].staged, None);
}

#[test]
fn noop_witness_does_not_turn_unpublished_intent_into_a_published_operation() {
    let fx = Fixture::new();
    fs::write(&fx.a, "{\n  \"key\": \"new\"\n}").unwrap();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash before publishing");
    failpoint::crash_at(None);
    assert_eq!(recover_now(&fx, &pointer), Some(RecoveryOutcome::Discarded));
    assert_eq!(fx.read(&fx.a)["key"], "new");
    assert_eq!(fx.read(&fx.b)["key"], "old");
    assert_eq!(*pointer.borrow(), None);
}

#[test]
fn unlink_failure_retains_journal_and_staging_for_retry() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("crash");
    failpoint::crash_at(Some("discard"));
    let guard = lock_app(&fx.app);
    assert!(recover(
        &fx.store,
        &fx.key.read().unwrap(),
        &guard,
        &fx.files(),
        &|_| panic!("unpublished target")
    )
    .is_err());
    failpoint::crash_at(None);
    drop(guard);
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
    assert!(!fx.temp_files().is_empty());
    assert_eq!(recover_now(&fx, &pointer), Some(RecoveryOutcome::Discarded));
    assert!(fx.temp_files().is_empty());
}

#[test]
fn refreshed_preimage_after_backup_and_intent_is_not_overwritten() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    let a = fx.a.clone();
    let mut visits = 0;
    failpoint::on_before_publish(Some(Box::new(move |index, _| {
        if index == 0 {
            visits += 1;
            if visits == 2 {
                fs::write(&a, br#"{"key":"refreshed after backup", "generation":4}"#).unwrap();
            }
        }
    })));
    let result = switch(&fx, &pointer);
    failpoint::on_before_publish(None);
    assert!(matches!(result, Err(AppError::Conflict(_))));
    assert_eq!(fx.read(&fx.a)["generation"], 4);
    assert_eq!(*pointer.borrow(), None);
    assert!(state::pending(&fx.store, &fx.key.read().unwrap(), &fx.app)
        .unwrap()
        .is_some());
}

#[test]
fn fresh_store_and_reauthenticated_key_recover_only_from_durable_journal() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("published:0"));
    switch(&fx, &pointer).expect_err("simulated process interruption");
    failpoint::crash_at(None);
    let (metadata, exported_key) = {
        let vault = fx.key.read().unwrap();
        (vault.metadata().clone(), vault.export_key())
    };
    let Fixture {
        key,
        _dir: directory,
        store,
        a,
        b,
        app,
    } = fx;
    drop(key);
    drop(store);
    drop(pointer);

    // No in-memory plans, old store, target callback state or original key
    // context survive. Reauthentication uses only synthetic fixture material.
    let reopened = DeviceStore::at(directory.path().join("device"));
    let key = RwLock::new(VaultContext::from_key(metadata, exported_key).unwrap());
    let target_path = directory.path().join("target-owner.json");
    let vault = key.read().unwrap();
    let guard = lock_app(&app);
    let commit = |target: &PendingTarget| {
        let bytes = serde_json::to_vec(&target.pointer).unwrap();
        crate::config_file_io::stage_write(&target_path, &bytes, Some(0o600), true)?.commit()?;
        assert_eq!(fs::read(&target_path).unwrap(), bytes);
        Ok(())
    };
    assert_eq!(
        recover(
            &reopened,
            &vault,
            &guard,
            &[LiveFile::shared(&a), LiveFile::shared(&b)],
            &commit
        )
        .unwrap(),
        Some(RecoveryOutcome::RolledForward)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&a).unwrap()).unwrap()["key"],
        "new"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&b).unwrap()).unwrap()["key"],
        "new"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&target_path).unwrap()).unwrap(),
        "B"
    );
    assert!(state::pending(&reopened, &vault, &app).unwrap().is_none());
    assert_eq!(
        recover(
            &reopened,
            &vault,
            &guard,
            &[LiveFile::shared(&a), LiveFile::shared(&b)],
            &|_| panic!("must not duplicate target")
        )
        .unwrap(),
        None
    );
}

#[test]
fn recovery_marks_external_planned_witness_before_partial_target_effects() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("interrupted before publication");
    failpoint::crash_at(None);
    let vault = fx.key.read().unwrap();
    let initial = state::pending(&fx.store, &vault, &fx.app).unwrap().unwrap();
    assert!(!initial.published);
    fs::write(
        &fx.a,
        fs::read(initial.files[0].staged.as_ref().unwrap()).unwrap(),
    )
    .unwrap();
    let guard = lock_app(&fx.app);
    recover(&fx.store, &vault, &guard, &fx.files(), &|target| {
        *pointer.borrow_mut() = target.pointer.clone();
        Err(AppError::Message("partial target failure".into()))
    })
    .expect_err("target owner partly committed");
    assert_eq!(*pointer.borrow(), Some("B".into()));
    assert_new(&fx);

    // Both planned witnesses can be refreshed after that interrupted recovery.
    // The durable publication barrier must survive even when neither hash does.
    fs::write(&fx.a, br#"{"key":"externally refreshed a"}"#).unwrap();
    fs::write(&fx.b, br#"{"key":"externally refreshed b"}"#).unwrap();
    let outcome = recover(&fx.store, &vault, &guard, &fx.files(), &|_| {
        panic!("unverified files must not repeat target effects")
    })
    .unwrap();
    assert!(
        matches!(outcome, Some(RecoveryOutcome::VerificationRequired { .. })),
        "{outcome:?}"
    );
    assert!(
        state::pending(&fx.store, &vault, &fx.app)
            .unwrap()
            .unwrap()
            .published
    );
    assert_eq!(fx.read(&fx.a)["key"], "externally refreshed a");
    assert_eq!(fx.read(&fx.b)["key"], "externally refreshed b");
}

#[test]
fn recovery_marker_save_failure_precedes_any_publication_or_target_effect() {
    let fx = Fixture::new();
    let pointer = RefCell::new(None);
    failpoint::crash_at(Some("pending"));
    switch(&fx, &pointer).expect_err("interrupted before publication");
    failpoint::crash_at(None);
    let vault = fx.key.read().unwrap();
    let pending = state::pending(&fx.store, &vault, &fx.app).unwrap().unwrap();
    fs::write(
        &fx.a,
        fs::read(pending.files[0].staged.as_ref().unwrap()).unwrap(),
    )
    .unwrap();
    let journal_before = fs::read(fx.store.state_path()).unwrap();
    let staged_before = fx.temp_files();
    let guard = lock_app(&fx.app);
    failpoint::crash_at(Some("mark"));
    let result = recover(&fx.store, &vault, &guard, &fx.files(), &|target| {
        *pointer.borrow_mut() = target.pointer.clone();
        Ok(())
    });
    failpoint::crash_at(None);
    assert!(result.is_err(), "the recovery marker must be durable first");
    assert_eq!(*pointer.borrow(), None);
    assert_eq!(fx.read(&fx.a)["key"], "new");
    assert_eq!(fx.read(&fx.b)["key"], "old");
    assert_eq!(fs::read(fx.store.state_path()).unwrap(), journal_before);
    assert_eq!(fx.temp_files(), staged_before);
    assert!(!fx.store.first_write_backup_dir().exists());
}

#[test]
fn u03_recovery_never_clears_a_replacement_journal() {
    let fx = Fixture::new();
    let guard = lock_app(&fx.app);
    let vault = fx.key.read().unwrap();
    let pending = Pending {
        op: state::op::SWITCH.into(),
        files: vec![],
        target: PendingTarget::pointer(Some("original".into())),
        published: true,
        extra: Default::default(),
    };
    state::set_pending(&fx.store, &vault, &fx.app, Some(pending)).unwrap();
    let replacement = Pending {
        op: state::op::SWITCH.into(),
        files: vec![],
        target: PendingTarget::pointer(Some("replacement".into())),
        published: true,
        extra: Default::default(),
    };
    let result = recover(&fx.store, &vault, &guard, &[], &|_| {
        state::set_pending(&fx.store, &vault, &fx.app, Some(replacement.clone()))
    });
    assert!(result.is_err());
    assert_eq!(
        state::pending(&fx.store, &vault, &fx.app).unwrap(),
        Some(replacement)
    );
}
