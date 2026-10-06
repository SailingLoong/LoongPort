//! LoongPort ownership-boundary contracts. These predicates do not authorize
//! sharing, publishing, migration, or cleanup; those remain separate operations.

use super::{floor, residue};
use serde_json::json;

#[test]
fn claude_resource_and_policy_fields_are_not_provider_owned() {
    for key in [
        "mcpServers",
        "hooks",
        "permissions",
        "enabledPlugins",
        "statusLine",
    ] {
        assert!(
            !floor::claude_floor_top(key),
            "{key} belongs to its existing owner"
        );
    }
}

#[test]
fn claude_feature_switches_do_not_become_auth_protocol_selectors() {
    for key in [
        "CLAUDE_CODE_USE_POWERSHELL_TOOL",
        "CLAUDE_CODE_USE_NATIVE_FILE_SEARCH",
        "CLAUDE_CODE_USE_COWORK_PLUGINS",
        "CLAUDE_CODE_USE_CCR_V2",
    ] {
        assert!(
            !floor::claude_floor_env(key),
            "{key} is a user feature switch"
        );
    }
    assert!(floor::claude_floor_env("CLAUDE_CODE_USE_BEDROCK"));
    assert!(floor::claude_floor_env("CLAUDE_CODE_OAUTH_REFRESH_TOKEN"));
}

#[test]
fn codex_connection_auth_and_model_fields_stay_out_of_ordinary_shared_fields() {
    for key in [
        "model_provider",
        "openai_base_url",
        "model",
        "review_model",
        "model_catalog_json",
        "experimental_bearer_token",
        "base_url",
        "wire_api",
    ] {
        assert!(
            floor::CODEX_FLOOR_TOP.contains(&key),
            "{key} is provider-scoped"
        );
    }
    // External catalog ownership still requires the separate reviewed R5 gate.
    // Being outside this floor is not, by itself, permission to migrate a key.
    for key in ["mcp_servers", "approval_policy", "sandbox_mode", "projects"] {
        assert!(!floor::CODEX_FLOOR_TOP.contains(&key));
    }
}

#[test]
fn codex_nested_ownership_targets_only_the_four_model_keys() {
    assert_eq!(
        floor::CODEX_FLOOR_NESTED,
        &[
            &["agents", "default_subagent_model"][..],
            &["agents", "default_subagent_reasoning_effort"][..],
            &["memories", "extract_model"][..],
            &["memories", "consolidation_model"][..],
        ]
    );
}

#[test]
fn gemini_cli_paths_and_sandbox_settings_are_not_provider_fields() {
    for key in [
        "GEMINI_CLI_HOME",
        "GEMINI_SANDBOX",
        "GEMINI_SYSTEM_MD",
        "DEBUG",
    ] {
        assert!(!floor::gemini_floor_env(key), "{key} is user-owned");
    }
    for key in [
        "GOOGLE_API_KEY",
        "GEMINI_API_KEY",
        "GEMINI_MODEL",
        "CODE_ASSIST_ENDPOINT",
    ] {
        assert!(floor::gemini_floor_env(key), "{key} is provider-scoped");
    }
}

#[test]
fn desktop_user_policy_seeds_are_separate_from_connection_fields() {
    for key in floor::DESKTOP_PROFILE_SEED {
        assert!(!floor::desktop_profile_floor(key));
    }
    assert!(floor::desktop_profile_floor("inferenceGatewayApiKey"));
    assert!(!floor::desktop_profile_floor("mcpServers"));
}

#[test]
fn residue_conversion_preserves_exact_input_and_only_adds_valid_numeric_spelling() {
    assert_eq!(
        residue::residue_values(&["262144", "fixture-value", "18446744073709551616"]),
        vec![
            json!("262144"),
            json!(262144),
            json!("fixture-value"),
            json!("18446744073709551616")
        ],
    );
}
