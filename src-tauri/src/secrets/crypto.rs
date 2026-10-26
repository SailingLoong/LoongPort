use super::SecretError;
use argon2::{Algorithm, Argon2, Block, Params, Version};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EncryptedPayload {
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KdfParameters {
    pub algorithm: String,
    pub version: u32,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WrappedKey {
    pub kdf: KdfParameters,
    pub payload: EncryptedPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UserTextKey {
    pub format_version: u32,
    pub kdf: KdfParameters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VaultMetadata {
    pub format_version: u32,
    pub algorithm: String,
    pub vault_id: String,
    pub key_id: String,
    pub revision: u64,
    pub verifier: EncryptedPayload,
    pub wrapped_key: Option<WrappedKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_text_key: Option<UserTextKey>,
}

#[derive(Clone)]
pub(crate) struct VaultContext {
    metadata: Box<VaultMetadata>,
    key: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for VaultContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultContext")
            .field("metadata", &self.metadata)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

const FORMAT_VERSION: u32 = 1;
const ALGORITHM: &str = "xchacha20poly1305";
const VALUE_PREFIX: &str = "lpenc1.";
const VERIFIER: &[u8] = b"LoongPort credential vault key v1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValueEnvelope {
    format_version: u32,
    vault_id: String,
    key_id: String,
    payload: EncryptedPayload,
}

impl UserTextKey {
    fn validate(&self) -> Result<(), SecretError> {
        if self.format_version != 1 {
            return Err(SecretError::UnsupportedFormat);
        }
        self.kdf.validate()?;
        if self.kdf.memory_kib != 64 * 1024
            || self.kdf.iterations != 3
            || self.kdf.parallelism != 1
            || URL_SAFE_NO_PAD
                .decode(&self.kdf.salt)
                .map_err(|_| SecretError::InvalidMetadata)?
                .len()
                != 16
        {
            return Err(SecretError::InvalidMetadata);
        }
        Ok(())
    }
}

impl VaultMetadata {
    /// Portable recovery eligibility belongs to the same metadata owner as authentication.
    pub(crate) fn recovery_input_available(&self) -> Result<bool, SecretError> {
        self.validate()?;
        Ok(self.user_text_key.is_some() || self.wrapped_key.is_some())
    }

    pub(crate) fn validate(&self) -> Result<(), SecretError> {
        if self.format_version != FORMAT_VERSION || self.algorithm != ALGORITHM {
            return Err(SecretError::UnsupportedFormat);
        }
        for id in [&self.vault_id, &self.key_id] {
            let parsed = uuid::Uuid::parse_str(id).map_err(|_| SecretError::InvalidMetadata)?;
            if parsed.is_nil() || parsed.to_string() != *id {
                return Err(SecretError::InvalidMetadata);
            }
        }
        if self.revision == 0 {
            return Err(SecretError::InvalidMetadata);
        }
        if let Some(source) = &self.user_text_key {
            source.validate()?;
        }
        validate_metadata_payload(&self.verifier, VERIFIER.len() + 16)?;
        if let Some(wrapped) = &self.wrapped_key {
            wrapped.kdf.validate()?;
            validate_metadata_payload(&wrapped.payload, 32 + 16)?;
        }
        Ok(())
    }
}

impl KdfParameters {
    fn validate(&self) -> Result<(), SecretError> {
        if self.algorithm != "argon2id" || self.version != 19 {
            return Err(SecretError::UnsupportedFormat);
        }
        // Bound both peak memory and total work before invoking Argon2.
        if self.memory_kib > 256 * 1024
            || self.iterations > 6
            || self.parallelism > 4
            || u64::from(self.memory_kib) * u64::from(self.iterations) > 768 * 1024
        {
            return Err(SecretError::ResourceLimit);
        }
        Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32))
            .map_err(|_| SecretError::InvalidMetadata)?;
        if self.salt.len() > 86 {
            return Err(SecretError::InvalidMetadata);
        }
        let salt = URL_SAFE_NO_PAD
            .decode(&self.salt)
            .map_err(|_| SecretError::InvalidMetadata)?;
        if !(16..=64).contains(&salt.len()) {
            return Err(SecretError::InvalidMetadata);
        }
        Ok(())
    }

    fn derive(&self, password: &str) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        self.validate()?;
        let salt = URL_SAFE_NO_PAD
            .decode(&self.salt)
            .map_err(|_| SecretError::InvalidMetadata)?;
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32))
            .map_err(|_| SecretError::InvalidMetadata)?;
        // The allocating Argon2 convenience method does not clear its workspace.
        let mut memory = Zeroizing::new(vec![Block::default(); params.block_count()]);
        let mut key = Zeroizing::new(vec![0; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into_with_memory(password.as_bytes(), &salt, &mut key, &mut memory)
            .map_err(|_| SecretError::KeyDerivationFailed)?;
        Ok(key)
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn parse_user_key_hex(encoded: &str) -> Result<Zeroizing<Vec<u8>>, SecretError> {
    if encoded.len() != 64 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SecretError::InvalidKeyLength);
    }
    let mut key = Zeroizing::new(vec![0; 32]);
    hex::decode_to_slice(encoded, &mut key).map_err(|_| SecretError::InvalidKeyLength)?;
    Ok(key)
}

impl VaultContext {
    /// One descriptor-driven user-input authentication boundary. The exact UTF-8
    /// input is never normalized and a failed text source never falls back to a wrapper.
    pub(crate) fn from_recovery_input(
        metadata: VaultMetadata,
        original: &str,
    ) -> Result<Self, SecretError> {
        metadata.validate()?;
        if metadata.user_text_key.is_some() {
            Self::from_user_text(metadata, original)
        } else {
            Self::from_password(metadata, original)
        }
    }

    pub(crate) fn create_generation_with_user_text(
        store_id: String,
        revision: u64,
        text: &str,
    ) -> Result<Self, SecretError> {
        if text.is_empty() {
            return Err(SecretError::UserTextRequired);
        }
        let descriptor = UserTextKey {
            format_version: 1,
            kdf: KdfParameters {
                algorithm: "argon2id".into(),
                version: 19,
                memory_kib: 64 * 1024,
                iterations: 3,
                parallelism: 1,
                salt: URL_SAFE_NO_PAD.encode(random_bytes::<16>()?),
            },
        };
        // The user's exact UTF-8 text directly determines the master key.
        // Randomness here is a public salt/identity/nonce, never a random master.
        let key = descriptor.kdf.derive(text)?;
        Self::new_with_source(store_id, revision, key, Some(descriptor))
    }
    pub(crate) fn from_user_text(metadata: VaultMetadata, text: &str) -> Result<Self, SecretError> {
        if text.is_empty() {
            return Err(SecretError::UserTextRequired);
        }
        metadata.validate()?;
        let descriptor = metadata
            .user_text_key
            .as_ref()
            .ok_or(SecretError::UserTextUnavailable)?;
        let key = descriptor.kdf.derive(text)?;
        Self::from_key(metadata, key)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn create_generation_with_user_key(
        store_id: String,
        revision: u64,
        key: Zeroizing<Vec<u8>>,
    ) -> Result<Self, SecretError> {
        Self::new_with_key(store_id, revision, key)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn create_with_user_key(key: Zeroizing<Vec<u8>>) -> Result<Self, SecretError> {
        if key.len() != 32 {
            return Err(SecretError::InvalidKeyLength);
        }
        Self::new_with_key(random_id()?, 1, key)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn replace_with_user_key(
        &self,
        key: Zeroizing<Vec<u8>>,
    ) -> Result<Self, SecretError> {
        let revision = self
            .metadata
            .revision
            .checked_add(1)
            .ok_or(SecretError::InvalidMetadata)?;
        Self::new_with_key(self.metadata.vault_id.clone(), revision, key)
    }

    pub(crate) fn generate() -> Result<Self, SecretError> {
        Self::new_with_random_key(random_id()?, 1)
    }

    fn new_with_random_key(vault_id: String, revision: u64) -> Result<Self, SecretError> {
        let mut key = Zeroizing::new(vec![0; 32]);
        getrandom::fill(&mut key).map_err(|_| SecretError::RandomUnavailable)?;
        Self::new_with_key(vault_id, revision, key)
    }

    fn new_with_key(
        vault_id: String,
        revision: u64,
        key: Zeroizing<Vec<u8>>,
    ) -> Result<Self, SecretError> {
        Self::new_with_source(vault_id, revision, key, None)
    }

    fn new_with_source(
        vault_id: String,
        revision: u64,
        key: Zeroizing<Vec<u8>>,
        user_text_key: Option<UserTextKey>,
    ) -> Result<Self, SecretError> {
        if key.len() != 32 {
            return Err(SecretError::InvalidKeyLength);
        }
        let mut metadata = VaultMetadata {
            format_version: FORMAT_VERSION,
            algorithm: ALGORITHM.into(),
            vault_id,
            key_id: random_id()?,
            revision,
            verifier: EncryptedPayload {
                nonce: String::new(),
                ciphertext: String::new(),
            },
            wrapped_key: None,
            user_text_key,
        };
        metadata.verifier = encrypt(&key, &verifier_aad(&metadata)?, VERIFIER)?;
        metadata.validate()?;
        Ok(Self {
            metadata: Box::new(metadata),
            key,
        })
    }

    pub(crate) fn metadata(&self) -> &VaultMetadata {
        &self.metadata
    }

    pub(crate) fn seal(&self, identity: &[&str], plaintext: &[u8]) -> Result<String, SecretError> {
        let aad = value_aad(&self.metadata, identity)?;
        let envelope = ValueEnvelope {
            format_version: FORMAT_VERSION,
            vault_id: self.metadata.vault_id.clone(),
            key_id: self.metadata.key_id.clone(),
            payload: encrypt(&self.key, &aad, plaintext)?,
        };
        let encoded = serde_json::to_vec(&envelope).map_err(|_| SecretError::InvalidEnvelope)?;
        Ok(format!("{VALUE_PREFIX}{}", URL_SAFE_NO_PAD.encode(encoded)))
    }

    pub(crate) fn open(
        &self,
        identity: &[&str],
        ciphertext: &str,
    ) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        let aad = value_aad(&self.metadata, identity)?;
        let encoded = ciphertext
            .strip_prefix(VALUE_PREFIX)
            .ok_or(SecretError::UnsupportedFormat)?;
        let decoded = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SecretError::InvalidEnvelope)?;
        let envelope: ValueEnvelope =
            serde_json::from_slice(&decoded).map_err(|_| SecretError::InvalidEnvelope)?;
        if envelope.format_version != FORMAT_VERSION {
            return Err(SecretError::UnsupportedFormat);
        }
        if envelope.vault_id != self.metadata.vault_id || envelope.key_id != self.metadata.key_id {
            return Err(SecretError::IdentityMismatch);
        }
        decrypt(&self.key, &aad, &envelope.payload)
    }

    pub(crate) fn with_password(&self, password: &str) -> Result<Self, SecretError> {
        if password.chars().count() < 12 {
            return Err(SecretError::PasswordTooShort);
        }
        let mut metadata = self.metadata.clone();
        metadata.revision = metadata
            .revision
            .checked_add(1)
            .ok_or(SecretError::InvalidMetadata)?;
        let kdf = KdfParameters {
            algorithm: "argon2id".into(),
            version: 19,
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
            salt: URL_SAFE_NO_PAD.encode(random_bytes::<16>()?),
        };
        let kek = kdf.derive(password)?;
        metadata.verifier = encrypt(&self.key, &verifier_aad(&metadata)?, VERIFIER)?;
        let payload = encrypt(&kek, &wrapping_aad(&metadata, &kdf)?, &self.key)?;
        metadata.wrapped_key = Some(WrappedKey { kdf, payload });
        Ok(Self {
            metadata,
            key: self.key.clone(),
        })
    }

    pub(crate) fn from_password(
        metadata: VaultMetadata,
        password: &str,
    ) -> Result<Self, SecretError> {
        metadata.validate()?;
        let wrapped = metadata
            .wrapped_key
            .as_ref()
            .ok_or(SecretError::PasswordUnavailable)?;
        let kek = wrapped.kdf.derive(password)?;
        let key = decrypt(
            &kek,
            &wrapping_aad(&metadata, &wrapped.kdf)?,
            &wrapped.payload,
        )
        .map_err(|_| SecretError::PasswordRejected)?;
        Self::from_key(metadata, key)
    }

    pub(crate) fn from_key(
        metadata: VaultMetadata,
        key: Zeroizing<Vec<u8>>,
    ) -> Result<Self, SecretError> {
        if key.len() != 32 {
            return Err(SecretError::InvalidKeyLength);
        }
        metadata.validate()?;
        let verifier = decrypt(&key, &verifier_aad(&metadata)?, &metadata.verifier)
            .map_err(|_| SecretError::KeyRejected)?;
        if verifier.as_slice() != VERIFIER {
            return Err(SecretError::KeyRejected);
        }
        Ok(Self {
            metadata: Box::new(metadata),
            key,
        })
    }

    /// The lifecycle owner must migrate protected values before publishing this context.
    /// The previous password wrapper belongs to the old key and is not carried forward.
    pub(crate) fn rotate_key(&self) -> Result<Self, SecretError> {
        if self.metadata.user_text_key.is_some() {
            return Err(SecretError::GenerationOwnerRequired);
        }
        let revision = self
            .metadata
            .revision
            .checked_add(1)
            .ok_or(SecretError::InvalidMetadata)?;
        Self::new_with_random_key(self.metadata.vault_id.clone(), revision)
    }

    /// Only lifecycle and operating-system credential-store adapters export keys.
    pub(crate) fn export_key(&self) -> Zeroizing<Vec<u8>> {
        self.key.clone()
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], SecretError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| SecretError::RandomUnavailable)?;
    Ok(bytes)
}

fn random_id() -> Result<String, SecretError> {
    Ok(uuid::Builder::from_random_bytes(random_bytes()?)
        .into_uuid()
        .to_string())
}

fn value_aad(metadata: &VaultMetadata, identity: &[&str]) -> Result<Vec<u8>, SecretError> {
    if identity.is_empty() || identity.iter().all(|part| part.is_empty()) {
        return Err(SecretError::InvalidIdentity);
    }
    // A JSON tuple preserves component boundaries, including empty strings.
    // Revision is deliberately absent: password rewrapping does not rewrite values.
    serde_json::to_vec(&(
        "loongport.value",
        metadata.format_version,
        &metadata.algorithm,
        &metadata.vault_id,
        &metadata.key_id,
        identity,
    ))
    .map_err(|_| SecretError::InvalidIdentity)
}

fn verifier_aad(metadata: &VaultMetadata) -> Result<Vec<u8>, SecretError> {
    if let Some(source) = &metadata.user_text_key {
        return serde_json::to_vec(&(
            "loongport.user-text-key-verifier",
            metadata.format_version,
            &metadata.algorithm,
            &metadata.vault_id,
            &metadata.key_id,
            metadata.revision,
            source,
        ))
        .map_err(|_| SecretError::InvalidMetadata);
    }

    serde_json::to_vec(&(
        "loongport.key-verifier",
        metadata.format_version,
        &metadata.algorithm,
        &metadata.vault_id,
        &metadata.key_id,
        metadata.revision,
    ))
    .map_err(|_| SecretError::InvalidMetadata)
}

fn wrapping_aad(metadata: &VaultMetadata, kdf: &KdfParameters) -> Result<Vec<u8>, SecretError> {
    serde_json::to_vec(&(
        "loongport.wrapped-key",
        metadata.format_version,
        &metadata.algorithm,
        &metadata.vault_id,
        &metadata.key_id,
        metadata.revision,
        kdf,
    ))
    .map_err(|_| SecretError::InvalidMetadata)
}

fn validate_metadata_payload(
    payload: &EncryptedPayload,
    ciphertext_len: usize,
) -> Result<(), SecretError> {
    if payload.nonce.len() != 32 || payload.ciphertext.len() > 128 {
        return Err(SecretError::InvalidMetadata);
    }
    let (nonce, ciphertext) = decode_payload(payload).map_err(|_| SecretError::InvalidMetadata)?;
    if nonce.len() != 24 || ciphertext.len() != ciphertext_len {
        return Err(SecretError::InvalidMetadata);
    }
    Ok(())
}

fn decode_payload(payload: &EncryptedPayload) -> Result<(XNonce, Vec<u8>), SecretError> {
    let nonce = URL_SAFE_NO_PAD
        .decode(&payload.nonce)
        .map_err(|_| SecretError::InvalidEnvelope)?;
    let nonce = XNonce::try_from(nonce.as_slice()).map_err(|_| SecretError::InvalidEnvelope)?;
    let ciphertext = URL_SAFE_NO_PAD
        .decode(&payload.ciphertext)
        .map_err(|_| SecretError::InvalidEnvelope)?;
    if ciphertext.len() < 16 {
        return Err(SecretError::InvalidEnvelope);
    }
    Ok((nonce, ciphertext))
}

fn encrypt(key: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<EncryptedPayload, SecretError> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| SecretError::InvalidKeyLength)?;
    let nonce = XNonce::from(random_bytes::<24>()?);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| SecretError::AuthenticationFailed)?;
    Ok(EncryptedPayload {
        nonce: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
    })
}

fn decrypt(
    key: &[u8],
    aad: &[u8],
    payload: &EncryptedPayload,
) -> Result<Zeroizing<Vec<u8>>, SecretError> {
    let (nonce, ciphertext) = decode_payload(payload)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| SecretError::InvalidKeyLength)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| SecretError::AuthenticationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    const ROW: &[&str] = &[
        "database",
        "providers",
        "settings_config",
        "example",
        "codex",
    ];
    const PASSWORD: &str = "example recovery phrase one";

    #[test]
    fn generates_an_authenticated_vault() {
        assert!(
            VaultContext::generate().is_ok(),
            "a new vault must generate its key and verifier"
        );
    }

    #[test]
    fn restored_key_reads_randomized_ciphertext() {
        let vault = VaultContext::generate().unwrap();
        let first = vault.seal(ROW, b"credential-canary").unwrap();
        let second = vault.seal(ROW, b"credential-canary").unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("credential-canary"));
        let restored =
            VaultContext::from_key(vault.metadata().clone(), vault.export_key()).unwrap();
        assert_eq!(&*restored.open(ROW, &first).unwrap(), b"credential-canary");
        assert_eq!(&*restored.open(ROW, &second).unwrap(), b"credential-canary");
        assert!(format!("{vault:?}").contains("[REDACTED]"));
        assert!(!format!("{vault:?}").contains(&format!("{:?}", &*vault.export_key())));
    }

    #[test]
    fn identity_components_cannot_be_reordered_or_joined_ambiguously() {
        let vault = VaultContext::generate().unwrap();
        let ciphertext = vault.seal(&["ab", "c"], b"value").unwrap();
        for identity in [
            &["a", "bc"][..],
            &["c", "ab"][..],
            &["abc"][..],
            &["ab", "c", ""][..],
        ] {
            assert_eq!(
                vault.open(identity, &ciphertext).unwrap_err(),
                SecretError::AuthenticationFailed
            );
        }
        assert_eq!(
            vault.seal(&[], b"value").unwrap_err(),
            SecretError::InvalidIdentity
        );
        assert_eq!(
            vault.open(&[], &ciphertext).unwrap_err(),
            SecretError::InvalidIdentity
        );
    }

    fn edit_envelope(ciphertext: &str, change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut value: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(ciphertext.strip_prefix("lpenc1.").unwrap())
                .unwrap(),
        )
        .unwrap();
        change(&mut value);
        format!(
            "lpenc1.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
        )
    }

    #[test]
    fn envelope_identity_and_tag_edits_are_rejected() {
        let vault = VaultContext::generate().unwrap();
        let another = VaultContext::generate().unwrap();
        let ciphertext = vault.seal(ROW, b"secret").unwrap();
        for field in ["vaultId", "keyId"] {
            let modified = edit_envelope(&ciphertext, |value| {
                value[field] = serde_json::json!(another.metadata().vault_id)
            });
            assert_eq!(
                vault.open(ROW, &modified).unwrap_err(),
                SecretError::IdentityMismatch
            );
        }
        let modified = edit_envelope(&ciphertext, |value| {
            let mut encrypted = URL_SAFE_NO_PAD
                .decode(value["payload"]["ciphertext"].as_str().unwrap())
                .unwrap();
            encrypted[0] ^= 1;
            value["payload"]["ciphertext"] = serde_json::json!(URL_SAFE_NO_PAD.encode(encrypted));
        });
        assert_eq!(
            vault.open(ROW, &modified).unwrap_err(),
            SecretError::AuthenticationFailed
        );
    }

    #[test]
    fn malformed_and_unknown_envelopes_never_fall_back_to_plaintext() {
        let vault = VaultContext::generate().unwrap();
        for input in ["plain-secret", "lpenc2.e30"] {
            assert_eq!(
                vault.open(ROW, input).unwrap_err(),
                SecretError::UnsupportedFormat
            );
        }
        for input in ["lpenc1.", "lpenc1.%%%", "lpenc1.e30"] {
            assert_eq!(
                vault.open(ROW, input).unwrap_err(),
                SecretError::InvalidEnvelope
            );
        }
        let ciphertext = vault.seal(ROW, b"").unwrap();
        let changed = edit_envelope(&ciphertext, |value| value["formatVersion"] = 2.into());
        assert_eq!(
            vault.open(ROW, &changed).unwrap_err(),
            SecretError::UnsupportedFormat
        );
        let truncated = edit_envelope(&ciphertext, |value| {
            value["payload"]["ciphertext"] = "AA".into()
        });
        assert_eq!(
            vault.open(ROW, &truncated).unwrap_err(),
            SecretError::InvalidEnvelope
        );
    }

    #[test]
    fn key_verifier_rejects_wrong_material_and_metadata_identity() {
        let vault = VaultContext::generate().unwrap();
        assert_eq!(
            VaultContext::from_key(vault.metadata().clone(), Zeroizing::new(vec![0; 31]))
                .unwrap_err(),
            SecretError::InvalidKeyLength
        );
        assert_eq!(
            VaultContext::from_key(vault.metadata().clone(), Zeroizing::new(vec![0; 32]))
                .unwrap_err(),
            SecretError::KeyRejected
        );
        let mut modified = vault.metadata().clone();
        modified.vault_id = VaultContext::generate()
            .unwrap()
            .metadata()
            .vault_id
            .clone();
        assert_eq!(
            VaultContext::from_key(modified, vault.export_key()).unwrap_err(),
            SecretError::KeyRejected
        );
        let mut modified = vault.metadata().clone();
        modified.vault_id = "invalid-id".into();
        assert_eq!(
            VaultContext::from_key(modified, vault.export_key()).unwrap_err(),
            SecretError::InvalidMetadata
        );
    }

    #[test]
    fn rewrap_changes_password_without_reencrypting_values() {
        let vault = VaultContext::generate().unwrap();
        let ciphertext = vault.seal(ROW, b"unchanged secret").unwrap();
        let wrapped = vault.with_password(PASSWORD).unwrap();
        let rewrapped = wrapped
            .with_password("example recovery phrase two")
            .unwrap();
        assert_eq!(
            rewrapped.metadata().revision,
            wrapped.metadata().revision + 1
        );
        assert_eq!(*rewrapped.export_key(), *vault.export_key());
        let restored = VaultContext::from_password(
            rewrapped.metadata().clone(),
            "example recovery phrase two",
        )
        .unwrap();
        assert_eq!(
            &*restored.open(ROW, &ciphertext).unwrap(),
            b"unchanged secret"
        );
        assert_eq!(
            VaultContext::from_password(rewrapped.metadata().clone(), PASSWORD).unwrap_err(),
            SecretError::PasswordRejected
        );
        let old_snapshot =
            VaultContext::from_password(wrapped.metadata().clone(), PASSWORD).unwrap();
        assert_eq!(
            &*old_snapshot.open(ROW, &ciphertext).unwrap(),
            b"unchanged secret"
        );
    }

    #[test]
    fn rotation_changes_only_key_generation_and_keeps_old_data_explicit() {
        let original = VaultContext::generate()
            .unwrap()
            .with_password(PASSWORD)
            .unwrap();
        let ciphertext = original.seal(ROW, b"rotate canary").unwrap();
        let rotated = original.rotate_key().unwrap();
        assert_eq!(rotated.metadata().vault_id, original.metadata().vault_id);
        assert_ne!(rotated.metadata().key_id, original.metadata().key_id);
        assert_eq!(
            rotated.metadata().revision,
            original.metadata().revision + 1
        );
        assert!(rotated.metadata().wrapped_key.is_none());
        assert_eq!(
            rotated.open(ROW, &ciphertext).unwrap_err(),
            SecretError::IdentityMismatch
        );
        assert_eq!(
            VaultContext::from_key(rotated.metadata().clone(), original.export_key()).unwrap_err(),
            SecretError::KeyRejected
        );
        let decrypted = original.open(ROW, &ciphertext).unwrap();
        let migrated = rotated.seal(ROW, &decrypted).unwrap();
        assert_eq!(&*rotated.open(ROW, &migrated).unwrap(), b"rotate canary");
        assert_eq!(
            original.open(ROW, &migrated).unwrap_err(),
            SecretError::IdentityMismatch
        );
    }

    #[test]
    fn new_password_policy_counts_characters_and_requires_a_wrapper_to_restore() {
        let vault = VaultContext::generate().unwrap();
        assert_eq!(
            vault.with_password("short").unwrap_err(),
            SecretError::PasswordTooShort
        );
        assert_eq!(
            VaultContext::from_password(vault.metadata().clone(), PASSWORD).unwrap_err(),
            SecretError::PasswordUnavailable
        );
        assert!(vault.with_password("恢复口令足够长十二个字啊").is_ok());
    }

    #[test]
    fn untrusted_kdf_limits_are_checked_before_derivation() {
        let vault = VaultContext::generate()
            .unwrap()
            .with_password(PASSWORD)
            .unwrap();
        for field in ["memoryKib", "iterations", "parallelism"] {
            let mut raw = serde_json::to_value(vault.metadata()).unwrap();
            raw["wrappedKey"]["kdf"][field] = u32::MAX.into();
            let modified = serde_json::from_value(raw).unwrap();
            assert_eq!(
                VaultContext::from_password(modified, PASSWORD).unwrap_err(),
                SecretError::ResourceLimit
            );
        }
        let mut modified = vault.metadata().clone();
        modified.wrapped_key.as_mut().unwrap().kdf.version = 16;
        assert_eq!(
            VaultContext::from_password(modified, PASSWORD).unwrap_err(),
            SecretError::UnsupportedFormat
        );
    }

    #[test]
    fn wrapping_authenticates_revision_and_kdf_metadata() {
        let vault = VaultContext::generate()
            .unwrap()
            .with_password(PASSWORD)
            .unwrap();
        let mut modified = vault.metadata().clone();
        modified.revision += 1;
        assert_eq!(
            VaultContext::from_password(modified, PASSWORD).unwrap_err(),
            SecretError::PasswordRejected
        );
        let mut modified = vault.metadata().clone();
        modified.wrapped_key.as_mut().unwrap().kdf.iterations += 1;
        assert_eq!(
            VaultContext::from_password(modified, PASSWORD).unwrap_err(),
            SecretError::PasswordRejected
        );
    }
}

#[cfg(test)]
mod user_key_tests {
    use super::*;
    #[test]
    fn strict_hex_parser_uses_exact_user_material_without_normalizing_input() {
        let key = "01aB".repeat(16);
        let parsed = parse_user_key_hex(&key).unwrap();
        assert_eq!(&*parsed, &hex::decode(&key).unwrap());
        for invalid in [
            "".into(),
            "x".repeat(64),
            "0".repeat(63),
            "0".repeat(65),
            format!(" {}", "0".repeat(64)),
            "é".repeat(32),
        ] {
            assert!(parse_user_key_hex(&invalid).is_err());
        }
    }
    #[test]
    fn user_key_is_master_key_not_password_around_a_random_master() {
        let input = Zeroizing::new(vec![0x2a; 32]);
        let vault = VaultContext::create_with_user_key(input.clone()).unwrap();
        assert_eq!(&*vault.export_key(), &*input);
        assert!(vault.metadata().wrapped_key.is_none());
        let opened = VaultContext::from_key(vault.metadata().clone(), input).unwrap();
        let cipher = vault.seal(&["test"], b"synthetic-credential").unwrap();
        assert_eq!(
            &*opened.open(&["test"], &cipher).unwrap(),
            b"synthetic-credential"
        );
        assert!(
            VaultContext::from_key(vault.metadata().clone(), Zeroizing::new(vec![0x2b; 32]))
                .is_err()
        );
        for len in [0, 1, 31, 33, 64] {
            assert!(VaultContext::create_with_user_key(Zeroizing::new(vec![0; len])).is_err());
        }
    }
    #[test]
    fn user_rekey_preserves_store_identity_but_not_generation_or_old_decryption() {
        let old = VaultContext::create_with_user_key(Zeroizing::new(vec![1; 32])).unwrap();
        let old_cipher = old.seal(&["old"], b"preserved-old-data").unwrap();
        let next = old
            .replace_with_user_key(Zeroizing::new(vec![2; 32]))
            .unwrap();
        assert_eq!(old.metadata().vault_id, next.metadata().vault_id);
        assert_ne!(old.metadata().key_id, next.metadata().key_id);
        assert_eq!(next.metadata().revision, old.metadata().revision + 1);
        assert_eq!(&*next.export_key(), &[2; 32]);
        assert!(next.open(&["old"], &old_cipher).is_err());
        assert_eq!(
            &*old.open(&["old"], &old_cipher).unwrap(),
            b"preserved-old-data"
        );
    }
    #[test]
    fn explicit_store_generation_accepts_only_canonical_identity_and_exact_user_key() {
        let store = "11111111-1111-4111-8111-111111111111";
        let key = Zeroizing::new(vec![23; 32]);
        let next =
            VaultContext::create_generation_with_user_key(store.into(), 2, key.clone()).unwrap();
        assert_eq!(next.metadata().vault_id, store);
        assert_eq!(next.metadata().revision, 2);
        assert_eq!(&*next.export_key(), &*key);
        assert!(next.metadata().wrapped_key.is_none());
        for id in [
            "invalid",
            "00000000-0000-0000-0000-000000000000",
            "11111111111141118111111111111111",
        ] {
            assert!(
                VaultContext::create_generation_with_user_key(id.into(), 2, key.clone()).is_err()
            );
        }
        assert!(
            VaultContext::create_generation_with_user_key(store.into(), 0, key.clone()).is_err()
        );
        assert!(VaultContext::create_generation_with_user_key(
            store.into(),
            2,
            Zeroizing::new(vec![23; 31])
        )
        .is_err());
    }
}

#[cfg(test)]
mod user_text_tests {
    use super::*;
    const STORE: &str = "11111111-1111-4111-8111-111111111111";
    #[test]
    fn exact_unicode_text_is_the_master_source_and_cache_reopen_keeps_salt() {
        let text = "  中文 e\u{301} 🔐 Ab7  ";
        let vault = VaultContext::create_generation_with_user_text(STORE.into(), 2, text).unwrap();
        let descriptor = vault.metadata().user_text_key.as_ref().unwrap();
        assert_eq!(descriptor.format_version, 1);
        assert_eq!(&*descriptor.kdf.derive(text).unwrap(), &*vault.export_key());
        assert!(vault.metadata().wrapped_key.is_none());
        let bytes = serde_json::to_vec(vault.metadata()).unwrap();
        assert!(!String::from_utf8(bytes.clone()).unwrap().contains(text));
        let cached = crate::secrets::key_store::user_text::encode(text).unwrap();
        let recovered = crate::secrets::key_store::user_text::decode(&cached).unwrap();
        assert_eq!(&*recovered, text);
        let reopened =
            VaultContext::from_user_text(serde_json::from_slice(&bytes).unwrap(), &recovered)
                .unwrap();
        assert_eq!(&*reopened.export_key(), &*vault.export_key());
        assert_eq!(serde_json::to_vec(reopened.metadata()).unwrap(), bytes);
        assert!(VaultContext::from_user_text(vault.metadata().clone(), text.trim()).is_err());
    }
    #[test]
    fn no_length_or_character_strength_policy_is_applied_to_nonempty_text() {
        for text in ["7", " "] {
            let vault =
                VaultContext::create_generation_with_user_text(STORE.into(), 1, text).unwrap();
            let reopened = VaultContext::from_user_text(vault.metadata().clone(), text).unwrap();
            assert_eq!(&*reopened.export_key(), &*vault.export_key());
        }
        assert!(VaultContext::create_generation_with_user_text(STORE.into(), 1, "").is_err());
    }
    #[test]
    fn text_never_guesses_legacy_raw_key_from_shape_and_legacy_metadata_stays_legacy() {
        let text = "aB".repeat(32);
        let vault = VaultContext::create_generation_with_user_text(STORE.into(), 1, &text).unwrap();
        assert_ne!(&*vault.export_key(), &hex::decode(&text).unwrap());
        let legacy = VaultContext::create_generation_with_user_key(
            STORE.into(),
            1,
            Zeroizing::new(vec![3; 32]),
        )
        .unwrap();
        assert!(legacy.metadata().user_text_key.is_none());
        assert!(VaultContext::from_user_text(legacy.metadata().clone(), &"03".repeat(32)).is_err());
        assert!(VaultContext::from_key(legacy.metadata().clone(), legacy.export_key()).is_ok());
        assert!(!serde_json::to_string(legacy.metadata())
            .unwrap()
            .contains("userTextKey"));
    }
}

#[cfg(test)]
mod user_text_rotation_tests {
    use super::*;
    #[test]
    fn user_text_mode_cannot_be_replaced_by_legacy_random_rotation() {
        let vault =
            VaultContext::create_generation_with_user_text(random_id().unwrap(), 1, "x").unwrap();
        let metadata = vault.metadata().clone();
        let key = vault.export_key();
        assert!(vault.rotate_key().is_err());
        assert_eq!(vault.metadata(), &metadata);
        assert_eq!(vault.export_key().as_slice(), key.as_slice());
        VaultContext::new_with_key(random_id().unwrap(), 1, Zeroizing::new(vec![4; 32]))
            .unwrap()
            .rotate_key()
            .unwrap();
    }
}
