use super::*;
use crate::error::AppError;
use crate::live::engine::DeviceStore;
use crate::secrets::owned_file::{DeviceFile, DEVICE_STATE_FILE};
use crate::secrets::VaultContext;
use serde_json::json;
use std::fs;
use std::sync::{Arc, Barrier, RwLock, RwLockReadGuard};

fn state_file() -> DeviceFile {
    DeviceFile::registered(DEVICE_STATE_FILE).unwrap()
}

fn write_fixture(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    bytes: &[u8],
) -> Vec<u8> {
    let sealed = state_file().encode(vault, bytes).unwrap();
    fs::write(store.state_path(), &sealed).unwrap();
    sealed
}

fn sample_pending() -> Pending {
    Pending {
        op: op::SWITCH.into(),
        files: vec![PendingFile {
            path: PathBuf::from("/synthetic/client.json"),
            private: Some(true),
            pre: None,
            planned: Some("a".repeat(64)),
            staged: Some(PathBuf::from("/synthetic/client.json.tmp.1")),
            extra: Map::new(),
        }],
        target: PendingTarget::pointer(Some("synthetic-sensitive-provider".into())),
        published: false,
        extra: Map::new(),
    }
}

#[test]
fn missing_state_reads_are_empty_without_creating_storage() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path().join("not-created"));
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();

    assert_eq!(load(&store, &vault).unwrap(), LiveState::default());
    assert_eq!(pending(&store, &vault, "codex").unwrap(), None);
    assert_eq!(
        mode_state(&store, &vault, "codex").unwrap(),
        ModeState::default()
    );
    assert_eq!(written(&store, &vault, "codex").unwrap(), None);
    assert_eq!(
        stack(&store, &vault, "codex").unwrap(),
        StackState::default()
    );
    assert!(!stack_mode(&store, &vault, "codex").unwrap());
    assert!(apps_with_pending(&store, &vault).unwrap().is_empty());
    assert!(!store.root().exists());
}

#[test]
fn pending_round_trips_as_private_authenticated_ciphertext_and_clears() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();

    set_pending(&store, &vault, "codex", Some(sample_pending())).unwrap();
    assert_eq!(
        pending(&store, &vault, "codex").unwrap(),
        Some(sample_pending())
    );
    assert_eq!(apps_with_pending(&store, &vault).unwrap(), vec!["codex"]);

    let sealed = fs::read(store.state_path()).unwrap();
    assert!(sealed.starts_with(b"lpenc1."));
    for canary in [
        "synthetic-sensitive-provider",
        "/synthetic/client.json",
        "codex",
    ] {
        assert!(!String::from_utf8_lossy(&sealed).contains(canary));
    }
    let plaintext = state_file().decode(&vault, &sealed).unwrap();
    assert_eq!(
        decode(&plaintext).unwrap().apps["codex"].pending,
        Some(sample_pending())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.state_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    set_pending(&store, &vault, "codex", None).unwrap();
    assert_eq!(load(&store, &vault).unwrap(), LiveState::default());
    assert_eq!(pending(&store, &vault, "codex").unwrap(), None);
}

#[test]
fn getters_read_committed_modes_written_and_stack_without_changing_ciphertext() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();
    let state: LiveState = serde_json::from_value(json!({
        "version": 1,
        "apps": {
            "codex": {
                "mode": "proxy", "attached": true, "proxy_route": "synthetic-provider",
                "written": {"tables": ["synthetic-table"]},
                "stack": {"enabled": true, "members": ["synthetic-provider"], "keys": {"key": "synthetic-provider"}}
            },
            "claude": {"mode": "direct", "stack": {"enabled": true}}
        }
    })).unwrap();
    update(&store, &vault, |current| {
        *current = state.clone();
        Ok(())
    })
    .unwrap();
    let before = fs::read(store.state_path()).unwrap();

    assert_eq!(
        mode_states(&store, &vault, ["codex", "absent", "claude"]).unwrap(),
        [
            state.apps["codex"].mode_state(),
            ModeState::default(),
            state.apps["claude"].mode_state()
        ]
    );
    assert_eq!(
        written(&store, &vault, "codex").unwrap(),
        state.apps["codex"].written
    );
    assert_eq!(
        stack(&store, &vault, "codex").unwrap(),
        state.apps["codex"].stack
    );
    assert!(stack_mode(&store, &vault, "codex").unwrap());
    assert!(!stack_mode(&store, &vault, "claude").unwrap());
    assert_eq!(fs::read(store.state_path()).unwrap(), before);
}

#[test]
fn failed_change_keeps_original_ciphertext_and_does_not_create_missing_state() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();
    let change = |current: &mut LiveState| -> Result<(), AppError> {
        current.apps.insert(
            "codex".into(),
            AppLiveState {
                mode: Some(Mode::Proxy),
                ..Default::default()
            },
        );
        Err(AppError::Config("synthetic.rejected_change".into()))
    };
    assert!(update(&store, &vault, change).is_err());
    assert!(!store.state_path().exists());

    set_pending(&store, &vault, "codex", Some(sample_pending())).unwrap();
    let before = fs::read(store.state_path()).unwrap();
    assert!(update(&store, &vault, change).is_err());
    assert_eq!(fs::read(store.state_path()).unwrap(), before);
}

#[test]
fn invalid_authenticated_state_is_never_reset_renamed_or_overwritten() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();

    for invalid in [
        b"synthetic-secret-not-json".as_slice(),
        br#"{}"#,
        br#"{"version":0}"#,
        br#"{"version":2}"#,
        br#"{"version":1,"version":1}"#,
        br#"{"version":1,"apps":{"codex":{},"codex":{}}}"#,
        br#"{"version":1,"future":{"secret":"a","secret":"b"}}"#,
        br#"{"version":1,"apps":{"codex":{"mode":"synthetic-secret-mode"}}}"#,
    ] {
        let before = write_fixture(&store, &vault, invalid);
        let error = load(&store, &vault).unwrap_err();
        assert!(!error.to_string().contains("synthetic-secret"));
        assert!(set_pending(&store, &vault, "codex", None).is_err());
        assert_eq!(fs::read(store.state_path()).unwrap(), before);
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
    }
}

#[test]
fn plaintext_wrong_key_and_wrong_file_identity_are_not_accepted_or_repaired() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();
    let other_vault = VaultContext::generate().unwrap();
    let plaintext = br#"{"version":1,"apps":{}}"#;

    for bytes in [
        plaintext.to_vec(),
        state_file().encode(&other_vault, plaintext).unwrap(),
        DeviceFile::registered("codex-login-stash.json")
            .unwrap()
            .encode(&vault, plaintext)
            .unwrap(),
    ] {
        fs::write(store.state_path(), &bytes).unwrap();
        assert!(load(&store, &vault).is_err());
        assert!(set_pending(&store, &vault, "codex", None).is_err());
        assert_eq!(fs::read(store.state_path()).unwrap(), bytes);
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
    }
}

fn unsupported_apps() -> Vec<Value> {
    vec![
        json!({"futureApp":"synthetic-secret-keep"}),
        json!({"contract":{"version":1,"key":"key","futureContract":true}}),
        json!({"contract":{"version":2,"key":"key"}}),
        json!({"written":{"futureWritten":true}}),
        json!({"stack":{"futureStack":true}}),
        json!({"pending":{"op":"future-op","files":[]}}),
        json!({"pending":{"op":"apply","files":[],"futurePending":true}}),
        json!({"pending":{"op":"apply","files":[{"path":"/synthetic/client","pre":null,"private":true,"futureFile":true}]}}),
        json!({"pending":{"op":"apply","files":[{"path":"/synthetic/client","pre":null}]}}),
        json!({"pending":{"op":"apply","files":[],"target":{"futureTarget":true}}}),
        json!({"pending":{"op":"apply","files":[],"target":{"state":{"futureMode":true}}}}),
        json!({"pending":{"op":"apply","files":[],"target":{"state":{"contract":{"version":2,"key":"key"}}}}}),
        json!({"pending":{"op":"apply","files":[],"target":{"written":{"futureWritten":true}}}}),
        json!({"pending":{"op":"apply","files":[],"target":{"stack":{"futureStack":true}}}}),
    ]
}

#[test]
fn untouched_future_data_survives_known_app_updates_but_cannot_be_changed_or_removed() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();

    for unsupported in unsupported_apps() {
        let value =
            json!({"version":1,"futureRoot":{"keep":true},"apps":{"future-app":unsupported}});
        write_fixture(&store, &vault, &serde_json::to_vec(&value).unwrap());
        let expected = load(&store, &vault).unwrap();
        set_pending(&store, &vault, "codex", Some(sample_pending())).unwrap();
        let updated = load(&store, &vault).unwrap();
        assert_eq!(updated.extra, expected.extra);
        assert_eq!(updated.apps["future-app"], expected.apps["future-app"]);

        let before = fs::read(store.state_path()).unwrap();
        assert!(update(&store, &vault, |current| {
            current.apps.remove("future-app");
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(store.state_path()).unwrap(), before);
        assert!(update(&store, &vault, |current| {
            current.apps.get_mut("future-app").unwrap().attached = true;
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(store.state_path()).unwrap(), before);
    }
}

#[test]
fn updates_cannot_introduce_unknown_fields_or_unsupported_contracts() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();
    set_pending(&store, &vault, "codex", Some(sample_pending())).unwrap();
    let before = fs::read(store.state_path()).unwrap();

    for unsupported in unsupported_apps() {
        assert!(update(&store, &vault, |current| {
            current.apps.insert(
                "future-app".into(),
                serde_json::from_value(unsupported).unwrap(),
            );
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(store.state_path()).unwrap(), before);
    }
}

#[test]
fn updates_cannot_change_unknown_root_fields_or_the_state_version() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();
    let before = write_fixture(
        &store,
        &vault,
        br#"{"version":1,"futureRoot":"synthetic-secret-keep","apps":{}}"#,
    );

    assert!(update(&store, &vault, |current| {
        current.extra.clear();
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(store.state_path()).unwrap(), before);
    assert!(update(&store, &vault, |current| {
        current.extra.insert("newUnknown".into(), json!(true));
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(store.state_path()).unwrap(), before);
    assert!(update(&store, &vault, |current| {
        current.version = 2;
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(store.state_path()).unwrap(), before);
}

#[test]
fn update_returns_closure_result_and_only_prunes_genuinely_empty_apps() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = RwLock::new(VaultContext::generate().unwrap());
    let vault = vault_lock.read().unwrap();
    let result = update(&store, &vault, |current| {
        current.apps.insert("empty".into(), AppLiveState::default());
        current.apps.insert(
            "written".into(),
            AppLiveState {
                written: Some(Written::default()),
                ..Default::default()
            },
        );
        current.apps.insert(
            "stack".into(),
            AppLiveState {
                stack: StackState {
                    keys: BTreeMap::from([("key".into(), "provider".into())]),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        Ok(41)
    })
    .unwrap();
    assert_eq!(result, 41);
    assert_eq!(
        load(&store, &vault)
            .unwrap()
            .apps
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["stack", "written"]
    );
}

#[test]
fn concurrent_read_modify_write_preserves_every_app() {
    let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(fixture.path());
    let vault_lock = Arc::new(RwLock::new(VaultContext::generate().unwrap()));
    let start = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let store = store.clone();
            let vault_lock = Arc::clone(&vault_lock);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                let vault = vault_lock.read().unwrap();
                start.wait();
                update(&store, &vault, |current| {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    current.apps.insert(
                        format!("app-{index}"),
                        AppLiveState {
                            mode: Some(Mode::Direct),
                            ..Default::default()
                        },
                    );
                    Ok(())
                })
                .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let vault = vault_lock.read().unwrap();
    let state = load(&store, &vault).unwrap();
    assert_eq!(state.apps.len(), 8);
    for index in 0..8 {
        assert_eq!(state.apps[&format!("app-{index}")].mode, Some(Mode::Direct));
    }
}
