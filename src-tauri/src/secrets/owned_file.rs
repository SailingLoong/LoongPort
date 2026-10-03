//! Single registry and authenticated codec for application-owned credential files.
use crate::error::AppError;
use crate::secrets::VaultContext;
use std::path::PathBuf;
use zeroize::Zeroizing;

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
}

/// A credential document created and owned by the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnedFile {
    pub(super) relative: PathBuf,
}

pub(crate) const PROFILE_FILE: &str = "zcode_account_profiles.json";
pub(crate) const JOURNAL_FILE: &str = "zcode_account_transaction.json";
pub(crate) const RECOVERY_FILE: &str = "zcode_account_recovery.json";
const ZCODE_FILES: [&str; 3] = [PROFILE_FILE, JOURNAL_FILE, RECOVERY_FILE];

const LEGACY_CONFIG_FILES: [&str; 3] = ["config.json", "config.json.bak", "config.json.migrated"];
pub(super) const BACKUP_PATTERNS: [(&str, &str, &str); 6] = [
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
                || ZCODE_FILES.contains(&name)
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

    pub(crate) fn allows_legacy_plaintext(&self) -> bool {
        !ZCODE_FILES
            .iter()
            .any(|name| self.relative == std::path::Path::new(name))
    }

    pub(crate) fn relative_path(&self) -> &std::path::Path {
        &self.relative
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
            .map_err(crate::secrets::error::secret_error)
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
            .map_err(crate::secrets::error::secret_error)
    }
}

fn matches_backup_name(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|v| v.strip_suffix(suffix))
        .is_some_and(|stamp| {
            !stamp.is_empty() && stamp.bytes().all(|v| v.is_ascii_digit() || v == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_preserves_legacy_files_and_auth_export_membership() {
        assert_eq!(
            AUTH_FILES.map(CredentialFile::filename),
            [
                "copilot_auth.json",
                "codex_oauth_auth.json",
                "xai_oauth_auth.json"
            ]
        );
        for path in [
            "config.json",
            "config.json.bak",
            "config.json.migrated",
            "copilot_auth.json",
            "codex_oauth_auth.json",
            "xai_oauth_auth.json",
            "backups/backup_20260101.json",
            "backups/codex-auth-123_456.json",
            "backups/env-backup-123.json",
            "backups/hermes/hermes_123.yaml",
            "backups/hermes/hermes_123.yml",
            "backups/openclaw/openclaw_123.json5",
        ] {
            let owned = OwnedFile::registered(path).unwrap();
            assert_eq!(owned.relative_path(), std::path::Path::new(path));
        }
        for path in [
            "../config.json",
            "/config.json",
            "backups/unrelated.json",
            "backups/backup_a.json",
            "folder/config.json",
        ] {
            assert!(OwnedFile::registered(path).is_err());
        }
    }

    #[test]
    fn zcode_files_are_registered_with_distinct_authenticated_identities() {
        let vault = VaultContext::generate().unwrap();
        let names = [
            "zcode_account_profiles.json",
            "zcode_account_transaction.json",
            "zcode_account_recovery.json",
        ];
        for name in names {
            let file = OwnedFile::registered(name).expect("new account file must be protected");
            let sealed = file.encode(&vault, b"synthetic-sensitive-canary").unwrap();
            assert!(!String::from_utf8_lossy(&sealed).contains("synthetic-sensitive-canary"));
            assert_eq!(
                &*file.decode(&vault, &sealed).unwrap(),
                b"synthetic-sensitive-canary"
            );
            for other in names.into_iter().filter(|other| *other != name) {
                assert!(OwnedFile::registered(other)
                    .unwrap()
                    .decode(&vault, &sealed)
                    .is_err());
            }
            assert!(file.decode(&vault, b"{}").is_err());
        }
    }
}

/// Caller holds the existing sync owner and, when available, the vault write guard.
/// This admission check never prevents unlocking the vault for account recovery.
pub(crate) fn ensure_no_pending_zcode_transaction(root: &std::path::Path) -> Result<(), AppError> {
    let path = root.join(JOURNAL_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(&path, error)),
        Ok(_) => Err(AppError::Config("secret.zcode_recovery_required".into())),
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn pending_journal_blocks_maintenance_without_parsing_or_deleting_it() {
        let root = tempfile::tempdir().unwrap();
        ensure_no_pending_zcode_transaction(root.path()).unwrap();
        let path = root.path().join(JOURNAL_FILE);
        std::fs::write(&path, b"corrupt-or-interrupted-journal").unwrap();
        assert!(ensure_no_pending_zcode_transaction(root.path()).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"corrupt-or-interrupted-journal"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(ensure_no_pending_zcode_transaction(root.path()).is_err());
    }
    #[test]
    fn new_account_files_cannot_be_upgraded_from_legacy_plaintext() {
        for name in ZCODE_FILES {
            assert!(!OwnedFile::registered(name)
                .unwrap()
                .allows_legacy_plaintext());
        }
        for name in AUTH_FILES
            .map(CredentialFile::filename)
            .into_iter()
            .chain(LEGACY_CONFIG_FILES)
        {
            assert!(OwnedFile::registered(name)
                .unwrap()
                .allows_legacy_plaintext());
        }
    }
    #[cfg(unix)]
    #[test]
    fn dangling_journal_symlink_blocks_maintenance() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("missing", root.path().join(JOURNAL_FILE)).unwrap();
        assert!(ensure_no_pending_zcode_transaction(root.path()).is_err());
    }
}
