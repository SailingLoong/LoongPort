//! Encrypted creation intent for one reviewed official account and personal project.
//! Existing intent is recover-only. A fresh no-POST proof cannot clear older uncertainty.
use super::core::OAuthFamily;
use crate::secrets::{
    owned_file::{OwnedFile, KEY_INTENT_FILE},
    VaultContext,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KeyScope {
    family: String,
    account_id: String,
    organization_id: String,
    project_id: String,
}
impl KeyScope {
    pub(crate) fn new(
        family: OAuthFamily,
        account_id: &str,
        organization_id: &str,
        project_id: &str,
    ) -> Result<Self, IntentError> {
        let scope = Self {
            family: match family {
                OAuthFamily::BigModel => "bigmodel",
                OAuthFamily::Zai => "zai",
            }
            .into(),
            account_id: account_id.into(),
            organization_id: organization_id.into(),
            project_id: project_id.into(),
        };
        scope.validate()?;
        Ok(scope)
    }
    fn validate(&self) -> Result<(), IntentError> {
        if !matches!(self.family.as_str(), "bigmodel" | "zai")
            || [&self.account_id, &self.organization_id, &self.project_id]
                .into_iter()
                .any(|value| {
                    value.is_empty()
                        || value.len() > 1024
                        || value.trim() != value
                        || value.chars().any(char::is_control)
                })
        {
            return Err(IntentError::Invalid);
        }
        Ok(())
    }
    fn same_project(&self, other: &Self) -> bool {
        self.family == other.family
            && self.organization_id == other.organization_id
            && self.project_id == other.project_id
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum IntentState {
    Pending,
    Created,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KeyIntent {
    request_id: String,
    scope: KeyScope,
    pub state: IntentState,
}
pub(crate) struct FreshIntent {
    request_id: String,
    scope: KeyScope,
}
impl FreshIntent {
    /// Exact non-secret receipt, usable only after the owner has copied the
    /// corresponding project Key. It never grants another remote create.
    pub(crate) fn receipt(&self) -> KeyIntent {
        KeyIntent {
            request_id: self.request_id.clone(),
            scope: self.scope.clone(),
            state: IntentState::Pending,
        }
    }
}
pub(crate) enum Reservation {
    Fresh(FreshIntent),
    Existing(KeyIntent),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntentError {
    Invalid,
    Limit,
    Authentication,
    Stale,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyIntentLedger {
    version: u32,
    entries: Vec<KeyIntent>,
}
impl Default for KeyIntentLedger {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}
impl KeyIntentLedger {
    pub(crate) fn reserve(&mut self, scope: KeyScope) -> Result<Reservation, IntentError> {
        scope.validate()?;
        if let Some(record) = self.get(&scope) {
            return Ok(Reservation::Existing(record.clone()));
        }
        if self.entries.len() >= 64 {
            return Err(IntentError::Limit);
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        self.entries.push(KeyIntent {
            request_id: request_id.clone(),
            scope: scope.clone(),
            state: IntentState::Pending,
        });
        Ok(Reservation::Fresh(FreshIntent { request_id, scope }))
    }
    pub(crate) fn mark_created(&mut self, grant: &FreshIntent) -> Result<(), IntentError> {
        let record = self
            .entries
            .iter_mut()
            .find(|entry| entry.request_id == grant.request_id && entry.scope == grant.scope)
            .ok_or(IntentError::Stale)?;
        record.state = IntentState::Created;
        Ok(())
    }
    pub(crate) fn clear_unsubmitted(&mut self, grant: &FreshIntent) -> Result<(), IntentError> {
        let index = self
            .entries
            .iter()
            .position(|entry| {
                entry.request_id == grant.request_id
                    && entry.scope == grant.scope
                    && entry.state == IntentState::Pending
            })
            .ok_or(IntentError::Stale)?;
        self.entries.remove(index);
        Ok(())
    }
    /// Caller has rediscovered and copied the matching Key, or saved the completed session.
    pub(crate) fn clear_resolved(&mut self, record: &KeyIntent) -> Result<(), IntentError> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.request_id == record.request_id && entry.scope == record.scope)
            .ok_or(IntentError::Stale)?;
        self.entries.remove(index);
        Ok(())
    }
    pub(crate) fn get(&self, scope: &KeyScope) -> Option<&KeyIntent> {
        // Cache identity is not a remote resource owner. Different local slots
        // must not independently grant two creates against the same project.
        self.entries
            .iter()
            .find(|entry| entry.scope.same_project(scope))
    }
    /// Presence only. A present request still requires the exact scope/state
    /// checks in clear_unsubmitted / clear_resolved.
    pub(crate) fn contains_request(&self, record: &KeyIntent) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.request_id == record.request_id)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub(crate) fn seal(&self, vault: &VaultContext) -> Result<Vec<u8>, IntentError> {
        self.validate()?;
        let bytes =
            zeroize::Zeroizing::new(serde_json::to_vec(self).map_err(|_| IntentError::Invalid)?);
        if bytes.len() > 64 * 1024 {
            return Err(IntentError::Limit);
        }
        OwnedFile::registered(KEY_INTENT_FILE)
            .map_err(|_| IntentError::Authentication)?
            .encode(vault, &bytes)
            .map_err(|_| IntentError::Authentication)
    }
    pub(crate) fn open(bytes: &[u8], vault: &VaultContext) -> Result<Self, IntentError> {
        if bytes.len() > 128 * 1024 {
            return Err(IntentError::Limit);
        }
        let plaintext = OwnedFile::registered(KEY_INTENT_FILE)
            .map_err(|_| IntentError::Authentication)?
            .decode(vault, bytes)
            .map_err(|_| IntentError::Authentication)?;
        if plaintext.len() > 64 * 1024 {
            return Err(IntentError::Limit);
        }
        let result: Self = serde_json::from_slice(&plaintext).map_err(|_| IntentError::Invalid)?;
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), IntentError> {
        if self.version != 1 || self.entries.len() > 64 {
            return Err(IntentError::Invalid);
        }
        let mut scopes = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        for record in &self.entries {
            record.scope.validate()?;
            if uuid::Uuid::parse_str(&record.request_id)
                .map(|id| id.to_string())
                .ok()
                .as_deref()
                != Some(&record.request_id)
                || !ids.insert(&record.request_id)
                || !scopes.insert((
                    &record.scope.family,
                    &record.scope.organization_id,
                    &record.scope.project_id,
                ))
            {
                return Err(IntentError::Invalid);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "key_intent_tests.rs"]
mod tests;
