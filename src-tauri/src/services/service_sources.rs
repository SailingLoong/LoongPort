//! Read-only shared sources and existing application references.
//!
//! This projection does not own credentials, billing, provisioning or routing.
//! The caller supplies only verified managed account/configuration facts.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Stable non-secret native source identity, independent of app projection IDs.
pub(crate) fn native_source_id(platform: &str, account: &str, model: &str) -> String {
    let bytes = serde_json::to_vec(&(platform, account, model)).expect("string tuple");
    format!("native:{platform}:{}", hex::encode(Sha256::digest(bytes)))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SourceAccountReference {
    Relay {
        id: i64,
    },
    Vendor {
        id: i64,
    },
    #[serde(rename_all = "camelCase")]
    LocalProvider {
        platform: String,
        id: String,
        origin_app: String,
        origin_provider_id: String,
    },
    NativeAccount {
        platform: String,
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
}

/// Legacy site-only association is display compatibility, not account evidence.
pub fn verified_relay_account(expected: Option<i64>, recorded: Option<i64>) -> bool {
    matches!((expected, recorded), (Some(expected), Some(recorded)) if expected == recorded)
}

/// Read identity columns only; account sessions and credentials are not needed.
pub fn relay_account_identities(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<HashMap<i64, Option<i64>>> {
    let mut stmt = conn.prepare("SELECT id, account_id FROM loongport_relay")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceUsageState {
    Current,
    Enabled,
    Inactive,
}

/// Reuse the existing presentation facts: additive Apps can enable multiple sources.
pub fn source_usage_state(
    additive: bool,
    is_current: bool,
    is_in_config: bool,
) -> SourceUsageState {
    if additive {
        if is_in_config {
            SourceUsageState::Enabled
        } else {
            SourceUsageState::Inactive
        }
    } else if is_current {
        SourceUsageState::Current
    } else {
        SourceUsageState::Inactive
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSourceBinding {
    pub app_id: String,
    pub provider_id: String,
    pub model: Option<String>,
    pub usage_state: SourceUsageState,
    pub can_select: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSourceTarget {
    pub app_id: String,
    pub model: String,
    pub can_select: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSource {
    /// Existing managed provider identity, shared by its application projections.
    pub source_id: String,
    pub account: SourceAccountReference,
    pub name: String,
    pub service_name: Option<String>,
    pub account_label: Option<String>,
    pub configuration_name: Option<String>,
    /// Configured applications, not a prediction of protocol compatibility.
    pub bindings: Vec<ServiceSourceBinding>,
    /// Verified transient adapter capability, never a fabricated persisted binding.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supported_targets: Vec<ServiceSourceTarget>,
}

/// Only display facts enter this projection; no credential-bearing Provider does.
pub struct ServiceSourceCandidate {
    pub source_id: String,
    pub account: SourceAccountReference,
    pub name: String,
    pub service_name: Option<String>,
    pub account_label: Option<String>,
    pub configuration_name: Option<String>,
    pub binding: ServiceSourceBinding,
}

/// Keep the existing source identity and all application-local selection facts.
/// Conflicting ownership or duplicate application references fail closed for that
/// source. Names never establish identity; absence never implies compatibility.
pub fn project_service_sources(candidates: Vec<ServiceSourceCandidate>) -> Vec<ServiceSource> {
    let mut indices = HashMap::<String, usize>::new();
    let mut sources = Vec::<ServiceSource>::new();
    let mut invalid = HashSet::new();
    for candidate in candidates {
        if candidate.source_id.is_empty()
            || candidate.binding.app_id.is_empty()
            || !match &candidate.account {
                SourceAccountReference::NativeAccount {
                    platform,
                    id,
                    region,
                } => {
                    ((platform == "minimax_code"
                        && matches!(region.as_deref(), Some("cn" | "global")))
                        || (matches!(
                            platform.as_str(),
                            "workbuddy" | "qoder_sdk" | "qoder_cn" | "zcode_start_plan"
                        ) && region.is_none()))
                        && candidate.binding.model.as_deref().is_some_and(|model| {
                            native_source_id(platform, id, model) == candidate.source_id
                        })
                }
                SourceAccountReference::LocalProvider {
                    platform,
                    id,
                    origin_app,
                    origin_provider_id,
                } => {
                    platform == "openrouter"
                        && id == &candidate.source_id
                        && id.starts_with("local:openrouter:")
                        && origin_app == &candidate.binding.app_id
                        && origin_provider_id == &candidate.binding.provider_id
                }
                _ => candidate.binding.provider_id == candidate.source_id,
            }
        {
            invalid.insert(candidate.source_id);
            continue;
        }
        if let Some(&index) = indices.get(&candidate.source_id) {
            let source = &mut sources[index];
            if source.account != candidate.account
                || source
                    .bindings
                    .iter()
                    .any(|binding| binding.app_id == candidate.binding.app_id)
            {
                invalid.insert(candidate.source_id);
                continue;
            }
            source.bindings.push(candidate.binding);
        } else {
            indices.insert(candidate.source_id.clone(), sources.len());
            sources.push(ServiceSource {
                source_id: candidate.source_id,
                account: candidate.account,
                name: candidate.name,
                service_name: candidate.service_name,
                account_label: candidate.account_label,
                configuration_name: candidate.configuration_name,
                bindings: vec![candidate.binding],
                supported_targets: Vec::new(),
            });
        }
    }
    sources.retain(|source| !invalid.contains(&source.source_id));
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(app: &str, id: &str, account: i64) -> ServiceSourceCandidate {
        ServiceSourceCandidate {
            source_id: id.into(),
            account: SourceAccountReference::Relay { id: account },
            name: "Standard".into(),
            service_name: Some("Example service".into()),
            account_label: Some("Work".into()),
            configuration_name: Some("Standard".into()),
            binding: ServiceSourceBinding {
                app_id: app.into(),
                provider_id: id.into(),
                model: Some(format!("{app}-model")),
                usage_state: source_usage_state(false, app == "codex", false),
                can_select: app != "gemini",
            },
        }
    }

    #[test]
    fn relay_ownership_requires_matching_recorded_account_even_for_a_single_row() {
        assert!(!verified_relay_account(Some(7), None));
        assert!(!verified_relay_account(None, None));
        assert!(!verified_relay_account(None, Some(7)));
        assert!(!verified_relay_account(Some(7), Some(8)));
        assert!(verified_relay_account(Some(7), Some(7)));
    }

    #[test]
    fn account_identity_read_does_not_need_or_return_secrets() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE loongport_relay(id INTEGER, account_id INTEGER); INSERT INTO loongport_relay VALUES(1,7),(2,NULL);").unwrap();
        let accounts = relay_account_identities(&conn).unwrap();
        assert_eq!(accounts.get(&1), Some(&Some(7)));
        assert_eq!(accounts.get(&2), Some(&None));
        assert!(!accounts.contains_key(&3));
    }

    #[test]
    fn additive_usage_comes_from_the_existing_presentation_owner() {
        assert_eq!(
            source_usage_state(false, true, false),
            SourceUsageState::Current
        );
        assert_eq!(
            source_usage_state(false, false, true),
            SourceUsageState::Inactive
        );
        assert_eq!(
            source_usage_state(true, false, true),
            SourceUsageState::Enabled
        );
        assert_eq!(
            source_usage_state(true, true, false),
            SourceUsageState::Inactive
        );
    }

    #[test]
    fn one_source_keeps_multiple_app_references_and_their_own_facts() {
        let sources = project_service_sources(vec![
            candidate("codex", "shared", 7),
            candidate("claude", "shared", 7),
            candidate("gemini", "shared", 7),
        ]);
        assert_eq!(sources.len(), 1);
        let source = &sources[0];
        assert_eq!(source.source_id, "shared");
        assert_eq!(source.bindings.len(), 3);
        assert_eq!(source.bindings[0].app_id, "codex");
        assert_eq!(source.bindings[0].usage_state, SourceUsageState::Current);
        assert_eq!(source.bindings[1].usage_state, SourceUsageState::Inactive);
        assert!(!source.bindings[2].can_select);
        assert_eq!(source.bindings[1].model.as_deref(), Some("claude-model"));
    }

    #[test]
    fn same_names_do_not_merge_distinct_plans_or_accounts() {
        let sources = project_service_sources(vec![
            candidate("codex", "plan-a", 7),
            candidate("codex", "plan-b", 7),
            candidate("codex", "other-account", 8),
        ]);
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].source_id, "plan-a");
        assert_eq!(sources[1].source_id, "plan-b");
        assert_eq!(sources[2].account, SourceAccountReference::Relay { id: 8 });
    }

    #[test]
    fn conflicting_ownership_is_excluded_instead_of_guessing() {
        let mut vendor = candidate("gemini", "mixed", 7);
        vendor.account = SourceAccountReference::Vendor { id: 7 };
        let sources = project_service_sources(vec![
            candidate("codex", "wrong-account", 7),
            candidate("claude", "wrong-account", 8),
            candidate("codex", "mixed", 7),
            vendor,
            candidate("codex", "valid", 7),
        ]);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_id, "valid");
    }

    #[test]
    fn duplicate_or_mismatched_app_references_are_not_publishable() {
        let mut wrong = candidate("codex", "wrong", 7);
        wrong.binding.provider_id = "another-source".into();
        let sources = project_service_sources(vec![
            candidate("codex", "duplicate", 7),
            candidate("codex", "duplicate", 7),
            wrong,
            candidate("claude", "valid", 7),
        ]);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_id, "valid");
    }

    #[test]
    fn dto_contains_references_only_and_no_credentials_or_billing() {
        let source = project_service_sources(vec![candidate("codex", "shared", 7)]).remove(0);
        let value = serde_json::to_value(source).unwrap();
        assert_eq!(value["sourceId"], "shared");
        assert_eq!(value["account"], serde_json::json!({"kind":"relay","id":7}));
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "account",
                "accountLabel",
                "bindings",
                "configurationName",
                "name",
                "serviceName",
                "sourceId"
            ]
        );
        let mut binding_keys: Vec<_> = value["bindings"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        binding_keys.sort_unstable();
        assert_eq!(
            binding_keys,
            ["appId", "canSelect", "model", "providerId", "usageState"]
        );
        let ts = include_str!("../../../src/lib/api/serviceSources.ts");
        for key in keys.into_iter().chain(binding_keys) {
            assert!(
                ts.contains(&format!("{key}:")),
                "TypeScript field missing: {key}"
            );
        }
    }
}
