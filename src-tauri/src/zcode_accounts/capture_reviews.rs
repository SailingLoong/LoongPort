//! Ephemeral, bounded review capabilities. Contains revisions, never credentials.
use std::collections::HashMap;
use std::time::{Duration, Instant};

const MAX_REVIEWS: usize = 16;
const LIFETIME: Duration = Duration::from_secs(120);
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Binding {
    pub vault_root: std::path::PathBuf,
    pub storage: crate::secrets::storage::StorageIdentity,
    pub context_revision: String,
    pub catalog_revision: String,
}
pub(super) struct Review {
    pub binding: Binding,
    pub native_revision: String,
    pub identity: String,
    issued: Instant,
}
#[derive(Default)]
pub(super) struct Reviews(HashMap<String, Review>);
impl Reviews {
    pub fn issue(
        &mut self,
        binding: Binding,
        native_revision: String,
        identity: String,
        now: Instant,
    ) -> String {
        self.0.retain(|_, review| {
            now.saturating_duration_since(review.issued) < LIFETIME && review.binding != binding
        });
        if self.0.len() >= MAX_REVIEWS {
            if let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, review)| review.issued)
                .map(|(id, _)| id.clone())
            {
                self.0.remove(&oldest);
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.0.insert(
            id.clone(),
            Review {
                binding,
                native_revision,
                identity,
                issued: now,
            },
        );
        id
    }
    /// Consume before validating: stale, failed or repeated submissions need a new review.
    pub fn consume(&mut self, id: &str, now: Instant) -> Option<Review> {
        let review = self.0.remove(id)?;
        (now.saturating_duration_since(review.issued) < LIFETIME).then_some(review)
    }
    #[cfg(test)]
    fn take(&mut self, id: &str, binding: &Binding, now: Instant) -> Option<Review> {
        self.consume(id, now)
            .filter(|review| review.binding == *binding)
    }
    pub fn cancel(&mut self, id: &str) {
        self.0.remove(id);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            vault_root: "/synthetic/vault".into(),
            storage: super::super::synthetic_storage_identity(1),
            context_revision: "context".into(),
            catalog_revision: "catalog".into(),
        }
    }
    #[test]
    fn one_use_cancel_expiry_and_unknown() {
        let mut reviews = Reviews::default();
        let now = Instant::now();
        let b = binding();
        let id = reviews.issue(b.clone(), "native".into(), "identity".into(), now);
        let taken = reviews.take(&id, &b, now).unwrap();
        assert_eq!(taken.native_revision, "native");
        assert_eq!(taken.identity, "identity");
        assert!(reviews.take(&id, &b, now).is_none());
        let id = reviews.issue(b.clone(), "native".into(), "identity".into(), now);
        reviews.cancel(&id);
        assert!(reviews.take(&id, &b, now).is_none());
        let id = reviews.issue(b.clone(), "native".into(), "identity".into(), now);
        assert!(reviews.take(&id, &b, now + LIFETIME).is_none());
        assert!(reviews.take("unknown", &b, now).is_none());
    }
    fn changed_storage(
        identity: &crate::secrets::storage::StorageIdentity,
        field: u8,
    ) -> crate::secrets::storage::StorageIdentity {
        let mut identity = identity.clone();
        let crate::secrets::storage::StorageIdentity::Plaintext(plain) = &mut identity else {
            unreachable!()
        };
        match field {
            0 => plain.store_id = "33333333-3333-4333-8333-333333333333".into(),
            1 => plain.generation_id = "44444444-4444-4444-8444-444444444444".into(),
            _ => plain.revision += 1,
        }
        identity
    }
    #[test]
    fn every_binding_dimension_invalidates_and_failed_take_consumes() {
        let now = Instant::now();
        let b = binding();
        let changes = [
            Binding {
                vault_root: "/other".into(),
                ..b.clone()
            },
            Binding {
                storage: changed_storage(&b.storage, 0),
                ..b.clone()
            },
            Binding {
                storage: changed_storage(&b.storage, 1),
                ..b.clone()
            },
            Binding {
                storage: changed_storage(&b.storage, 2),
                ..b.clone()
            },
            Binding {
                context_revision: "other".into(),
                ..b.clone()
            },
            Binding {
                catalog_revision: "other".into(),
                ..b.clone()
            },
        ];
        for changed in changes {
            let mut reviews = Reviews::default();
            let id = reviews.issue(b.clone(), "native".into(), "identity".into(), now);
            assert!(reviews.take(&id, &changed, now).is_none());
            assert!(reviews.take(&id, &b, now).is_none());
        }
    }
    #[test]
    fn bounded_capacity_and_new_preview_supersedes_same_binding() {
        let mut reviews = Reviews::default();
        let now = Instant::now();
        let b = binding();
        let old = reviews.issue(b.clone(), "native".into(), "identity".into(), now);
        let new = reviews.issue(b.clone(), "native".into(), "identity".into(), now);
        assert!(reviews.take(&old, &b, now).is_none());
        assert!(reviews.take(&new, &b, now).is_some());
        for index in 0..100 {
            let mut changed = b.clone();
            changed.context_revision = index.to_string();
            reviews.issue(
                changed,
                "native".into(),
                "identity".into(),
                now + Duration::from_millis(index),
            );
        }
        assert_eq!(reviews.0.len(), MAX_REVIEWS);
    }
}
