use super::*;
use serde_json::json;

fn provider() -> Value {
    json!({"name":"Test","kind":"custom","enabled":true,"api":"openai-completions","options":{"baseURL":"https://example.invalid/v1","apiKey":"fixture-secret"},"models":{"model":{"name":"Model","future":42}}})
}

#[test]
fn changes_preserve_native_accounts_unknown_fields_and_selected_models() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let original = "defaultModel: minimax/M3\ncustom_provider:\n  official:\n    kind: minimax\n    secret: keep\nminimax_api:\n  apiKey: keep-secret\nunknown: [a, b]\n";
    fs::write(&path, original).unwrap();
    let before = read(&path).unwrap();
    transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |_| Ok(()),
        || Ok(()),
    )
    .unwrap();
    let after = read(&path).unwrap();
    assert_eq!(
        after["custom_provider"]["official"],
        before["custom_provider"]["official"]
    );
    for field in ["defaultModel", "minimax_api", "unknown"] {
        assert_eq!(after[field], before[field]);
    }
    assert_eq!(
        after["custom_provider"]["test"]["models"]["model"]["future"],
        42
    );
    assert!(transact_at(
        &path,
        "official",
        Some(provider()),
        false,
        None,
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
}

#[test]
fn referenced_main_or_light_model_cannot_be_removed_or_disabled() {
    for field in ["defaultModel", "defaultLightModel"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let text=format!("{field}: custom_provider:test/model\ncustom_provider:\n  test:\n    models:\n      model: {{}}\n");
        fs::write(&path, &text).unwrap();
        for candidate in [
            None,
            Some(json!({"enabled":false,"models":{"model":{}}})),
            Some(json!({"models":{"other":{}}})),
        ] {
            assert!(
                transact_at(&path, "test", candidate, false, None, |_| Ok(()), || Ok(())).is_err()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }
}

#[test]
fn revision_conflict_and_failed_backup_never_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "unknown: keep\n").unwrap();
    assert!(transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        Some("stale"),
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
    assert!(transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |_| Err(AppError::Config("backup unavailable".into())),
        || Ok(())
    )
    .is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "unknown: keep\n");
}

#[test]
fn failed_commit_restores_original_bytes_while_holding_native_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let bytes = b"# exact original\nunknown: keep\n";
    fs::write(&path, bytes).unwrap();
    let result: Result<(), AppError> = transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |_| Ok(()),
        || {
            assert!(path.with_extension("yaml.lock").exists());
            Err(AppError::Database("injected".into()))
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(!path.with_extension("yaml.lock").exists());
}

#[test]
fn malformed_file_and_active_lock_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "custom_provider: [invalid]\n").unwrap();
    assert!(transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "custom_provider: [invalid]\n"
    );
    fs::write(&path, "{}\n").unwrap();
    fs::create_dir(path.with_extension("yaml.lock")).unwrap();
    assert!(transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "{}\n");
}

#[test]
fn duplicate_creation_and_concurrent_external_edit_do_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "custom_provider:\n  test: {name: original}\n").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let result: Result<(), AppError> = transact_at(
        &path,
        "test",
        Some(provider()),
        false,
        None,
        |_| Ok(()),
        || {
            fs::write(&path, "external: newer\n").unwrap();
            Err(AppError::Database("injected".into()))
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "external: newer\n");
}

#[test]
fn reviewed_restore_is_bound_to_target_and_unchanged_written_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let original = b"# original\nunknown: keep\n";
    fs::write(&path, original).unwrap();
    let mut saved = None;
    transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |receipt| {
            saved = Some(receipt.clone());
            Ok(())
        },
        || Ok(()),
    )
    .unwrap();
    let receipt = saved.unwrap();
    assert!(restore_at(
        &dir.path().join("other.yaml"),
        &receipt,
        &receipt.after_revision,
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
    fs::write(&path, "external: newer\n").unwrap();
    assert!(restore_at(
        &path,
        &receipt,
        &receipt.after_revision,
        |_| Ok(()),
        || Ok(())
    )
    .is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "external: newer\n");
    // A fresh write creates a new receipt that can restore its exact preimage.
    let mut saved = None;
    transact_at(
        &path,
        "test",
        Some(provider()),
        true,
        None,
        |receipt| {
            saved = Some(receipt.clone());
            Ok(())
        },
        || Ok(()),
    )
    .unwrap();
    let receipt = saved.unwrap();
    restore_at(
        &path,
        &receipt,
        &receipt.after_revision,
        |_| Ok(()),
        || Ok(()),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "external: newer\n");
}

#[test]
fn unknown_provider_kinds_and_shapes_are_never_listed_or_overwritten() {
    for native in ["kind: true", "kind: null", "kind: {}", "kind: native-login"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let text = format!("custom_provider:\n  native:\n    {native}\n    token: preserve\n");
        fs::write(&path, &text).unwrap();
        assert!(providers_from(&read(&path).unwrap()).unwrap().is_empty());
        for replacement in [None, Some(provider())] {
            assert!(transact_at(
                &path,
                "native",
                replacement,
                false,
                None,
                |_| Ok(()),
                || Ok(())
            )
            .is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    let text = "custom_provider:\n  opaque: opaque-native-value\n";
    fs::write(&path, text).unwrap();
    assert!(providers_from(&read(&path).unwrap()).unwrap().is_empty());
    assert!(transact_at(&path, "opaque", None, false, None, |_| Ok(()), || Ok(())).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn default_provider_references_are_read_only_deduplicated_native_facts() {
    let document = parse(Some(b"defaultModel: custom_provider:primary/model/with/slashes\ndefaultLightModel: custom_provider:light/small\nunknown: keep\n")).unwrap();
    let before = document.clone();
    assert_eq!(
        referenced_provider_ids_from(&document),
        ["primary".into(), "light".into()].into_iter().collect()
    );
    assert_eq!(document, before);
    for (main, light, expected) in [
        (
            "custom_provider:same/one",
            "custom_provider:same/two",
            vec!["same"],
        ),
        ("minimax/M3", "custom_provider:light/small", vec!["light"]),
        (
            "custom_provider:missing-slash",
            "custom_provider:/model",
            vec![],
        ),
        ("custom_provider:id/", "other_provider:id/model", vec![]),
    ] {
        let document = parse(Some(
            format!("defaultModel: {main}\ndefaultLightModel: {light}\n").as_bytes(),
        ))
        .unwrap();
        assert_eq!(
            referenced_provider_ids_from(&document),
            expected.into_iter().map(str::to_string).collect()
        );
    }
    assert!(referenced_provider_ids_from(&parse(None).unwrap()).is_empty());
}

#[test]
fn validation_rejects_non_object_model_entries() {
    for model in [json!(null), json!(false), json!("opaque"), json!([])] {
        let mut candidate = provider();
        candidate["models"]["model"] = model;
        assert!(validate_provider("test", &candidate).is_err());
    }
}

#[test]
fn validation_rejects_non_boolean_enabled_flags() {
    for enabled in [json!(null), json!("false"), json!(0), json!({})] {
        let mut candidate = provider();
        candidate["enabled"] = enabled.clone();
        assert!(validate_provider("test", &candidate).is_err());
        let mut candidate = provider();
        candidate["models"]["model"]["enabled"] = enabled;
        assert!(validate_provider("test", &candidate).is_err());
    }
}

#[test]
fn validation_rejects_non_string_header_maps() {
    for headers in [
        json!(null),
        json!([]),
        json!("header"),
        json!({"X-Test":false}),
        json!({"X-Test":42}),
    ] {
        let mut candidate = provider();
        candidate["options"]["headers"] = headers;
        assert!(validate_provider("test", &candidate).is_err());
    }
}

#[test]
fn validation_rejects_endpoint_userinfo() {
    for endpoint in [
        "https://user@example.invalid/v1",
        "https://:password@example.invalid/v1",
        "http://user:password@example.invalid/v1",
    ] {
        let mut candidate = provider();
        candidate["options"]["baseURL"] = json!(endpoint);
        assert!(validate_provider("test", &candidate).is_err());
    }
}

#[test]
fn validation_accepts_known_fields_and_preserves_opaque_extensions() {
    let mut candidate = provider();
    candidate["enabled"] = json!(false);
    candidate["models"]["model"]["enabled"] = json!(true);
    candidate["models"]["model"]["opaque"] = json!([1, "keep"]);
    candidate["options"]["headers"] = json!({"X-Test":"keep"});
    candidate["options"]["opaque"] = json!({"nested":true});
    candidate["opaque"] = json!({"future":42});
    let before = candidate.clone();
    assert!(validate_provider("test", &candidate).is_ok());
    assert_eq!(candidate, before);
    candidate["options"]["baseURL"] = json!("http://127.0.0.1:1234/v1");
    assert!(validate_provider("test", &candidate).is_ok());
    candidate.as_object_mut().unwrap().remove("enabled");
    candidate["models"]["model"]
        .as_object_mut()
        .unwrap()
        .remove("enabled");
    candidate["options"]
        .as_object_mut()
        .unwrap()
        .remove("headers");
    assert!(validate_provider("test", &candidate).is_ok());
}

#[test]
fn oversized_write_is_rejected_before_backup_or_native_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "unknown: retained\n").unwrap();
    let mut candidate = provider();
    candidate["oversized"] = Value::String("x".repeat(MAX_CONFIG_BYTES as usize));
    let mut backed_up = false;
    assert!(transact_at(
        &path,
        "test",
        Some(candidate),
        true,
        None,
        |_| {
            backed_up = true;
            Ok(())
        },
        || Ok(())
    )
    .is_err());
    assert!(!backed_up);
    assert_eq!(fs::read_to_string(&path).unwrap(), "unknown: retained\n");
}
