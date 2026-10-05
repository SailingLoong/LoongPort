//! Authenticated bounded local operation evidence, never an admission capability.
use crate::secrets::{
    owned_file::{OwnedFile, OPERATION_FILE},
    VaultContext,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Accepted,
    Exited,
    TransactionUncertain,
    Committed,
    RestartVerified,
    RestartFailed,
    Failed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Record {
    pub request_id: String,
    pub context_id: String,
    pub native_root: [u64; 2],
    pub target: String,
    pub phase: Phase,
    pub refreshed: bool,
    pub restart_requested: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Log {
    records: Vec<Record>,
    retired: Vec<String>,
}
impl Log {
    pub fn get(&self, id: &str) -> Option<&Record> {
        self.records.iter().find(|record| record.request_id == id)
    }
    pub fn known(&self, id: &str) -> bool {
        self.get(id).is_some() || self.retired.iter().any(|old| old == id)
    }
    pub fn put(&mut self, record: Record) -> Result<(), ()> {
        if self.retired.contains(&record.request_id) {
            return Err(());
        }
        if uuid::Uuid::parse_str(&record.request_id)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(&record.request_id)
            || record.target.is_empty()
            || record.context_id.is_empty()
        {
            return Err(());
        }
        if let Some(old) = self
            .records
            .iter_mut()
            .find(|old| old.request_id == record.request_id)
        {
            if old.context_id != record.context_id
                || old.native_root != record.native_root
                || old.target != record.target
            {
                return Err(());
            }
            if matches!(
                old.phase,
                Phase::Committed | Phase::RestartVerified | Phase::RestartFailed
            ) && !matches!(
                record.phase,
                Phase::Committed | Phase::RestartVerified | Phase::RestartFailed
            ) {
                return Err(());
            }
            *old = record;
            return Ok(());
        }
        if self.records.len() >= 16 {
            let index = self
                .records
                .iter()
                .position(|record| {
                    matches!(
                        record.phase,
                        Phase::RestartVerified | Phase::RestartFailed | Phase::Failed
                    ) || (record.phase == Phase::Committed && !record.restart_requested)
                })
                .ok_or(())?;
            if self.retired.len() >= 1024 {
                return Err(());
            }
            self.retired.push(self.records.remove(index).request_id);
        }
        self.records.push(record);
        Ok(())
    }
    pub fn seal(&self, vault: &VaultContext) -> Result<Vec<u8>, ()> {
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(self).map_err(|_| ())?);
        if bytes.len() > 64 * 1024 {
            return Err(());
        }
        OwnedFile::registered(OPERATION_FILE)
            .map_err(|_| ())?
            .encode(vault, &bytes)
            .map_err(|_| ())
    }
    pub fn open(bytes: &[u8], vault: &VaultContext) -> Result<Self, ()> {
        if bytes.len() > 128 * 1024 {
            return Err(());
        }
        let plaintext = OwnedFile::registered(OPERATION_FILE)
            .map_err(|_| ())?
            .decode(vault, bytes)
            .map_err(|_| ())?;
        if plaintext.len() > 64 * 1024 {
            return Err(());
        }
        let parsed: Self = serde_json::from_slice(&plaintext).map_err(|_| ())?;
        if parsed.records.len() > 16 || parsed.retired.len() > 1024 {
            return Err(());
        }
        for id in &parsed.retired {
            if uuid::Uuid::parse_str(id)
                .map(|id| id.to_string())
                .ok()
                .as_deref()
                != Some(id.as_str())
            {
                return Err(());
            }
        }
        let retired: std::collections::BTreeSet<_> = parsed.retired.iter().collect();
        if retired.len() != parsed.retired.len()
            || parsed
                .records
                .iter()
                .any(|record| retired.contains(&record.request_id))
        {
            return Err(());
        }
        let mut checked = Self::default();
        for record in parsed.records {
            if checked.get(&record.request_id).is_some() {
                return Err(());
            }
            checked.put(record)?;
        }
        checked.retired = parsed.retired;
        Ok(checked)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> Record {
        Record {
            request_id: uuid::Uuid::new_v4().to_string(),
            context_id: "synthetic-source".into(),
            native_root: [1, 2],
            target: "opaque-target".into(),
            phase: Phase::Accepted,
            refreshed: false,
            restart_requested: false,
        }
    }
    #[test]
    fn authenticated_log_retains_commit_and_rejects_other_vault_and_rebinding() {
        let vault = VaultContext::generate().unwrap();
        let mut log = Log::default();
        let mut r = record();
        log.put(r.clone()).unwrap();
        r.phase = Phase::Committed;
        log.put(r.clone()).unwrap();
        let bytes = log.seal(&vault).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("opaque-target"));
        let mut open = Log::open(&bytes, &vault).unwrap();
        assert_eq!(open.get(&r.request_id).unwrap().phase, Phase::Committed);
        assert!(Log::open(&bytes, &VaultContext::generate().unwrap()).is_err());
        r.phase = Phase::Failed;
        assert!(open.put(r.clone()).is_err());
        r.phase = Phase::Committed;
        r.target = "other".into();
        assert!(open.put(r).is_err());
    }
    #[test]
    fn evicted_request_identity_remains_rejected_after_reopen() {
        let vault = VaultContext::generate().unwrap();
        let mut log = Log::default();
        let mut first = record();
        first.phase = Phase::Failed;
        log.put(first.clone()).unwrap();
        for _ in 0..16 {
            let mut done = record();
            done.phase = Phase::Failed;
            log.put(done).unwrap();
        }
        let mut reopened = Log::open(&log.seal(&vault).unwrap(), &vault).unwrap();
        assert!(reopened.get(&first.request_id).is_none());
        assert!(reopened.known(&first.request_id));
        assert!(reopened.put(first).is_err());
    }
    #[test]
    fn active_records_are_bounded_and_never_silently_evicted() {
        let mut log = Log::default();
        for _ in 0..16 {
            log.put(record()).unwrap();
        }
        assert!(log.put(record()).is_err());
        let mut done = log.records[0].clone();
        done.phase = Phase::Failed;
        log.put(done).unwrap();
        log.put(record()).unwrap();
        assert_eq!(log.records.len(), 16);
    }
}

#[cfg(test)]
mod bound_tests {
    use super::*;
    #[test]
    fn retirement_capacity_fails_closed_without_forgetting_the_first_identity() {
        let vault = VaultContext::generate().unwrap();
        let mut log = Log::default();
        let first = uuid::Uuid::new_v4().to_string();
        for index in 0..1040 {
            log.put(Record {
                request_id: if index == 0 {
                    first.clone()
                } else {
                    uuid::Uuid::new_v4().to_string()
                },
                context_id: "synthetic".into(),
                native_root: [1, 2],
                target: "opaque".into(),
                phase: Phase::Failed,
                refreshed: false,
                restart_requested: false,
            })
            .unwrap();
        }
        assert_eq!(log.retired.len(), 1024);
        let mut reopened = Log::open(&log.seal(&vault).unwrap(), &vault).unwrap();
        let candidate = Record {
            request_id: uuid::Uuid::new_v4().to_string(),
            context_id: "synthetic".into(),
            native_root: [1, 2],
            target: "opaque".into(),
            phase: Phase::Accepted,
            refreshed: false,
            restart_requested: false,
        };
        assert!(reopened.put(candidate).is_err());
        assert!(reopened.known(&first));
        assert_eq!(reopened.retired.len(), 1024);
    }
}
