//! Private authenticated evidence held only by the existing StartupCoordinator.
//! Authentication and queries do not publish a runtime session or write sources.
use super::*;
use crate::secrets::{key_store::KeyStore, owned_file::DeviceFile, session::SecretSession};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

pub(super) struct ReviewedInput {
    pub(super) path: PathBuf,
    revision: inspection::SourceRevision,
}
impl ReviewedInput {
    fn capture(path: PathBuf) -> Result<Self, AppError> {
        let revision = inspection::file_revision(&path)?;
        Ok(Self { path, revision })
    }
    fn verify(&self) -> Result<(), AppError> {
        inspection::verify_unchanged(&self.path, &self.revision)
    }
}

struct CheckpointCancellation {
    id: String,
    bytes: Vec<u8>,
    completed: bool,
}

pub(crate) struct AuthenticatedUpgrade {
    root: PathBuf,
    device: DeviceStore,
    session: Arc<SecretSession>,
    token: String,
    cancellation: Option<CheckpointCancellation>,
    database_checkpoint: Option<String>,
    pub(super) inputs: Vec<ReviewedInput>,
}

fn source_changed() -> AppError {
    AppError::Config("upgrade.source_changed".into())
}

fn client_paths(
    root: &Path,
    device: &DeviceStore,
    settings: &ReviewedInput,
) -> Result<Vec<PathBuf>, AppError> {
    if crate::config::get_app_config_dir() != root {
        return Err(source_changed());
    }
    settings.verify()?;
    crate::settings::reload_settings()?;
    crate::settings::bootstrap_settings()?;
    settings.verify()?;
    let mut paths = BTreeSet::new();
    for app in [
        crate::app_config::AppType::Claude,
        crate::app_config::AppType::Codex,
        crate::app_config::AppType::Gemini,
        crate::app_config::AppType::GrokBuild,
    ] {
        for file in crate::mode::controller::files(&app)? {
            // Existing registered device inputs and their membership are already
            // captured by the checkpoint owner. settings.json is not one of them.
            if file
                .path
                .strip_prefix(device.root())
                .ok()
                .is_some_and(|relative| DeviceFile::registered(relative).is_ok())
            {
                continue;
            }
            paths.insert(file.path);
        }
    }
    paths.insert(settings.path.clone());
    let codex = crate::codex_config::get_codex_config_path();
    let codex_revision = inspection::file_revision(&codex)?;
    if let Some(bytes) = crate::config_file_io::read_regular_file(&codex, checkpoint::MAX_BYTES)
        .map_err(|e| AppError::io(&codex, e))?
    {
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| AppError::Config("upgrade.invalid_client_configuration".into()))?;
        if let Some(catalog) = crate::codex_config::resolve_cc_switch_catalog_path(
            text,
            &crate::codex_config::get_codex_config_dir(),
        ) {
            paths.insert(catalog);
        }
    }
    inspection::verify_unchanged(&codex, &codex_revision)?;
    settings.verify()?;
    if crate::config::get_app_config_dir() != root {
        return Err(source_changed());
    }
    Ok(paths.into_iter().collect())
}

impl AuthenticatedUpgrade {
    pub(crate) fn authenticate(
        root: &Path,
        device: &DeviceStore,
        inspected: &UpgradeInspection,
        store: &dyn KeyStore,
        password: Option<&str>,
    ) -> Result<Self, AppError> {
        if inspected.is_recovery_required() {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
        if inspected.future_version().is_some() && !inspected.is_database_resume_candidate() {
            return Err(AppError::Config("upgrade.future_version".into()));
        }
        let UpgradeInspection::Stable(stable) = inspected else {
            return Err(source_changed());
        };
        if !stable.is_database_resume_candidate()
            && stable.source_versions
                != Some(SchemaVersions {
                    upstream: database::UPSTREAM4_SOURCE_SCHEMA_VERSION,
                    loongport: database::loongport_schema::LOONGPORT_SCHEMA_VERSION,
                })
        {
            return Err(AppError::Config("upgrade.unsupported_source".into()));
        }
        if stable.device.root() != device.root() || crate::config::get_app_config_dir() != root {
            return Err(source_changed());
        }
        let resumed = stable.is_database_resume_candidate();
        if resumed {
            stable.verify_resume_checkpoint(root)?;
        } else {
            inspected.verify_unchanged(root)?;
        }
        let settings = if resumed {
            None
        } else {
            Some(ReviewedInput::capture(crate::settings::settings_path())?)
        };
        let vault = session::authenticate_existing(root, store, password)?;
        let session = SecretSession::from_context(root.to_path_buf(), vault);
        if session.migration_pending()? || session.legacy_json_pending()? {
            return Err(AppError::Config("secret.migration_required".into()));
        }
        if resumed {
            let vault = session.read()?;
            let id = checkpoint::verified_database_id(root, device, &vault)?
                .ok_or_else(source_changed)?;
            stable.verify_resume_checkpoint(root)?;
            drop(vault);
            let result = Self {
                root: root.to_path_buf(),
                device: device.clone(),
                session,
                token: uuid::Uuid::new_v4().to_string(),
                cancellation: None,
                database_checkpoint: Some(id),
                inputs: Vec::new(),
            };
            result.verify(inspected)?;
            return Ok(result);
        }
        let settings = settings.ok_or_else(source_changed)?;
        {
            let vault = session.read()?;
            inspected.validate_device_state(&vault)?;
            inspected.validate_database(root, &vault)?;
            if let Some(bytes) =
                crate::config_file_io::read_regular_file(&settings.path, checkpoint::MAX_BYTES)
                    .map_err(|e| AppError::io(&settings.path, e))?
            {
                // Validate protected settings and pointer input without publishing
                // an unlocked SettingsStore or retaining unrelated sync credentials.
                crate::settings::decode_settings_with_vault(&bytes, &vault)?;
            }
        }
        settings.verify()?;
        let paths = client_paths(root, device, &settings)?;
        let inputs = paths
            .iter()
            .cloned()
            .map(ReviewedInput::capture)
            .collect::<Result<Vec<_>, _>>()?;
        if client_paths(root, device, &settings)? != paths {
            return Err(source_changed());
        }
        inspected.verify_unchanged(root)?;
        let result = Self {
            root: root.to_path_buf(),
            device: device.clone(),
            session,
            token: uuid::Uuid::new_v4().to_string(),
            cancellation: None,
            database_checkpoint: None,
            inputs,
        };
        result.verify(inspected)?;
        Ok(result)
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.inputs.iter().map(|input| input.path.clone()).collect()
    }

    fn verify(&self, inspected: &UpgradeInspection) -> Result<(), AppError> {
        if crate::config::get_app_config_dir() != self.root {
            return Err(source_changed());
        }
        if let Some(expected) = &self.database_checkpoint {
            let UpgradeInspection::Stable(stable) = inspected else {
                return Err(source_changed());
            };
            stable.verify_resume_checkpoint(&self.root)?;
            let vault = self.session.read()?;
            if checkpoint::verified_database_id(&self.root, &self.device, &vault)?.as_ref()
                != Some(expected)
            {
                return Err(source_changed());
            }
            return stable.verify_resume_checkpoint(&self.root);
        }
        inspected.verify_unchanged(&self.root)?;
        self.verify_inputs()?;
        inspected.verify_unchanged(&self.root)
    }

    fn verify_inputs(&self) -> Result<(), AppError> {
        if crate::config::get_app_config_dir() != self.root {
            return Err(source_changed());
        }
        let settings = self
            .inputs
            .iter()
            .find(|input| input.path == crate::settings::settings_path())
            .ok_or_else(source_changed)?;
        for input in &self.inputs {
            input.verify()?;
        }
        if client_paths(&self.root, &self.device, settings)? != self.paths() {
            return Err(source_changed());
        }
        for input in &self.inputs {
            input.verify()?;
        }
        Ok(())
    }

    pub(crate) fn prepare_checkpoint(
        &mut self,
        inspected: &mut UpgradeInspection,
        token: &str,
    ) -> Result<StartupUpgradeView, AppError> {
        self.prepare_checkpoint_with_hook(inspected, token, &mut |_| Ok(()))
    }

    pub(super) fn prepare_checkpoint_with_hook(
        &mut self,
        inspected: &mut UpgradeInspection,
        token: &str,
        hook: &mut dyn FnMut(checkpoint::Boundary) -> Result<(), AppError>,
    ) -> Result<StartupUpgradeView, AppError> {
        if token != self.token {
            return Err(source_changed());
        }
        if self.cancellation.is_some() || self.database_checkpoint.is_some() {
            return Err(source_changed());
        }
        let view = self.view(inspected)?;
        if view.checkpoint_id.is_some() {
            return Ok(view);
        }
        let directory_missing =
            !crate::secrets::files::device_directory_exists(self.device.root())?;
        let root = self.root.clone();
        let device = self.device.clone();
        let paths = self.paths();
        let session = self.session.clone();
        let vault = session.read()?;
        let creation =
            checkpoint::create_with_hook(&root, &device, &vault, &paths, &mut |boundary| {
                hook(boundary)?;
                if boundary == checkpoint::Boundary::CaptureReady && directory_missing {
                    for input in &mut self.inputs {
                        input.revision = inspection::acknowledge_checkpoint_directory(
                            &input.path,
                            &input.revision,
                            device.root(),
                        )?;
                    }
                }
                if boundary != checkpoint::Boundary::Published {
                    self.verify(inspected)?;
                }
                Ok(())
            });
        // A lost response may leave a complete artifact. Authenticate that exact
        // output and inventory before acknowledging only its addition to startup.
        let (id, output) =
            match checkpoint::verified_checkpoint_for_clients(&root, &device, &vault, &paths) {
                Ok(output) => output,
                Err(error) => return Err(creation.err().unwrap_or(error)),
            };
        if creation.as_ref().is_ok_and(|created| created != &id) {
            return Err(source_changed());
        }
        for input in &self.inputs {
            input.verify()?;
        }
        let UpgradeInspection::Stable(stable) = inspected else {
            return Err(source_changed());
        };
        stable.acknowledge_checkpoint(&root, &output)?;
        creation?;
        self.view(inspected)
    }

    pub(crate) fn cancel_checkpoint(
        &mut self,
        inspected: &mut UpgradeInspection,
        token: &str,
        id: &str,
    ) -> Result<StartupUpgradeView, AppError> {
        self.cancel_checkpoint_with_hook(inspected, token, id, &mut |_| Ok(()))
    }

    pub(super) fn cancel_checkpoint_with_hook(
        &mut self,
        inspected: &mut UpgradeInspection,
        token: &str,
        id: &str,
        hook: &mut dyn FnMut(checkpoint::CancellationBoundary) -> Result<(), AppError>,
    ) -> Result<StartupUpgradeView, AppError> {
        if token != self.token || self.database_checkpoint.is_some() {
            return Err(source_changed());
        }
        if let Some(cancellation) = &self.cancellation {
            if cancellation.id != id {
                return Err(source_changed());
            }
            if cancellation.completed {
                return self.view(inspected);
            }
        } else {
            self.verify(inspected)?;
            let vault = self.session.read()?;
            let (actual, bytes) = checkpoint::verified_checkpoint_for_clients(
                &self.root,
                &self.device,
                &vault,
                &self.paths(),
            )?;
            if actual != id {
                return Err(source_changed());
            }
            self.cancellation = Some(CheckpointCancellation {
                id: actual,
                bytes,
                completed: false,
            });
        }
        self.verify_cancellation(inspected)?;
        let cancellation = self.cancellation.as_ref().ok_or_else(source_changed)?;
        let vault = self.session.read()?;
        checkpoint::cancel_with_hook(
            &self.root,
            &self.device,
            &vault,
            &self.paths(),
            (id, &cancellation.bytes),
            hook,
        )?;
        self.verify_inputs()?;
        let UpgradeInspection::Stable(stable) = inspected else {
            return Err(source_changed());
        };
        stable.acknowledge_checkpoint_removed(&self.root, &cancellation.bytes)?;
        self.cancellation
            .as_mut()
            .ok_or_else(source_changed)?
            .completed = true;
        self.view(inspected)
    }

    fn verify_cancellation(&self, inspected: &UpgradeInspection) -> Result<(), AppError> {
        let cancellation = self.cancellation.as_ref().ok_or_else(source_changed)?;
        if cancellation.completed {
            return self.verify(inspected);
        }
        let UpgradeInspection::Stable(stable) = inspected else {
            return Err(source_changed());
        };
        if crate::config_file_io::read_regular_file(
            &self.device.root().join(checkpoint::FILE),
            checkpoint::MAX_BYTES,
        )
        .map_err(|e| AppError::io(self.device.root().join(checkpoint::FILE), e))?
        .is_some()
        {
            self.verify(inspected)?;
        } else {
            stable.verify_checkpoint_removed(&self.root, &cancellation.bytes)?;
        }
        self.verify_inputs()
    }

    pub(crate) fn stage_review(
        &self,
        inspected: &UpgradeInspection,
        token: &str,
    ) -> Result<StagedUpgradeReview, AppError> {
        if token != self.token || self.cancellation.is_some() || self.database_checkpoint.is_some()
        {
            return Err(source_changed());
        }
        let current = self.view(inspected)?;
        let id = current
            .checkpoint_id
            .ok_or_else(|| AppError::Config("upgrade.checkpoint_required".into()))?;
        let source_versions = current.source_versions.ok_or_else(source_changed)?;
        let vault = self.session.read()?;
        let settings_path = crate::settings::settings_path();
        let mut settings =
            match crate::config_file_io::read_regular_file(&settings_path, checkpoint::MAX_BYTES)
                .map_err(|error| AppError::io(&settings_path, error))?
            {
                Some(bytes) => crate::settings::decode_settings_with_vault(&bytes, &vault)?,
                None => crate::settings::AppSettings::default(),
            };
        let live = crate::mode::state::load(&self.device, &vault)?;
        let mut files = live_review::BoundFiles::new();
        for input in &self.inputs {
            input.verify()?;
            let bytes =
                crate::config_file_io::read_regular_file(&input.path, checkpoint::MAX_BYTES)
                    .map_err(|error| AppError::io(&input.path, error))?;
            input.verify()?;
            files.insert(input.path.clone(), bytes.map(zeroize::Zeroizing::new));
        }
        let config_path = crate::codex_config::get_codex_config_path();
        let managed_catalog = files
            .get(&config_path)
            .and_then(|bytes| bytes.as_ref())
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| {
                crate::codex_config::resolve_cc_switch_catalog_path(
                    text,
                    &crate::codex_config::get_codex_config_dir(),
                )
            });
        let managed_catalog_present = managed_catalog
            .map(|path| {
                files
                    .get(&path)
                    .map(Option::is_some)
                    .ok_or_else(source_changed)
            })
            .transpose()?;
        let (staged, apps) = checkpoint::stage_with_source_review(
            &self.root,
            &self.device,
            &vault,
            &id,
            |source| {
                staged_review::source_facts(
                    source,
                    &vault,
                    &mut settings,
                    &live,
                    &files,
                    managed_catalog_present,
                )
            },
        )?;
        let staged_versions = SchemaVersions {
            upstream: Database::get_user_version(&staged)?,
            loongport: database::loongport_schema::read_stored_version(&staged)?,
        };
        drop(staged);
        drop(vault);
        self.verify(inspected)?;
        Ok(StagedUpgradeReview {
            checkpoint_id: id,
            source_versions,
            staged_versions,
            apps,
            can_start_upgrade: false,
        })
    }

    pub(crate) fn view(
        &self,
        inspected: &UpgradeInspection,
    ) -> Result<StartupUpgradeView, AppError> {
        if let Some(id) = &self.database_checkpoint {
            self.verify(inspected)?;
            let mut view = StartupUpgradeView::blocked("database_verified");
            if let UpgradeInspection::Stable(stable) = inspected {
                view.source_versions = stable.source_versions;
            }
            view.checkpoint_present = true;
            view.checkpoint_id = Some(id.clone());
            view.review_token = Some(self.token.clone());
            return Ok(view);
        }
        if let Some(cancellation) = &self.cancellation {
            self.verify_cancellation(inspected)?;
            let mut view = StartupUpgradeView::blocked(if cancellation.completed {
                "cancelled"
            } else {
                "cancellation_requires_verification"
            });
            if let UpgradeInspection::Stable(stable) = inspected {
                view.source_versions = stable.source_versions;
            }
            view.review_token = Some(self.token.clone());
            if !cancellation.completed {
                view.checkpoint_id = Some(cancellation.id.clone());
                view.checkpoint_present = crate::config_file_io::read_regular_file(
                    &self.device.root().join(checkpoint::FILE),
                    checkpoint::MAX_BYTES,
                )
                .map_err(|e| AppError::io(self.device.root().join(checkpoint::FILE), e))?
                .is_some();
            }
            return Ok(view);
        }
        self.verify(inspected)?;
        let mut view = inspected.upgrade_view(&self.root)?;
        if view.checkpoint_present {
            let vault = self.session.read()?;
            view.checkpoint_id = Some(checkpoint::existing_id_for_clients(
                &self.root,
                &self.device,
                &vault,
                &self.paths(),
            )?);
        }
        view.status = if view.checkpoint_id.is_some() {
            "checkpoint_ready"
        } else {
            "ready_to_check"
        };
        view.requires_authentication = false;
        view.can_authenticate = false;
        view.can_check_and_backup = view.checkpoint_id.is_none();
        view.can_start_upgrade = false;
        view.review_token = Some(self.token.clone());
        Ok(view)
    }
}
