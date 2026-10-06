//! Fixed pjpv .zsb metadata limits, checked before expensive crypto work.
//! Acceptance here does not authenticate ciphertext or grant import permission.
//! Format reference: pjpv/zcode-switch f34225686dfef05d84c256a56f868719248f15ff.

pub(super) const MAX_BUNDLE_BYTES: usize = 10 * 1024 * 1024;
pub(super) const MAX_BUNDLE_ACCOUNTS: usize = 50;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BundleError {
    UnsupportedFormat,
    UnsupportedVersion,
    InvalidKdf,
    InvalidCipher,
    ResourceLimit,
}

/// Nonsecret metadata after bounded, strict JSON and base64 decoding. The
/// caller still rejects duplicate/unknown fields and authenticates AES-GCM.
pub(super) struct EnvelopeParameters<'a> {
    pub format: &'a str,
    pub version: u32,
    pub kdf: &'a str,
    pub iterations: u32,
    pub salt_bytes: usize,
    pub cipher: &'a str,
    pub nonce_bytes: usize,
    pub tag_bytes: usize,
    pub ciphertext_bytes: usize,
}

pub(super) fn validate_outer(
    file_bytes: usize,
    parameters: &EnvelopeParameters<'_>,
) -> Result<(), BundleError> {
    if file_bytes == 0
        || file_bytes > MAX_BUNDLE_BYTES
        || parameters.ciphertext_bytes > MAX_BUNDLE_BYTES
    {
        return Err(BundleError::ResourceLimit);
    }
    if parameters.format != "zsw-accounts-bundle" {
        return Err(BundleError::UnsupportedFormat);
    }
    if parameters.version != 1 {
        return Err(BundleError::UnsupportedVersion);
    }
    if parameters.kdf != "pbkdf2-hmac-sha256"
        || parameters.iterations != 100_000
        || parameters.salt_bytes != 16
    {
        return Err(BundleError::InvalidKdf);
    }
    if parameters.cipher != "aes-256-gcm"
        || parameters.nonce_bytes != 12
        || parameters.tag_bytes != 16
        || parameters.ciphertext_bytes == 0
    {
        return Err(BundleError::InvalidCipher);
    }
    Ok(())
}

pub(super) fn validate_inner(
    format: &str,
    version: u32,
    accounts: usize,
    plaintext_bytes: usize,
) -> Result<(), BundleError> {
    if plaintext_bytes == 0
        || plaintext_bytes > MAX_BUNDLE_BYTES
        || accounts == 0
        || accounts > MAX_BUNDLE_ACCOUNTS
    {
        return Err(BundleError::ResourceLimit);
    }
    if format != "zcode-accounts-bundle" {
        return Err(BundleError::UnsupportedFormat);
    }
    if version != 2 {
        return Err(BundleError::UnsupportedVersion);
    }
    Ok(())
}
