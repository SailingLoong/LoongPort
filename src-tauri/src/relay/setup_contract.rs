//! Non-secret, durable selected-relay setup contract. Resource existence is not
//! application connectivity; unknown outcomes are never permission to repeat.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelaySelection {
    pub app: String,
    pub group_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayAccountIdentity {
    pub identity: String,
    pub site_origin: String,
    pub account_label: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalConfiguration {
    Absent,
    Present,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChoiceCompatibility {
    Unknown,
    Configured,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownKeyStatus {
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayChoice {
    pub app: String,
    pub group_id: String,
    pub group_name: String,
    pub provider_id: String,
    pub local_configuration: LocalConfiguration,
    pub compatibility: ChoiceCompatibility,
    pub key_status: UnknownKeyStatus,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayDiscovery {
    pub schema_version: u32,
    pub relay_id: i64,
    pub account: RelayAccountIdentity,
    pub revision: String,
    pub choices: Vec<RelayChoice>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RelayPrepareRequest {
    Start {
        operation_id: String,
        relay_id: i64,
        discovery_revision: String,
        selections: Vec<RelaySelection>,
    },
    Resume {
        operation_id: String,
        expected_receipt_revision: u64,
        apps: Vec<String>,
    },
}
/// A typed owner-issued rejection is distinct from an ambiguous IPC failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayPreparationError {
    pub schema_version: u32,
    pub outcome: PreparationErrorOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub reason_code: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreparationErrorOutcome {
    RejectedBeforeOperation,
    Unknown,
}
impl RelayPreparationError {
    pub fn rejected(operation_id: &str, reason_code: &str) -> Self {
        Self {
            schema_version: 1,
            outcome: PreparationErrorOutcome::RejectedBeforeOperation,
            operation_id: Some(operation_id.into()),
            reason_code: reason_code.into(),
        }
    }
    pub fn unknown() -> Self {
        Self {
            schema_version: 1,
            outcome: PreparationErrorOutcome::Unknown,
            operation_id: None,
            reason_code: "preparationOutcomeUncertain".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyState {
    NotStarted,
    Requesting,
    Ready,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyOrigin {
    Created,
    Reused,
    Observed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayKeyFact {
    pub group_id: String,
    pub group_name: String,
    pub state: KeyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<KeyOrigin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationState {
    NotStarted,
    Existing,
    Prepared,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Compatibility {
    Unknown,
    Supported,
    Unsupported,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    NotRequested,
    ConfirmationRequired,
    Applying,
    Applied,
    FailedBeforeWrite,
    Unknown,
    Changed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayConnectionFact {
    /// Only constructed from the original routing owner's non-secret DTOs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<RelayConnectionIntent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quit_chatgpt: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_current: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_acknowledged: Option<bool>,
    pub state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayConnectionIntent {
    pub prepared_request: serde_json::Value,
    pub change: serde_json::Value,
    pub before_revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayTargetFact {
    pub app: String,
    pub group_id: String,
    pub provider_id: String,
    pub configuration: ConfigurationState,
    pub compatibility: Compatibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    pub connection: RelayConnectionFact,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupStatus {
    Running,
    Settled,
    NeedsReconcile,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelaySetupReceipt {
    pub schema_version: u32,
    pub operation_id: String,
    pub receipt_revision: u64,
    pub relay_id: i64,
    pub account_identity: String,
    pub selection_digest: String,
    pub selections: Vec<RelaySelection>,
    pub status: SetupStatus,
    pub groups: Vec<RelayKeyFact>,
    pub targets: Vec<RelayTargetFact>,
    pub created_key_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Stable non-secret identity shared by the setup and original provider owners.
pub(crate) fn account_identity_digest(
    backend: &str,
    site_origin: &str,
    account_id: i64,
) -> Result<String, &'static str> {
    let bytes = serde_json::to_vec(&(backend, site_origin.trim_end_matches('/'), account_id))
        .map_err(|_| "invalidIdentity")?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
/// Ephemeral session comparison only; never persisted, returned, or logged.
pub(crate) fn session_fingerprint(identity: &str, auth_token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(identity.as_bytes());
    digest.update([0]);
    digest.update(auth_token.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Repeated starts are scope checks only: never rediscover or execute them.
pub fn validate_repeat_start(
    receipt: &RelaySetupReceipt,
    relay_id: i64,
    revision: &str,
    selections: &[RelaySelection],
) -> Result<(), &'static str> {
    let discovery = RelayDiscovery {
        schema_version: 1,
        relay_id: receipt.relay_id,
        account: RelayAccountIdentity {
            identity: receipt.account_identity.clone(),
            site_origin: String::new(),
            account_label: String::new(),
        },
        revision: revision.into(),
        choices: receipt
            .targets
            .iter()
            .map(|t| RelayChoice {
                app: t.app.clone(),
                group_id: t.group_id.clone(),
                group_name: String::new(),
                provider_id: t.provider_id.clone(),
                local_configuration: LocalConfiguration::Absent,
                compatibility: ChoiceCompatibility::Unknown,
                key_status: UnknownKeyStatus::Unknown,
            })
            .collect(),
    };
    let digest = validate_start(
        &receipt.operation_id,
        relay_id,
        revision,
        selections,
        &discovery,
    )?;
    if digest != receipt.selection_digest {
        return Err("operationScopeChanged");
    }
    Ok(())
}

pub fn validate_resume(
    receipt: &RelaySetupReceipt,
    revision: u64,
    apps: &[String],
) -> Result<Vec<RelaySelection>, &'static str> {
    if revision != receipt.receipt_revision {
        return Err("receiptChanged");
    }
    if apps.is_empty() {
        return Err("emptySelection");
    }
    let mut seen = BTreeSet::new();
    let mut selections = Vec::new();
    for app in apps {
        if !seen.insert(app) {
            return Err("duplicateApplication");
        }
        let target = receipt
            .targets
            .iter()
            .find(|t| t.app == *app)
            .ok_or("selectionChanged")?;
        if target.configuration != ConfigurationState::Failed {
            return Err("reconcileRequired");
        }
        let group = receipt
            .groups
            .iter()
            .find(|g| g.group_id == target.group_id)
            .ok_or("selectionChanged")?;
        if matches!(group.state, KeyState::Requesting | KeyState::Unknown) {
            return Err("reconcileRequired");
        }
        selections.push(RelaySelection {
            app: app.clone(),
            group_id: target.group_id.clone(),
        });
    }
    Ok(selections)
}

/// Validate all confirmed choices before creating even the local operation row.
/// The returned digest binds the stable account, discovery and full selection.
pub fn validate_start(
    operation_id: &str,
    relay_id: i64,
    discovery_revision: &str,
    selections: &[RelaySelection],
    discovery: &RelayDiscovery,
) -> Result<String, &'static str> {
    let id = uuid::Uuid::parse_str(operation_id).map_err(|_| "invalidOperationId")?;
    if id.is_nil() || id.to_string() != operation_id {
        return Err("invalidOperationId");
    }
    if relay_id != discovery.relay_id || discovery.account.identity.is_empty() {
        return Err("identityChanged");
    }
    if discovery.schema_version != 1
        || discovery_revision.is_empty()
        || discovery_revision != discovery.revision
    {
        return Err("selectionChanged");
    }
    if selections.is_empty() {
        return Err("emptySelection");
    }
    let mut apps = BTreeSet::new();
    for selection in selections {
        if !apps.insert(&selection.app) {
            return Err("duplicateApplication");
        }
        if !discovery
            .choices
            .iter()
            .any(|choice| choice.app == selection.app && choice.group_id == selection.group_id)
        {
            return Err("selectionChanged");
        }
    }
    let mut ordered = selections.to_vec();
    ordered.sort_by(|a, b| (&a.app, &a.group_id).cmp(&(&b.app, &b.group_id)));
    let bytes = serde_json::to_vec(&(
        1u32,
        relay_id,
        &discovery.account.identity,
        discovery_revision,
        ordered,
    ))
    .map_err(|_| "invalidSelection")?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn initial_receipt(
    operation_id: &str,
    selections: &[RelaySelection],
    discovery: &RelayDiscovery,
    now: i64,
) -> Result<RelaySetupReceipt, &'static str> {
    let digest = validate_start(
        operation_id,
        discovery.relay_id,
        &discovery.revision,
        selections,
        discovery,
    )?;
    let mut groups = Vec::new();
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for selected in selections {
        let choice = discovery
            .choices
            .iter()
            .find(|c| c.app == selected.app && c.group_id == selected.group_id)
            .ok_or("selectionChanged")?;
        if seen.insert(choice.group_id.clone()) {
            groups.push(RelayKeyFact {
                group_id: choice.group_id.clone(),
                group_name: choice.group_name.clone(),
                state: KeyState::NotStarted,
                origin: None,
                remote_key_id: None,
                reason_code: None,
            });
        }
        targets.push(RelayTargetFact {
            app: choice.app.clone(),
            group_id: choice.group_id.clone(),
            provider_id: choice.provider_id.clone(),
            configuration: ConfigurationState::NotStarted,
            compatibility: Compatibility::Unknown,
            reason_code: None,
            connection: RelayConnectionFact {
                intent: None,
                quit_chatgpt: None,
                observed_current: None,
                write_acknowledged: None,
                state: ConnectionState::NotRequested,
                request_id: None,
                observed_revision: None,
                applied_revision: None,
                reason_code: None,
            },
        });
    }
    Ok(RelaySetupReceipt {
        schema_version: 1,
        operation_id: operation_id.into(),
        receipt_revision: 1,
        relay_id: discovery.relay_id,
        account_identity: discovery.account.identity.clone(),
        selection_digest: digest,
        selections: selections.to_vec(),
        status: SetupStatus::Running,
        groups,
        targets,
        created_key_count: 0,
        created_at: now,
        updated_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "f8d02319-67c1-426b-a931-c836de11320e";
    fn discovery() -> RelayDiscovery {
        RelayDiscovery {
            schema_version: 1,
            relay_id: 7,
            account: RelayAccountIdentity {
                identity: "site/account-7".into(),
                site_origin: "https://fixture.example".into(),
                account_label: "Fixture".into(),
            },
            revision: "revision-1".into(),
            choices: [("codex", "a"), ("claude", "a"), ("codex", "b")]
                .into_iter()
                .map(|(app, group)| RelayChoice {
                    app: app.into(),
                    group_id: group.into(),
                    group_name: "same display name".into(),
                    provider_id: format!("{app}-{group}"),
                    local_configuration: LocalConfiguration::Absent,
                    compatibility: ChoiceCompatibility::Unknown,
                    key_status: UnknownKeyStatus::Unknown,
                })
                .collect(),
        }
    }
    fn select(app: &str, group: &str) -> RelaySelection {
        RelaySelection {
            app: app.into(),
            group_id: group.into(),
        }
    }
    #[test]
    fn native_setup_contract_matches_the_frontend_wire_fixture() {
        let selected = vec![select("codex", "a")];
        let discovery = discovery();
        let expected = serde_json::json!({
            "discovery":discovery,
            "request":RelayPrepareRequest::Start {operation_id:ID.into(),relay_id:7,discovery_revision:discovery.revision.clone(),selections:selected.clone()},
            "receipt":initial_receipt(ID,&selected,&discovery,5).unwrap(),
        });
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/relay-setup-wire.json"
        ))
        .unwrap();
        assert_eq!(expected, fixture);
    }
    #[test]
    fn repeated_start_binds_original_discovery_and_resume_never_replays_unknown() {
        let d = discovery();
        let selections = [select("codex", "a")];
        let mut receipt = initial_receipt(ID, &selections, &d, 5).unwrap();
        validate_repeat_start(&receipt, 7, &d.revision, &selections).unwrap();
        assert!(validate_repeat_start(&receipt, 7, "changed-revision", &selections).is_err());
        assert!(validate_repeat_start(&receipt, 7, &d.revision, &[select("codex", "b")]).is_err());
        receipt.targets[0].configuration = ConfigurationState::Failed;
        receipt.groups[0].state = KeyState::Ready;
        assert_eq!(
            validate_resume(&receipt, 1, &["codex".into()]).unwrap(),
            selections
        );
        receipt.groups[0].state = KeyState::Unknown;
        assert_eq!(
            validate_resume(&receipt, 1, &["codex".into()]),
            Err("reconcileRequired")
        );
        assert_eq!(
            validate_resume(&receipt, 2, &["codex".into()]),
            Err("receiptChanged")
        );
    }
    #[test]
    fn rejects_empty_and_duplicate_app_confirmation() {
        let d = discovery();
        assert_eq!(
            validate_start(ID, 7, &d.revision, &[], &d),
            Err("emptySelection")
        );
        assert_eq!(
            validate_start(
                ID,
                7,
                &d.revision,
                &[select("codex", "a"), select("codex", "b")],
                &d
            ),
            Err("duplicateApplication")
        );
    }
    #[test]
    fn rejects_bad_operation_identity_stale_revision_and_forged_choice() {
        let d = discovery();
        let s = [select("codex", "a")];
        assert_eq!(
            validate_start("not-an-id", 7, &d.revision, &s, &d),
            Err("invalidOperationId")
        );
        assert_eq!(
            validate_start(ID, 8, &d.revision, &s, &d),
            Err("identityChanged")
        );
        assert_eq!(
            validate_start(ID, 7, "old", &s, &d),
            Err("selectionChanged")
        );
        assert_eq!(
            validate_start(ID, 7, &d.revision, &[select("forged", "a")], &d),
            Err("selectionChanged")
        );
    }
    #[test]
    fn digest_is_order_independent_but_binds_account_and_group_ids() {
        let d = discovery();
        let a = [select("codex", "a"), select("claude", "a")];
        let b = [a[1].clone(), a[0].clone()];
        assert_eq!(
            validate_start(ID, 7, &d.revision, &a, &d),
            validate_start(ID, 7, &d.revision, &b, &d)
        );
        let mut other = d.clone();
        other.account.identity = "site/account-8".into();
        assert_ne!(
            validate_start(ID, 7, &d.revision, &a, &d),
            validate_start(ID, 7, &other.revision, &a, &other)
        );
        assert_ne!(
            validate_start(ID, 7, &d.revision, &[select("codex", "a")], &d),
            validate_start(ID, 7, &d.revision, &[select("codex", "b")], &d)
        );
    }
    #[test]
    fn shared_group_has_one_key_fact_and_two_distinct_local_targets() {
        let receipt = initial_receipt(
            ID,
            &[select("codex", "a"), select("claude", "a")],
            &discovery(),
            5,
        )
        .unwrap();
        assert_eq!(receipt.groups.len(), 1);
        assert_eq!(receipt.targets.len(), 2);
        assert_eq!(receipt.created_key_count, 0);
        assert_eq!(receipt.groups[0].state, KeyState::NotStarted);
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value["targets"][0]["connection"]["state"], "notRequested");
        assert_eq!(
            serde_json::from_value::<RelaySetupReceipt>(value).unwrap(),
            receipt
        );
    }
    #[test]
    fn input_rejects_unrecognized_fields_including_credential_payloads() {
        let value = serde_json::json!({"kind":"start","operationId":ID,"relayId":7,"discoveryRevision":"r","selections":[{"app":"codex","groupId":"a","apiKey":"not-accepted"}]});
        assert!(serde_json::from_value::<RelayPrepareRequest>(value).is_err());
    }
}

#[cfg(test)]
mod preparation_error_tests {
    use super::*;
    #[test]
    fn owner_rejection_is_typed_bound_to_the_original_operation_and_nonsecret() {
        let rejected = RelayPreparationError::rejected(
            "f8d02319-67c1-426b-a931-c836de11320e",
            "selectionChanged",
        );
        assert_eq!(
            serde_json::to_value(rejected).unwrap(),
            serde_json::json!({"schemaVersion":1,"outcome":"rejectedBeforeOperation","operationId":"f8d02319-67c1-426b-a931-c836de11320e","reasonCode":"selectionChanged"})
        );
        assert_eq!(
            RelayPreparationError::unknown().outcome,
            PreparationErrorOutcome::Unknown
        );
        assert!(RelayPreparationError::unknown().operation_id.is_none());
    }
}

#[cfg(test)]
mod connection_reference_tests {
    use super::*;
    #[test]
    fn connection_retains_original_routing_reference_and_explicit_quit_choice() {
        let value = serde_json::json!({"state":"applying","requestId":"request-1","intent":{"preparedRequest":{"protocol":"native","request":{"id":"request-1","scope":"applicationRouting","draftDigest":"draft","revision":"before"}},"change":{"selection":{"providerId":"provider-a","model":null},"order":null,"catalogDecision":null,"catalogPreviewOnly":false},"beforeRevision":"overview-before"},"quitChatgpt":false});
        let fact: RelayConnectionFact = serde_json::from_value(value.clone())
            .expect("the durable original reference must survive restart");
        assert_eq!(serde_json::to_value(fact).unwrap(), value);
    }
}

/// Resource acknowledgement and current application scope are independent facts.
pub(crate) fn connection_observation_state(
    previous: &RelayConnectionFact,
    completed: bool,
    current: bool,
    revision: &str,
) -> ConnectionState {
    let applied_revision = previous.applied_revision.as_deref().or_else(|| {
        (previous.state == ConnectionState::Applied)
            .then_some(previous.observed_revision.as_deref())
            .flatten()
    });
    if !completed {
        ConnectionState::Unknown
    } else if !current || applied_revision.is_some_and(|original| original != revision) {
        ConnectionState::Changed
    } else {
        ConnectionState::Applied
    }
}
#[cfg(test)]
mod connection_observation_tests {
    use super::*;
    #[test]
    fn successful_same_provider_but_changed_revision_is_not_reported_still_applied() {
        let fact:RelayConnectionFact=serde_json::from_value(serde_json::json!({"state":"applied","observedRevision":"original","writeAcknowledged":true})).unwrap();
        assert_eq!(
            connection_observation_state(&fact, true, true, "external-edit"),
            ConnectionState::Changed
        );
        assert_eq!(
            connection_observation_state(&fact, true, true, "original"),
            ConnectionState::Applied
        );
    }
    #[test]
    fn current_target_without_original_acknowledgement_never_fabricates_success() {
        let fact: RelayConnectionFact =
            serde_json::from_value(serde_json::json!({"state":"unknown","observedCurrent":true}))
                .unwrap();
        assert_eq!(
            connection_observation_state(&fact, false, true, "now"),
            ConnectionState::Unknown
        );
    }
}
