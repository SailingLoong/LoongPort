//! App-local recovery inside the original authenticated startup owner. A DB
//! checkpoint is not app completion; these queries never authorize runtime.
use super::*;
use crate::{
    app_config::AppType,
    mode::{current, operation, state::Mode},
};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::sync::RwLockReadGuard;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpgradeProviderChoice {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpgradeAppReview {
    pub(crate) app_type: String,
    pub(crate) revision: String,
    pub(crate) saved_mode: Option<Mode>,
    pub(crate) has_pending_operation: Option<bool>,
    pub(crate) pointer_consistent: Option<bool>,
    pub(crate) live_status: &'static str,
    pub(crate) stored_fields_match: Option<bool>,
    pub(crate) can_recover_operation: bool,
    pub(crate) default_action: &'static str,
    pub(crate) default_takeover: bool,
    pub(crate) can_complete_app: bool,
    pub(crate) can_start_upgrade: bool,
    pub(crate) direct_provider_resolution: &'static str,
    pub(crate) retained_provider_id: Option<String>,
    pub(crate) keep_files_providers: Vec<UpgradeProviderChoice>,
    pub(crate) can_choose_provider: bool,
    pub(crate) can_choose_mode: bool,
    pub(crate) mode_route_providers: Vec<UpgradeProviderChoice>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpgradeModeChoice {
    pub(crate) mode: Mode,
    pub(crate) proxy_route: Option<String>,
}

enum KeepFilesChoice<'a> {
    Provider(&'a str),
    Mode(&'a UpgradeModeChoice),
}

struct AppCapture {
    view: UpgradeAppReview,
    files: Vec<ReviewedInput>,
    identity: inspection::DatabaseIdentity,
    flags: (bool, bool),
    finalized_revision: Option<String>,
    provider_digests: std::collections::BTreeMap<String, String>,
    native_pending: Option<crate::mode::state::Pending>,
    listener_running_revision: Option<String>,
    recovery_facts: RecoveryFacts,
}

/// Only the original target's own fields may change during replay. This is a
/// transient projection of the same reviewed app facts, not another journal.
// Original shared listener fields, by app row: app, address, port, enabled, logging.
type ListenerFacts = Vec<(String, String, u16, bool, bool)>;

fn listener_endpoint(facts: &ListenerFacts) -> Option<(&str, u16)> {
    let global = facts.iter().find(|row| row.0 == "claude")?;
    (global.2 != 0
        && global
            .1
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
        && facts.iter().all(|row| {
            (row.1.as_str(), row.2, row.3, row.4)
                == (global.1.as_str(), global.2, global.3, global.4)
        }))
    .then_some((global.1.as_str(), global.2))
}

#[derive(Clone, PartialEq)]
struct RecoveryFacts {
    live: Option<crate::mode::state::LiveState>,
    local: Option<String>,
    currents: Vec<String>,
    rows: std::collections::BTreeMap<String, String>,
    unowned_row_fields: serde_json::Value,
    order: crate::database::order_profiles::OrderSnapshot,
    flags: (bool, bool),
    preference: Option<String>,
    settings: serde_json::Value,
    codex_endpoint: Option<(String, u16)>,
    listener: Option<ListenerFacts>,
}
impl RecoveryFacts {
    fn verify_target_changes(
        &self,
        mut actual: Self,
        app: &AppType,
        target: &crate::mode::state::PendingTarget,
        listener_running: bool,
    ) -> Result<(), AppError> {
        if listener_running {
            let (Some(before), Some(now)) = (&self.listener, &mut actual.listener) else {
                return Err(source_changed());
            };
            // The admitted owner has started/reused a running listener. A late
            // false bit is drift, even when false was the pre-start value.
            if now.is_empty() || now.iter().any(|row| !row.3) {
                return Err(source_changed());
            }
            for row in now {
                if let Some(old) = before.iter().find(|old| old.0 == row.0) {
                    row.3 = old.3;
                }
            }
        }
        if let Some(id) = &target.pointer {
            if actual.local.as_ref() == Some(id) {
                actual.local = self.local.clone();
            }
            if actual.currents == [id.clone()] {
                actual.currents = self.currents.clone();
            }
        }
        if let Some(row) = &target.saved_row {
            let planned = operation::saved_provider(row)?;
            if actual.rows.get(&planned.id) == Some(&Database::provider_update_digest(&planned)?) {
                if let Some(before) = self.rows.get(&planned.id) {
                    actual.rows.insert(planned.id, before.clone());
                }
            }
        }
        let preference = operation::target_model_preference(target).map(|action| match action {
            crate::mode::state::ModelPreferenceAction::Set { model } => model,
            crate::mode::state::ModelPreferenceAction::Clear {} => String::new(),
        });
        if target
            .routing_order
            .as_ref()
            .is_some_and(|order| actual.order == order.planned)
        {
            actual.order = self.order.clone();
        }
        if preference.is_some() && actual.preference == preference {
            actual.preference = self.preference.clone();
        }
        if target
            .state
            .as_ref()
            .is_some_and(|mode| actual.flags.0 == mode.is_proxy())
        {
            actual.flags.0 = self.flags.0;
        }
        let mut before = self.clone();
        if let (Some(old), Some(now)) = (
            before
                .live
                .as_mut()
                .and_then(|live| live.apps.get_mut(app.as_str())),
            actual
                .live
                .as_mut()
                .and_then(|live| live.apps.get_mut(app.as_str())),
        ) {
            // The engine compares the exact current journal, including an
            // authenticated Codex generation adoption, before every mutation.
            old.pending = None;
            now.pending = None;
            if target
                .state
                .as_ref()
                .is_some_and(|mode| now.mode_state() == *mode)
            {
                now.set_mode_state(old.mode_state())
                    .map_err(|_| source_changed())?;
            }
            if target.written.is_some() && now.written == target.written {
                now.written = old.written.clone();
            }
        }
        if actual != before {
            return Err(source_changed());
        }
        Ok(())
    }
}

impl AuthenticatedUpgrade {
    fn app_settings(
        &self,
        app: &AppType,
        vault: &RwLockReadGuard<'_, VaultContext>,
    ) -> Result<crate::settings::AppSettings, AppError> {
        match crate::settings::read_upgrade_settings_with_vault(
            app,
            &self.session,
            vault,
            checkpoint::MAX_BYTES,
        ) {
            Err(AppError::Config(code)) if code == "settings.already_unlocked" => {
                crate::settings::read_native_app_settings_with_vault(
                    app,
                    &self.session,
                    vault,
                    checkpoint::MAX_BYTES,
                )
            }
            result => result,
        }
    }

    fn verify_app_session(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
    ) -> Result<(), AppError> {
        if token != self.token
            || self.database_checkpoint.is_none()
            || self.cancellation.is_some()
            || !crate::mode::controller::PROXY_APPS.contains(app)
        {
            return Err(source_changed());
        }
        self.verify(inspected)
    }

    fn verify_app_checkpoint_pinned(
        &self,
        inspected: &UpgradeInspection,
        vault: &VaultContext,
    ) -> Result<(), AppError> {
        let UpgradeInspection::Stable(stable) = inspected else {
            return Err(source_changed());
        };
        if crate::config::get_app_config_dir() != self.root {
            return Err(source_changed());
        }
        stable.verify_resume_checkpoint(&self.root)?;
        if checkpoint::verified_database_id(&self.root, &self.device, vault)?.as_ref()
            != self.database_checkpoint.as_ref()
        {
            return Err(source_changed());
        }
        stable.verify_resume_checkpoint(&self.root)
    }

    fn capture_app(
        &self,
        app: &AppType,
        vault: &RwLockReadGuard<'_, VaultContext>,
    ) -> Result<AppCapture, AppError> {
        self.capture_app_with_state(app, vault, None)
    }

    fn capture_app_with_state(
        &self,
        app: &AppType,
        vault: &RwLockReadGuard<'_, VaultContext>,
        pinned_state: Option<&crate::mode::state::LiveState>,
    ) -> Result<AppCapture, AppError> {
        self.capture_recovery_app(app, vault, pinned_state, false)
    }

    fn capture_recovery_app(
        &self,
        app: &AppType,
        vault: &RwLockReadGuard<'_, VaultContext>,
        pinned_state: Option<&crate::mode::state::LiveState>,
        include_listener: bool,
    ) -> Result<AppCapture, AppError> {
        let settings = self.app_settings(app, vault)?;
        capture_app_with_state(
            &self.device,
            &self.session,
            &self.token,
            app,
            vault,
            pinned_state,
            settings,
            include_listener,
        )
    }

    fn review_app_locked(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
    ) -> Result<AppCapture, AppError> {
        self.verify_app_session(inspected, token, app)?;
        let result = {
            let vault = self.session.read()?;
            self.verify_app_checkpoint_pinned(inspected, &vault)?;
            let first = self.capture_app(app, &vault)?;
            let second = self.capture_app(app, &vault)?;
            if first.view.revision != second.view.revision {
                return Err(source_changed());
            }
            self.verify_app_checkpoint_pinned(inspected, &vault)?;
            second
        };
        self.verify(inspected)?;
        Ok(result)
    }

    pub(crate) fn review_app(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
    ) -> Result<UpgradeAppReview, AppError> {
        let _guard = crate::live::engine::lock_app(app.as_str());
        Ok(self.review_app_locked(inspected, token, app)?.view)
    }

    pub(crate) fn select_provider(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
        provider_id: &str,
        state: &crate::store::AppState,
    ) -> Result<UpgradeAppReview, AppError> {
        self.select_keep_files(
            inspected,
            token,
            app,
            revision,
            KeepFilesChoice::Provider(provider_id),
            state,
        )
    }

    pub(crate) fn select_mode(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
        choice: &UpgradeModeChoice,
        state: &crate::store::AppState,
    ) -> Result<UpgradeAppReview, AppError> {
        self.select_keep_files(
            inspected,
            token,
            app,
            revision,
            KeepFilesChoice::Mode(choice),
            state,
        )
    }

    fn select_keep_files(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
        choice: KeepFilesChoice<'_>,
        state: &crate::store::AppState,
    ) -> Result<UpgradeAppReview, AppError> {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
        let guard = crate::live::engine::lock_app(app.as_str());
        if !std::sync::Arc::ptr_eq(&state.db.secrets, &self.session) {
            return Err(source_changed());
        }
        let captured = self.review_app_locked(inspected, token, app)?;
        let (provider_id, selected_mode) = match choice {
            KeepFilesChoice::Provider(id) if captured.view.can_choose_provider => (id, None),
            KeepFilesChoice::Mode(choice) if captured.view.can_choose_mode => {
                let id = captured
                    .view
                    .retained_provider_id
                    .as_deref()
                    .ok_or_else(source_changed)?;
                if (choice.mode == Mode::Direct && choice.proxy_route.is_some())
                    || (choice.mode == Mode::Proxy
                        && !choice.proxy_route.as_ref().is_some_and(|route| {
                            captured
                                .view
                                .mode_route_providers
                                .iter()
                                .any(|provider| &provider.id == route)
                        }))
                {
                    return Err(source_changed());
                }
                (
                    id,
                    Some(crate::mode::state::ModeState {
                        mode: Some(choice.mode),
                        proxy_route: choice.proxy_route.clone(),
                        ..Default::default()
                    }),
                )
            }
            _ => return Err(source_changed()),
        };
        if captured.view.revision != revision
            || !captured
                .view
                .keep_files_providers
                .iter()
                .any(|p| p.id == provider_id)
        {
            return Err(source_changed());
        }
        {
            let vault = self.session.read()?;
            self.verify_app_checkpoint_pinned(inspected, &vault)?;
            let path = self.root.join(crate::config::DB_FILE_NAME);
            inspection::verify_primary_identity(&path, &captured.identity)?;
            {
                let conn = state.db.conn.lock()?;
                inspection::verify_connection_primary_identity(&conn, &captured.identity)?;
            }
            // Refresh the original runtime owner under this same Vault guard.
            // Fresh unrelated settings must survive the selected pointer write.
            crate::settings::reload_settings_with_vault(&self.session, &vault)?;
            if self.capture_app(app, &vault)?.view.revision != revision {
                return Err(source_changed());
            }
            let admitted = crate::mode::controller::files(app)?;
            let mut patches = Vec::new();
            for file in &admitted {
                let bytes =
                    crate::config_file_io::read_regular_file(&file.path, checkpoint::MAX_BYTES)
                        .map_err(|error| AppError::io(&file.path, error))?;
                patches.push(crate::live::patch::Guarded {
                    expected_pre: crate::live::engine::digest(bytes.as_deref()),
                    then: bytes
                        .map(crate::live::patch::WholeFile::Write)
                        .unwrap_or(crate::live::patch::WholeFile::Delete),
                });
            }
            for file in &captured.files {
                file.verify()?;
            }
            let changes = admitted
                .into_iter()
                .zip(&patches)
                .map(|(file, patch)| operation::FileChange { file, patch })
                .collect::<Vec<_>>();
            let mut target = selected_mode
                .map(crate::mode::state::PendingTarget::mode)
                .unwrap_or_default();
            target.pointer = Some(provider_id.to_owned());
            let verify_provider = || {
                for id in std::iter::once(provider_id).chain(
                    target
                        .state
                        .as_ref()
                        .and_then(|mode| mode.proxy_route.as_deref()),
                ) {
                    let row = state
                        .db
                        .get_provider_by_id_with_vault(id, app.as_str(), &self.session, &vault)?
                        .ok_or_else(source_changed)?;
                    if captured.provider_digests.get(id)
                        != Some(&Database::provider_update_digest(&row)?)
                    {
                        return Err(source_changed());
                    }
                }
                for file in &captured.files {
                    file.verify()?;
                }
                Ok(())
            };
            let verify_cleanup = |live: &crate::mode::state::LiveState| {
                self.verify_app_checkpoint_pinned(inspected, &vault)?;
                inspection::verify_primary_identity(&path, &captured.identity)?;
                {
                    let conn = state.db.conn.lock()?;
                    inspection::verify_connection_primary_identity(&conn, &captured.identity)?;
                }
                verify_provider()?;
                let actual = self.capture_app_with_state(app, &vault, Some(live))?;
                let mode = target.state.as_ref().ok_or_else(source_changed)?;
                if live
                    .apps
                    .get(app.as_str())
                    .map(|entry| entry.mode_state())
                    .as_ref()
                    != Some(mode)
                    || actual.flags != (mode.is_proxy(), captured.flags.1)
                    || actual.view.pointer_consistent != Some(true)
                    || !actual.view.can_recover_operation
                    || actual.finalized_revision.as_ref() != Some(&actual.view.revision)
                {
                    return Err(source_changed());
                }
                self.verify_app_checkpoint_pinned(inspected, &vault)
            };
            operation::run_checked(
                &self.device,
                &vault,
                &guard,
                crate::mode::state::op::APPLY,
                &changes,
                target.clone(),
                &operation::RunChecks {
                    commit_target: &|actual| {
                        if actual != &target {
                            return Err(source_changed());
                        }
                        self.verify_app_checkpoint_pinned(inspected, &vault)?;
                        inspection::verify_primary_identity(&path, &captured.identity)?;
                        operation::failpoint::hit("upgrade:provider_target")?;
                        if target.state.is_some() {
                            operation::failpoint::hit("upgrade:mode_target")?;
                        }
                        verify_provider()?;
                        let commit = if actual.state.is_some() {
                            operation::commit_upgrade_mode_choice
                        } else {
                            operation::commit_target
                        };
                        commit(&state.db, &self.session, &self.device, &vault, app, actual)?;
                        verify_provider()?;
                        self.verify_app_checkpoint_pinned(inspected, &vault)
                    },
                    before_cleanup: target.state.as_ref().map(|_| {
                        &verify_cleanup
                            as &dyn Fn(&crate::mode::state::LiveState) -> Result<(), AppError>
                    }),
                },
            )?;
        }
        Ok(self.review_app_locked(inspected, token, app)?.view)
    }

    pub(crate) fn recover_app(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
    ) -> Result<UpgradeAppReview, AppError> {
        self.recover_app_with_runtime(inspected, token, app, revision, None)
    }

    pub(crate) fn recover_app_with_state(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
        state: &crate::store::AppState,
    ) -> Result<UpgradeAppReview, AppError> {
        self.recover_app_with_runtime(inspected, token, app, revision, Some(state))
    }

    fn recover_app_with_runtime(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
        runtime: Option<&crate::store::AppState>,
    ) -> Result<UpgradeAppReview, AppError> {
        let _switch = runtime.map(|state| {
            futures::executor::block_on(state.proxy_service.lock_recovery_for_app(app.as_str()))
        });
        if runtime.is_some_and(|state| !std::sync::Arc::ptr_eq(&state.db.secrets, &self.session)) {
            return Err(source_changed());
        }
        let guard = crate::live::engine::lock_app(app.as_str());
        let captured = self.review_app_locked(inspected, token, app)?;
        if captured.view.revision != revision || !captured.view.can_recover_operation {
            return Err(source_changed());
        }
        if captured.finalized_revision.is_none() {
            let state = runtime.ok_or_else(source_changed)?;
            let pending = captured
                .native_pending
                .as_ref()
                .ok_or_else(source_changed)?;
            // Codex owns manager -> app -> vault lock order. Keep the service
            // switch lock, but release this query guard before entering it.
            drop(guard);
            let verify_identity = |db: &Database, vault: &RwLockReadGuard<'_, VaultContext>| {
                if !std::sync::Arc::ptr_eq(&db.secrets, &self.session) {
                    return Err(source_changed());
                }
                self.verify_app_checkpoint_pinned(inspected, vault)?;
                let path = self.root.join(crate::config::DB_FILE_NAME);
                inspection::verify_primary_identity(&path, &captured.identity)?;
                inspection::verify_connection_primary_identity(
                    &*db.conn.lock()?,
                    &captured.identity,
                )
            };
            let endpoint = captured
                .recovery_facts
                .listener
                .as_ref()
                .and_then(listener_endpoint);
            let listener_required = match endpoint {
                Some((address, port)) => crate::mode::controller::recovery_listener_required(
                    &state.proxy_service,
                    app,
                    pending,
                    address,
                    port,
                )?,
                None => false,
            };
            let listener_ready = std::cell::Cell::new(false);
            let verify_listener = || -> Result<(), AppError> {
                if listener_ready.get() {
                    let (address, port) = endpoint.ok_or_else(source_changed)?;
                    futures::executor::block_on(
                        state.proxy_service.verify_recovery_listener(address, port),
                    )
                    .map_err(AppError::Message)?;
                }
                Ok(())
            };
            let admit = |db: &Database, vault: &RwLockReadGuard<'_, VaultContext>| {
                verify_identity(db, vault)?;
                verify_listener()?;
                let actual = self.capture_app(app, vault)?;
                let expected_revision = if listener_ready.get() {
                    captured
                        .listener_running_revision
                        .as_deref()
                        .ok_or_else(source_changed)?
                } else {
                    revision
                };
                if actual.view.revision != expected_revision
                    || actual.native_pending.as_ref() != Some(pending)
                {
                    return Err(source_changed());
                }
                for file in &captured.files {
                    file.verify()?;
                }
                verify_identity(db, vault)
            };
            let verify = |vault: &RwLockReadGuard<'_, VaultContext>,
                          current: &crate::mode::state::Pending,
                          live: Option<&crate::mode::state::LiveState>| {
                verify_identity(&state.db, vault)?;
                verify_listener()?;
                if current.op != pending.op || current.target != pending.target {
                    return Err(source_changed());
                }
                // EXIT/DETACH may replace attached state before cleanup. Keep
                // checking the original listener facts through that same replay.
                let actual = self.capture_recovery_app(
                    app,
                    vault,
                    live,
                    captured.recovery_facts.listener.is_some(),
                )?;
                captured.recovery_facts.verify_target_changes(
                    actual.recovery_facts,
                    app,
                    &pending.target,
                    listener_ready.get(),
                )?;
                verify_identity(&state.db, vault)
            };
            let mut started = false;
            if listener_required {
                let (address, port) = endpoint.ok_or_else(source_changed)?;
                let before_start = || {
                    let _app = crate::live::engine::lock_app(app.as_str());
                    let vault = self.session.read().map_err(|error| error.to_string())?;
                    admit(&state.db, &vault).map_err(|error| error.to_string())
                };
                operation::failpoint::hit("upgrade:listener_start")?;
                started = futures::executor::block_on(state.proxy_service.start_recovery_listener(
                    address,
                    port,
                    &before_start,
                ))
                .map_err(AppError::Message)?;
                listener_ready.set(true);
            }
            let result =
                operation::failpoint::hit("upgrade:native_recovery_owner").and_then(|()| {
                    crate::mode::controller::recover_locked_with_checks(
                        &state.proxy_service,
                        app,
                        Some(&operation::AppRecoveryChecks {
                            pending,
                            admit: &admit,
                            verify: &verify,
                        }),
                    )
                });
            if let Err(error) = result {
                if started {
                    if let Err(cleanup) = futures::executor::block_on(state.proxy_service.stop()) {
                        return Err(AppError::Message(format!(
                            "{error}; listener cleanup failed: {cleanup}"
                        )));
                    }
                }
                return Err(error);
            }
            // Discarded/Abandoned/VerificationRequired retain their original
            // meaning; only a new read reports the actual app outcome.
            return self.review_app(inspected, token, app);
        }
        {
            let vault = self.session.read()?;
            self.verify_app_checkpoint_pinned(inspected, &vault)?;
            let path = self.root.join(crate::config::DB_FILE_NAME);
            crate::mode::operation::failpoint::hit("recover:database_open")?;
            let reopened = if runtime.is_none() {
                Some(Database::from_connection(
                    rusqlite::Connection::open_with_flags(
                        &path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                    )?,
                    self.session.clone(),
                ))
            } else {
                None
            };
            let db = runtime
                .map(|state| state.db.as_ref())
                .or(reopened.as_ref())
                .ok_or_else(source_changed)?;
            inspection::verify_primary_identity(&path, &captured.identity)?;
            {
                let conn = db.conn.lock()?;
                inspection::verify_connection_primary_identity(&conn, &captured.identity)?;
            }
            let runtime_ready = runtime.is_some()
                && crate::settings::read_native_app_settings_with_vault(
                    app,
                    &self.session,
                    &vault,
                    checkpoint::MAX_BYTES,
                )
                .is_ok();
            if runtime_ready {
                crate::settings::reload_settings_with_vault(&self.session, &vault)?;
            }
            let publish_pointer = |id: &str| {
                if !runtime_ready {
                    return Err(source_changed());
                }
                crate::settings::set_current_provider_with_vault(
                    app,
                    Some(id),
                    &self.session,
                    &vault,
                )
            };
            let before = || {
                self.verify_app_checkpoint_pinned(inspected, &vault)?;
                if self.capture_app(app, &vault)?.view.revision != revision {
                    return Err(source_changed());
                }
                Ok(())
            };
            let pointer = |id: &str| {
                inspection::verify_primary_identity(&path, &captured.identity)?;
                self.verify_app_checkpoint_pinned(inspected, &vault)?;
                let mut settings = self.app_settings(app, &vault)?;
                if crate::settings::current_provider_from_settings(&mut settings, app).as_deref()
                    != Some(id)
                {
                    return Err(source_changed());
                }
                for file in &captured.files {
                    file.verify()?;
                }
                Ok(())
            };
            let after = || {
                self.verify_app_checkpoint_pinned(inspected, &vault)?;
                inspection::verify_primary_identity(&path, &captured.identity)?;
                let actual = self.capture_app(app, &vault)?;
                if captured.finalized_revision.as_ref() != Some(&actual.view.revision) {
                    return Err(source_changed());
                }
                for file in &captured.files {
                    file.verify()?;
                }
                Ok(())
            };
            let outcome = operation::recover_published_pointer(
                db,
                &self.device,
                &vault,
                &guard,
                app,
                &operation::PointerRecoveryChecks {
                    before_target: &before,
                    pointer: &pointer,
                    publish_pointer: runtime_ready
                        .then_some(&publish_pointer as &dyn Fn(&str) -> Result<(), AppError>),
                    after_target: &after,
                    before_cleanup: &|live| {
                        self.verify_app_checkpoint_pinned(inspected, &vault)?;
                        inspection::verify_primary_identity(&path, &captured.identity)?;
                        let actual = self.capture_app_with_state(app, &vault, Some(live))?;
                        if captured.finalized_revision.as_ref() != Some(&actual.view.revision) {
                            return Err(source_changed());
                        }
                        self.verify_app_checkpoint_pinned(inspected, &vault)?;
                        inspection::verify_primary_identity(&path, &captured.identity)
                    },
                },
            )?;
            if outcome != Some(operation::RecoveryOutcome::RolledForward) {
                return Err(source_changed());
            }
        }
        Ok(self.review_app_locked(inspected, token, app)?.view)
    }
}

// Startup review and native AppWrite consume the same transient app facts.
#[allow(clippy::too_many_arguments)]
fn capture_app_with_state(
    device: &DeviceStore,
    session: &std::sync::Arc<crate::secrets::session::SecretSession>,
    token: &str,
    app: &AppType,
    vault: &RwLockReadGuard<'_, VaultContext>,
    pinned_state: Option<&crate::mode::state::LiveState>,
    mut settings: crate::settings::AppSettings,
    include_listener: bool,
) -> Result<AppCapture, AppError> {
    let root = session.root();
    let local = crate::settings::current_provider_from_settings(&mut settings, app);
    // Cleanup borrows the snapshot already pinned by the original state
    // lock; ordinary queries capture one authenticated shared envelope.
    let live = match pinned_state {
        Some(live) => Some(live.clone()),
        None => crate::mode::state::read_review_snapshot(device, vault)?
            .app_view(app.as_str())
            .ok(),
    };
    let mode = live
        .as_ref()
        .and_then(|live| current::known_mode_from_state(live, app).ok());
    let pending = live.as_ref().map(|live| {
        live.apps
            .get(app.as_str())
            .and_then(|entry| entry.pending.as_ref())
    });
    let path = root.join(crate::config::DB_FILE_NAME);
    let captured = inspection::capture(&path)?.ok_or_else(source_changed)?;
    let db_revision = captured.revision;
    let identity = db_revision.primary_identity()?;
    let db = Database::from_connection(captured.image, session.clone());
    let (rows, currents) = {
        let conn = db.conn.lock()?;
        let rows = Database::get_all_providers_on_connection(&conn, vault, app.as_str())?;
        let mut statement = conn
            .prepare("SELECT id FROM providers WHERE app_type=?1 AND is_current<>0 ORDER BY id")?;
        let ids = statement
            .query_map([app.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        (rows, ids)
    };
    // Official non-unified routes retain a dormant custom table at the
    // original global listener endpoint. Read only the captured database.
    let codex_endpoint = if *app == AppType::Codex && !settings.unify_codex_session_history {
        let conn = db.conn.lock()?;
        conn.query_row(
            "SELECT listen_address, listen_port FROM proxy_config WHERE app_type='claude'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u16>(1)?)),
        )
        .optional()?
    } else {
        None
    };
    let attached_pending = pending.flatten().is_some_and(|pending| {
        mode.as_ref().is_some_and(|mode| mode.attached)
            || pending
                .target
                .state
                .as_ref()
                .is_some_and(|mode| mode.attached)
    });
    let listener = if attached_pending || include_listener {
        let conn = db.conn.lock()?;
        let mut statement = conn.prepare("SELECT app_type, listen_address, listen_port, proxy_enabled, enable_logging FROM proxy_config ORDER BY app_type")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u16>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })?
            .collect::<Result<ListenerFacts, _>>()?;
        Some(rows)
    } else {
        None
    };
    let flags = db.get_proxy_flags_checked(app.as_str())?;
    let order = crate::database::order_profiles::snapshot(&db, app.as_str())?;
    let preference = crate::proxy::auto_strategy::get_model_pref_checked(&db, app.as_str())?;
    let pointer_consistent = (currents.len() <= 1).then(|| {
        local
            .as_ref()
            .is_some_and(|id| rows.contains_key(id) && currents.first() == Some(id))
    });
    let admitted = crate::mode::controller::files(app)?;
    let target = pending.flatten().and_then(|pending| {
        operation::published_pointer_target(app.as_str(), pending, &admitted).ok()
    });
    let compatible = live
        .as_ref()
        .and_then(|live| live.apps.get(app.as_str()))
        .is_some_and(|entry| {
            crate::mode::state::validate_app_evidence_for_update(app.as_str(), entry).is_ok()
        });
    let missing_mode = live.as_ref().is_some_and(|live| {
        super::staged_review::mode_resolution(live, app, mode.as_ref()) == "missing"
            && operation::mode_choice_source_matches(
                live,
                app,
                &crate::mode::state::ModeState {
                    mode: Some(Mode::Direct),
                    ..Default::default()
                },
            )
    });
    let mode_target = pending
        .flatten()
        .and_then(|pending| pending.target.state.as_ref());
    let mode_target_matches = mode_target.is_some_and(|target| {
        live.as_ref()
            .is_some_and(|live| operation::mode_choice_source_matches(live, app, target))
    });
    let mut bound = live_review::BoundFiles::new();
    let mut files = Vec::new();
    for file in &admitted {
        let input = ReviewedInput::capture(file.path.clone())?;
        let bytes = crate::config_file_io::read_regular_file(&file.path, checkpoint::MAX_BYTES)
            .map_err(|error| AppError::io(&file.path, error))?;
        input.verify()?;
        bound.insert(file.path.clone(), bytes.map(zeroize::Zeroizing::new));
        files.push(input);
    }
    let catalog = if *app == AppType::Codex {
        bound
            .get(&crate::codex_config::get_codex_config_path())
            .and_then(|bytes| bytes.as_ref())
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| {
                crate::codex_config::resolve_cc_switch_catalog_path(
                    text,
                    &crate::codex_config::get_codex_config_dir(),
                )
            })
    } else {
        None
    };
    let catalog_present = if let Some(path) = catalog {
        let input = ReviewedInput::capture(path.clone())?;
        let bytes = crate::config_file_io::read_regular_file(&path, checkpoint::MAX_BYTES)
            .map_err(|error| AppError::io(&path, error))?;
        input.verify()?;
        let present = bytes.is_some();
        bound.insert(path, bytes.map(zeroize::Zeroizing::new));
        files.push(input);
        Some(present)
    } else {
        None
    };
    // A detached Proxy keeps its route, but its native files represent the
    // independent direct pointer. Verify those files without changing mode.
    let detached_mode_verified = compatible
        && mode.as_ref().is_some_and(|mode| {
            !mode.attached
                && (mode.mode == Some(Mode::Direct)
                    || (mode.mode == Some(Mode::Proxy)
                        && mode
                            .proxy_route
                            .as_ref()
                            .is_some_and(|id| rows.contains_key(id))))
        });
    let candidate = local
        .as_ref()
        .filter(|_| {
            pending == Some(None) && pointer_consistent == Some(true) && detached_mode_verified
        })
        .and_then(|id| rows.get(id));
    let written = live
        .as_ref()
        .and_then(|live| live.apps.get(app.as_str()))
        .and_then(|entry| entry.written.as_ref());
    let inspect_candidate = |row: Option<&crate::provider::Provider>| {
        let retired = (*app == AppType::GrokBuild
            && (compatible || missing_mode)
            && written.is_none_or(|written| written.codex.is_none()))
        .then(|| crate::services::provider::grok_direct::retired_tables_from_written(written, row));
        let mut facts = live_review::inspect_with_grok_retired(
            app,
            &bound,
            row,
            catalog_present,
            retired.as_deref(),
        )?;
        if *app == AppType::Codex {
            facts.native_completion_match = row.and_then(|row| {
                let pre = crate::services::provider::codex_direct::files()
                    .iter()
                    .map(|file| {
                        live_review::bytes(&bound, &file.path)
                            .map(|bytes| bytes.map(<[u8]>::to_vec))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                crate::services::provider::codex_direct::native_completion_match(
                    row,
                    &rows,
                    &settings,
                    vault,
                    pre,
                    written,
                    codex_endpoint.as_ref(),
                )
            });
        }
        Ok::<_, AppError>(facts)
    };
    let client = inspect_candidate(candidate)?;
    // Only a complete native owned-field proof grants admission. Unsupported
    // Codex auth stores, managed generations and catalog discovery stay unknown.
    let target_native_proven = if let Some(row) = target.and_then(|id| rows.get(id)) {
        let proof = inspect_candidate(Some(row))?;
        proof.status == "parsed"
            && proof.marker == Some(false)
            && proof.native_completion_match == Some(true)
    } else {
        false
    };
    let forward_pointer_proven = target_native_proven
        && (mode_target_matches || detached_mode_verified)
        && preference.as_ref().is_none_or(String::is_empty)
        && crate::settings::read_native_app_settings_with_vault(
            app,
            session,
            vault,
            checkpoint::MAX_BYTES,
        )
        .is_ok();
    // The original no-op APPLY witnesses identify a keep-files pointer intent.
    // A newer row must still match the native files even if its pointer was
    // published before the interruption. Empty legacy intents retain their path.
    let keep_files_pending = pending.flatten().is_some_and(|pending| {
        pending.op == crate::mode::state::op::APPLY
            && !pending.files.is_empty()
            && pending.files.iter().all(|file| file.pre == file.planned)
    });
    let native_pending = pending
        .flatten()
        .filter(|pending| {
            target.is_none()
                && compatible
                && mode.is_some()
                && (!attached_pending
                    || (listener.as_ref().and_then(listener_endpoint).is_some()
                        && pending
                            .target
                            .state
                            .as_ref()
                            .filter(|mode| mode.attached)
                            .or_else(|| mode.as_ref().filter(|mode| mode.attached))
                            .is_some_and(|mode| mode.contract.is_some())))
                && (!pending.files.is_empty()
                    || pending.target.saved_row.is_some()
                    || pending.target.model_preference.is_some()
                    || pending.target.routing_order.is_some()
                    || pending.target.state.is_some())
                && operation::validate_pending(app.as_str(), pending, &admitted).is_ok()
                && crate::settings::read_native_app_settings_with_vault(
                    app,
                    session,
                    vault,
                    checkpoint::MAX_BYTES,
                )
                .is_ok()
        })
        .cloned();
    if let Some(pending) = &native_pending {
        for staged in pending.files.iter().filter_map(|file| file.staged.as_ref()) {
            files.push(ReviewedInput::capture(staged.clone())?);
        }
    }
    let can_recover_operation = compatible
        && if mode_target.is_some() {
            mode_target_matches
        } else {
            mode.as_ref().is_some_and(|mode| {
                !mode.attached
                    && (mode.mode == Some(Mode::Direct)
                        || mode
                            .proxy_route
                            .as_ref()
                            .is_some_and(|id| rows.contains_key(id)))
            })
        }
        && mode_target.is_none_or(|mode| {
            mode.proxy_route
                .as_ref()
                .is_none_or(|id| rows.contains_key(id))
        })
        && target.is_some_and(|id| {
            rows.contains_key(id)
                && ((local.as_deref() == Some(id) && (!keep_files_pending || target_native_proven))
                    || forward_pointer_proven)
        })
        && (currents.len() <= 1 || forward_pointer_proven)
        || native_pending.is_some();
    let can_complete_app = candidate.is_some()
        && client.status == "parsed"
        && client.marker == Some(false)
        && client.native_completion_match == Some(true);
    let direct_provider_resolution = super::staged_review::direct_provider_resolution(
        local.as_deref(),
        &currents.iter().map(String::as_str).collect::<Vec<_>>(),
        |id| rows.contains_key(id),
    );
    let retained_provider_id = (direct_provider_resolution == "preserved")
        .then(|| local.clone().or_else(|| currents.first().cloned()))
        .flatten();
    let mode_native_proven = missing_mode
        && local.as_ref().is_some_and(|id| !id.is_empty())
        && pointer_consistent == Some(true)
        && local
            .as_ref()
            .and_then(|id| rows.get(id))
            .is_some_and(|row| {
                inspect_candidate(Some(row)).is_ok_and(|facts| {
                    facts.status == "parsed"
                        && facts.marker == Some(false)
                        && facts.native_completion_match == Some(true)
                })
            });
    let can_choose_mode = mode_native_proven
        && pending.flatten().is_none()
        && preference.as_ref().is_none_or(String::is_empty);
    let mut keep_files_providers = Vec::new();
    if detached_mode_verified
        && preference.as_ref().is_none_or(String::is_empty)
        && pending == Some(None)
        && pointer_consistent != Some(true)
    {
        for row in rows.values() {
            if retained_provider_id
                .as_ref()
                .is_some_and(|id| id != &row.id)
            {
                continue;
            }
            let facts = inspect_candidate(Some(row))?;
            if facts.status == "parsed"
                && facts.marker == Some(false)
                && facts.native_completion_match == Some(true)
            {
                keep_files_providers.push(UpgradeProviderChoice {
                    id: row.id.clone(),
                    name: row.name.clone(),
                });
            }
        }
    }
    let can_choose_provider = !keep_files_providers.is_empty();
    if can_choose_mode {
        if let Some(row) = local.as_ref().and_then(|id| rows.get(id)) {
            keep_files_providers.push(UpgradeProviderChoice {
                id: row.id.clone(),
                name: row.name.clone(),
            });
        }
    }
    let mode_route_providers = if can_choose_mode {
        rows.values()
            .filter(|row| !row.id.is_empty())
            .map(|row| UpgradeProviderChoice {
                id: row.id.clone(),
                name: row.name.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let retained_provider_id = retained_provider_id
        .filter(|id| keep_files_providers.iter().any(|choice| &choice.id == id));
    // Salt private evidence with this session token. No row, path, credential
    // or journal is serialized into the public DTO or a persistent receipt.
    let revisions: Vec<_> = files.iter().map(|input| &input.revision).collect();
    let digest_evidence = |currents: &[String],
                           preference: &Option<String>,
                           local: &Option<String>,
                           projected_live: &Option<crate::mode::state::LiveState>,
                           projected_flags: (bool, bool),
                           projected_listener: &Option<ListenerFacts>|
     -> Result<String, AppError> {
        let bytes = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                token,
                app.as_str(),
                projected_live,
                local,
                currents,
                &rows,
                projected_flags,
                preference,
                &revisions,
                &identity,
                (*app == AppType::Codex).then_some((
                    settings.preserve_codex_official_auth_on_switch,
                    settings.unify_codex_session_history,
                )),
                &codex_endpoint,
                &order,
                projected_listener,
            ))
            .map_err(|source| AppError::JsonSerialize { source })?,
        );
        Ok(hex::encode(Sha256::digest(&*bytes)))
    };
    let revision = digest_evidence(&currents, &preference, &local, &live, flags, &listener)?;
    let listener_running_revision = listener
        .as_ref()
        .map(|rows| {
            let mut rows = rows.clone();
            for row in &mut rows {
                row.3 = true;
            }
            digest_evidence(&currents, &preference, &local, &live, flags, &Some(rows))
        })
        .transpose()?;
    let finalized_revision = target
        .map(|id| {
            let mut projected = live.clone();
            let mut projected_flags = flags;
            if let Some(target) = mode_target {
                projected
                    .as_mut()
                    .and_then(|live| live.apps.get_mut(app.as_str()))
                    .ok_or_else(source_changed)?
                    .set_mode_state(target.clone())
                    .map_err(|_| source_changed())?;
                projected_flags.0 = target.is_proxy();
            }
            digest_evidence(
                &[id.to_owned()],
                &Some(String::new()),
                &Some(id.to_owned()),
                &projected,
                projected_flags,
                &listener,
            )
        })
        .transpose()?;
    for file in &files {
        file.verify()?;
    }
    inspection::verify_unchanged(&path, &db_revision)?;
    let provider_digests = rows
        .iter()
        .map(|(id, row)| Database::provider_update_digest(row).map(|digest| (id.clone(), digest)))
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let mut settings_evidence =
        serde_json::to_value(&settings).map_err(|source| AppError::JsonSerialize { source })?;
    if let Some(fields) = settings_evidence.as_object_mut() {
        for key in [
            "currentProviderClaude",
            "currentProviderCodex",
            "currentProviderGemini",
            "currentProviderGrokbuild",
        ] {
            fields.remove(key);
        }
    }
    let recovery_facts = RecoveryFacts {
        live: live.clone(),
        local: local.clone(),
        currents: currents.clone(),
        rows: provider_digests.clone(),
        unowned_row_fields: serde_json::to_value(
            rows.iter()
                .map(|(id, row)| {
                    (
                        id,
                        (
                            row.in_failover_queue,
                            row.meta.as_ref().map(|meta| &meta.custom_endpoints),
                        ),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .map_err(|source| AppError::JsonSerialize { source })?,
        order,
        flags,
        preference: preference.clone(),
        settings: settings_evidence,
        codex_endpoint,
        listener,
    };
    Ok(AppCapture {
        view: UpgradeAppReview {
            app_type: app.as_str().into(),
            revision,
            saved_mode: mode.and_then(|mode| mode.mode),
            has_pending_operation: pending.map(|value| value.is_some()),
            pointer_consistent,
            live_status: client.status,
            stored_fields_match: client.stored_fields_match,
            can_recover_operation,
            default_action: "keep_files",
            default_takeover: false,
            can_complete_app,
            can_start_upgrade: false,
            direct_provider_resolution,
            retained_provider_id,
            keep_files_providers,
            can_choose_provider,
            can_choose_mode,
            mode_route_providers,
        },
        files,
        identity,
        flags,
        finalized_revision,
        provider_digests,
        native_pending,
        listener_running_revision,
        recovery_facts,
    })
}

/// The original app writer may use a freshly proven app after DB publication.
/// The checkpoint remains owned by startup and continues pausing both sync directions.
/// Caller holds the existing app lock and this database's original vault guard.
pub(crate) fn ensure_native_app_write_admitted(
    db: &Database,
    device: &DeviceStore,
    app: &AppType,
    vault: &RwLockReadGuard<'_, VaultContext>,
) -> Result<(), AppError> {
    match checkpoint::ensure_no_pending_checkpoint(device) {
        Ok(()) => return Ok(()),
        Err(AppError::Config(code)) if code == "upgrade.checkpoint_pending" => {}
        Err(error) => return Err(error),
    }
    let root = db.secret_session().root();
    if crate::config::get_app_config_dir() != root {
        return Err(source_changed());
    }
    {
        let conn = db.conn.lock()?;
        if conn.path().map(Path::new) != Some(root.join(crate::config::DB_FILE_NAME).as_path()) {
            return Err(source_changed());
        }
        crate::database::vault::check_identity(&conn, vault)?;
    }
    let checkpoint_path = device.root().join(checkpoint::FILE);
    let revision = inspection::file_revision(&checkpoint_path)?;
    let id = checkpoint::verified_database_id(root, device, vault)?
        .ok_or_else(|| AppError::Config("upgrade.migration_required".into()))?;
    let settings = crate::settings::read_native_app_settings_with_vault(
        app,
        &db.secrets,
        vault,
        checkpoint::MAX_BYTES,
    )?;
    let first =
        capture_app_with_state(device, &db.secrets, &id, app, vault, None, settings, false)?;
    if !first.view.can_complete_app {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    let settings = crate::settings::read_native_app_settings_with_vault(
        app,
        &db.secrets,
        vault,
        checkpoint::MAX_BYTES,
    )?;
    let second =
        capture_app_with_state(device, &db.secrets, &id, app, vault, None, settings, false)?;
    if !second.view.can_complete_app || first.view.revision != second.view.revision {
        return Err(source_changed());
    }
    {
        let conn = db.conn.lock()?;
        inspection::verify_connection_primary_identity(&conn, &second.identity)?;
    }
    if checkpoint::verified_database_id(root, device, vault)?.as_deref() != Some(id.as_str()) {
        return Err(source_changed());
    }
    inspection::verify_unchanged(&checkpoint_path, &revision)
}
