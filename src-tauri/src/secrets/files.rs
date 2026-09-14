//! Registered credential files and their persistence boundary.
use std::path::PathBuf;
use zeroize::Zeroizing;

use super::{session::SecretSession, VaultContext};
use crate::error::AppError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialFile {
    Copilot,
    Codex,
    Xai,
}

pub(crate) const AUTH_FILES: [CredentialFile; 3] = [
    CredentialFile::Copilot,
    CredentialFile::Codex,
    CredentialFile::Xai,
];

impl CredentialFile {
    pub(crate) const fn filename(self) -> &'static str {
        match self {
            Self::Copilot => "copilot_auth.json",
            Self::Codex => "codex_oauth_auth.json",
            Self::Xai => "xai_oauth_auth.json",
        }
    }

    pub(crate) fn path(self, session: &SecretSession) -> PathBuf {
        session.root().join(self.filename())
    }

    pub(crate) fn encode(
        self,
        vault: &VaultContext,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        OwnedFile::registered(self.filename())?.encode(vault, plaintext)
    }

    pub(crate) fn decode(
        self,
        vault: &VaultContext,
        bytes: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        OwnedFile::registered(self.filename())?.decode(vault, bytes)
    }

    pub(crate) fn read(
        self,
        session: &SecretSession,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        let vault = session.read()?;
        let path = self.path(session);
        match std::fs::read(&path) {
            Ok(bytes) => self.decode(&vault, &bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AppError::io(&path, error)),
        }
    }

    pub(crate) fn write(self, session: &SecretSession, plaintext: &[u8]) -> Result<(), AppError> {
        // Keep the same key generation through encryption, atomic publication and sync.
        let vault = session.read()?;
        let encrypted = self.encode(&vault, plaintext)?;
        let path = self.path(session);
        super::session::ensure_owned_directory(
            session.root(),
            path.parent()
                .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?,
        )?;
        super::session::write_durable(&path, &encrypted)
    }

    pub(crate) fn remove(self, session: &SecretSession) -> Result<(), AppError> {
        let _vault = session.read()?;
        let path = self.path(session);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io(&path, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_removes_plaintext_and_binds_the_registered_file() {
        let vault = VaultContext::generate().unwrap();
        for file in AUTH_FILES {
            let encrypted = file
                .encode(&vault, br#"{"token":"credential-canary"}"#)
                .unwrap();
            assert!(!String::from_utf8_lossy(&encrypted).contains("credential-canary"));
            assert_eq!(
                &*file.decode(&vault, &encrypted).unwrap(),
                br#"{"token":"credential-canary"}"#
            );
            for other in AUTH_FILES.into_iter().filter(|other| *other != file) {
                assert!(other.decode(&vault, &encrypted).is_err());
            }
        }
    }

    #[test]
    fn file_store_reads_only_authenticated_data_and_writes_private_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let session = SecretSession::from_context(
            dir.path().to_path_buf(),
            VaultContext::generate().unwrap(),
        );
        let file = CredentialFile::Codex;
        assert!(file.read(&session).unwrap().is_none());
        file.write(&session, b"credential-canary").unwrap();
        let raw = std::fs::read(file.path(&session)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("credential-canary"));
        assert_eq!(
            &*file.read(&session).unwrap().unwrap(),
            b"credential-canary"
        );
        let unrelated = SecretSession::from_context(
            dir.path().to_path_buf(),
            VaultContext::generate().unwrap(),
        );
        assert!(file.read(&unrelated).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(file.path(&session))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::write(file.path(&session), b"plaintext-canary").unwrap();
        assert!(file.read(&session).is_err());
        file.remove(&session).unwrap();
        assert!(file.read(&session).unwrap().is_none());
    }
}

/// A credential document created and owned by the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnedFile {
    relative: PathBuf,
}

pub(crate) struct OwnedFileMigration {
    pub file: OwnedFile,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub ciphertext: Vec<u8>,
}

const LEGACY_CONFIG_FILES: [&str; 3] = ["config.json", "config.json.bak", "config.json.migrated"];
const BACKUP_PATTERNS: [(&str, &str, &str); 6] = [
    ("backups", "backup_", ".json"),
    ("backups", "codex-auth-", ".json"),
    ("backups", "env-backup-", ".json"),
    ("backups/hermes", "hermes_", ".yaml"),
    ("backups/hermes", "hermes_", ".yml"),
    ("backups/openclaw", "openclaw_", ".json5"),
];

impl OwnedFile {
    pub(crate) fn registered(relative: impl AsRef<std::path::Path>) -> Result<Self, AppError> {
        let relative = relative.as_ref();
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(AppError::Config("secret.unregistered_file".into()));
        }
        let name = relative.file_name().and_then(|v| v.to_str()).unwrap_or("");
        let parent = relative
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
            .to_string_lossy()
            .replace('\\', "/");
        let valid = (parent.is_empty()
            && (LEGACY_CONFIG_FILES.contains(&name)
                || AUTH_FILES.iter().any(|file| file.filename() == name)))
            || BACKUP_PATTERNS.iter().any(|(directory, prefix, suffix)| {
                parent == *directory && matches_backup_name(name, prefix, suffix)
            });
        if !valid {
            return Err(AppError::Config("secret.unregistered_file".into()));
        }
        Ok(Self {
            relative: relative.to_path_buf(),
        })
    }

    pub(crate) fn at_path(
        session: &SecretSession,
        path: &std::path::Path,
    ) -> Result<Self, AppError> {
        Self::registered(
            path.strip_prefix(session.root())
                .map_err(|_| AppError::Config("secret.unregistered_file".into()))?,
        )
    }

    pub(crate) fn relative_path(&self) -> &std::path::Path {
        &self.relative
    }
    pub(crate) fn path(&self, session: &SecretSession) -> PathBuf {
        session.root().join(&self.relative)
    }

    pub(crate) fn encode(
        &self,
        vault: &VaultContext,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        let identity = self.relative.to_string_lossy().replace('\\', "/");
        vault
            .seal(&["file", &identity, "content"], plaintext)
            .map(String::into_bytes)
            .map_err(super::inventory::secret_error)
    }

    pub(crate) fn decode(
        &self,
        vault: &VaultContext,
        bytes: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        let identity = self.relative.to_string_lossy().replace('\\', "/");
        let ciphertext = std::str::from_utf8(bytes)
            .map_err(|_| AppError::Config("secret.invalid_envelope".into()))?;
        vault
            .open(&["file", &identity, "content"], ciphertext)
            .map_err(super::inventory::secret_error)
    }

    pub(crate) fn read(&self, session: &SecretSession) -> Result<Zeroizing<Vec<u8>>, AppError> {
        let vault = session.read()?;
        let path = self.path(session);
        let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
        self.decode(&vault, &bytes)
    }

    pub(crate) fn write(&self, session: &SecretSession, plaintext: &[u8]) -> Result<(), AppError> {
        let vault = session.read()?;
        let bytes = self.encode(&vault, plaintext)?;
        let path = self.path(session);
        super::session::ensure_owned_directory(
            session.root(),
            path.parent()
                .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?,
        )?;
        super::session::write_durable(&path, &bytes)
    }
}

fn matches_backup_name(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|v| v.strip_suffix(suffix))
        .is_some_and(|stamp| {
            !stamp.is_empty() && stamp.bytes().all(|v| v.is_ascii_digit() || v == b'_')
        })
}

pub(crate) fn stage_owned_files_with_vault(
    root: &std::path::Path,
    vault: &VaultContext,
    legacy_allowed: bool,
) -> Result<Vec<OwnedFileMigration>, AppError> {
    let mut paths = Vec::new();
    let directories = std::iter::once("")
        .chain(BACKUP_PATTERNS.iter().map(|entry| entry.0))
        .flat_map(|directory| {
            [
                PathBuf::from(directory),
                PathBuf::from("backups/vault-recovery").join(directory),
            ]
        })
        .collect::<std::collections::BTreeSet<_>>();
    for directory in directories {
        let path = root.join(&directory);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(AppError::io(&path, error)),
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(AppError::Config("secret.invalid_storage_path".into()))
            }
            Ok(_) => {}
        }
        for entry in std::fs::read_dir(&path).map_err(|e| AppError::io(&path, e))? {
            let entry = entry.map_err(|e| AppError::io(&path, e))?;
            let relative = directory.join(entry.file_name());
            let identity = relative
                .strip_prefix("backups/vault-recovery")
                .unwrap_or(&relative);
            if let Ok(file) = OwnedFile::registered(identity) {
                if !entry
                    .file_type()
                    .map_err(|e| AppError::io(entry.path(), e))?
                    .is_file()
                {
                    return Err(AppError::Config("secret.invalid_storage_path".into()));
                }
                paths.push((file, entry.path()));
            }
        }
    }
    paths.sort_by(|a, b| a.1.cmp(&b.1));
    let mut plans = Vec::new();
    for (file, source) in paths {
        let bytes = Zeroizing::new(std::fs::read(&source).map_err(|e| AppError::io(&source, e))?);
        let recovery = source.starts_with(root.join("backups/vault-recovery"));
        let ciphertext = if bytes.starts_with(b"lpenc") || !legacy_allowed || recovery {
            file.decode(vault, &bytes)?;
            bytes.to_vec()
        } else {
            validate_legacy_document(&file, &bytes)?;
            file.encode(vault, &bytes)?
        };
        plans.push(OwnedFileMigration {
            file,
            destination: source.clone(),
            source,
            ciphertext,
        });
    }
    Ok(plans)
}

fn validate_legacy_document(file: &OwnedFile, bytes: &[u8]) -> Result<(), AppError> {
    let invalid = || AppError::Config("secret.invalid_legacy_file".into());
    let extension = file
        .relative
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("");
    let valid = match extension {
        "yaml" | "yml" => serde_yaml::from_slice::<serde_yaml::Value>(bytes)
            .map(|v| v.is_mapping())
            .map_err(|_| invalid())?,
        "json5" => {
            json5::from_str::<serde_json::Value>(std::str::from_utf8(bytes).map_err(|_| invalid())?)
                .map(|v| v.is_object())
                .map_err(|_| invalid())?
        }
        _ => serde_json::from_slice::<serde_json::Value>(bytes)
            .map(|v| v.is_object())
            .map_err(|_| invalid())?,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod owned_tests {
    use super::*;
    #[test]
    fn migration_stages_registered_credentials_and_rejects_plaintext_after_upgrade() {
        let root = tempfile::tempdir().unwrap();
        let vault = VaultContext::generate().unwrap();
        std::fs::create_dir_all(root.path().join("backups/hermes")).unwrap();
        std::fs::write(
            root.path()
                .join("backups/hermes/hermes_20260914_010203.yaml"),
            "key: backup-canary\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("config.json.bak"),
            r#"{"key":"backup-canary"}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("backups/unrelated.json"), "unrelated").unwrap();
        assert!(stage_owned_files_with_vault(root.path(), &vault, false).is_err());
        let plans = stage_owned_files_with_vault(root.path(), &vault, true).unwrap();
        assert_eq!(plans.len(), 2);
        for plan in plans {
            assert!(!String::from_utf8_lossy(&plan.ciphertext).contains("backup-canary"));
            assert!(
                String::from_utf8_lossy(&plan.file.decode(&vault, &plan.ciphertext).unwrap())
                    .contains("backup-canary")
            );
            assert!(
                String::from_utf8_lossy(&std::fs::read(&plan.source).unwrap())
                    .contains("backup-canary")
            );
            std::fs::write(&plan.destination, &plan.ciphertext).unwrap();
        }
        assert_eq!(
            stage_owned_files_with_vault(root.path(), &vault, false)
                .unwrap()
                .len(),
            2
        );
        let recovery = root.path().join("backups/vault-recovery");
        std::fs::create_dir_all(&recovery).unwrap();
        std::fs::write(recovery.join("config.json.bak"), b"lpenc1.invalid").unwrap();
        assert!(stage_owned_files_with_vault(root.path(), &vault, true).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("backups/unrelated.json")).unwrap(),
            "unrelated"
        );
        assert!(OwnedFile::registered("../config.json").is_err());
        assert!(OwnedFile::registered("backups/unrelated.json").is_err());
    }
}
