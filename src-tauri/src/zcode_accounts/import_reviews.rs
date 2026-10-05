//! Backend-owned, bounded bundle review. Secrets never enter preview DTOs.
use super::capture_reviews::Binding;
use super::core::AccountSnapshot;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const LIFETIME: Duration = Duration::from_secs(120);
const CAPACITY: usize = 2;
pub(super) struct Review {
    pub binding: Binding,
    pub accounts: Vec<Option<AccountSnapshot>>,
    issued: Instant,
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
        self.0.retain(|_, review| review.binding != binding);
        if self.0.len() >= CAPACITY {
            if let Some(id) = self
                .0
                .iter()
                .min_by_key(|(_, review)| review.issued)
                .map(|(id, _)| id.clone())
            {
                self.0.remove(&id);
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.0.insert(
            id.clone(),
            Review {
                binding,
                accounts,
                issued: now,
            },
        );
        id
    }
    pub fn prune(&mut self, now: Instant) {
        self.0
            .retain(|_, review| now.saturating_duration_since(review.issued) < LIFETIME);
    }
    pub fn consume(&mut self, id: &str, now: Instant) -> Option<Review> {
        let result = self.0.remove(id);
        self.prune(now);
        result.filter(|review| now.saturating_duration_since(review.issued) < LIFETIME)
    }
    pub fn cancel(&mut self, id: &str) {
        self.0.remove(id);
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
