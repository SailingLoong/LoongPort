//! Pure ZCode account-switch decisions. Not connected to a live command.
//!
//! The eventual adapter must authenticate the native session and its identity,
//! enforce version/process/lock gates, and protect every persisted snapshot.
//! Nothing in this module reads a file, environment, credential store or network.
//! Contract: zai-org/ZCode 29628c9acdb81b703bbd4080c207a0e7ce5e276e,
//! services/model-provider/accountProviderCredentialKey.ts and oauthCredentialRepo.ts.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreError {
    InvalidDocument,
    DocumentTooLarge,
    InvalidIdentity,
    MissingSessionCredential,
    DifferentContext,
    DifferentFamily,
    SourceChanged,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialDocument(BTreeMap<String, String>);

impl CredentialDocument {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(CoreError::DocumentTooLarge);
        }
        serde_json::from_slice::<StrictRecord>(bytes)
            .map(|record| Self(record.0))
            .map_err(|_| CoreError::InvalidDocument)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, CoreError> {
        let bytes = serde_json::to_vec(&self.0).map_err(|_| CoreError::InvalidDocument)?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(CoreError::DocumentTooLarge);
        }
        Ok(bytes)
    }
}

struct StrictRecord(BTreeMap<String, String>);

impl<'de> Deserialize<'de> for StrictRecord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RecordVisitor;
        impl<'de> Visitor<'de> for RecordVisitor {
            type Value = StrictRecord;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a credential string map without duplicate keys")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate credential key"));
                    }
                }
                Ok(StrictRecord(values))
            }
        }
        deserializer.deserialize_map(RecordVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OAuthFamily {
    Zai,
    BigModel,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AccountIdentity {
    context: String,
    family: OAuthFamily,
    account_id: String,
}

impl AccountIdentity {
    /// Normalizes the official key identity; does not authenticate it.
    pub fn new(context: &str, family: OAuthFamily, account_id: &str) -> Result<Self, CoreError> {
        let account_id = js_trim(account_id);
        if js_trim(context).is_empty() || account_id.is_empty() || account_id == "unknown" {
            return Err(CoreError::InvalidIdentity);
        }
        Ok(Self {
            context: context.into(),
            family,
            account_id: account_id.into(),
        })
    }

    pub fn credential_keys(&self) -> Vec<String> {
        let provider = match self.family {
            OAuthFamily::Zai => "zai",
            OAuthFamily::BigModel => "bigmodel",
        };
        let account = encode_uri_component(&self.account_id);
        vec![
            "oauth:active_provider".into(),
            format!("oauth:{provider}:access_token"),
            format!("oauth:{provider}:refresh_token"),
            format!("oauth:{provider}:user_info"),
            "zcodejwttoken".into(),
            format!("account-provider:coding-plan:account:{provider}-individual-coding-plan:account:{account}:api-key"),
            format!("account-provider:start-plan:account:{provider}-start-plan:account:{account}:api-key"),
        ]
    }
}

#[derive(Clone)]
pub struct AccountSnapshot {
    identity: AccountIdentity,
    values: BTreeMap<String, Option<String>>,
}

impl AccountSnapshot {
    /// The caller must already have authenticated this document's native identity.
    pub fn capture(
        identity: AccountIdentity,
        document: &CredentialDocument,
    ) -> Result<Self, CoreError> {
        let keys = identity.credential_keys();
        for index in [0, 1, 3] {
            if document.get(&keys[index]).is_none_or(str::is_empty) {
                return Err(CoreError::MissingSessionCredential);
            }
        }
        let values = keys
            .into_iter()
            .map(|key| {
                let value = document.0.get(&key).cloned();
                (key, value)
            })
            .collect();
        Ok(Self { identity, values })
    }
}

pub struct SwitchPlan {
    source: CredentialDocument,
    fresh_source: AccountSnapshot,
    target: AccountSnapshot,
}

impl SwitchPlan {
    pub fn prepare(
        current: &CredentialDocument,
        identity: &AccountIdentity,
        target: &AccountSnapshot,
    ) -> Result<Self, CoreError> {
        if identity.context != target.identity.context {
            return Err(CoreError::DifferentContext);
        }
        if identity.family != target.identity.family {
            return Err(CoreError::DifferentFamily);
        }
        let fresh_source = AccountSnapshot::capture(identity.clone(), current)?;
        Ok(Self {
            source: current.clone(),
            target: if identity == &target.identity {
                fresh_source.clone()
            } else {
                target.clone()
            },
            fresh_source,
        })
    }

    pub fn apply(&self, current: &CredentialDocument) -> Result<CredentialDocument, CoreError> {
        if current != &self.source {
            return Err(CoreError::SourceChanged);
        }
        if self.is_noop() {
            return Ok(current.clone());
        }
        let mut result = current.clone();
        for (key, value) in &self.target.values {
            match value {
                Some(value) => {
                    result.0.insert(key.clone(), value.clone());
                }
                None => {
                    result.0.remove(key);
                }
            }
        }
        result.to_bytes()?;
        Ok(result)
    }

    pub fn fresh_source(&self) -> &AccountSnapshot {
        &self.fresh_source
    }

    pub fn is_noop(&self) -> bool {
        self.fresh_source.identity == self.target.identity
    }
}

// Rust's is_whitespace differs from ECMAScript trim (notably U+0085 and U+FEFF).
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c| {
        matches!(c,
            '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
            '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
            '\u{205f}' | '\u{3000}' | '\u{feff}')
    })
}

fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
