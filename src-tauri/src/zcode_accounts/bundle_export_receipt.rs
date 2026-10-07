//! Bounded encrypted export evidence, separate from native switch recovery.
use super::{valid_request_id, ExportFailure};
use crate::secrets::{
    owned_file::{OwnedFile, BUNDLE_EXPORT_FILE},
    VaultContext,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[cfg(unix)]
use zeroize::Zeroizing;

const MAX_RECEIPTS: usize = 1024;
const MAX_PLAINTEXT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ReceiptPhase {
    Prepared,
    Saved,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExportTarget {
    pub context_id: String,
    pub destination: String,
    pub count: usize,
    pub ciphertext_sha256: [u8; 32],
    pub ciphertext_bytes: usize,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Receipt {
    pub request_id: String,
    pub phase: ReceiptPhase,
    pub target: Option<ExportTarget>,
    pub failure: Option<ExportFailure>,
}
impl Receipt {
    #[cfg(unix)]
    pub(super) fn prepared(request_id: &str) -> Self {
        Self {
            request_id: request_id.into(),
            phase: ReceiptPhase::Prepared,
            target: None,
            failure: None,
        }
    }
    fn validate(&self) -> Result<(), ExportFailure> {
        if !valid_request_id(&self.request_id)
            || match self.phase {
                ReceiptPhase::Prepared => self.failure.is_some(),
                ReceiptPhase::Saved => self.target.is_none() || self.failure.is_some(),
                ReceiptPhase::Failed => {
                    self.failure.is_none() || self.failure == Some(ExportFailure::ResultUnknown)
                }
            }
        {
            return Err(ExportFailure::SavedDataInvalid);
        }
        if let Some(target) = &self.target {
            if target.context_id.is_empty()
                || target.context_id.len() > 512
                || !super::valid_destination_shape(std::path::Path::new(&target.destination))
                || !(1..=50).contains(&target.count)
                || !(1..=super::super::bundle_limits::MAX_BUNDLE_BYTES)
                    .contains(&target.ciphertext_bytes)
            {
                return Err(ExportFailure::SavedDataInvalid);
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptLedger {
    version: u32,
    records: Vec<Receipt>,
}
impl Default for ReceiptLedger {
    fn default() -> Self {
        Self {
            version: 1,
            records: Vec::new(),
        }
    }
}
impl ReceiptLedger {
    pub(crate) fn get(&self, id: &str) -> Option<&Receipt> {
        self.records.iter().find(|record| record.request_id == id)
    }
    #[cfg(unix)]
    pub(crate) fn put(&mut self, receipt: Receipt) -> Result<(), ExportFailure> {
        receipt.validate()?;
        if let Some(old) = self
            .records
            .iter_mut()
            .find(|old| old.request_id == receipt.request_id)
        {
            if old == &receipt {
                return Ok(());
            }
            if old.phase != ReceiptPhase::Prepared
                || old
                    .target
                    .as_ref()
                    .is_some_and(|target| Some(target) != receipt.target.as_ref())
                || (receipt.phase == ReceiptPhase::Saved && old.target.is_none())
            {
                return Err(ExportFailure::SavedDataInvalid);
            }
            *old = receipt;
        } else {
            if self.records.len() >= MAX_RECEIPTS {
                return Err(ExportFailure::ResourceLimit);
            }
            if receipt.phase != ReceiptPhase::Prepared || receipt.target.is_some() {
                return Err(ExportFailure::SavedDataInvalid);
            }
            self.records.push(receipt);
        }
        Ok(())
    }
    #[cfg(unix)]
    pub(crate) fn seal(&self, vault: &VaultContext) -> Result<Vec<u8>, ExportFailure> {
        let bytes =
            Zeroizing::new(serde_json::to_vec(self).map_err(|_| ExportFailure::SavedDataInvalid)?);
        if bytes.len() > MAX_PLAINTEXT {
            return Err(ExportFailure::ResourceLimit);
        }
        OwnedFile::registered(BUNDLE_EXPORT_FILE)
            .map_err(|_| ExportFailure::Storage)?
            .encode(vault, &bytes)
            .map_err(|_| ExportFailure::Storage)
    }
    pub(crate) fn open(bytes: &[u8], vault: &VaultContext) -> Result<Self, ExportFailure> {
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(ExportFailure::ResourceLimit);
        }
        let plaintext = OwnedFile::registered(BUNDLE_EXPORT_FILE)
            .map_err(|_| ExportFailure::Storage)?
            .decode(vault, bytes)
            .map_err(|_| ExportFailure::SavedDataInvalid)?;
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(ExportFailure::ResourceLimit);
        }
        let ledger: Self =
            serde_json::from_slice(&plaintext).map_err(|_| ExportFailure::SavedDataInvalid)?;
        if ledger.version != 1 || ledger.records.len() > MAX_RECEIPTS {
            return Err(ExportFailure::SavedDataInvalid);
        }
        let mut ids = BTreeSet::new();
        for record in &ledger.records {
            record.validate()?;
            if !ids.insert(&record.request_id) {
                return Err(ExportFailure::SavedDataInvalid);
            }
        }
        Ok(ledger)
    }
}
