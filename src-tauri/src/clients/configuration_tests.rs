use super::*;
use serde_json::json;
use std::sync::RwLock;

#[test]
fn preview_apply_conflict_and_disconnect_preserve_user_edits() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("models.json");
    let store = DeviceStore::at(dir.path().join("device"));
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    std::fs::write(
        &path,
        br#"{"models":[{"id":"personal","extra":42}],"unknown":true}"#,
    )
    .unwrap();
    let owner = Configuration::new(store);
    let entry = json!({"id":"model-a","vendor":"LoongPort","apiKey":"synthetic-client-secret","url":"http://127.0.0.1:1234/v1/chat/completions"});
    let request = Connection {
        client_id: "workbuddy".into(),
        credential_ref: "ref-a".into(),
        route_app: "codex".into(),
        models: vec!["model-a".into()],
    };
    let preview = owner
        .preview(&vault, &path, &request, &json!([entry.clone()]))
        .unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("model-a"));
    std::fs::write(&path, br#"{"models":[],"external":true}"#).unwrap();
    assert!(owner
        .apply(
            &vault,
            &path,
            &request,
            &json!([entry.clone()]),
            &preview.token
        )
        .is_err());
    let preview = owner
        .preview(&vault, &path, &request, &json!([entry.clone()]))
        .unwrap();
    owner
        .apply(
            &vault,
            &path,
            &request,
            &json!([entry.clone()]),
            &preview.token,
        )
        .unwrap();
    let mut current: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    current["external"] = json!("edited");
    current["models"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"new-personal"}));
    std::fs::write(&path, serde_json::to_vec(&current).unwrap()).unwrap();
    owner.disconnect(&vault, &path, "workbuddy").unwrap();
    let restored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        restored,
        json!({"models":[{"id":"new-personal"}],"external":"edited"})
    );
    let encrypted = std::fs::read(
        owner
            .store
            .state_path()
            .parent()
            .unwrap()
            .join("client-connections.json"),
    )
    .unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("synthetic-client-secret"));
}

#[test]
fn final_conflict_check_preserves_external_bytes_and_clears_intent() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, b"[]").unwrap();
    let owner = Configuration::new(DeviceStore::at(dir.path().join("device")));
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    let plan = engine::plan(
        &LiveFile::private(&path),
        &WholeFile::Write(b"[1]".to_vec()),
    )
    .unwrap();
    std::fs::write(&path, b"[2]").unwrap();
    assert!(owner
        .commit(&vault, &mut State::default(), "workbuddy", plan, None)
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"[2]");
    assert!(owner.load(&vault).unwrap().clients["workbuddy"]
        .pending
        .is_none());
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".tmp.")));
}

#[test]
fn explicit_recovery_recognizes_before_and_after_without_replaying() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("models.json");
    let owner = Configuration::new(DeviceStore::at(dir.path().join("device")));
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    for bytes in [b"[]".as_slice(), b"[1]", b"[2]"] {
        std::fs::write(&path, bytes).unwrap();
        let mut state = State::default();
        state.clients.insert(
            "workbuddy".into(),
            Record {
                active: None,
                pending: Some(Intent {
                    path_identity: engine::sha256_hex(path.to_string_lossy().as_bytes()),
                    pre: engine::digest(Some(b"[]")),
                    post: engine::digest(Some(b"[1]")),
                    next: None,
                }),
            },
        );
        owner.save(&vault, &state).unwrap();
        let result = owner.recover(&vault, &path, "workbuddy");
        assert_eq!(result.is_ok(), bytes != b"[2]");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            owner.load(&vault).unwrap().clients["workbuddy"]
                .pending
                .is_some(),
            bytes == b"[2]"
        );
    }
}

#[test]
fn changed_owned_entry_is_not_removed_or_silently_adopted() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let path = dir.path().join("models.json");
    let owner = Configuration::new(DeviceStore::at(dir.path().join("device")));
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    let request = Connection {
        client_id: "workbuddy".into(),
        credential_ref: "ref-a".into(),
        route_app: "codex".into(),
        models: vec!["model-a".into()],
    };
    let entries = json!([{"id":"model-a","vendor":"LoongPort","apiKey":"fixture-key"}]);
    let preview = owner.preview(&vault, &path, &request, &entries).unwrap();
    owner
        .apply(&vault, &path, &request, &entries, &preview.token)
        .unwrap();
    let first = std::fs::read(&path).unwrap();
    let preview = owner.preview(&vault, &path, &request, &entries).unwrap();
    assert!(!preview.changed);
    owner
        .apply(&vault, &path, &request, &entries, &preview.token)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), first);
    let edited = br#"[{"id":"model-a","vendor":"LoongPort","apiKey":"user-edited"}]"#;
    std::fs::write(&path, edited).unwrap();
    assert!(owner.disconnect(&vault, &path, "workbuddy").is_err());
    assert!(owner.preview(&vault, &path, &request, &entries).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), edited);
}
