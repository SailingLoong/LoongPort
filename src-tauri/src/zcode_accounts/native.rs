//! Native ciphertext codec with explicit inputs only. Local authentication is
//! not a claim that an OAuth token remains valid at its remote provider.

use super::core::{
    js_trim, AccountIdentity, AccountSnapshot, CoreError, CredentialDocument, OAuthFamily,
    StrictRecord, MAX_DOCUMENT_BYTES,
};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeError {
    MissingContext,
    MissingSecret,
    UnsupportedCipher,
    InvalidEnvelope,
    AuthenticationFailed,
    InvalidSession,
    AmbiguousIdentity,
    UnsupportedProvider,
    Core(CoreError),
}

pub struct NativeCipher {
    context: String,
    key: Zeroizing<Vec<u8>>,
}

impl NativeCipher {
    pub(super) fn context(&self) -> &str {
        &self.context
    }
    pub fn new(context: &str, secret: &str) -> Result<Self, NativeError> {
        if context.trim().is_empty() {
            return Err(NativeError::MissingContext);
        }
        if secret.is_empty() {
            return Err(NativeError::MissingSecret);
        }
        let mut digest = Sha256::digest(secret.as_bytes());
        let key = Zeroizing::new(digest.to_vec());
        digest.as_mut_slice().zeroize();
        Ok(Self {
            context: context.into(),
            key,
        })
    }

    pub fn decrypt(&self, encoded: &str) -> Result<Zeroizing<String>, NativeError> {
        let payload = encoded
            .strip_prefix("enc:v1:")
            .ok_or(NativeError::UnsupportedCipher)?;
        if encoded.len() > MAX_DOCUMENT_BYTES {
            return Err(NativeError::InvalidEnvelope);
        }
        let parts: Vec<_> = payload.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
            return Err(NativeError::InvalidEnvelope);
        }
        let decode = |value: &str| {
            URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| NativeError::InvalidEnvelope)
        };
        let iv: [u8; 12] = decode(parts[0])?
            .try_into()
            .map_err(|_| NativeError::InvalidEnvelope)?;
        let tag = decode(parts[1])?;
        if tag.len() != 16 {
            return Err(NativeError::InvalidEnvelope);
        }
        let mut ciphertext = decode(parts[2])?;
        // Native stores IV.tag.ciphertext; RustCrypto consumes ciphertext || tag.
        ciphertext.extend_from_slice(&tag);
        let cipher =
            Aes256Gcm::new_from_slice(&self.key).map_err(|_| NativeError::InvalidEnvelope)?;
        let plaintext = cipher
            .decrypt(&Nonce::from(iv), ciphertext.as_slice())
            .map_err(|_| NativeError::AuthenticationFailed)?;
        String::from_utf8(plaintext)
            .map(Zeroizing::new)
            .map_err(|_| NativeError::InvalidEnvelope)
    }

    pub fn inspect(&self, document: &CredentialDocument) -> Result<AccountSnapshot, NativeError> {
        let active = self.decrypt(
            document
                .get("oauth:active_provider")
                .ok_or(NativeError::InvalidSession)?,
        )?;
        let (family, provider) = match active.as_str() {
            "zai" => (OAuthFamily::Zai, "zai"),
            "bigmodel" => (OAuthFamily::BigModel, "bigmodel"),
            _ => return Err(NativeError::UnsupportedProvider),
        };
        let user = self.decrypt(
            document
                .get(&format!("oauth:{provider}:user_info"))
                .ok_or(NativeError::InvalidSession)?,
        )?;
        let profile = serde_json::from_str::<StrictRecord<serde_json::Value>>(&user)
            .map_err(|_| NativeError::InvalidSession)?
            .0;
        let string = |key: &str| profile.get(key).and_then(serde_json::Value::as_str);
        if let (Some(id), Some(raw_id)) = (string("id"), string("user_id")) {
            if js_trim(id) != js_trim(raw_id) {
                return Err(NativeError::AmbiguousIdentity);
            }
        }
        let account_id = if string("id").is_some()
            && string("username").is_some()
            && string("displayName").is_some()
        {
            string("id")
        } else if family == OAuthFamily::Zai {
            string("user_id")
        } else {
            None
        }
        .ok_or(NativeError::InvalidSession)?;
        let identity =
            AccountIdentity::new(&self.context, family, account_id).map_err(NativeError::Core)?;
        // Authenticate every present selected value, without touching unrelated stores.
        for key in identity.credential_keys() {
            if let Some(raw) = document.get(&key) {
                if js_trim(&self.decrypt(raw)?).is_empty() {
                    return Err(NativeError::InvalidSession);
                }
            }
        }
        AccountSnapshot::capture(identity, document).map_err(NativeError::Core)
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
pub(crate) mod tests;
