//! Backend-owned, bounded bundle review. Secrets never enter preview DTOs.
use super::capture_reviews::Binding;
use super::core::AccountSnapshot;
use super::session_checks::{SessionCheckDisplay, SessionCheckReport};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const LIFETIME: Duration = Duration::from_secs(600);
const CAPACITY: usize = 2;
pub(super) struct Review {
    pub binding: Binding,
    pub accounts: Vec<Option<AccountSnapshot>>,
    pub context: Option<super::library_context::LibraryContext>,
    issued: Instant,
    generation: u64,
    busy: bool,
    cancelled: Arc<AtomicBool>,
    selected: Vec<ImportChoice>,
    checked: BTreeMap<usize, SessionCheckReport>,
    status: &'static str,
}
#[derive(Default)]
pub(super) struct Reviews(HashMap<String, Review>);
impl Reviews {
    pub fn issue(
        &mut self,
        binding: Binding,
        accounts: Vec<Option<AccountSnapshot>>,
        now: Instant,
    ) -> String {
        self.prune(now);
        self.0.retain(|_, review| {
            let keep = review.binding != binding;
            if !keep {
                review.cancelled.store(true, Ordering::Release);
            }
            keep
        });
        if self.0.len() >= CAPACITY {
            if let Some(id) = self
                .0
                .iter()
                .min_by_key(|(_, review)| review.issued)
                .map(|(id, _)| id.clone())
            {
                self.cancel(&id);
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.0.insert(
            id.clone(),
            Review {
                binding,
                accounts,
                context: None,
                issued: now,
                generation: 0,
                busy: false,
                cancelled: Arc::new(AtomicBool::new(false)),
                selected: vec![],
                checked: BTreeMap::new(),
                status: "preview",
            },
        );
        id
    }
    pub fn prune(&mut self, now: Instant) {
        for review in self.0.values_mut() {
            if review.busy && review.cancelled.load(Ordering::Acquire) {
                review.busy = false;
                review.checked.clear();
                review.status = "failed";
            }
        }
        self.0.retain(|_, review| {
            let keep = now.saturating_duration_since(review.issued) < LIFETIME;
            if !keep {
                review.cancelled.store(true, Ordering::Release);
            }
            keep
        });
    }
    pub fn consume(&mut self, id: &str, now: Instant) -> Option<Review> {
        let result = self.0.remove(id);
        if let Some(review) = &result {
            review.cancelled.store(true, Ordering::Release);
        }
        self.prune(now);
        result.filter(|review| now.saturating_duration_since(review.issued) < LIFETIME)
    }
    pub fn cancel(&mut self, id: &str) {
        if let Some(review) = self.0.remove(id) {
            review.cancelled.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportChoice {
    pub index: usize,
    pub update_duplicate: bool,
}
#[derive(Debug)]
pub(super) enum CheckFailure {
    Missing,
    Busy,
    Changed,
    Invalid,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckRow {
    pub index: usize,
    pub capabilities: Option<SessionCheckDisplay>,
    pub error: Option<super::transaction::AccountActionError>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckProgress {
    pub preview_id: String,
    pub status: &'static str,
    pub selected: Vec<ImportChoice>,
    pub rows: Vec<CheckRow>,
    pub completed: usize,
    pub total: usize,
    pub error: Option<super::transaction::AccountActionError>,
}
pub(super) struct CheckLease {
    pub preview_id: String,
    pub generation: u64,
    pub binding: Binding,
    pub accounts: Vec<(usize, AccountSnapshot)>,
    cancelled: Arc<AtomicBool>,
    expires: Instant,
}
impl CheckLease {
    pub fn check(&self) -> Result<(), super::official::OfficialError> {
        if self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.expires {
            Err(super::official::OfficialError::Cancelled)
        } else {
            Ok(())
        }
    }
}
impl Drop for CheckLease {
    fn drop(&mut self) {
        // Covers a caller disappearing before an owned worker is handed off.
        // The next owner access releases only this generation's busy state.
        self.cancelled.store(true, Ordering::Release);
    }
}
impl Reviews {
    pub fn attach_context(
        &mut self,
        id: &str,
        context: super::library_context::LibraryContext,
    ) -> Result<(), CheckFailure> {
        let review = self.0.get_mut(id).ok_or(CheckFailure::Missing)?;
        if review.binding.context_revision != context.context_id() || review.busy {
            return Err(CheckFailure::Changed);
        }
        review.context = Some(context);
        Ok(())
    }
    pub fn review_context(
        &mut self,
        id: &str,
        now: Instant,
    ) -> Option<super::library_context::LibraryContext> {
        self.prune(now);
        self.0.get(id)?.context.clone()
    }
    pub fn begin_check(
        &mut self,
        id: &str,
        binding: &Binding,
        mut selected: Vec<ImportChoice>,
        now: Instant,
    ) -> Result<CheckLease, CheckFailure> {
        self.prune(now);
        let review = self.0.get_mut(id).ok_or(CheckFailure::Missing)?;
        if &review.binding != binding {
            return Err(CheckFailure::Changed);
        }
        if review.busy {
            return Err(CheckFailure::Busy);
        }
        if selected.is_empty() || selected.len() > 50 {
            return Err(CheckFailure::Invalid);
        }
        selected.sort_by_key(|choice| choice.index);
        let mut indices = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut accounts = vec![];
        for choice in &selected {
            let account = review
                .accounts
                .get(choice.index)
                .and_then(Option::as_ref)
                .ok_or(CheckFailure::Invalid)?;
            if !indices.insert(choice.index) || !identities.insert(account.identity().opaque_id()) {
                return Err(CheckFailure::Invalid);
            }
            accounts.push((choice.index, account.clone()));
        }
        review.cancelled.store(true, Ordering::Release);
        review.cancelled = Arc::new(AtomicBool::new(false));
        review.generation = review
            .generation
            .checked_add(1)
            .ok_or(CheckFailure::Invalid)?;
        review.busy = true;
        review.selected = selected;
        review.checked.clear();
        review.status = "checking";
        Ok(CheckLease {
            preview_id: id.into(),
            generation: review.generation,
            binding: binding.clone(),
            accounts,
            cancelled: Arc::clone(&review.cancelled),
            expires: review.issued + LIFETIME,
        })
    }
    pub fn record_result(
        &mut self,
        lease: &CheckLease,
        index: usize,
        report: SessionCheckReport,
    ) -> Result<(), CheckFailure> {
        lease.check().map_err(|_| CheckFailure::Changed)?;
        let review = self
            .0
            .get_mut(&lease.preview_id)
            .ok_or(CheckFailure::Missing)?;
        if review.generation != lease.generation || !review.busy {
            return Err(CheckFailure::Changed);
        }
        let snapshot = lease
            .accounts
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, s)| s)
            .ok_or(CheckFailure::Invalid)?;
        if report.display().selected_profile_id != snapshot.identity().opaque_id() {
            return Err(CheckFailure::Invalid);
        }
        review.checked.insert(index, report);
        Ok(())
    }
    pub fn finish_check(
        &mut self,
        lease: &CheckLease,
        reports: Vec<(usize, SessionCheckReport)>,
    ) -> Result<(), CheckFailure> {
        lease.check().map_err(|_| CheckFailure::Changed)?;
        for (index, report) in reports {
            self.record_result(lease, index, report)?;
        }
        let review = self
            .0
            .get_mut(&lease.preview_id)
            .ok_or(CheckFailure::Missing)?;
        if review.generation != lease.generation
            || !review.busy
            || review.checked.len() != review.selected.len()
        {
            return Err(CheckFailure::Changed);
        }
        review.busy = false;
        review.status = "ready";
        Ok(())
    }
    pub fn abandon_check(&mut self, id: &str, generation: u64) {
        if let Some(review) = self.0.get_mut(id) {
            if review.generation == generation && review.busy {
                review.cancelled.store(true, Ordering::Release);
                review.busy = false;
                review.checked.clear();
                review.status = "failed";
            }
        }
    }
    pub fn check_progress(&mut self, id: &str, now: Instant) -> Option<CheckProgress> {
        self.prune(now);
        let review = self.0.get(id)?;
        Some(CheckProgress {
            preview_id: id.into(),
            status: review.status,
            selected: review.selected.clone(),
            rows: review
                .selected
                .iter()
                .map(|choice| CheckRow {
                    index: choice.index,
                    capabilities: review
                        .checked
                        .get(&choice.index)
                        .map(SessionCheckReport::display),
                    error: None,
                })
                .collect(),
            completed: review.checked.len(),
            total: review.selected.len(),
            error: (review.status == "failed").then_some(super::transaction::AccountActionError {
                code: "zcode.account.bundle_check_failed",
                remedy: "queryOriginal",
                committed: false,
            }),
        })
    }
    pub fn review_binding(&mut self, id: &str, now: Instant) -> Option<Binding> {
        self.prune(now);
        self.0.get(id).map(|review| review.binding.clone())
    }
    pub fn consume_checked(
        &mut self,
        id: &str,
        binding: &Binding,
        selected: &[ImportChoice],
        now: Instant,
    ) -> Option<(Review, BTreeMap<usize, SessionCheckReport>)> {
        self.prune(now);
        let review = self.0.get(id)?;
        let mut choices = selected.to_vec();
        choices.sort_by_key(|choice| choice.index);
        if review.busy
            || review.status != "ready"
            || &review.binding != binding
            || choices != review.selected
        {
            return None;
        }
        let mut review = self.consume(id, now)?;
        let checked = std::mem::take(&mut review.checked);
        Some((review, checked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding(revision: u64) -> Binding {
        Binding {
            vault_root: "/synthetic/vault".into(),
            vault_id: "v".into(),
            key_id: "k".into(),
            vault_revision: revision,
            context_revision: "c".into(),
            catalog_revision: "p".into(),
        }
    }
    #[test]
    fn import_review_is_bounded_expiring_cancelable_and_one_use() {
        let now = Instant::now();
        let mut reviews = Reviews::default();
        let first = reviews.issue(binding(1), vec![], now);
        let newer = reviews.issue(binding(1), vec![], now);
        assert!(reviews.consume(&first, now).is_none());
        reviews.cancel(&newer);
        assert!(reviews.consume(&newer, now).is_none());
        let first = reviews.issue(binding(1), vec![], now);
        reviews.issue(binding(2), vec![], now + Duration::from_secs(1));
        let last = reviews.issue(binding(3), vec![], now + Duration::from_secs(2));
        assert_eq!(reviews.0.len(), CAPACITY);
        assert!(reviews.consume(&first, now).is_none());
        assert!(reviews
            .consume(&last, now + Duration::from_secs(2))
            .is_some());
        assert!(reviews.consume(&last, now).is_none());
        // Keep the synthetic clock monotonic: the second retained capability
        // was issued after `now` and is not expired at `now + LIFETIME`.
        let final_now = now + Duration::from_secs(3);
        let id = reviews.issue(binding(1), vec![], final_now);
        assert!(reviews.consume(&id, final_now + LIFETIME).is_none());
        assert!(reviews.0.is_empty());
    }
}

#[cfg(test)]
mod owned_check_tests {
    use super::super::core::OAuthFamily;
    use super::super::native::{
        tests::{native_document, TEST_CONTEXT, TEST_SECRET},
        NativeCipher,
    };
    use super::*;
    fn binding() -> Binding {
        Binding {
            vault_root: "/synthetic/vault".into(),
            vault_id: "v".into(),
            key_id: "k".into(),
            vault_revision: 1,
            context_revision: "c".into(),
            catalog_revision: "p".into(),
        }
    }
    fn account() -> AccountSnapshot {
        NativeCipher::new(TEST_CONTEXT, TEST_SECRET)
            .unwrap()
            .inspect(&native_document(OAuthFamily::Zai, "account", "synthetic"))
            .unwrap()
    }
    #[test]
    fn check_work_is_owned_cancelled_and_rejects_late_or_changed_selection() {
        let now = Instant::now();
        let mut reviews = Reviews::default();
        let id = reviews.issue(binding(), vec![Some(account())], now);
        let selected = vec![ImportChoice {
            index: 0,
            update_duplicate: false,
        }];
        let lease = reviews
            .begin_check(&id, &binding(), selected.clone(), now)
            .unwrap();
        assert!(reviews
            .begin_check(&id, &binding(), selected.clone(), now)
            .is_err());
        assert!(reviews
            .consume_checked(&id, &binding(), &selected, now)
            .is_none());
        reviews.cancel(&id);
        assert!(lease.check().is_err());
        assert!(reviews.finish_check(&lease, vec![]).is_err());
    }
    #[test]
    fn cancelled_old_check_cannot_publish_or_release_successor() {
        let now = Instant::now();
        let mut reviews = Reviews::default();
        let id = reviews.issue(binding(), vec![Some(account())], now);
        let selected = vec![ImportChoice {
            index: 0,
            update_duplicate: false,
        }];
        let old = reviews
            .begin_check(&id, &binding(), selected.clone(), now)
            .unwrap();
        reviews.abandon_check(&id, old.generation);
        let new = reviews.begin_check(&id, &binding(), selected, now).unwrap();
        reviews.abandon_check(&id, old.generation);
        assert_eq!(reviews.check_progress(&id, now).unwrap().status, "checking");
        assert!(reviews.finish_check(&old, vec![]).is_err());
        assert!(new.check().is_ok());
    }
}

#[cfg(test)]
mod dropped_check_tests {
    use super::*;
    #[test]
    fn dropped_unpolled_check_is_released_without_discarding_a_new_worker() {
        let native = super::super::native::NativeCipher::new(
            super::super::native::tests::TEST_CONTEXT,
            super::super::native::tests::TEST_SECRET,
        )
        .unwrap();
        let snapshot = native
            .inspect(&super::super::native::tests::native_document(
                super::super::core::OAuthFamily::Zai,
                "test",
                "synthetic",
            ))
            .unwrap();
        let binding = Binding {
            vault_root: "/synthetic/vault".into(),
            vault_id: "v".into(),
            key_id: "k".into(),
            vault_revision: 1,
            context_revision: "c".into(),
            catalog_revision: "r".into(),
        };
        let mut reviews = Reviews::default();
        let now = Instant::now();
        let id = reviews.issue(binding.clone(), vec![Some(snapshot)], now);
        let choices = vec![ImportChoice {
            index: 0,
            update_duplicate: false,
        }];
        let lease = reviews
            .begin_check(&id, &binding, choices.clone(), now)
            .unwrap();
        drop(lease);
        assert_eq!(reviews.check_progress(&id, now).unwrap().status, "failed");
        let new = reviews.begin_check(&id, &binding, choices, now).unwrap();
        assert!(new.check().is_ok());
        assert_eq!(reviews.check_progress(&id, now).unwrap().selected.len(), 1);
    }
}
