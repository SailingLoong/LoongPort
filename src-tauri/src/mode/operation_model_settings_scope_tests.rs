use super::*;
use serde_json::json;

fn target(scope: state::SaveScope) -> PendingTarget {
    let provider = crate::provider::Provider::with_id(
        "tier".into(),
        "Tier".into(),
        json!({"env": {"ANTHROPIC_MODEL": "before"}}),
        None,
    );
    PendingTarget {
        save_request: Some(state::SaveRequest {
            id: "33333333-3333-4333-8333-333333333333".into(),
            scope,
            provider_id: Some(provider.id.clone()),
            draft_digest: "a".repeat(64),
            revision: "b".repeat(64),
        }),
        saved_row: Some(state::SavedRow {
            before: Database::provider_update_digest(&provider).unwrap(),
            provider: Database::provider_update_value(&provider).unwrap(),
            clear_model_preference: false,
        }),
        ..Default::default()
    }
}

#[test]
fn model_settings_scope_uses_the_existing_apply_operation() {
    let target = target(state::SaveScope::ProviderModelSettings);
    for app in ["claude", "codex", "gemini", "grokbuild"] {
        validate_operation(app, state::op::APPLY, &target).unwrap();
    }
    assert!(validate_operation("claude", state::op::SWITCH, &target).is_err());
    assert!(validate_operation("opencode", state::op::APPLY, &target).is_err());
}

#[test]
fn model_settings_scope_requires_its_original_exact_row() {
    let mut target = target(state::SaveScope::ProviderModelSettings);
    target.save_request.as_mut().unwrap().provider_id = Some("other".into());
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
    target.saved_row = None;
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
}

#[test]
fn model_settings_scope_cannot_smuggle_a_pointer_or_model_preference_action() {
    let mut target = target(state::SaveScope::ProviderModelSettings);
    target.pointer = Some("tier".into());
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
    target.model_preference = Some(state::ModelPreferenceAction::Clear {});
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
}

#[test]
fn model_settings_scope_cannot_change_proxy_route_or_mode() {
    let mut target = target(state::SaveScope::ProviderModelSettings);
    target.state = Some(state::ModeState {
        mode: Some(state::Mode::Proxy),
        attached: true,
        proxy_route: Some("other".into()),
        ..Default::default()
    });
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
    target.state.as_mut().unwrap().mode = Some(state::Mode::Direct);
    target.state.as_mut().unwrap().proxy_route = None;
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
}

#[test]
fn model_settings_scope_accepts_the_original_proxy_contract_projection() {
    let mut target = target(state::SaveScope::ProviderModelSettings);
    target.state = Some(state::ModeState {
        mode: Some(state::Mode::Proxy),
        attached: true,
        proxy_route: Some("tier".into()),
        contract: Some(state::Contract {
            version: super::super::contract::CONTRACT_VERSION,
            key: "synthetic".into(),
            exclusive: Default::default(),
            extra: Default::default(),
        }),
        ..Default::default()
    });
    validate_operation("claude", state::op::APPLY, &target).unwrap();
    target.state.as_mut().unwrap().attached = false;
    assert!(validate_operation("claude", state::op::APPLY, &target).is_err());
}

#[test]
fn legacy_full_provider_scope_keeps_its_existing_target_effects() {
    let mut target = target(state::SaveScope::Provider);
    target.pointer = Some("tier".into());
    validate_operation("claude", state::op::APPLY, &target).unwrap();
}

#[test]
fn model_settings_receipts_reuse_identity_without_cross_scope_aliasing() {
    let target = target(state::SaveScope::ProviderModelSettings);
    let request = target.save_request.unwrap();
    let entry = state::AppLiveState {
        last_save: Some(state::SaveReceipt {
            request: request.clone(),
            outcome: state::SaveOutcome::Completed,
            verified_at_ms: 1,
        }),
        ..Default::default()
    };
    assert_eq!(
        saved_request_status("claude", &entry, &request).unwrap(),
        "completed"
    );
    let mut different = request.clone();
    different.scope = state::SaveScope::Provider;
    assert_eq!(
        saved_request_status("claude", &entry, &different).unwrap(),
        "conflict"
    );
    assert!(
        saved_request_view("claude", &entry, state::SaveScope::Provider, Some("tier"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        saved_request_view(
            "claude",
            &entry,
            state::SaveScope::ProviderModelSettings,
            Some("tier")
        )
        .unwrap()
        .unwrap()
        .request,
        request
    );
}

#[test]
fn model_settings_pending_uses_original_publication_status_and_identity() {
    let target = target(state::SaveScope::ProviderModelSettings);
    let request = target.save_request.clone().unwrap();
    let mut entry = state::AppLiveState {
        pending: Some(state::Pending {
            op: state::op::APPLY.into(),
            files: vec![],
            target,
            published: false,
            extra: Default::default(),
        }),
        ..Default::default()
    };
    assert_eq!(
        saved_request_status("claude", &entry, &request).unwrap(),
        "pending"
    );
    entry.pending.as_mut().unwrap().published = true;
    assert_eq!(
        saved_request_status("claude", &entry, &request).unwrap(),
        "verificationRequired"
    );
    let mut different = request;
    different.scope = state::SaveScope::Provider;
    assert_eq!(
        saved_request_status("claude", &entry, &different).unwrap(),
        "conflict"
    );
}

#[test]
fn model_settings_recovery_outcomes_remain_in_original_receipt() {
    for (outcome, status) in [
        (state::SaveOutcome::Discarded, "discarded"),
        (state::SaveOutcome::Abandoned, "abandoned"),
    ] {
        let request = target(state::SaveScope::ProviderModelSettings)
            .save_request
            .unwrap();
        let entry = state::AppLiveState {
            last_save: Some(state::SaveReceipt {
                request: request.clone(),
                outcome,
                verified_at_ms: 1,
            }),
            ..Default::default()
        };
        assert_eq!(
            saved_request_status("claude", &entry, &request).unwrap(),
            status
        );
        let mut different = request;
        different.id = "44444444-4444-4444-8444-444444444444".into();
        assert_eq!(
            saved_request_status("claude", &entry, &different).unwrap(),
            "unknown"
        );
    }
}

#[test]
fn model_settings_scope_keeps_the_original_row_owned_preference_clear() {
    let mut target = target(state::SaveScope::ProviderModelSettings);
    target.saved_row.as_mut().unwrap().clear_model_preference = true;
    validate_operation("claude", state::op::APPLY, &target).unwrap();
    assert_eq!(
        target_model_preference(&target),
        Some(state::ModelPreferenceAction::Clear {})
    );
    assert!(target.model_preference.is_none());
    assert!(target.pointer.is_none());
}

#[test]
fn model_settings_scope_without_a_row_clear_preserves_model_preference() {
    let target = target(state::SaveScope::ProviderModelSettings);
    validate_operation("claude", state::op::APPLY, &target).unwrap();
    assert!(!target.saved_row.as_ref().unwrap().clear_model_preference);
    assert_eq!(target_model_preference(&target), None);
}
