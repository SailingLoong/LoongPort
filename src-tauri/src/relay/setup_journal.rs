//! Protocol-owned checkpoints. No credential material crosses this interface.
use super::setup_contract::ConfigurationState;
use crate::error::AppError;
#[derive(Clone, Debug)]
pub enum KeyEvent {
    BeforeCreate,
    Created(Option<String>),
    Reused(String),
    Observed(String),
    OutcomeUnknown,
}
#[derive(Clone, Copy)]
pub(crate) enum PreparationIssue {
    PreparationIncomplete,
    RemoteReadFailed,
    RemoteOutcomeUnknown,
    ModelCatalogUnavailable,
    ModelUnsupported,
    LocalSaveFailed,
}
impl PreparationIssue {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::PreparationIncomplete => "preparationIncomplete",
            Self::RemoteReadFailed => "remoteReadFailed",
            Self::RemoteOutcomeUnknown => "remoteOutcomeUnknown",
            Self::ModelCatalogUnavailable => "modelCatalogUnavailable",
            Self::ModelUnsupported => "modelUnsupported",
            Self::LocalSaveFailed => "localSaveFailed",
        }
    }
}
pub(crate) trait KeyPreparationJournal: Send + Sync {
    /// The caller must propagate failure: a failed durable preflight is not
    /// permission to POST, and failed acknowledgement storage stops later work.
    fn record(&self, group_id: &str, event: KeyEvent) -> Result<(), AppError>;
    fn record_issue(
        &self,
        group: &str,
        app: Option<&str>,
        issue: PreparationIssue,
    ) -> Result<(), AppError>;
    fn record_model_support(&self, group: &str, apps: &[String]) -> Result<(), AppError>;
    fn local_authority(&self) -> Result<PreparationAuthority, AppError> {
        Err(AppError::Config("relaySetup.executionNotBound".into()))
    }
    fn record_local(
        &self,
        _app: &str,
        _provider_id: &str,
        _fact: ConfigurationState,
    ) -> Result<(), AppError> {
        Err(AppError::Config("relaySetup.localRecorderRequired".into()))
    }
}

/// Ephemeral authority, never serialized into a backup or accepted from IPC.
/// The actual vault writer rechecks it under its pinned guard and transaction.
#[derive(Clone)]
pub(crate) struct PreparationAuthority {
    pub operation_id: String,
    pub storage_binding: String,
    pub account_identity: String,
    pub session_fingerprint: String,
    pub lifecycle_epoch: u64,
}

/// Test-only explicit confirmation backed by the same SQLite ledger as setup.
/// Each group gets a separate operation so inventory tests do not bypass the
/// production one-group-per-application contract. No HTTP or key owner is mocked.
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub(crate) mod test_support {
    use super::*;
    use crate::database::relay_setup_operations as ledger;
    use crate::relay::setup_contract::*;
    use std::{collections::BTreeMap, sync::Mutex};
    const BIND: &str = "protocol-fixture-storage";
    pub(crate) struct ConfirmedJournal {
        conn: Mutex<rusqlite::Connection>,
        operations: BTreeMap<String, String>,
    }
    impl ConfirmedJournal {
        pub(crate) fn new(groups: &[&str]) -> Self {
            Self::for_app(groups, "codex")
        }
        pub(crate) fn for_app(groups: &[&str], app: &str) -> Self {
            let mut conn = rusqlite::Connection::open_in_memory().unwrap();
            ledger::create_schema(&conn).unwrap();
            let mut operations = BTreeMap::new();
            for (index, group) in groups.iter().enumerate() {
                if operations.contains_key(*group) {
                    continue;
                }
                let operation = format!("126d1b27-5645-4ffb-8427-{:012x}", index + 1);
                let discovery = RelayDiscovery {
                    schema_version: 1,
                    relay_id: 7,
                    account: RelayAccountIdentity {
                        identity: "fixture/account".into(),
                        site_origin: "https://fixture.example".into(),
                        account_label: "Fixture".into(),
                    },
                    revision: "fixture-revision".into(),
                    choices: vec![RelayChoice {
                        app: app.into(),
                        group_id: (*group).into(),
                        group_name: (*group).into(),
                        provider_id: format!("fixture-{index}"),
                        local_configuration: LocalConfiguration::Absent,
                        compatibility: ChoiceCompatibility::Unknown,
                        key_status: UnknownKeyStatus::Unknown,
                    }],
                };
                let receipt = initial_receipt(
                    &operation,
                    &[RelaySelection {
                        app: app.into(),
                        group_id: (*group).into(),
                    }],
                    &discovery,
                    1,
                )
                .unwrap();
                ledger::begin_on(&mut conn, BIND, &receipt).unwrap();
                operations.insert((*group).into(), operation);
            }
            Self {
                conn: Mutex::new(conn),
                operations,
            }
        }
        fn operation(&self, group: &str) -> Result<&str, AppError> {
            self.operations
                .get(group)
                .map(String::as_str)
                .ok_or_else(|| AppError::Config("fixture group was not confirmed".into()))
        }
    }
    impl KeyPreparationJournal for ConfirmedJournal {
        fn record(&self, group: &str, event: KeyEvent) -> Result<(), AppError> {
            ledger::key_event_on(
                &mut self.conn.lock().unwrap(),
                BIND,
                self.operation(group)?,
                group,
                event,
                2,
            )
            .map(|_| ())
        }
        fn record_issue(
            &self,
            group: &str,
            app: Option<&str>,
            issue: PreparationIssue,
        ) -> Result<(), AppError> {
            ledger::preparation_issue_on(
                &mut self.conn.lock().unwrap(),
                BIND,
                self.operation(group)?,
                group,
                app,
                issue,
                2,
            )
            .map(|_| ())
        }
        fn record_model_support(&self, group: &str, apps: &[String]) -> Result<(), AppError> {
            ledger::model_support_on(
                &mut self.conn.lock().unwrap(),
                BIND,
                self.operation(group)?,
                group,
                apps,
                2,
            )
            .map(|_| ())
        }
    }
}
