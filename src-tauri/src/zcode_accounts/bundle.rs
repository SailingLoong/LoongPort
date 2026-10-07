//! Fixed .zsb encoding/authentication and bounded, strict structure inspection.
//! This module never accesses a path, imports config, or grants account scope.
use super::bundle_limits::{
    validate_inner, validate_outer, BundleError, EnvelopeParameters, MAX_BUNDLE_BYTES,
};
use super::core::{AccountSnapshot, StrictRecord, MAX_DOCUMENT_BYTES};
use super::native::NativeCipher;
use aes_gcm::{
    aead::{Aead, AeadInOut, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{borrow::Cow, collections::BTreeMap, io, num::NonZeroU32};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BundleFailure {
    Limits(BundleError),
    Envelope,
    Authentication,
    Inner,
    Password,
    Account,
    Encryption,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum EntryFailure {
    Shape,
    ResourceLimit,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    version: u32,
    kdf: Kdf,
    cipher: Cipher,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Kdf {
    algo: String,
    iters: u32,
    salt: String,
}
#[derive(Deserialize, Serialize)]
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

pub(super) struct BundleExportAccount<'a> {
    pub snapshot: &'a AccountSnapshot,
    pub created_at: &'a str,
}

#[derive(Serialize)]
struct ExportInner<'a> {
    format: &'static str,
    version: u32,
    #[serde(rename = "exportedAt")]
    exported_at: &'a str,
    accounts: Vec<ExportAccount<'a>>,
}

#[derive(Serialize)]
struct ExportAccount<'a> {
    name: &'a str,
    #[serde(rename = "createdAt")]
    created_at: &'a str,
    credentials: &'a BTreeMap<String, String>,
    config: Option<()>,
}

/// Same-environment backup of exactly the supplied saved snapshots. The native
/// cipher authenticates their scope and supplies the official profile name;
/// callers cannot substitute LoongPort labels or capability metadata. Inner
/// enc:v1 values are retained byte-for-byte, so this is not portable migration.
pub(super) fn encode_bundle(
    accounts: &[BundleExportAccount<'_>],
    native: &NativeCipher,
    password: &str,
    exported_at: &str,
) -> Result<Vec<u8>, BundleFailure> {
    validate_password(password)?;
    validate_inner("zcode-accounts-bundle", 2, accounts.len(), 1).map_err(BundleFailure::Limits)?;
    let mut envelope = Envelope {
        format: "zsw-accounts-bundle".into(),
        version: 1,
        kdf: Kdf {
            algo: "pbkdf2-hmac-sha256".into(),
            iters: 100_000,
            salt: STANDARD.encode([0u8; 16]),
        },
        cipher: Cipher {
            algo: "aes-256-gcm".into(),
            nonce: STANDARD.encode([0u8; 12]),
            tag: STANDARD.encode([0u8; 16]),
            data: String::new(),
        },
    };
    // Standard base64 expands every three ciphertext bytes to four. All other
    // outer fields have fixed serialized sizes; reject before allocating the
    // plaintext, requesting randomness, or running the password KDF.
    let overhead = json_size(&envelope, MAX_BUNDLE_BYTES)?;
    let max_plaintext = (MAX_BUNDLE_BYTES - overhead) / 4 * 3;
    let mut documents = Vec::with_capacity(accounts.len());
    let mut names = Vec::with_capacity(accounts.len());
    let mut credential_bytes = 0;
    for account in accounts {
        let document = account.snapshot.scoped_document();
        credential_bytes += json_size(document.entries(), MAX_DOCUMENT_BYTES)?;
        if credential_bytes > max_plaintext {
            return Err(BundleFailure::Limits(BundleError::ResourceLimit));
        }
        let name = native
            .profile_label(account.snapshot)
            .map_err(|_| BundleFailure::Account)?
            .unwrap_or_default();
        documents.push(document);
        names.push(Zeroizing::new(name));
    }
    let inner = ExportInner {
        format: "zcode-accounts-bundle",
        version: 2,
        exported_at,
        accounts: accounts
            .iter()
            .zip(&documents)
            .zip(&names)
            .map(|((account, document), name)| ExportAccount {
                name,
                created_at: account.created_at,
                credentials: document.entries(),
                config: None,
            })
            .collect(),
    };
    let plaintext_bytes = json_size(&inner, max_plaintext)?;
    // Exact capacity includes the GCM tag, avoiding reallocations that could
    // leave former plaintext allocations behind. Encryption happens in place.
    let mut buffer = Zeroizing::new(Vec::with_capacity(plaintext_bytes + 16));
    serde_json::to_writer(&mut *buffer, &inner).map_err(|_| BundleFailure::Inner)?;
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut salt).map_err(|_| BundleFailure::Encryption)?;
    getrandom::fill(&mut nonce).map_err(|_| BundleFailure::Encryption)?;
    let key = derive_key(password, &salt);
    let cipher = Aes256Gcm::new_from_slice(&*key).map_err(|_| BundleFailure::Encryption)?;
    cipher
        .encrypt_in_place(&Nonce::from(nonce), b"", &mut *buffer)
        .map_err(|_| BundleFailure::Encryption)?;
    let (data, tag) = buffer.split_at(plaintext_bytes);
    envelope.kdf.salt = STANDARD.encode(salt);
    envelope.cipher.nonce = STANDARD.encode(nonce);
    envelope.cipher.tag = STANDARD.encode(tag);
    envelope.cipher.data = STANDARD.encode(data);
    let file = serde_json::to_vec(&envelope).map_err(|_| BundleFailure::Envelope)?;
    if file.len() > MAX_BUNDLE_BYTES {
        return Err(BundleFailure::Limits(BundleError::ResourceLimit));
    }
    Ok(file)
}

/// Counts JSON bytes without storing any plaintext or allocating a large image.
fn json_size(value: &impl Serialize, limit: usize) -> Result<usize, BundleFailure> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit - self.bytes {
                return Err(io::Error::other("bundle size limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| BundleFailure::Limits(BundleError::ResourceLimit))?;
    Ok(counter.bytes)
}

fn validate_password(password: &str) -> Result<(), BundleFailure> {
    if password.trim().is_empty() || password.len() > 4096 {
        return Err(BundleFailure::Password);
    }
    Ok(())
}

fn derive_key(password: &str, salt: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(100_000).expect("fixed positive iteration count"),
        salt,
        password.as_bytes(),
        &mut *key,
    );
    key
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
    validate_password(password)?;
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
    let key = derive_key(password, &salt);
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
