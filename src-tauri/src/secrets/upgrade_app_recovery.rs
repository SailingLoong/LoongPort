//! App-local recovery inside the original authenticated startup owner. A DB
//! checkpoint is not app completion; these queries never authorize runtime.
use super::*;
use crate::{
    app_config::AppType,
    mode::{current, operation, state::Mode},
};
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
}

struct AppCapture {
    view: UpgradeAppReview,
    files: Vec<ReviewedInput>,
    live: Option<crate::mode::state::LiveState>,
    identity: inspection::DatabaseIdentity,
    finalized_revision: Option<String>,
    provider_digests: std::collections::BTreeMap<String, String>,
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
        let settings = self.app_settings(app, vault)?;
        capture_app_with_state(
            &self.device,
            &self.session,
            &self.token,
            app,
            vault,
            pinned_state,
            settings,
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
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
        let guard = crate::live::engine::lock_app(app.as_str());
        if !std::sync::Arc::ptr_eq(&state.db.secrets, &self.session) {
            return Err(source_changed());
        }
        let captured = self.review_app_locked(inspected, token, app)?;
        if captured.view.revision != revision
            || !captured.view.can_choose_provider
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
            let target = crate::mode::state::PendingTarget::pointer(Some(provider_id.to_owned()));
            operation::run(
                &self.device,
                &vault,
                &guard,
                crate::mode::state::op::APPLY,
                &changes,
                target.clone(),
                &|actual| {
                    if actual != &target {
                        return Err(source_changed());
                    }
                    self.verify_app_checkpoint_pinned(inspected, &vault)?;
                    inspection::verify_primary_identity(&path, &captured.identity)?;
                    operation::failpoint::hit("upgrade:provider_target")?;
                    let verify_provider = || {
                        let row = state
                            .db
                            .get_provider_by_id_with_vault(
                                provider_id,
                                app.as_str(),
                                &self.session,
                                &vault,
                            )?
                            .ok_or_else(source_changed)?;
                        if captured.provider_digests.get(provider_id)
                            != Some(&Database::provider_update_digest(&row)?)
                        {
                            return Err(source_changed());
                        }
                        for file in &captured.files {
                            file.verify()?;
                        }
                        Ok(())
                    };
                    verify_provider()?;
                    operation::commit_target(
                        &state.db,
                        &self.session,
                        &self.device,
                        &vault,
                        app,
                        actual,
                    )?;
                    verify_provider()?;
                    self.verify_app_checkpoint_pinned(inspected, &vault)
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
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()))
        });
        if runtime.is_some_and(|state| !std::sync::Arc::ptr_eq(&state.db.secrets, &self.session)) {
            return Err(source_changed());
        }
        let guard = crate::live::engine::lock_app(app.as_str());
        let captured = self.review_app_locked(inspected, token, app)?;
        if captured.view.revision != revision || !captured.view.can_recover_operation {
            return Err(source_changed());
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
                        if Some(live) != captured.live.as_ref() {
                            return Err(source_changed());
                        }
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
fn capture_app_with_state(
    device: &DeviceStore,
    session: &std::sync::Arc<crate::secrets::session::SecretSession>,
    token: &str,
    app: &AppType,
    vault: &RwLockReadGuard<'_, VaultContext>,
    pinned_state: Option<&crate::mode::state::LiveState>,
    mut settings: crate::settings::AppSettings,
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
    let flags = db.get_proxy_flags_checked(app.as_str())?;
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
        .is_some_and(|entry| crate::mode::state::validate_app_for_update(entry).is_ok());
    let mut bound = live_review::BoundFiles::new();
    let mut files = Vec::new();
    for file in admitted {
        let input = ReviewedInput::capture(file.path.clone())?;
        let bytes = crate::config_file_io::read_regular_file(&file.path, checkpoint::MAX_BYTES)
            .map_err(|error| AppError::io(&file.path, error))?;
        input.verify()?;
        bound.insert(file.path, bytes.map(zeroize::Zeroizing::new));
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
    let client = live_review::inspect(app, &bound, candidate, catalog_present)?;
    // Only a complete native owned-field proof grants admission. Codex and
    // Grok remain unresolved until their route/catalog or cleanup is proven.
    let target_native_proven = if let Some(row) = target.and_then(|id| rows.get(id)) {
        let proof = live_review::inspect(app, &bound, Some(row), catalog_present)?;
        proof.status == "parsed"
            && proof.marker == Some(false)
            && proof.native_completion_match == Some(true)
    } else {
        false
    };
    let forward_pointer_proven = target_native_proven
        && mode
            .as_ref()
            .is_some_and(|mode| mode.mode == Some(Mode::Direct))
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
    let can_recover_operation = compatible
        && mode.as_ref().is_some_and(|mode| {
            !mode.attached
                && (mode.mode == Some(Mode::Direct)
                    || mode
                        .proxy_route
                        .as_ref()
                        .is_some_and(|id| rows.contains_key(id)))
        })
        && target.is_some_and(|id| {
            rows.contains_key(id)
                && ((local.as_deref() == Some(id) && (!keep_files_pending || target_native_proven))
                    || forward_pointer_proven)
        })
        && (currents.len() <= 1 || forward_pointer_proven);
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
    let mut keep_files_providers = Vec::new();
    if compatible
        && preference.as_ref().is_none_or(String::is_empty)
        && pending == Some(None)
        && mode
            .as_ref()
            .is_some_and(|mode| mode.mode == Some(Mode::Direct) && !mode.attached)
        && pointer_consistent != Some(true)
    {
        for row in rows.values() {
            if retained_provider_id
                .as_ref()
                .is_some_and(|id| id != &row.id)
            {
                continue;
            }
            let facts = live_review::inspect(app, &bound, Some(row), catalog_present)?;
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
    let retained_provider_id = retained_provider_id
        .filter(|id| keep_files_providers.iter().any(|choice| &choice.id == id));
    // Salt private evidence with this session token. No row, path, credential
    // or journal is serialized into the public DTO or a persistent receipt.
    let revisions: Vec<_> = files.iter().map(|input| &input.revision).collect();
    let digest_evidence = |currents: &[String],
                           preference: &Option<String>,
                           local: &Option<String>|
     -> Result<String, AppError> {
        let bytes = zeroize::Zeroizing::new(
            serde_json::to_vec(&(
                token,
                app.as_str(),
                &live,
                local,
                currents,
                &rows,
                flags,
                preference,
                &revisions,
                &identity,
            ))
            .map_err(|source| AppError::JsonSerialize { source })?,
        );
        Ok(hex::encode(Sha256::digest(&*bytes)))
    };
    let revision = digest_evidence(&currents, &preference, &local)?;
    let finalized_revision = target
        .map(|id| digest_evidence(&[id.to_owned()], &Some(String::new()), &Some(id.to_owned())))
        .transpose()?;
    for file in &files {
        file.verify()?;
    }
    inspection::verify_unchanged(&path, &db_revision)?;
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
        },
        files,
        live,
        identity,
        finalized_revision,
        provider_digests: rows
            .iter()
            .map(|(id, row)| {
                Database::provider_update_digest(row).map(|digest| (id.clone(), digest))
            })
            .collect::<Result<_, _>>()?,
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
    let first = capture_app_with_state(device, &db.secrets, &id, app, vault, None, settings)?;
    if !first.view.can_complete_app {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    let settings = crate::settings::read_native_app_settings_with_vault(
        app,
        &db.secrets,
        vault,
        checkpoint::MAX_BYTES,
    )?;
    let second = capture_app_with_state(device, &db.secrets, &id, app, vault, None, settings)?;
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
