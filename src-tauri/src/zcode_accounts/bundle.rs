//! Fixed .zsb authentication and bounded, strict structure inspection.
//! This module never accesses a path, imports config, or grants account scope.
use super::bundle_limits::{
    validate_inner, validate_outer, BundleError, EnvelopeParameters, MAX_BUNDLE_BYTES,
};
use super::core::{StrictRecord, MAX_DOCUMENT_BYTES};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{borrow::Cow, num::NonZeroU32};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BundleFailure {
    Limits(BundleError),
    Envelope,
    Authentication,
    Inner,
    Password,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum EntryFailure {
    Shape,
    ResourceLimit,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    version: u32,
    kdf: Kdf,
    cipher: Cipher,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Kdf {
    algo: String,
    iters: u32,
    salt: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cipher {
    algo: String,
    nonce: String,
    tag: String,
    data: String,
}

pub(super) struct OpenedBundle {
    plaintext: Zeroizing<Vec<u8>>,
}
pub(super) struct ParsedAccount<'a> {
    pub credentials: &'a RawValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inner<'a> {
    #[serde(borrow)]
    format: Cow<'a, str>,
    version: u32,
    #[serde(rename = "exportedAt", borrow)]
    exported_at: &'a RawValue,
    #[serde(borrow)]
    accounts: Vec<&'a RawValue>,
}
impl Drop for Inner<'_> {
    fn drop(&mut self) {
        if let Cow::Owned(format) = &mut self.format {
            format.zeroize();
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountShape<'a> {
    #[serde(borrow)]
    name: &'a RawValue,
    #[serde(rename = "createdAt", borrow)]
    created_at: &'a RawValue,
    #[serde(borrow)]
    credentials: &'a RawValue,
    #[serde(borrow)]
    config: &'a RawValue,
}

pub(super) fn open_bundle(file: &[u8], password: &str) -> Result<OpenedBundle, BundleFailure> {
    if file.is_empty() || file.len() > MAX_BUNDLE_BYTES {
        return Err(BundleFailure::Limits(BundleError::ResourceLimit));
    }
    if password.trim().is_empty() || password.len() > 4096 {
        return Err(BundleFailure::Password);
    }
    let envelope: Envelope = serde_json::from_slice(file).map_err(|_| BundleFailure::Envelope)?;
    let decode = |text: &str| STANDARD.decode(text).map_err(|_| BundleFailure::Envelope);
    let salt = decode(&envelope.kdf.salt)?;
    let nonce = decode(&envelope.cipher.nonce)?;
    let tag = decode(&envelope.cipher.tag)?;
    let mut data = decode(&envelope.cipher.data)?;
    validate_outer(
        file.len(),
        &EnvelopeParameters {
            format: &envelope.format,
            version: envelope.version,
            kdf: &envelope.kdf.algo,
            iterations: envelope.kdf.iters,
            salt_bytes: salt.len(),
            cipher: &envelope.cipher.algo,
            nonce_bytes: nonce.len(),
            tag_bytes: tag.len(),
            ciphertext_bytes: data.len(),
        },
    )
    .map_err(BundleFailure::Limits)?;
    let mut key = Zeroizing::new([0u8; 32]);
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(100_000).expect("fixed positive iteration count"),
        &salt,
        password.as_bytes(),
        &mut *key,
    );
    let cipher = Aes256Gcm::new_from_slice(&*key).map_err(|_| BundleFailure::Envelope)?;
    let iv: [u8; 12] = nonce.try_into().map_err(|_| BundleFailure::Envelope)?;
    data.extend_from_slice(&tag);
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(&Nonce::from(iv), data.as_slice())
            .map_err(|_| BundleFailure::Authentication)?,
    );
    let payload = OpenedBundle { plaintext };
    payload.entries()?;
    Ok(payload)
}

impl OpenedBundle {
    pub(super) fn entries(
        &self,
    ) -> Result<Vec<Result<ParsedAccount<'_>, EntryFailure>>, BundleFailure> {
        inspect_inner(&self.plaintext)
    }
}

fn json_string(raw: &RawValue) -> bool {
    raw.get().starts_with('"')
}

fn inspect_inner(
    plaintext: &[u8],
) -> Result<Vec<Result<ParsedAccount<'_>, EntryFailure>>, BundleFailure> {
    if plaintext.is_empty() || plaintext.len() > MAX_BUNDLE_BYTES {
        return Err(BundleFailure::Limits(BundleError::ResourceLimit));
    }
    let mut inner: Inner<'_> =
        serde_json::from_slice(plaintext).map_err(|_| BundleFailure::Inner)?;
    validate_inner(
        &inner.format,
        inner.version,
        inner.accounts.len(),
        plaintext.len(),
    )
    .map_err(BundleFailure::Limits)?;
    if !json_string(inner.exported_at) {
        return Err(BundleFailure::Inner);
    }
    Ok(std::mem::take(&mut inner.accounts)
        .into_iter()
        .map(|raw| {
            let account: AccountShape<'_> =
                serde_json::from_str(raw.get()).map_err(|_| EntryFailure::Shape)?;
            if !json_string(account.name)
                || !json_string(account.created_at)
                || !(account.config.get() == "null" || account.config.get().starts_with('{'))
            {
                return Err(EntryFailure::Shape);
            }
            if account.credentials.get().len() > MAX_DOCUMENT_BYTES {
                return Err(EntryFailure::ResourceLimit);
            }
            let record: StrictRecord<&RawValue> =
                serde_json::from_str(account.credentials.get()).map_err(|_| EntryFailure::Shape)?;
            if record.0.is_empty() || record.0.values().any(|raw| !json_string(raw)) {
                return Err(EntryFailure::Shape);
            }
            Ok(ParsedAccount {
                credentials: account.credentials,
            })
        })
        .collect())
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
