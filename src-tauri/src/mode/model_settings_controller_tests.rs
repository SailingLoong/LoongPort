//! Real controller/SQLite/journal integration. Uses only synthetic TestHome rows.
use super::*;
use crate::codex_reasoning::ReasoningIntent;
use crate::services::provider::model_settings::{self, FieldChange, ModelSettingsPatch};

#[test]
#[serial_test::serial]
fn model_settings_real_save_preserves_ownership_and_lost_reply_identity() {
    for (user_edited, source_changed) in [(false, false), (true, false), (false, true)] {
        let fx = Fixture::for_app(AppType::Codex);
        fx.state
            .db
            .set_user_edited("codex", "b", user_edited)
            .unwrap();
        let before = fx
            .state
            .db
            .get_provider_by_id("b", "codex")
            .unwrap()
            .unwrap();
        if source_changed {
            let bound = model_settings::apply_patch(
                &AppType::Codex,
                &before,
                &ModelSettingsPatch {
                    reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
                    ..Default::default()
                },
            )
            .unwrap();
            let mut changed = bound;
            changed.settings_config["config"] = json!(changed.settings_config["config"]
                .as_str()
                .unwrap()
                .replace("b.example.invalid", "new-b.example.invalid"));
            fx.state.db.save_provider("codex", &changed).unwrap();
        }
        let native = read_current(&fx.path).unwrap();
        let patch = ModelSettingsPatch {
            confirm_source_change: source_changed,
            source_change_revision: source_changed.then(|| {
                crate::database::Database::provider_update_digest(
                    &fx.state
                        .db
                        .get_provider_by_id("b", "codex")
                        .unwrap()
                        .unwrap(),
                )
                .unwrap()
            }),
            model: source_changed.then(|| FieldChange::Set("model-b".into())),
            reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
            ..Default::default()
        };
        let preview = model_settings::preview(
            &fx.state,
            &AppType::Codex,
            "b",
            &patch,
            "d15d3030-2000-4000-8000-000000000001",
        )
        .unwrap();
        assert_eq!(preview["status"], "ready");
        assert_eq!(preview["action"], "saveOnly");
        let request: state::SaveRequest =
            serde_json::from_value(preview["request"].clone()).unwrap();
        assert_eq!(request.scope, state::SaveScope::ProviderModelSettings);
        let result =
            model_settings::confirm(&fx.state, &AppType::Codex, "b", &patch, request.clone())
                .unwrap();
        assert_eq!(result["status"], "completed");
        let saved = fx
            .state
            .db
            .get_provider_by_id("b", "codex")
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.settings_config["auth"],
            before.settings_config["auth"]
        );
        assert_eq!(saved.name, before.name);
        assert_eq!(
            fx.state.db.get_user_edited("codex", "b").unwrap(),
            user_edited
        );
        assert_eq!(read_current(&fx.path).unwrap(), native);
        let digest = crate::database::Database::provider_update_digest(&saved).unwrap();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        assert_eq!(
            model_settings::confirm(&fx.state, &AppType::Codex, "b", &patch, request.clone())
                .unwrap()["status"],
            "completed"
        );
        assert_eq!(
            crate::services::provider::edit::query(&fx.state, &AppType::Codex, request).unwrap()
                ["status"],
            "completed"
        );
        assert_eq!(
            crate::database::Database::provider_update_digest(
                &fx.state
                    .db
                    .get_provider_by_id("b", "codex")
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            digest
        );
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
    }
}

#[test]
#[serial_test::serial]
fn model_settings_real_stale_row_cannot_publish_partial_override() {
    let fx = Fixture::for_app(AppType::Codex);
    let patch = ModelSettingsPatch {
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let preview = model_settings::preview(
        &fx.state,
        &AppType::Codex,
        "b",
        &patch,
        "d15d3030-2000-4000-8000-000000000002",
    )
    .unwrap();
    assert_eq!(preview["status"], "ready");
    let request: state::SaveRequest = serde_json::from_value(preview["request"].clone()).unwrap();
    let mut concurrent = fx
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    concurrent.name = "Concurrent legitimate edit".into();
    fx.state.db.save_provider("codex", &concurrent).unwrap();
    let digest = crate::database::Database::provider_update_digest(&concurrent).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let native = read_current(&fx.path).unwrap();
    let result = model_settings::confirm(&fx.state, &AppType::Codex, "b", &patch, request).unwrap();
    // Reconstructing against the changed row cannot match the original full draft digest.
    assert_eq!(result["status"], "conflict");
    assert_eq!(
        crate::database::Database::provider_update_digest(
            &fx.state
                .db
                .get_provider_by_id("b", "codex")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        digest
    );
    assert_eq!(
        std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
        journal
    );
    assert_eq!(read_current(&fx.path).unwrap(), native);
}

#[test]
#[serial_test::serial]
fn model_settings_real_source_confirmation_binds_read_cleanup_revision() {
    let fx = Fixture::for_app(AppType::Codex);
    let row = fx
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    let mut changed = model_settings::apply_patch(
        &AppType::Codex,
        &row,
        &ModelSettingsPatch {
            reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
            ..Default::default()
        },
    )
    .unwrap();
    changed.settings_config["config"] = json!(changed.settings_config["config"]
        .as_str()
        .unwrap()
        .replace("b.example.invalid", "new-b.example.invalid"));
    changed.settings_config["modelCatalog"] =
        json!({"models":[{"model":"another-owned-model","description":"keep note"}]});
    fx.state.db.save_provider("codex", &changed).unwrap();
    let read = model_settings::read(&fx.state, &AppType::Codex, "b").unwrap();
    assert_eq!(read["sourceChanged"], true);
    let mut patch = ModelSettingsPatch {
        confirm_source_change: true,
        source_change_revision: Some(read["revision"].as_str().unwrap().into()),
        model: Some(FieldChange::Set("model-b".into())),
        reasoning: Some(FieldChange::Set(ReasoningIntent::ServiceDefault)),
        ..Default::default()
    };
    let prepared_draft = model_settings::apply_patch(&AppType::Codex, &changed, &patch).unwrap();
    let expected_row_revision = patch.source_change_revision.clone().unwrap();
    // A concurrent legitimate row publication changes the cleanup scope the
    // user originally saw. The old confirmation cannot authorize this scope.
    changed
        .meta
        .as_mut()
        .unwrap()
        .application_model_overrides
        .as_mut()
        .unwrap()
        .declaration_models
        .insert("another-owned-model".into());
    fx.state.db.save_provider("codex", &changed).unwrap();
    let digest = crate::database::Database::provider_update_digest(&changed).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let native = read_current(&fx.path).unwrap();
    // Field-level reconstruction alone deliberately cannot distinguish this
    // expanded cleanup scope: both cleaned row images are equal. The original
    // locked owner must retain the first read's exact revision as well.
    model_settings::validate_change(&AppType::Codex, &changed, &prepared_draft).unwrap();
    let locked_preview = crate::services::provider::edit::preview_model_settings(
        &fx.state,
        &AppType::Codex,
        prepared_draft,
        "b",
        "d15d3030-2000-4000-8000-000000000005",
        &expected_row_revision,
    )
    .unwrap();
    assert_eq!(locked_preview["status"], "blocked");
    assert_eq!(locked_preview["code"], "sourceChanged");
    assert!(model_settings::preview(
        &fx.state,
        &AppType::Codex,
        "b",
        &patch,
        "d15d3030-2000-4000-8000-000000000003"
    )
    .is_err());
    assert_eq!(
        crate::database::Database::provider_update_digest(
            &fx.state
                .db
                .get_provider_by_id("b", "codex")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        digest
    );
    assert_eq!(
        std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
        journal
    );
    assert_eq!(read_current(&fx.path).unwrap(), native);
    let read = model_settings::read(&fx.state, &AppType::Codex, "b").unwrap();
    assert_eq!(
        read["sourceChangeEffects"]["declarationModels"],
        json!(["another-owned-model"])
    );
    patch.source_change_revision = Some(read["revision"].as_str().unwrap().into());
    let preview = model_settings::preview(
        &fx.state,
        &AppType::Codex,
        "b",
        &patch,
        "d15d3030-2000-4000-8000-000000000004",
    )
    .unwrap();
    assert_eq!(preview["status"], "ready");
    let request: state::SaveRequest = serde_json::from_value(preview["request"].clone()).unwrap();
    assert_eq!(
        model_settings::confirm(&fx.state, &AppType::Codex, "b", &patch, request.clone()).unwrap()
            ["status"],
        "completed"
    );
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let digest = crate::database::Database::provider_update_digest(
        &fx.state
            .db
            .get_provider_by_id("b", "codex")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        model_settings::confirm(&fx.state, &AppType::Codex, "b", &patch, request).unwrap()
            ["status"],
        "completed"
    );
    assert_eq!(
        std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
        journal
    );
    assert_eq!(
        crate::database::Database::provider_update_digest(
            &fx.state
                .db
                .get_provider_by_id("b", "codex")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        digest
    );
}
