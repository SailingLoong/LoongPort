//! Native-compatible image construction for one complete official login result.
//! Building an image is not a quota, connection or native-activation proof.
use super::core::{AccountIdentity, AccountSnapshot, CredentialDocument, OAuthFamily};
use super::native::{NativeCipher, NativeError};
use super::official::{BusinessToken, CodingKey, PollReady};

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
