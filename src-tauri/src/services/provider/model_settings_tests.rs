use super::*;
fn provider() -> Provider {
    let mut p = Provider::with_id(
        "same-id".into(),
        "unchanged".into(),
        json!({"auth":{"OPENAI_API_KEY":"fixture-not-secret"},"config":r#"model = "alias"
model_provider = "relay"
[model_providers.relay]
base_url = "https://relay.example/v1"
wire_api = "chat"
"#,"modelCatalog":{"models":[{"model":"alias","reasoningMode":"supported","reasoningLevels":["low","high"]},{"model":"other","reasoningLevels":[]}]}}),
        Some("https://relay.example".into()),
    );
    p.meta = Some(crate::provider::ProviderMeta {
        loongport_account_id: Some(7),
        ..Default::default()
    });
    p
}

#[test]
fn narrow_save_does_not_mutate_other_fields() {
    let p = provider();
    let patch = ModelSettingsPatch {
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let n = apply_patch(&AppType::Codex, &p, &patch).unwrap();
    validate_change(&AppType::Codex, &p, &n).unwrap();
    assert_eq!(p.settings_config["auth"], n.settings_config["auth"]);
    assert_eq!(
        p.settings_config["modelCatalog"],
        n.settings_config["modelCatalog"]
    );
}
#[test]
fn credential_endpoint_group_other_model_forgery_rejected() {
    let p = provider();
    let n = apply_patch(
        &AppType::Codex,
        &p,
        &ModelSettingsPatch {
            reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
            ..Default::default()
        },
    )
    .unwrap();
    let mut variants = vec![];
    let mut v = n.clone();
    v.settings_config["auth"]["OPENAI_API_KEY"] = json!("changed");
    variants.push(v);
    let mut v = n.clone();
    v.name = "changed".into();
    variants.push(v);
    let mut v = n.clone();
    v.meta.as_mut().unwrap().loongport_account_id = Some(99);
    variants.push(v);
    let mut v = n.clone();
    v.settings_config["modelCatalog"]["models"][1]["reasoningLevels"] = json!(["high"]);
    variants.push(v);
    let mut v = n.clone();
    v.settings_config["config"] = json!(v.settings_config["config"]
        .as_str()
        .unwrap()
        .replace("relay.example/v1", "other.example/v1"));
    variants.push(v);
    for v in variants {
        assert!(validate_change(&AppType::Codex, &p, &v).is_err());
    }
}
#[test]
fn app_identity_is_not_shared() {
    let p = provider();
    assert!(apply_patch(&AppType::Claude, &p, &ModelSettingsPatch::default()).is_err());
}
#[test]
fn refresh_retains_override_and_observes_latest_default() {
    let p = provider();
    let n = apply_patch(
        &AppType::Codex,
        &p,
        &ModelSettingsPatch {
            model: Some(FieldChange::Set("alias".into())),
            reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
            ..Default::default()
        },
    )
    .unwrap();
    let mut generated = p.clone();
    project(
        &mut generated,
        Some("new-default"),
        Some(&ReasoningIntent::Effort("high".into())),
    )
    .unwrap();
    let mut refreshed = generated.clone();
    merge_managed_refresh(&AppType::Codex, Some(&n), &generated, &mut refreshed).unwrap();
    assert_eq!(
        crate::relay::provider_config::selected_model(&AppType::Codex, &refreshed.settings_config)
            .as_deref(),
        Some("alias")
    );
    assert_eq!(
        owned(&refreshed).inherited.unwrap().model.as_deref(),
        Some("new-default")
    );
    assert_eq!(
        legacy_intent(&refreshed).unwrap(),
        ReasoningIntent::ServiceDefault
    );
}
#[test]
fn no_inheritance_cannot_restore_or_forge_snapshot() {
    let p = provider();
    assert!(apply_patch(
        &AppType::Codex,
        &p,
        &ModelSettingsPatch {
            model: Some(FieldChange::Restore),
            ..Default::default()
        }
    )
    .is_err());
    let mut n = apply_patch(&AppType::Codex, &p, &ModelSettingsPatch::default()).unwrap();
    n.meta
        .as_mut()
        .unwrap()
        .application_model_overrides
        .as_mut()
        .unwrap()
        .inherited = Some(ManagedDefaults {
        source_identity: source_identity(&AppType::Codex, &p).unwrap(),
        observed_revision: "forged".into(),
        model: Some("forged".into()),
        reasoning: ReasoningIntent::ServiceDefault,
    });
    assert!(validate_change(&AppType::Codex, &p, &n).is_err());
}
#[test]
fn legacy_metadata_deserializes_without_new_store() {
    let p = provider();
    let round: Provider = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    assert_eq!(owned(&round), ApplicationModelOverrides::default());
}

#[test]
fn refresh_does_not_upgrade_explicit_low() {
    let p = provider();
    let edited = apply_patch(
        &AppType::Codex,
        &p,
        &ModelSettingsPatch {
            reasoning: Some(FieldChange::Set(ReasoningIntent::Effort("low".into()))),
            ..Default::default()
        },
    )
    .unwrap();
    let mut generated = p.clone();
    project(
        &mut generated,
        None,
        Some(&ReasoningIntent::Effort("high".into())),
    )
    .unwrap();
    let mut refreshed = generated.clone();
    merge_managed_refresh(&AppType::Codex, Some(&edited), &generated, &mut refreshed).unwrap();
    assert_eq!(
        legacy_intent(&refreshed).unwrap(),
        ReasoningIntent::Effort("low".into())
    );
    let restored = apply_patch(
        &AppType::Codex,
        &refreshed,
        &ModelSettingsPatch {
            reasoning: Some(FieldChange::Restore),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        legacy_intent(&restored).unwrap(),
        ReasoningIntent::Effort("high".into())
    );
    assert!(owned(&restored).reasoning.is_none());
    validate_change(&AppType::Codex, &refreshed, &restored).unwrap();
}
#[test]
fn changed_source_does_not_reuse_defaults_or_overrides() {
    let p = provider();
    let mut generated = p.clone();
    let mut next = p.clone();
    merge_managed_refresh(&AppType::Codex, Some(&p), &generated, &mut next).unwrap();
    next.website_url = Some("https://different.example".into());
    assert!(apply_patch(
        &AppType::Codex,
        &next,
        &ModelSettingsPatch {
            model: Some(FieldChange::Restore),
            ..Default::default()
        }
    )
    .is_err());
    generated.meta.as_mut().unwrap().loongport_account_id = Some(8);
    assert!(merge_managed_refresh(
        &AppType::Codex,
        Some(&p),
        &generated,
        &mut generated.clone()
    )
    .is_err());
}
#[test]
fn repeated_field_save_is_idempotent() {
    let p = provider();
    let patch = ModelSettingsPatch {
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let once = apply_patch(&AppType::Codex, &p, &patch).unwrap();
    let twice = apply_patch(&AppType::Codex, &once, &patch).unwrap();
    assert_eq!(
        serde_json::to_value(&once).unwrap(),
        serde_json::to_value(&twice).unwrap()
    );
    validate_change(&AppType::Codex, &once, &twice).unwrap();
}
#[test]
fn declaration_restore_removes_only_owned_fields_not_other_model() {
    let p = provider();
    let old_other = p.settings_config["modelCatalog"]["models"][1].clone();
    let patch = ModelSettingsPatch {
        declaration: Some(DeclarationChange {
            model: "alias".into(),
            change: FieldChange::Set(ModelDeclaration {
                reasoning_mode: ReasoningMode::Unknown,
                reasoning_levels: vec![],
                default_reasoning_level: None,
            }),
        }),
        ..Default::default()
    };
    let edited = apply_patch(&AppType::Codex, &p, &patch).unwrap();
    assert_eq!(
        edited.settings_config["modelCatalog"]["models"][0]["reasoningLevels"],
        json!([])
    );
    let restored = apply_patch(
        &AppType::Codex,
        &edited,
        &ModelSettingsPatch {
            declaration: Some(DeclarationChange {
                model: "alias".into(),
                change: FieldChange::Restore,
            }),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(restored.settings_config["modelCatalog"]["models"][0]
        .get("reasoningLevels")
        .is_none());
    assert_eq!(
        restored.settings_config["modelCatalog"]["models"][1],
        old_other
    );
    validate_change(&AppType::Codex, &edited, &restored).unwrap();
}

#[test]
fn legacy_manual_projection_is_preserved_while_new_defaults_are_observed() {
    let mut old = provider();
    project(
        &mut old,
        None,
        Some(&ReasoningIntent::Effort("high".into())),
    )
    .unwrap();
    let mut defaults = provider();
    project(&mut defaults, None, Some(&ReasoningIntent::ServiceDefault)).unwrap();
    // This is the existing provision owner's user_edited preservation branch.
    let mut manual = old.clone();
    merge_managed_refresh(&AppType::Codex, Some(&old), &defaults, &mut manual).unwrap();
    assert_eq!(manual.settings_config, old.settings_config);
    assert_eq!(
        owned(&manual).inherited.unwrap().reasoning,
        ReasoningIntent::ServiceDefault
    );
}

#[test]
fn config_default_off_and_low_have_distinct_exact_projections() {
    for (intent, expected) in [
        (ReasoningIntent::ServiceDefault, None),
        (ReasoningIntent::Off, Some("none")),
        (ReasoningIntent::Effort("low".into()), Some("low")),
    ] {
        let mut p = provider();
        let auth = p.settings_config["auth"].clone();
        project(&mut p, None, Some(&intent)).unwrap();
        let doc = codex_config(&p).unwrap();
        assert_eq!(
            doc.get("model_reasoning_effort")
                .and_then(toml_edit::Item::as_str),
            expected
        );
        assert_eq!(p.settings_config["auth"], auth);
        assert_eq!(
            doc["model_providers"]["relay"]["base_url"].as_str(),
            Some("https://relay.example/v1")
        );
    }
}

#[test]
fn vendor_key_refresh_preserves_manual_endpoint_without_inheriting_official_scope() {
    let mut previous = provider();
    previous.settings_config["config"] =
        json!(crate::relay::provider_config::codex_config_toml_with_wire(
            "Manual",
            "https://custom.example/v1",
            "manual-alias",
            "chat"
        ));
    let mut generated = previous.clone();
    generated.settings_config["config"] =
        json!(crate::relay::provider_config::codex_config_toml_with_wire(
            "Official",
            "https://official.example/v1",
            "official-model",
            "responses"
        ));
    let mut next = previous.clone();
    next.settings_config["auth"]["OPENAI_API_KEY"] = json!("synthetic-refreshed-key");
    merge_managed_refresh(&AppType::Codex, Some(&previous), &generated, &mut next).unwrap();
    assert_eq!(
        next.settings_config["config"],
        previous.settings_config["config"]
    );
    assert_eq!(
        next.settings_config["auth"]["OPENAI_API_KEY"],
        "synthetic-refreshed-key"
    );
    assert!(owned(&next).inherited.is_none());
}

#[test]
fn relay_first_import_scope_projection_does_not_block_row_or_claim_default_observation() {
    let mut generated = provider();
    generated.settings_config["config"] =
        json!(crate::relay::provider_config::codex_config_toml_with_wire(
            "Relay",
            "https://relay.example/v1",
            "service-model",
            "responses"
        ));
    let mut next = generated.clone();
    assert!(crate::relay::site_config::apply_segment_to_app(
        &AppType::Codex,
        &json!({"model_providers":{"custom":{"wire_api":"chat"}}}),
        &mut next.settings_config,
    )
    .unwrap());
    assert_eq!(
        codex_config(&next).unwrap()["model_providers"]["custom"]["wire_api"].as_str(),
        Some("chat")
    );
    let expected = next.settings_config.clone();
    merge_managed_refresh(&AppType::Codex, None, &generated, &mut next).unwrap();
    assert_eq!(next.settings_config, expected);
    assert!(owned(&next).inherited.is_none());
}

#[test]
fn same_manual_scope_refresh_preserves_explicit_override_without_default_observation() {
    let p = provider();
    let previous = apply_patch(
        &AppType::Codex,
        &p,
        &ModelSettingsPatch {
            reasoning: Some(FieldChange::Set(ReasoningIntent::Effort("low".into()))),
            ..Default::default()
        },
    )
    .unwrap();
    let mut generated = p.clone();
    generated.settings_config["config"] =
        json!(crate::relay::provider_config::codex_config_toml_with_wire(
            "Official",
            "https://official.example/v1",
            "official-model",
            "responses"
        ));
    let mut next = previous.clone();
    next.settings_config["auth"]["OPENAI_API_KEY"] = json!("synthetic-new-key");
    merge_managed_refresh(&AppType::Codex, Some(&previous), &generated, &mut next).unwrap();
    assert_eq!(
        legacy_intent(&next).unwrap(),
        ReasoningIntent::Effort("low".into())
    );
    assert!(owned_scope_matches(&next));
    assert!(owned(&next).inherited.is_none());
}

#[test]
fn changed_endpoint_keeps_evidence_but_invalidates_owned_request_scope() {
    let p = provider();
    let previous = apply_patch(
        &AppType::Codex,
        &p,
        &ModelSettingsPatch {
            reasoning: Some(FieldChange::Set(ReasoningIntent::Effort("low".into()))),
            ..Default::default()
        },
    )
    .unwrap();
    let old_binding = owned(&previous).source_identity;
    let mut generated = p.clone();
    generated.settings_config["config"] =
        json!(crate::relay::provider_config::codex_config_toml_with_wire(
            "New endpoint",
            "https://new-endpoint.example/v1",
            "new-model",
            "responses"
        ));
    let mut next = generated.clone();
    merge_managed_refresh(&AppType::Codex, Some(&previous), &generated, &mut next).unwrap();
    assert_eq!(next.settings_config, generated.settings_config);
    assert_eq!(owned(&next).source_identity, old_binding);
    assert_eq!(
        owned(&next).reasoning,
        Some(ReasoningIntent::Effort("low".into()))
    );
    assert!(owned(&next).inherited.is_none());
    assert!(!owned_scope_matches(&next));
}

fn stale_owned_models() -> Provider {
    let mut p = provider();
    p.settings_config["modelCatalog"]["models"][0]["description"] = json!("keep x note");
    p.settings_config["modelCatalog"]["models"][1]["description"] = json!("keep y note");
    let binding = source_identity(&AppType::Codex, &p).unwrap();
    p.meta.as_mut().unwrap().application_model_overrides = Some(ApplicationModelOverrides {
        source_identity: Some(binding),
        model: Some("alias".into()),
        reasoning: Some(ReasoningIntent::Effort("low".into())),
        declaration_models: ["alias".into(), "other".into()].into_iter().collect(),
        transport: true,
        ..Default::default()
    });
    p.meta.as_mut().unwrap().codex_chat_reasoning =
        crate::codex_reasoning::reasoning_template("openaiChatEffort");
    p.settings_config["modelCatalog"]["models"]
        .as_array_mut()
        .unwrap()
        .push(json!({"model":"manual","reasoningLevels":[],"description":"untouched"}));
    p.settings_config["config"] = json!(p.settings_config["config"]
        .as_str()
        .unwrap()
        .replace("relay.example/v1", "new.example/v1"));
    p
}

#[test]
fn explicit_source_confirmation_clears_only_owned_fields_across_models() {
    let p = stale_owned_models();
    let patch = ModelSettingsPatch {
        confirm_source_change: true,
        source_change_revision: Some(
            crate::database::Database::provider_update_digest(&p).unwrap(),
        ),
        model: Some(FieldChange::Set("alias".into())),
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let next = apply_patch(&AppType::Codex, &p, &patch).unwrap();
    validate_change(&AppType::Codex, &p, &next).unwrap();
    let mut forged = next.clone();
    forged.settings_config["auth"]["OPENAI_API_KEY"] = json!("forged-key");
    assert!(validate_change(&AppType::Codex, &p, &forged).is_err());
    let mut forged = next.clone();
    forged.settings_config["modelCatalog"]["models"][2]["description"] = json!("forged-note");
    assert!(validate_change(&AppType::Codex, &p, &forged).is_err());
    assert!(owned_scope_matches(&next));
    assert!(owned(&next).inherited.is_none());
    assert!(owned(&next).declaration_models.is_empty());
    assert!(!owned(&next).transport);
    assert!(next.meta.as_ref().unwrap().codex_chat_reasoning.is_none());
    let rows = next.settings_config["modelCatalog"]["models"]
        .as_array()
        .unwrap();
    for row in &rows[..2] {
        assert!(row.get("reasoningMode").is_none());
        assert!(row.get("reasoningLevels").is_none());
        assert!(row.get("defaultReasoningLevel").is_none());
        assert!(row.get("description").is_some());
    }
    assert_eq!(rows[2], p.settings_config["modelCatalog"]["models"][2]);
    assert_eq!(next.settings_config["auth"], p.settings_config["auth"]);
    assert_eq!(
        codex_config(&next).unwrap()["model_providers"]["relay"]["base_url"].as_str(),
        Some("https://new.example/v1")
    );
}

#[test]
fn source_confirmation_requires_actual_change_and_two_explicit_choices() {
    let p = provider();
    let patch = ModelSettingsPatch {
        confirm_source_change: true,
        model: Some(FieldChange::Set("alias".into())),
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    assert!(apply_patch(&AppType::Codex, &p, &patch).is_err());
    let stale = stale_owned_models();
    assert!(apply_patch(
        &AppType::Codex,
        &stale,
        &ModelSettingsPatch {
            confirm_source_change: true,
            model: Some(FieldChange::Set("alias".into())),
            ..Default::default()
        }
    )
    .is_err());
    assert!(apply_patch(
        &AppType::Codex,
        &stale,
        &ModelSettingsPatch {
            model: Some(FieldChange::Set("alias".into())),
            reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
fn source_confirmation_rejects_a_changed_cleanup_snapshot_before_preview() {
    let p = stale_owned_models();
    let patch = ModelSettingsPatch {
        confirm_source_change: true,
        source_change_revision: Some(
            crate::database::Database::provider_update_digest(&p).unwrap(),
        ),
        model: Some(FieldChange::Set("alias".into())),
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let mut concurrent = p.clone();
    concurrent
        .meta
        .as_mut()
        .unwrap()
        .application_model_overrides
        .as_mut()
        .unwrap()
        .declaration_models
        .insert("another-owned-model".into());
    assert!(apply_patch(&AppType::Codex, &concurrent, &patch).is_err());
    assert!(apply_patch(&AppType::Codex, &p, &patch).is_ok());
}

#[test]
fn clearing_stale_missing_declarations_does_not_create_catalog_rows() {
    let mut p = stale_owned_models();
    p.meta
        .as_mut()
        .unwrap()
        .application_model_overrides
        .as_mut()
        .unwrap()
        .declaration_models
        .insert("already-removed".into());
    let patch = ModelSettingsPatch {
        confirm_source_change: true,
        source_change_revision: Some(
            crate::database::Database::provider_update_digest(&p).unwrap(),
        ),
        model: Some(FieldChange::Set("alias".into())),
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let next = apply_patch(&AppType::Codex, &p, &patch).unwrap();
    assert_eq!(
        next.settings_config["modelCatalog"]["models"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    validate_change(&AppType::Codex, &p, &next).unwrap();
}
