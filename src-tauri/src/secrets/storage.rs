//! Explicit runtime storage codecs. This is not a key or mode persistence owner.
use super::{SecretError, VaultContext, VaultMetadata};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlaintextIdentity {
    pub format_version: u32,
    pub store_id: String,
    pub generation_id: String,
    pub revision: u64,
}
impl PlaintextIdentity {
    pub(crate) fn validate(&self) -> Result<(), SecretError> {
        if self.format_version != 1 {
            return Err(SecretError::UnsupportedFormat);
        }
        if self.revision == 0 {
            return Err(SecretError::InvalidMetadata);
        }
        for id in [&self.store_id, &self.generation_id] {
            let parsed = uuid::Uuid::parse_str(id).map_err(|_| SecretError::InvalidMetadata)?;
            if parsed.is_nil() || parsed.to_string() != *id {
                return Err(SecretError::InvalidMetadata);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StorageIdentity {
    Encrypted(Box<VaultMetadata>),
    Plaintext(PlaintextIdentity),
}
impl StorageIdentity {
    pub(crate) fn store_id(&self) -> &str {
        match self {
            Self::Encrypted(v) => &v.vault_id,
            Self::Plaintext(v) => &v.store_id,
        }
    }
    pub(crate) fn generation_id(&self) -> &str {
        match self {
            Self::Encrypted(v) => &v.key_id,
            Self::Plaintext(v) => &v.generation_id,
        }
    }
    pub(crate) fn revision(&self) -> u64 {
        match self {
            Self::Encrypted(v) => v.revision,
            Self::Plaintext(v) => v.revision,
        }
    }
}
#[derive(Clone)]
pub(crate) enum StorageContext {
    Encrypted(VaultContext),
    Plaintext(PlaintextIdentity),
}
impl From<VaultContext> for StorageContext {
    fn from(vault: VaultContext) -> Self {
        Self::Encrypted(vault)
    }
}
impl StorageContext {
    pub(crate) fn identity(&self) -> StorageIdentity {
        match self {
            Self::Encrypted(v) => StorageIdentity::Encrypted(v.metadata().clone().into()),
            Self::Plaintext(v) => StorageIdentity::Plaintext(v.clone()),
        }
    }
    pub(crate) fn encrypted(&self) -> Result<&VaultContext, AppError> {
        match self {
            Self::Encrypted(v) => Ok(v),
            Self::Plaintext(_) => Err(AppError::Config("protection.encryption_required".into())),
        }
    }
    pub(crate) fn is_plaintext(&self) -> bool {
        matches!(self, Self::Plaintext(_))
    }
    pub(crate) fn seal(&self, identity: &[&str], plaintext: &[u8]) -> Result<String, SecretError> {
        match self {
            Self::Encrypted(v) => v.seal(identity, plaintext),
            Self::Plaintext(v) => {
                validate_plaintext(v, identity, plaintext)?;
                std::str::from_utf8(plaintext)
                    .map(str::to_owned)
                    .map_err(|_| SecretError::InvalidEnvelope)
            }
        }
    }
    pub(crate) fn open(
        &self,
        identity: &[&str],
        encoded: &str,
    ) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        match self {
            Self::Encrypted(v) => v.open(identity, encoded),
            Self::Plaintext(v) => {
                validate_plaintext(v, identity, encoded.as_bytes())?;
                Ok(Zeroizing::new(encoded.as_bytes().to_vec()))
            }
        }
    }
    pub(crate) fn encode_bytes(
        &self,
        identity: &[&str],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, SecretError> {
        match self {
            Self::Encrypted(v) => v.seal(identity, plaintext).map(String::into_bytes),
            Self::Plaintext(v) => {
                validate_plaintext(v, identity, plaintext)?;
                Ok(plaintext.to_vec())
            }
        }
    }
    pub(crate) fn decode_bytes(
        &self,
        identity: &[&str],
        encoded: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        match self {
            Self::Encrypted(v) => {
                let encoded =
                    std::str::from_utf8(encoded).map_err(|_| SecretError::InvalidEnvelope)?;
                v.open(identity, encoded)
            }
            Self::Plaintext(v) => {
                validate_plaintext(v, identity, encoded)?;
                Ok(Zeroizing::new(encoded.to_vec()))
            }
        }
    }
}
fn validate_plaintext(
    metadata: &PlaintextIdentity,
    identity: &[&str],
    bytes: &[u8],
) -> Result<(), SecretError> {
    metadata.validate()?;
    if identity.is_empty() || identity.iter().all(|part| part.is_empty()) {
        return Err(SecretError::InvalidIdentity);
    }
    // Reserved encryption namespaces can never be silently treated as credentials.
    if bytes.starts_with(b"lpenc") {
        return Err(SecretError::UnsupportedFormat);
    }
    Ok(())
}
#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
