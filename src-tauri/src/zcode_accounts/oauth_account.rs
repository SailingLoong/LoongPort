//! Native-compatible image construction for one complete official login result.
//! Building an image is not a quota, connection or native-activation proof.
use super::core::{AccountIdentity, AccountSnapshot, CredentialDocument, OAuthFamily};
use super::native::{NativeCipher, NativeError};
use super::official::{BusinessToken, CodingKey, PollReady};

/// Reads the selected session's business credential; its cache identity is not
/// an upstream authorization proof.
pub(crate) fn saved_business_token(
    native: &NativeCipher,
    snapshot: &AccountSnapshot,
) -> Result<BusinessToken, NativeError> {
    let keys = snapshot.identity().credential_keys();
    let document = snapshot.scoped_document();
    let encrypted = document.get(&keys[1]).ok_or(NativeError::InvalidSession)?;
    let secret = native.decrypt(encrypted)?;
    BusinessToken::from_stored(snapshot.identity().family(), &secret)
        .map_err(|_| NativeError::InvalidSession)
}

/// Non-secret continuity binding for an explicitly selected saved session.
/// Encryption nonces and catalog metadata are not credentials.
pub(crate) fn saved_session_binding(
    native: &NativeCipher,
    snapshot: &AccountSnapshot,
) -> Result<[u8; 32], NativeError> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"zcode-saved-session:v1\0");
    digest.update(snapshot.identity().opaque_id());
    let document = snapshot.scoped_document();
    for key in snapshot.identity().credential_keys() {
        match document.get(&key) {
            Some(value) => {
                let plain = native.decrypt(value)?;
                digest.update([1]);
                digest.update((plain.len() as u64).to_be_bytes());
                digest.update(plain.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    Ok(digest.finalize().into())
}

pub(crate) fn saved_coding_key(
    native: &NativeCipher,
    snapshot: &AccountSnapshot,
) -> Option<CodingKey> {
    let keys = snapshot.identity().credential_keys();
    let document = snapshot.scoped_document();
    let secret = native.decrypt(document.get(&keys[5])?).ok()?;
    CodingKey::new(&secret).ok()
}

pub(crate) fn complete_coding_snapshot(
    native: &NativeCipher,
    snapshot: &AccountSnapshot,
    key: Option<&CodingKey>,
) -> Result<AccountSnapshot, NativeError> {
    let Some(key) = key else {
        return Ok(snapshot.clone());
    };
    let document = snapshot.scoped_document();
    if native.inspect(&document)?.identity() != snapshot.identity() {
        return Err(NativeError::InvalidSession);
    }
    let mut fields: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&document.to_bytes().map_err(NativeError::Core)?)
            .map_err(|_| NativeError::InvalidSession)?;
    fields.insert(
        snapshot.identity().credential_keys()[5].clone(),
        native.encrypt(key.expose())?,
    );
    let bytes = serde_json::to_vec(&fields).map_err(|_| NativeError::InvalidSession)?;
    native.inspect(&CredentialDocument::parse(&bytes).map_err(NativeError::Core)?)
}

pub(crate) fn coding_only_change(original: &AccountSnapshot, candidate: &AccountSnapshot) -> bool {
    if original.identity() != candidate.identity() {
        return false;
    }
    let before = original.scoped_document();
    let after = candidate.scoped_document();
    original
        .identity()
        .credential_keys()
        .iter()
        .enumerate()
        .all(|(index, key)| index == 5 || before.get(key) == after.get(key))
}

pub(crate) fn build_snapshot(
    native: &NativeCipher,
    ready: &PollReady,
    business: &BusinessToken,
    coding_key: Option<&CodingKey>,
) -> Result<AccountSnapshot, NativeError> {
    if ready.family != business.family() {
        return Err(NativeError::InvalidSession);
    }
    let identity = AccountIdentity::new(native.context(), ready.family, &ready.user.id)
        .map_err(NativeError::Core)?;
    let family = match ready.family {
        OAuthFamily::BigModel => "bigmodel",
        OAuthFamily::Zai => "zai",
    };
    let display = ready
        .user
        .name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            ready
                .user
                .email
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or(&ready.user.id);
    #[derive(serde::Serialize)]
    struct RawProfile<'a> {
        user_id: &'a str,
        name: Option<&'a str>,
        email: Option<&'a str>,
    }
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct UserInfo<'a> {
        id: &'a str,
        username: &'a str,
        display_name: &'a str,
        raw_profile: RawProfile<'a>,
    }
    let user = zeroize::Zeroizing::new(
        serde_json::to_string(&UserInfo {
            id: &ready.user.id,
            username: display,
            display_name: display,
            raw_profile: RawProfile {
                user_id: &ready.user.id,
                name: ready.user.name.as_deref(),
                email: ready.user.email.as_deref(),
            },
        })
        .map_err(|_| NativeError::InvalidSession)?,
    );
    let values = [
        Some(family),
        Some(business.expose()),
        ready.refresh_token.as_ref().map(|token| token.expose()),
        Some(user.as_str()),
        Some(ready.start_jwt.expose()),
        coding_key.map(CodingKey::expose),
        Some(ready.start_jwt.expose()),
    ];
    let mut encrypted = std::collections::BTreeMap::new();
    for (key, value) in identity.credential_keys().into_iter().zip(values) {
        if let Some(value) = value {
            encrypted.insert(key, native.encrypt(value)?);
        }
    }
    let bytes = serde_json::to_vec(&encrypted).map_err(|_| NativeError::InvalidSession)?;
    let document = CredentialDocument::parse(&bytes).map_err(NativeError::Core)?;
    native.inspect(&document)
}

#[cfg(test)]
#[path = "oauth_account_tests.rs"]
mod tests;
