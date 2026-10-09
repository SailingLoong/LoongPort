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
}

struct AppCapture {
    view: UpgradeAppReview,
    files: Vec<ReviewedInput>,
    live: Option<crate::mode::state::LiveState>,
    identity: inspection::DatabaseIdentity,
    finalized_revision: Option<String>,
}

impl AuthenticatedUpgrade {
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
        let mut settings = crate::settings::read_upgrade_settings_with_vault(
            app,
            &self.session,
            vault,
            checkpoint::MAX_BYTES,
        )?;
        let local = crate::settings::current_provider_from_settings(&mut settings, app);
        // Cleanup borrows the snapshot already pinned by the original state
        // lock; ordinary queries capture one authenticated shared envelope.
        let live = match pinned_state {
            Some(live) => Some(live.clone()),
            None => crate::mode::state::read_review_snapshot(&self.device, vault)?
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
        let path = self.root.join(crate::config::DB_FILE_NAME);
        let captured = inspection::capture(&path)?.ok_or_else(source_changed)?;
        let db_revision = captured.revision;
        let identity = db_revision.primary_identity()?;
        let db = Database::from_connection(captured.image, self.session.clone());
        let (rows, currents) = {
            let conn = db.conn.lock()?;
            let rows = Database::get_all_providers_on_connection(&conn, vault, app.as_str())?;
            let mut statement = conn.prepare(
                "SELECT id FROM providers WHERE app_type=?1 AND is_current<>0 ORDER BY id",
            )?;
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
        let target = pending
            .flatten()
            .and_then(|pending| operation::published_pointer_target(app.as_str(), pending).ok());
        let compatible = live
            .as_ref()
            .and_then(|live| live.apps.get(app.as_str()))
            .is_some_and(|entry| crate::mode::state::validate_app_for_update(entry).is_ok());
        let can_recover_operation = compatible
            && mode.as_ref().is_some_and(|mode| {
                !mode.attached
                    && (mode.mode == Some(Mode::Direct)
                        || mode
                            .proxy_route
                            .as_ref()
                            .is_some_and(|id| rows.contains_key(id)))
            })
            && target.is_some_and(|id| local.as_deref() == Some(id) && rows.contains_key(id))
            && currents.len() <= 1;
        let mut bound = live_review::BoundFiles::new();
        let mut files = Vec::new();
        for file in crate::mode::controller::files(app)? {
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
        let candidate = local
            .as_ref()
            .filter(|_| {
                pending == Some(None)
                    && pointer_consistent == Some(true)
                    && mode
                        .as_ref()
                        .is_some_and(|mode| mode.mode == Some(Mode::Direct) && !mode.attached)
            })
            .and_then(|id| rows.get(id));
        let client = live_review::inspect(app, &bound, candidate, catalog_present)?;
        // Salt private evidence with this session token. No row, path, credential
        // or journal is serialized into the public DTO or a persistent receipt.
        let revisions: Vec<_> = files.iter().map(|input| &input.revision).collect();
        let digest_evidence =
            |currents: &[String], preference: &Option<String>| -> Result<String, AppError> {
                let bytes = zeroize::Zeroizing::new(
                    serde_json::to_vec(&(
                        &self.token,
                        app.as_str(),
                        &live,
                        &local,
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
        let revision = digest_evidence(&currents, &preference)?;
        let finalized_revision = target
            .map(|id| digest_evidence(&[id.to_owned()], &Some(String::new())))
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
                can_complete_app: false,
                can_start_upgrade: false,
            },
            files,
            live,
            identity,
            finalized_revision,
        })
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

    pub(crate) fn recover_app(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
        app: &AppType,
        revision: &str,
    ) -> Result<UpgradeAppReview, AppError> {
        let guard = crate::live::engine::lock_app(app.as_str());
        let captured = self.review_app_locked(inspected, token, app)?;
        if captured.view.revision != revision || !captured.view.can_recover_operation {
            return Err(source_changed());
        }
        {
            let vault = self.session.read()?;
            self.verify_app_checkpoint_pinned(inspected, &vault)?;
            let path = self.root.join(crate::config::DB_FILE_NAME);
            let db = Database::from_connection(
                rusqlite::Connection::open_with_flags(
                    &path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                )?,
                self.session.clone(),
            );
            inspection::verify_primary_identity(&path, &captured.identity)?;
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
                let mut settings = crate::settings::read_upgrade_settings_with_vault(
                    app,
                    &self.session,
                    &vault,
                    checkpoint::MAX_BYTES,
                )?;
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
                &db,
                &self.device,
                &vault,
                &guard,
                app,
                &operation::PointerRecoveryChecks {
                    before_target: &before,
                    pointer: &pointer,
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
