use super::*;
use serde_json::json;

fn input() -> ZCodeProviderInput {
    ZCodeProviderInput {
        id: None,
        revision: "missing".into(),
        name: "Example".into(),
        api_type: "openai-responses".into(),
        base_url: "https://api.example/v1".into(),
        api_key: Some("test-key-fictional".into()),
        models: vec!["example-model".into()],
    }
}

#[test]
fn paths_honor_explicit_file_then_data_base() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let base = dir.path().join("base");
    let file = dir.path().join("custom.json");
    assert_eq!(
        resolve_path(&home, None, None).unwrap(),
        home.join(".zcode/v2/provider_config.json")
    );
    assert_eq!(
        resolve_path(&home, base.to_str(), None).unwrap(),
        base.join(".zcode/v2/provider_config.json")
    );
    assert_eq!(
        resolve_path(&home, base.to_str(), file.to_str()).unwrap(),
        file
    );
    assert!(resolve_path(&home, Some("relative"), None).is_err());
    assert!(resolve_path(&home, None, Some("relative.json")).is_err());
}

#[test]
fn adds_native_provider_without_exposing_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    save_at(&path, input()).unwrap();
    let native: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(native["schemaVersion"], 1);
    let rule = &native["config"]["providerConfigRules"]["providerRules"][0];
    assert!(rule["providerId"]
        .as_str()
        .unwrap()
        .starts_with(MANAGED_PREFIX));
    assert_eq!(rule["config"]["api"]["type"], "openai-responses");
    assert_eq!(rule["config"]["access"]["apiKey"], "test-key-fictional");
    assert_eq!(rule["config"]["personalModelIds"], json!(["example-model"]));
    let view = read_at(&path).unwrap();
    assert_eq!(view.providers.len(), 1);
    assert!(view.providers[0].has_api_key);
    assert!(!serde_json::to_string(&view)
        .unwrap()
        .contains("test-key-fictional"));
    assert!(!path.with_extension("json.lock").exists());
}

#[test]
fn preserves_other_providers_models_default_and_owned_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    save_at(&path, input()).unwrap();
    let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    doc["config"]["providerConfigRules"]["providerRules"]
        .as_array_mut()
        .unwrap()
        .push(json!({"providerId":"account:bigmodel", "config": {"visibility":"hidden"}}));
    doc["config"]["modelConfigRules"]["providerModelRules"] =
        json!([{"providerId":"elsewhere","modelId":"other","config":{}}]);
    doc["config"]["defaultModelSelection"] = json!({"providerId":"elsewhere","modelId":"other"});
    doc["config"]["providerOrder"] = json!(["elsewhere"]);
    doc["config"]["providerConfigRules"]["providerRules"][0]["config"]["api"]["headers"] =
        json!({"X-Example":"keep"});
    fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let view = read_at(&path).unwrap();
    let mut edit = input();
    edit.id = Some(view.providers[0].id.clone());
    edit.revision = view.revision;
    edit.api_key = None;
    edit.name = "Updated".into();
    save_at(&path, edit).unwrap();
    let updated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        updated["config"]["defaultModelSelection"],
        doc["config"]["defaultModelSelection"]
    );
    assert_eq!(
        updated["config"]["modelConfigRules"],
        doc["config"]["modelConfigRules"]
    );
    assert_eq!(
        updated["config"]["providerOrder"],
        doc["config"]["providerOrder"]
    );
    assert_eq!(
        updated["config"]["providerConfigRules"]["providerRules"][1],
        doc["config"]["providerConfigRules"]["providerRules"][1]
    );
    assert_eq!(
        updated["config"]["providerConfigRules"]["providerRules"][0]["config"]["api"]["headers"],
        json!({"X-Example":"keep"})
    );
    assert_eq!(
        updated["config"]["providerConfigRules"]["providerRules"][0]["config"]["access"]["apiKey"],
        "test-key-fictional"
    );
}

#[test]
fn refuses_unknown_schema_corruption_and_stale_revision_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    for bytes in [
        b"{\"schemaVersion\":2,\"config\":{}}".as_slice(),
        b"broken".as_slice(),
        b"{\"schemaVersion\":1,\"config\":{}}".as_slice(),
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(save_at(&path, input()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::remove_file(&path).unwrap();
    save_at(&path, input()).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(save_at(&path, input()).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn remove_only_owned_provider_and_references() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    save_at(&path, input()).unwrap();
    let view = read_at(&path).unwrap();
    let id = view.providers[0].id.clone();
    assert!(remove_at(&path, "account:bigmodel", &view.revision).is_err());
    remove_at(&path, &id, &view.revision).unwrap();
    assert!(read_at(&path).unwrap().providers.is_empty());
}

#[test]
fn rejects_invalid_protocol_endpoint_models_and_missing_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let mut bad = input();
    bad.api_type = "gemini".into();
    assert!(save_at(&path, bad).is_err());
    let mut bad = input();
    bad.base_url = "file:///tmp/a".into();
    assert!(save_at(&path, bad).is_err());
    let mut bad = input();
    bad.models = vec![];
    assert!(save_at(&path, bad).is_err());
    let mut bad = input();
    bad.api_key = None;
    assert!(save_at(&path, bad).is_err());
    assert!(!path.exists());
}

#[test]
fn respects_zcode_directory_lock_and_reloads_after_wait() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let lock = PathBuf::from(format!("{}.lock", path.display()));
    fs::create_dir(&lock).unwrap();
    fs::write(
        lock.join("owner-zcode.json"),
        json!({"pid":std::process::id(),"createdAt":now_ms(),"token":"zcode"}).to_string(),
    )
    .unwrap();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || save_at(&worker_path, input()));
    std::thread::sleep(Duration::from_millis(80));
    assert!(!path.exists());
    fs::remove_file(lock.join("owner-zcode.json")).unwrap();
    fs::remove_dir(&lock).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(read_at(&path).unwrap().providers.len(), 1);
}

#[test]
fn native_and_template_providers_are_read_only_even_with_our_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let mut doc = empty_document();
    doc["config"]["providerConfigRules"]["providerRules"] = json!([
        {"providerId":"native", "config":{"access":{"type":"api-key"},"api":{"type":"openai-responses"}}},
        {"providerId":"loongport-template", "templateId":"official", "config":{"access":{"type":"api-key"},"api":{"type":"openai-responses"}}},
        {"providerId":"loongport-account", "config":{"access":{"type":"zhipu-account"},"api":{"type":"openai-responses"}}}
    ]);
    fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let view = read_at(&path).unwrap();
    assert!(view.providers.iter().all(|provider| !provider.managed));
    let before = fs::read(&path).unwrap();
    for provider in view.providers {
        let mut edit = input();
        edit.id = Some(provider.id.clone());
        edit.revision = view.revision.clone();
        assert!(save_at(&path, edit).is_err());
        assert!(remove_at(&path, &provider.id, &view.revision).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn malformed_owned_api_is_rejected_without_panicking_or_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let mut doc = empty_document();
    doc["config"]["providerConfigRules"]["providerRules"] = json!([
        {"providerId":"loongport-malformed", "config":{"access":{"type":"api-key","apiKey":"fake"},"api":"broken"}}
    ]);
    fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let view = read_at(&path).unwrap();
    let mut edit = input();
    edit.id = Some("loongport-malformed".into());
    edit.revision = view.revision;
    let before = fs::read(&path).unwrap();
    assert!(save_at(&path, edit).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn stale_delete_and_lock_timeout_preserve_the_file_and_foreign_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let view = save_at(&path, input()).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(remove_at(&path, &view.providers[0].id, "stale").is_err());
    let directory = path.with_extension("json.lock");
    fs::create_dir(&directory).unwrap();
    let owner = directory.join("owner-native.json");
    fs::write(&owner, "native-lock").unwrap();
    assert!(FileLock::acquire(&path, Duration::from_millis(5)).is_err());
    assert_eq!(fs::read(&owner).unwrap(), b"native-lock");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn concurrent_saves_from_one_revision_do_not_lose_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                save_at(&path, input())
            })
        })
        .collect();
    let successes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().is_ok())
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1);
    assert_eq!(read_at(&path).unwrap().providers.len(), 1);
}

#[test]
fn all_native_protocols_round_trip_and_models_are_deduplicated() {
    for protocol in [
        "anthropic-messages",
        "openai-chat-completions",
        "openai-responses",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider_config.json");
        let mut value = input();
        value.api_type = protocol.into();
        value.models = vec![" first ".into(), "first".into(), "second".into()];
        let view = save_at(&path, value).unwrap();
        assert_eq!(view.providers[0].api_type, protocol);
        assert_eq!(view.providers[0].models, ["first", "second"]);
    }
}

#[test]
fn oversized_output_is_refused_before_replacing_the_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let mut value = input();
    value.api_key = Some("x".repeat(MAX_FILE_BYTES as usize));
    assert!(save_at(&path, value).is_err());
    assert!(!path.exists());
}

#[test]
fn native_commands_and_protocols_match_the_frontend_contract() {
    let api = include_str!("../../src/lib/api/zcode.ts");
    let commands = include_str!("lib.rs");
    let modules = include_str!("commands/mod.rs");
    assert!(modules.contains("mod zcode;"));
    assert!(modules.contains("pub use zcode::*;"));
    for name in [
        "get_zcode_config",
        "save_zcode_provider",
        "remove_zcode_provider",
    ] {
        assert!(api.contains(&format!("\"{name}\"")));
        assert!(commands.contains(&format!("commands::{name},")));
    }
    for protocol in [
        "anthropic-messages",
        "openai-chat-completions",
        "openai-responses",
    ] {
        assert!(api.contains(&format!("\"{protocol}\"")));
    }
}

#[test]
fn deleting_owned_provider_preserves_other_rules_and_default_selection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider_config.json");
    let view = save_at(&path, input()).unwrap();
    let id = &view.providers[0].id;
    let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    doc["config"]["modelConfigRules"]["providerModelRules"] = json!([
        {"providerId":id,"modelId":"example-model","config":{}},
        {"providerId":"native","modelId":"native-model","config":{}}
    ]);
    doc["config"]["modelConfigRules"]["manualProviderModelRules"] = json!([
        {"providerId":id,"modelId":"manual-model","config":{}}
    ]);
    doc["config"]["providerOrder"] = json!([id, "native"]);
    doc["config"]["defaultModelSelection"] = json!({"providerId":id,"modelId":"example-model"});
    fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let view = read_at(&path).unwrap();
    remove_at(&path, id, &view.revision).unwrap();
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        after["config"]["modelConfigRules"]["providerModelRules"],
        json!([
            {"providerId":"native","modelId":"native-model","config":{}}
        ])
    );
    assert_eq!(
        after["config"]["modelConfigRules"]["manualProviderModelRules"],
        json!([])
    );
    assert_eq!(after["config"]["providerOrder"], json!(["native"]));
    assert_eq!(
        after["config"]["defaultModelSelection"],
        doc["config"]["defaultModelSelection"]
    );
}

#[test]
fn native_credential_urls_stay_backend_only_and_cannot_be_overwritten() {
    for endpoint in [
        "https://user:fake-url-secret@api.example/v1",
        "https://api.example/v1?api_key=fake-url-secret",
        "https://api.example/v1#fake-url-secret",
        "not-a-url-fake-url-secret",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider_config.json");
        let mut doc = empty_document();
        doc["config"]["providerConfigRules"]["providerRules"] = json!([
            {"providerId":"loongport-native-url", "providerName":"Native example", "config":{
                "access":{"type":"api-key","apiKey":"fake-access-secret"},
                "api":{"type":"openai-responses","baseUrl":endpoint},
                "personalModelIds":["example-model"]
            }}
        ]);
        fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        let view = read_at(&path).unwrap();
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(!encoded.contains("fake-url-secret"));
        assert!(!encoded.contains("fake-access-secret"));
        assert!(view.providers[0].base_url.is_empty());
        assert!(!view.providers[0].managed);
        let mut edit = input();
        edit.id = Some(view.providers[0].id.clone());
        edit.revision = view.revision.clone();
        edit.name = "Unrelated rename".into();
        assert!(save_at(&path, edit).is_err());
        assert!(remove_at(&path, &view.providers[0].id, &view.revision).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
