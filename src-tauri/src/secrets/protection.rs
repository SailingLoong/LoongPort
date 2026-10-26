//! Sole durable protection-choice record, owned by the original SecretSession.
use super::{
    storage::{PlaintextIdentity, StorageContext},
    VaultMetadata,
};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const FILE: &str = "protection.json";
pub(crate) const MAX_RECORD_BYTES: u64 = 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProtectionRecord {
    pub format_version: u32,
    pub choice_complete: bool,
    pub storage: ProtectionStorage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) commitment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cleanup: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum ProtectionStorage {
    Encrypted {
        #[serde(rename = "vaultId")]
        vault_id: String,
        #[serde(rename = "keyId")]
        key_id: String,
    },
    Plaintext {
        identity: PlaintextIdentity,
    },
}
fn invalid() -> AppError {
    AppError::Config("protection.invalid_record".into())
}
impl ProtectionRecord {
    /// New encrypted choices authenticate the complete durable choice, including
    /// a pending cleanup. Legacy raw-key records remain readable; new text-key
    /// metadata never authenticates an unsigned or stripped choice record.
    pub(crate) fn encrypted(
        vault: &super::VaultContext,
        cleanup: Option<&str>,
    ) -> Result<Self, AppError> {
        let mut record = Self {
            format_version: 1,
            choice_complete: true,
            storage: ProtectionStorage::Encrypted {
                vault_id: vault.metadata().vault_id.clone(),
                key_id: vault.metadata().key_id.clone(),
            },
            commitment: None,
            cleanup: cleanup
                .map(|value| vault.seal(&["protection-record", "cleanup"], value.as_bytes()))
                .transpose()
                .map_err(super::error::secret_error)?,
        };
        let body = record.committed_body()?;
        record.commitment = Some(
            vault
                .seal(&["protection-record", "choice"], body.as_bytes())
                .map_err(super::error::secret_error)?,
        );
        Ok(record)
    }
    fn committed_body(&self) -> Result<zeroize::Zeroizing<String>, AppError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            format_version: u32,
            choice_complete: bool,
            storage: &'a ProtectionStorage,
            cleanup: &'a Option<String>,
        }
        serde_json::to_string(&Body {
            format_version: self.format_version,
            choice_complete: self.choice_complete,
            storage: &self.storage,
            cleanup: &self.cleanup,
        })
        .map(zeroize::Zeroizing::new)
        .map_err(|_| invalid())
    }
    pub(crate) fn verify_commitment(&self, context: &StorageContext) -> Result<(), AppError> {
        let invalid_auth = || AppError::Config("protection.record_authentication_failed".into());
        match context {
            StorageContext::Plaintext(_) => {
                if self.commitment.is_some() || self.cleanup.is_some() {
                    return Err(invalid_auth());
                }
            }
            StorageContext::Encrypted(vault) => match &self.commitment {
                Some(commitment) => {
                    let expected = self.committed_body()?;
                    let actual = vault
                        .open(&["protection-record", "choice"], commitment)
                        .map_err(|_| invalid_auth())?;
                    if actual.as_slice() != expected.as_bytes() {
                        return Err(invalid_auth());
                    }
                }
                None if vault.metadata().user_text_key.is_some() || self.cleanup.is_some() => {
                    return Err(invalid_auth())
                }
                None => {} // compatibility for the pre-text, explicitly bound raw-key record
            },
        }
        Ok(())
    }
    pub(super) fn cleanup_manifest(
        &self,
        vault: &super::VaultContext,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, AppError> {
        self.matches_context(&StorageContext::Encrypted(vault.clone()))?;
        self.verify_commitment(&StorageContext::Encrypted(vault.clone()))?;
        self.cleanup
            .as_ref()
            .map(|value| {
                vault
                    .open(&["protection-record", "cleanup"], value)
                    .map_err(super::error::secret_error)
            })
            .transpose()
    }
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, AppError> {
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid());
        }
        let record: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        record.validate()?;
        Ok(record)
    }
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if self.format_version != 1 {
            return Err(invalid());
        }
        match &self.storage {
            ProtectionStorage::Plaintext { identity } => identity.validate().map_err(|_| invalid()),
            ProtectionStorage::Encrypted { vault_id, key_id } => {
                if !self.choice_complete {
                    return Err(invalid());
                }
                for id in [vault_id, key_id] {
                    let parsed = uuid::Uuid::parse_str(id).map_err(|_| invalid())?;
                    if parsed.is_nil() || parsed.to_string() != *id {
                        return Err(invalid());
                    }
                }
                Ok(())
            }
        }
    }
    pub(crate) fn validate_vault(&self, vault: Option<&VaultMetadata>) -> Result<(), AppError> {
        self.validate()?;
        match (&self.storage, vault) {
            (ProtectionStorage::Plaintext { .. }, None) => Ok(()),
            (ProtectionStorage::Encrypted { vault_id, key_id }, Some(v))
                if *vault_id == v.vault_id && *key_id == v.key_id =>
            {
                v.validate().map_err(|_| invalid())
            }
            _ => Err(AppError::Config("protection.identity_mismatch".into())),
        }
    }
    pub(crate) fn should_prompt(&self, service_onboarding_complete: bool) -> bool {
        !self.choice_complete && service_onboarding_complete
    }
    pub(crate) fn matches_context(&self, context: &StorageContext) -> Result<(), AppError> {
        self.validate()?;
        match (&self.storage, context) {
            (ProtectionStorage::Plaintext { identity }, StorageContext::Plaintext(actual))
                if identity == actual =>
            {
                Ok(())
            }
            (ProtectionStorage::Encrypted { .. }, StorageContext::Encrypted(v)) => {
                self.validate_vault(Some(v.metadata()))
            }
            _ => Err(AppError::Config("protection.identity_mismatch".into())),
        }
    }
}

pub(crate) fn open_context<F>(
    record: &ProtectionRecord,
    vault: Option<&VaultMetadata>,
    provided_key: Option<zeroize::Zeroizing<Vec<u8>>>,
    load: F,
) -> Result<StorageContext, AppError>
where
    F: FnOnce(&str, &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, AppError>,
{
    let context = authenticate_context(record, vault, provided_key, load)?;
    if record.cleanup.is_some() {
        return Err(AppError::Config("protection.cleanup_required".into()));
    }
    Ok(context)
}

/// Only the original generation recovery owner may authenticate a pending target.
/// This is not a runtime admission API; its caller must finish the signed intent.
pub(super) fn open_cleanup_context<F>(
    record: &ProtectionRecord,
    vault: Option<&VaultMetadata>,
    provided_key: Option<zeroize::Zeroizing<Vec<u8>>>,
    load: F,
) -> Result<StorageContext, AppError>
where
    F: FnOnce(&str, &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, AppError>,
{
    if record.cleanup.is_none() {
        return Err(AppError::Config("protection.cleanup_not_pending".into()));
    }
    authenticate_context(record, vault, provided_key, load)
}

fn authenticate_context<F>(
    record: &ProtectionRecord,
    vault: Option<&VaultMetadata>,
    provided_key: Option<zeroize::Zeroizing<Vec<u8>>>,
    load: F,
) -> Result<StorageContext, AppError>
where
    F: FnOnce(&str, &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, AppError>,
{
    record.validate_vault(vault)?;
    let context = match &record.storage {
        ProtectionStorage::Plaintext { identity } => {
            if provided_key.is_some() {
                return Err(AppError::Config("protection.unexpected_key".into()));
            }
            Ok(StorageContext::Plaintext(identity.clone()))
        }
        ProtectionStorage::Encrypted { vault_id, key_id } => {
            let metadata = vault.ok_or_else(invalid)?.clone();
            // provided_key is already authenticated internal master material, never IPC text.
            let context = match provided_key {
                Some(key) => super::VaultContext::from_key(metadata, key),
                None => {
                    let bytes = load(vault_id, key_id)?
                        .ok_or_else(|| AppError::Config("secret.key_missing".into()))?;
                    super::key_store::user_text::authenticate(metadata, bytes)
                }
            };
            context
                .map(StorageContext::Encrypted)
                .map_err(super::error::secret_error)
        }
    }?;
    record.verify_commitment(&context)?;
    Ok(context)
}

pub(crate) fn read_with_bytes(root: &Path) -> Result<Option<(ProtectionRecord, Vec<u8>)>, AppError> {
    crate::config_file_io::read_regular_file(&root.join(FILE), MAX_RECORD_BYTES)
        .map_err(|_|invalid())?.map(|bytes|ProtectionRecord::parse(&bytes).map(|record|(record,bytes))).transpose()
}
pub(crate) fn read(root: &Path) -> Result<Option<ProtectionRecord>, AppError> {
    Ok(read_with_bytes(root)?.map(|(record,_)|record))
}

/// Same-mode choice is not a credential generation. Hold the original session
/// and DB guards, preserve its identity, and CAS the exact reviewed record.
pub(crate) fn complete_plaintext_choice(db:&crate::Database, expected:&[u8])->Result<(),AppError> {
    let session=db.secret_session();
    let context=session.write()?;
    if !context.is_plaintext() {return Err(AppError::Config("protection.conversion_required".into()));}
    let conn=db.conn.lock()?;
    crate::database::storage::check_identity(&conn,&context)?;
    let (mut record,bytes)=read_with_bytes(session.root())?.ok_or_else(invalid)?;
    if bytes!=expected {return Err(AppError::Config("protection.review_changed".into()));}
    record.matches_context(&context)?;
    if record.choice_complete {return Ok(());}
    record.choice_complete=true;
    let target=serde_json::to_vec(&record).map_err(|_|invalid())?;
    if read_with_bytes(session.root())?.map(|(_,bytes)|bytes).as_deref()!=Some(expected) {return Err(AppError::Config("protection.review_changed".into()));}
    crate::config_file_io::write_durable(&session.root().join(FILE),&target)?;
    if read_with_bytes(session.root())?.map(|(_,bytes)|bytes).as_deref()!=Some(target.as_slice()) {return Err(AppError::Config("protection.choice_verification_failed".into()));}
    Ok(())
}

/// A passive proof of an empty original data/device scope. Diagnostic entries
/// come only from the original diagnostic owner, never IPC input.
pub(crate) struct FreshScope {
    root: std::path::PathBuf,
    device: std::path::PathBuf,
    diagnostics: Vec<std::ffi::OsString>,
}
impl FreshScope {
    pub(crate) fn capture(
        root: &Path,
        device: &Path,
        diagnostics: &[std::ffi::OsString],
    ) -> Result<Self, AppError> {
        let proof = Self {
            root: root.to_path_buf(),
            device: device.to_path_buf(),
            diagnostics: diagnostics.to_vec(),
        };
        proof.verify_unchanged()?;
        Ok(proof)
    }
    pub(crate) fn verify_unchanged(&self) -> Result<(), AppError> {
        empty_except_diagnostics(&self.root, &self.diagnostics)?;
        if self.device != self.root {
            empty_except_diagnostics(&self.device, &self.diagnostics)?;
        }
        Ok(())
    }
    pub(crate) fn publish_initial_plaintext(&self) -> Result<ProtectionRecord, AppError> {
        self.verify_unchanged()?;
        let record = ProtectionRecord {
            commitment: None,
            cleanup: None,
            format_version: 1,
            choice_complete: false,
            storage: ProtectionStorage::Plaintext {
                identity: PlaintextIdentity {
                    format_version: 1,
                    store_id: uuid::Uuid::new_v4().to_string(),
                    generation_id: uuid::Uuid::new_v4().to_string(),
                    revision: 1,
                },
            },
        };
        record.validate()?;
        crate::config_file_io::ensure_private_directory(&self.root)?;
        self.verify_unchanged()?;
        let bytes = serde_json::to_vec(&record).map_err(|_| invalid())?;
        // Never replace a competing record, including an unreadable/symlink one.
        crate::config_file_io::write_durable_new(&self.root.join(FILE), &bytes)?;
        if read(&self.root)?.as_ref() != Some(&record) {
            return Err(AppError::Config("protection.source_changed".into()));
        }
        Ok(record)
    }
}
fn empty_except_diagnostics(
    root: &Path,
    diagnostics: &[std::ffi::OsString],
) -> Result<(), AppError> {
    if !crate::config_file_io::directory_exists_no_alias(root)? {
        return Ok(());
    }
    for entry in std::fs::read_dir(root).map_err(|e| AppError::io(root, e))? {
        let entry = entry.map_err(|e| AppError::io(root, e))?;
        if !diagnostics.contains(&entry.file_name()) {
            return Err(AppError::Config(
                "protection.existing_data_requires_review".into(),
            ));
        }
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| AppError::io(&path, e))?;
        if meta.file_type().is_symlink() {
            return Err(AppError::Config("secret.invalid_storage_path".into()));
        }
        if meta.is_dir() {
            crate::config_file_io::directory_exists_no_alias(&path)?;
            for (count, child) in std::fs::read_dir(&path)
                .map_err(|e| AppError::io(&path, e))?
                .enumerate()
            {
                if count >= 1024 {
                    return Err(AppError::Config(
                        "protection.existing_data_requires_review".into(),
                    ));
                }
                let child = child.map_err(|e| AppError::io(&path, e))?;
                let meta = std::fs::symlink_metadata(child.path())
                    .map_err(|e| AppError::io(child.path(), e))?;
                if !meta.is_file() || meta.file_type().is_symlink() {
                    return Err(AppError::Config("secret.invalid_storage_path".into()));
                }
            }
        } else if !meta.is_file() {
            return Err(AppError::Config("secret.invalid_storage_path".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "protection_tests.rs"]
mod tests;

#[path="protection/generation_files.rs"]
pub(crate) mod generation_files;
#[path="protection/generation.rs"]
pub(crate) mod generation;
