//! Completed imports live only in the authenticated ProfileCatalog image.
//! These bounded receipts contain identities and outcomes, never credentials.
use super::checkpoint::CheckpointError;
use super::transaction::CaptureCommitOutcome;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// Same retained-operation bound as login receipts. Never evict: a full catalog
// rejects the next import before publishing any account or completion result.
pub(super) const MAX_IMPORT_RECEIPTS: usize = super::checkpoint::MAX_LOGIN_RECEIPTS;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportReceiptRow {
    pub index: usize,
    pub account_id: String,
    pub family: String,
    pub candidate_revision: String,
    pub update_duplicate: bool,
    pub outcome: CaptureCommitOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportReceipt {
    pub request_id: String,
    pub catalog_revision: String,
    pub rows: Vec<ImportReceiptRow>,
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
impl ImportReceipt {
    pub(super) fn validate(&self) -> Result<(), CheckpointError> {
        if self.rows.is_empty() || self.rows.len() > super::bundle_limits::MAX_BUNDLE_ACCOUNTS {
            return Err(CheckpointError::ResourceLimit);
        }
        let mut indices = BTreeSet::new();
        let mut identities = BTreeSet::new();
        if uuid::Uuid::parse_str(&self.request_id).is_err()
            || !(self.catalog_revision == "absent"
                || self
                    .catalog_revision
                    .strip_prefix("sha256:")
                    .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
                    .is_some_and(|bytes| bytes.len() == 32))
            || self.rows.iter().any(|row| {
                row.index >= super::bundle_limits::MAX_BUNDLE_ACCOUNTS
                    || !digest(&row.account_id)
                    || !digest(&row.candidate_revision)
                    || !matches!(row.family.as_str(), "zai" | "bigmodel")
                    || !indices.insert(row.index)
                    || !identities.insert(&row.account_id)
            })
        {
            return Err(CheckpointError::InvalidPayload);
        }
        Ok(())
    }
    pub(crate) fn matches_request(&self, revision: &str, choices: &[(usize, bool)]) -> bool {
        self.catalog_revision == revision
            && self.rows.len() == choices.len()
            && self.rows.iter().zip(choices).all(|(row, (index, update))| {
                row.index == *index && row.update_duplicate == *update
            })
    }
    pub(crate) fn outcomes(&self) -> Vec<CaptureCommitOutcome> {
        self.rows.iter().map(|row| row.outcome).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ImportResultStatus {
    Completed,
    NotRecorded,
    Unknown,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportResult {
    pub(crate) request_id: String,
    pub(crate) status: ImportResultStatus,
    rows: Vec<ImportResultRow>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResultRow {
    index: usize,
    id: String,
    family: String,
    outcome: CaptureCommitOutcome,
}
impl From<ImportReceipt> for ImportResult {
    fn from(receipt: ImportReceipt) -> Self {
        Self {
            request_id: receipt.request_id,
            status: ImportResultStatus::Completed,
            rows: receipt
                .rows
                .into_iter()
                .map(|row| ImportResultRow {
                    index: row.index,
                    id: row.account_id,
                    family: row.family,
                    outcome: row.outcome,
                })
                .collect(),
        }
    }
}

impl ImportResult {
    pub(crate) fn unresolved(request_id: &str, status: ImportResultStatus) -> Self {
        Self {
            request_id: request_id.into(),
            status,
            rows: Vec::new(),
        }
    }
}
