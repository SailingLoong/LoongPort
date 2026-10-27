//! Per-library automatic consent, mutated only under the existing runtime owner.
use super::Record;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Scope {
    pub(super) data_root: PathBuf,
    pub(super) enabled: bool,
    pub(super) participants: Vec<String>,
    pub(super) records: BTreeMap<String, Record>,
    #[serde(default)]
    pub(super) consent_revision: u64,
}
impl Scope {
    pub(super) fn revision(&self, context: &str) -> String {
        format!("{context}:{}", self.consent_revision)
    }
    pub(super) fn update(
        &mut self,
        context: &str,
        expected: &str,
        enabled: bool,
        participants: Vec<String>,
        valid: &BTreeSet<String>,
    ) -> Result<(), &'static str> {
        if expected != self.revision(context) {
            return Err("changed");
        }
        if participants.len() > 100
            || participants.iter().collect::<BTreeSet<_>>().len() != participants.len()
        {
            return Err("invalid");
        }
        if participants.iter().any(|id| !valid.contains(id)) {
            return Err("changed");
        }
        let next = self.consent_revision.checked_add(1).ok_or("unavailable")?;
        self.consent_revision = next;
        self.enabled = enabled;
        self.participants = participants;
        self.records.retain(|id, _| valid.contains(id));
        Ok(())
    }
}
