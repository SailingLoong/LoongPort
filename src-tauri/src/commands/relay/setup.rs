//! Selected setup orchestration. Protocol, provider persistence and application
//! routing keep their original owners; this module binds their durable facts.
use super::*;
#[path = "setup_connection.rs"]
mod connection;
use crate::database::relay_setup_operations as ledger;
use crate::relay::setup_contract::*;
use crate::relay::setup_journal::{
    KeyEvent, KeyPreparationJournal, PreparationAuthority, PreparationIssue,
};
pub use connection::*;
use sha2::{Digest, Sha256};

fn setup_error(code: &str) -> AppError {
    AppError::Config(format!("relaySetup.{code}"))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn digest(value: &impl serde::Serialize) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(value).map_err(|_| setup_error("invalidIdentity"))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn account_identity(account: &creds::RelayAccount) -> Result<String, AppError> {
    let id = account
        .account_id
        .ok_or_else(|| setup_error("authenticationRequired"))?;
    account_identity_digest(account.backend_kind.as_str(), &account.site_origin, id)
        .map_err(setup_error)
}
/// Discovery never renews a session, probes-and-clears credentials, or backfills
/// identity. Those mutations belong to the explicit existing login owner.
fn read_account(state: &AppState, relay_id: i64) -> Result<creds::RelayAccount, AppError> {
    let account = with_conn(state, |conn, vault| creds::get(conn, vault, relay_id))?
        .ok_or_else(|| setup_error("accountUnavailable"))?;
    if account.auth_token.is_empty()
        || !account.token_looks_valid(chrono::Utc::now().timestamp())
        || account.account_id.is_none()
    {
        return Err(setup_error("authenticationRequired"));
    }
    Ok(account)
}
pub(super) async fn discover(
    state: &AppState,
    account: &creds::RelayAccount,
    fallback: Option<sub2api::BrowserApiFallback>,
) -> Result<RelayDiscovery, AppError> {
    let mut choices = provision::discover_backend_choices(account, fallback)
        .await?
        .into_iter()
        .map(|choice| {
            let existing = state
                .db
                .get_provider_by_id(&choice.provider_id, &choice.app)?;
            let present = existing.as_ref().is_some_and(|provider| {
                provision::validate_selected_provider_identity(account, &choice.group_id, provider)
                    .is_ok()
            });
            Ok(RelayChoice {
                app: choice.app,
                group_id: choice.group_id,
                group_name: choice.name,
                provider_id: choice.provider_id,
                local_configuration: if present {
                    LocalConfiguration::Present
                } else {
                    LocalConfiguration::Absent
                },
                compatibility: if present {
                    ChoiceCompatibility::Configured
                } else {
                    ChoiceCompatibility::Unknown
                },
                key_status: UnknownKeyStatus::Unknown,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    choices.sort_by(|a, b| (&a.app, &a.group_id).cmp(&(&b.app, &b.group_id)));
    let identity = account_identity(account)?;
    let revision = digest(&(1u32, account.id, &identity, &choices))?;
    Ok(RelayDiscovery {
        schema_version: 1,
        relay_id: account.id,
        account: RelayAccountIdentity {
            identity,
            site_origin: account.site_origin.clone(),
            account_label: account.account_label.clone(),
        },
        revision,
        choices,
    })
}

#[derive(Clone)]
struct Journal {
    db: Arc<crate::database::Database>,
    operation: String,
    binding: String,
    authority: Option<PreparationAuthority>,
}
impl Journal {
    fn new(state: &AppState, operation: &str) -> Result<Self, AppError> {
        let binding = state.db.secrets.read()?.identity().store_id().to_owned();
        Ok(Self {
            db: state.db.clone(),
            operation: operation.into(),
            binding,
            authority: None,
        })
    }
    fn for_account(mut self, account: &creds::RelayAccount) -> Result<Self, AppError> {
        let identity = account_identity(account)?;
        let fingerprint = session_fingerprint(&identity, &account.auth_token);
        let epoch = self.with_storage(|conn, storage| {
            let current = creds::get(conn, storage, account.id)?
                .ok_or_else(|| setup_error("accountChanged"))?;
            let current_identity = account_identity(&current)?;
            if current_identity != identity
                || session_fingerprint(&current_identity, &current.auth_token) != fingerprint
            {
                return Err(setup_error("accountChanged"));
            }
            Ok(self.db.secrets.lifecycle_epoch())
        })?;
        self.authority = Some(PreparationAuthority {
            operation_id: self.operation.clone(),
            storage_binding: self.binding.clone(),
            account_identity: identity,
            session_fingerprint: fingerprint,
            lifecycle_epoch: epoch,
        });
        Ok(self)
    }
    fn with<T>(
        &self,
        action: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.with_storage(|conn, _| action(conn))
    }
    fn with_storage<T>(
        &self,
        action: impl FnOnce(
            &mut rusqlite::Connection,
            &crate::secrets::storage::StorageContext,
        ) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let storage = self.db.secrets.read()?;
        if storage.identity().store_id() != self.binding {
            return Err(setup_error("storageChanged"));
        }
        let mut conn = self
            .db
            .conn
            .lock()
            .map_err(|_| setup_error("storageUnavailable"))?;
        crate::database::storage::check_identity(&conn, &storage)?;
        action(&mut conn, &storage)
    }
    fn receipt(&self) -> Result<RelaySetupReceipt, AppError> {
        self.with(|conn| ledger::bound_on(conn, &self.binding, &self.operation))
    }
    fn review(&self, app: &str) -> Result<(), AppError> {
        self.with(|conn| {
            ledger::configuration_review_on(conn, &self.binding, &self.operation, app, now())
        })
        .map(|_| ())
    }
}
impl KeyPreparationJournal for Journal {
    fn record_issue(
        &self,
        group: &str,
        app: Option<&str>,
        issue: PreparationIssue,
    ) -> Result<(), AppError> {
        self.with(|conn| {
            ledger::preparation_issue_on(
                conn,
                &self.binding,
                &self.operation,
                group,
                app,
                issue,
                now(),
            )
        })
        .map(|_| ())
    }
    fn record_model_support(&self, group: &str, apps: &[String]) -> Result<(), AppError> {
        self.with(|conn| {
            ledger::model_support_on(conn, &self.binding, &self.operation, group, apps, now())
        })
        .map(|_| ())
    }
    fn record(&self, group: &str, event: KeyEvent) -> Result<(), AppError> {
        if matches!(&event, KeyEvent::BeforeCreate) {
            let authority = self.local_authority()?;
            self.with_storage(|conn, storage| {
                ledger::key_event_authorized_on(conn, &authority, group, event, now(), |tx| {
                    crate::database::Database::validate_relay_setup_authority_on(
                        tx,
                        storage,
                        &authority,
                        self.db.secrets.lifecycle_epoch(),
                    )
                    .map(|_| ())
                })
            })
            .map(|_| ())
        } else {
            // Acknowledgements remain recordable after account changes: facts
            // already observed must not be erased by a later loss of authority.
            self.with(|conn| {
                ledger::key_event_on(conn, &self.binding, &self.operation, group, event, now())
            })
            .map(|_| ())
        }
    }
    fn local_authority(&self) -> Result<PreparationAuthority, AppError> {
        self.authority
            .clone()
            .ok_or_else(|| setup_error("executionNotBound"))
    }
    fn record_local(
        &self,
        app: &str,
        provider_id: &str,
        fact: ConfigurationState,
    ) -> Result<(), AppError> {
        self.with(|conn| {
            let receipt = ledger::bound_on(conn, &self.binding, &self.operation)?;
            if !receipt
                .targets
                .iter()
                .any(|target| target.app == app && target.provider_id == provider_id)
            {
                return Err(setup_error("selectionChanged"));
            }
            ledger::local_fact_on(conn, &self.binding, &self.operation, app, fact, now())
                .map(|_| ())
        })
    }
}
fn read_local_fact(
    state: &AppState,
    account: &creds::RelayAccount,
    journal: &Journal,
    target: &RelayTargetFact,
) -> Result<bool, AppError> {
    let Some(provider) = state
        .db
        .get_provider_by_id(&target.provider_id, &target.app)?
    else {
        return Ok(false);
    };
    if provision::validate_selected_provider_identity(account, &target.group_id, &provider).is_err()
    {
        journal.review(&target.app)?;
    } else {
        journal.record_local(
            &target.app,
            &target.provider_id,
            ConfigurationState::Existing,
        )?;
    }
    Ok(true)
}
fn finish_local_observations(
    state: &AppState,
    account: &creds::RelayAccount,
    journal: &Journal,
    selections: &[RelaySelection],
) -> Result<RelaySetupReceipt, AppError> {
    let receipt = journal.receipt()?;
    // This owner has finished the protocol invocation under the account lock.
    // An outstanding attempt is now uncertain, not an indefinitely running job.
    for group in receipt
        .groups
        .iter()
        .filter(|group| group.state == KeyState::Requesting)
    {
        journal.record(&group.group_id, KeyEvent::OutcomeUnknown)?;
    }
    for target in receipt
        .targets
        .iter()
        .filter(|target| selections.iter().any(|s| s.app == target.app))
    {
        if matches!(
            target.configuration,
            ConfigurationState::Existing | ConfigurationState::Prepared
        ) {
            continue;
        }
        let present = read_local_fact(state, account, journal, target)?;
        // No local attempt was started, so this is definitely no local write.
        // Unknown is retained when the provider cannot now be found; absence is
        // not proof of the historical outcome of an interrupted local attempt.
        if !present && target.configuration == ConfigurationState::NotStarted {
            let issue = if receipt.groups.iter().any(|group| {
                group.group_id == target.group_id
                    && matches!(group.state, KeyState::Unknown | KeyState::Requesting)
            }) {
                PreparationIssue::RemoteOutcomeUnknown
            } else {
                PreparationIssue::PreparationIncomplete
            };
            journal.record_issue(&target.group_id, Some(&target.app), issue)?;
        }
    }
    journal.receipt()
}
async fn run_preparation(
    state: &AppState,
    account: &creds::RelayAccount,
    journal: Arc<Journal>,
    selections: &[RelaySelection],
    fallback: Option<sub2api::BrowserApiFallback>,
) -> Result<RelaySetupReceipt, AppError> {
    let receipt = journal.receipt()?;
    let mut missing = Vec::new();
    for selection in selections {
        let target = receipt
            .targets
            .iter()
            .find(|t| t.app == selection.app && t.group_id == selection.group_id)
            .ok_or_else(|| setup_error("selectionChanged"))?;
        if !read_local_fact(state, account, &journal, target)? {
            missing.push(provision::RelayTierSelection {
                app: selection.app.clone(),
                group_id: selection.group_id.clone(),
            });
        }
    }
    if !missing.is_empty() {
        // Only the original protocol owner can create/reveal. It records a claim
        // before POST and its acknowledgement before catalog/local processing.
        let result = provision::provision_backend_scope(
            account,
            fallback,
            Some(&missing),
            Some(journal.as_ref()),
        )
        .await;
        if let Ok(batch) = result {
            let current = read_account(state, account.id)?;
            if account_identity(&current)? != receipt.account_identity {
                return Err(setup_error("accountChanged"));
            }
            let state = state.clone();
            let account = account.clone();
            let recorded = journal.clone();
            // Even an error here must be resolved via the stored facts below,
            // never translated into "nothing was written" from an aggregate.
            let _ = tauri::async_runtime::spawn_blocking(move || {
                provision::persist_provision_batch_scope(
                    &state,
                    &account,
                    batch,
                    provision::ProvisionPersistenceIntent::SelectedPrepare,
                    Some(recorded.as_ref()),
                )
            })
            .await;
        }
    }
    finish_local_observations(state, account, &journal, selections)
}

#[tauri::command]
pub async fn relay_discover_tiers(
    app: tauri::AppHandle,
    relay_id: i64,
) -> Result<RelayDiscovery, String> {
    let state = app.state::<AppState>();
    let account = read_account(&state, relay_id).map_err(|e| e.to_string())?;
    discover(&state, &account, Some(browser_api_fallback(&app)))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn relay_get_setup_operation(
    app: tauri::AppHandle,
    relay_id: i64,
    operation_id: Option<String>,
    history_offset: Option<usize>,
) -> Result<Option<RelaySetupReceipt>, String> {
    let state = app.state::<AppState>();
    // Expired authentication does not prevent reading this device's evidence.
    let account = with_conn(&state, |conn, vault| creds::get(conn, vault, relay_id))
        .and_then(|value| value.ok_or_else(|| setup_error("accountUnavailable")))
        .map_err(|e| e.to_string())?;
    let identity = account_identity(&account).map_err(|e| e.to_string())?;
    let journal =
        Journal::new(&state, operation_id.as_deref().unwrap_or("")).map_err(|e| e.to_string())?;
    journal
        .with(|conn| {
            ledger::query_account_on(
                conn,
                &journal.binding,
                relay_id,
                &identity,
                operation_id.as_deref(),
                history_offset,
            )
        })
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn relay_prepare_tiers(
    app: tauri::AppHandle,
    request: RelayPrepareRequest,
) -> Result<RelaySetupReceipt, RelayPreparationError> {
    let state = app.state::<AppState>();
    prepare(&state, request, Some(browser_api_fallback(&app))).await
}
impl From<AppError> for RelayPreparationError {
    fn from(_: AppError) -> Self {
        Self::unknown()
    }
}
pub(super) async fn prepare(
    state: &AppState,
    request: RelayPrepareRequest,
    fallback: Option<sub2api::BrowserApiFallback>,
) -> Result<RelaySetupReceipt, RelayPreparationError> {
    match request {
        RelayPrepareRequest::Start {
            operation_id,
            relay_id,
            discovery_revision,
            selections,
        } => {
            let journal = Journal::new(&state, &operation_id)?;
            if let Some(receipt) = journal.with(|conn| ledger::read_on(conn, &operation_id))? {
                validate_repeat_start(&receipt, relay_id, &discovery_revision, &selections)
                    .map_err(setup_error)?;
                return journal.receipt().map_err(Into::into);
            }
            let account = read_account(state, relay_id).map_err(|_| {
                RelayPreparationError::rejected(&operation_id, "accountUnavailable")
            })?;
            let lock = provision::relay_provision_lock(&account)?;
            let _guard = lock.lock().await;
            let fresh = read_account(state, account.id).map_err(|_| {
                RelayPreparationError::rejected(&operation_id, "accountUnavailable")
            })?;
            if account_identity(&fresh)? != account_identity(&account)? {
                return Err(RelayPreparationError::rejected(
                    &operation_id,
                    "accountChanged",
                ));
            }
            let account = fresh;
            // A second window may have installed the operation while this waited.
            if let Some(receipt) = journal.with(|conn| ledger::read_on(conn, &operation_id))? {
                validate_repeat_start(&receipt, relay_id, &discovery_revision, &selections)
                    .map_err(setup_error)?;
                return journal.receipt().map_err(Into::into);
            }
            let discovered = discover(state, &account, fallback.clone())
                .await
                .map_err(|_| RelayPreparationError::rejected(&operation_id, "discoveryFailed"))?;
            validate_start(
                &operation_id,
                relay_id,
                &discovery_revision,
                &selections,
                &discovered,
            )
            .map_err(|code| RelayPreparationError::rejected(&operation_id, code))?;
            let initial = initial_receipt(&operation_id, &selections, &discovered, now())
                .map_err(|code| RelayPreparationError::rejected(&operation_id, code))?;
            let (receipt, inserted) =
                journal.with(|conn| ledger::begin_on(conn, &journal.binding, &initial))?;
            if !inserted {
                return Ok(receipt);
            }
            run_preparation(
                state,
                &account,
                Arc::new(journal.for_account(&account)?),
                &selections,
                fallback,
            )
            .await
            .map_err(Into::into)
        }
        RelayPrepareRequest::Resume {
            operation_id,
            expected_receipt_revision,
            apps,
        } => {
            let journal = Journal::new(&state, &operation_id)?;
            let prior = journal.receipt()?;
            let account = read_account(&state, prior.relay_id)?;
            let lock = provision::relay_provision_lock(&account)?;
            let _guard = lock.lock().await;
            let fresh = read_account(&state, account.id)?;
            if account_identity(&fresh)? != account_identity(&account)? {
                return Err(setup_error("accountChanged").into());
            }
            let account = fresh;
            let receipt = journal.receipt()?;
            if account_identity(&account)? != receipt.account_identity {
                return Err(setup_error("accountChanged").into());
            }
            let selections =
                validate_resume(&receipt, expected_receipt_revision, &apps).map_err(setup_error)?;
            let discovered = discover(&state, &account, fallback.clone()).await?;
            if selections.iter().any(|s| {
                !discovered
                    .choices
                    .iter()
                    .any(|c| c.app == s.app && c.group_id == s.group_id)
            }) {
                return Err(setup_error("selectionChanged").into());
            }
            // Begin only the explicitly confirmed failed projections. This clears
            // the previous attempt's phase reason, not its durable remote facts.
            let journal = journal.for_account(&account)?;
            journal.with(|conn| {
                ledger::begin_resume_on(
                    conn,
                    &journal.binding,
                    &journal.operation,
                    expected_receipt_revision,
                    &apps,
                    now(),
                )
            })?;
            run_preparation(state, &account, Arc::new(journal), &selections, fallback)
                .await
                .map_err(Into::into)
        }
    }
}

#[tauri::command]
pub async fn relay_reconcile_setup_operation(
    app: tauri::AppHandle,
    operation_id: String,
) -> Result<RelaySetupReceipt, String> {
    reconcile_operation(&app, &operation_id)
        .await
        .map_err(|e| e.to_string())
}
async fn reconcile_operation(
    app: &tauri::AppHandle,
    operation: &str,
) -> Result<RelaySetupReceipt, AppError> {
    let state = app.state::<AppState>();
    let journal = Journal::new(&state, operation)?;
    let account = connection::account_for_operation(&state, &journal)?;
    let lock = provision::relay_provision_lock(&account)?;
    let _guard = lock.lock().await;
    let account = connection::account_for_operation(&state, &journal)?;
    let receipt = journal.receipt()?;
    if account_identity(&account)? != receipt.account_identity {
        return Err(setup_error("accountChanged"));
    }
    connection::observe_connections(&state, &journal)?;
    let uncertain = receipt
        .groups
        .iter()
        .filter(|g| matches!(g.state, KeyState::Requesting | KeyState::Unknown))
        .map(|g| g.group_id.clone())
        .collect::<Vec<_>>();
    for group in receipt
        .groups
        .iter()
        .filter(|g| g.state == KeyState::Requesting)
    {
        journal.record(&group.group_id, KeyEvent::OutcomeUnknown)?;
    }
    if !uncertain.is_empty() {
        read_account(&state, account.id)?;
        match account.backend_kind {
            discovery::BackendKind::Sub2Api => {
                let client = sub2api::Client::new(
                    &account.site_origin,
                    &account.auth_token,
                    account.account_id,
                    account.user_agent.as_deref(),
                    account.cf_clearance.as_deref(),
                )?
                .with_browser_fallback(browser_api_fallback(app));
                let ids = uncertain
                    .iter()
                    .map(|id| {
                        id.parse::<i64>()
                            .map_err(|_| setup_error("invalidIdentity"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                crate::relay::provision::observe_selected_keys(&client, &ids, &journal).await?;
            }
            discovery::BackendKind::NewApi => {
                let client = newapi::NewApiClient::with_optional_account_id(
                    &account.site_origin,
                    &account.auth_token,
                    account.account_id,
                )?;
                let ids = uncertain
                    .into_iter()
                    .map(newapi::GroupIdentity)
                    .collect::<Vec<_>>();
                newapi_provision::observe_selected_tokens(
                    &client,
                    account
                        .account_id
                        .ok_or_else(|| setup_error("authenticationRequired"))?,
                    &ids,
                    &journal,
                )
                .await?;
            }
        }
    }
    finish_local_observations(&state, &account, &journal, &receipt.selections)?;
    connection::observe_connections(&state, &journal)?;
    journal.receipt()
}
