//! Monotonic first-upgrade conversion. The authenticated completion marker is
//! published only after every credential owner has opened successfully.

use super::{
    files,
    session::{write_durable, SecretSession},
};
use crate::error::AppError;

pub(crate) fn prepare_files(session: &SecretSession, legacy_allowed: bool) -> Result<(), AppError> {
    let vault = session.read()?;
    let settings_path = crate::settings::settings_path();
    let settings = match std::fs::read(&settings_path) {
        Ok(bytes) => {
            let ciphertext = match crate::settings::decode_settings_with_vault(&bytes, &vault) {
                Ok(_) => bytes.clone(),
                Err(_) if legacy_allowed => {
                    crate::settings::encrypt_legacy_settings_with_vault(&bytes, &vault)?
                }
                Err(error) => return Err(error),
            };
            Some((bytes, ciphertext))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(AppError::io(&settings_path, e)),
    };
    let plans = files::stage_owned_files_with_vault(session.root(), &vault, legacy_allowed)?;
    for plan in plans {
        let current =
            std::fs::read(&plan.destination).map_err(|e| AppError::io(&plan.destination, e))?;
        if current == plan.ciphertext {
            continue;
        }
        let recovery = session
            .root()
            .join("backups/vault-recovery")
            .join(plan.file.relative_path());
        super::session::ensure_owned_directory(
            session.root(),
            recovery
                .parent()
                .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?,
        )?;
        write_durable(&recovery, &plan.ciphertext)?;
        write_durable(&plan.destination, &plan.ciphertext)?;
    }
    if let Some((original, ciphertext)) = settings {
        if original != ciphertext {
            super::session::ensure_owned_directory(
                session.root(),
                &session.root().join("backups/vault-recovery"),
            )?;
            write_durable(
                &session.root().join("backups/vault-recovery/settings.json"),
                &ciphertext,
            )?;
            write_durable(&settings_path, &ciphertext)?;
        }
    }
    crate::database::vault::migrate_backups(session.root(), &vault, legacy_allowed)
}
