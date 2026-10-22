//! Policy mutations share the existing encrypted Provider transaction, never a
//! second account pool or reward ledger.
use super::*;
use crate::services::provider::qoder_daily::automatic::{self, Lease, Request};

impl Database {
    pub(crate) fn save_qoder_automatic(
        &self,
        request: &Request,
        subject: Option<String>,
        epoch: u64,
    ) -> Result<Provider, AppError> {
        let vault = self.secrets.read()?;
        let mut conn = lock_conn!(self.conn);
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut rows = Self::get_all_providers_on_connection(&tx, &vault, "codex")?;
        let mut provider = rows
            .shift_remove(&request.provider_id)
            .ok_or_else(automatic::denied)?;
        if request.enabled
            && rows.values().any(|other| {
                automatic::active(other).is_some_and(|policy| policy.subject == subject)
            })
        {
            return Err(AppError::Config(
                "qoder.automatic_subject_already_enabled".into(),
            ));
        }
        let state = provider
            .meta
            .as_mut()
            .and_then(|m| m.qoder_campaign_state.as_mut())
            .ok_or_else(automatic::denied)?;
        if self.secrets.lifecycle_epoch() != epoch
            || state.generation != request.credential_generation
            || state.automatic.revision != request.expected_revision
            || request.policy_version != automatic::POLICY_VERSION
            || request.expected_revision >= automatic::MAX_REVISION
            || (request.enabled && subject.as_ref().is_none_or(|uid| uid.trim().is_empty()))
        {
            return Err(automatic::denied());
        }
        self.secrets.ensure_available()?;
        state.automatic.revision += 1;
        state.automatic.enabled = request.enabled;
        state.automatic.policy_version = automatic::POLICY_VERSION;
        state.automatic.credential_generation = request.enabled.then(|| state.generation.clone());
        if request.enabled {
            state.automatic.subject = subject;
        }
        let mut meta = provider.meta.clone().ok_or_else(automatic::denied)?;
        meta.custom_endpoints.clear();
        let encrypted = seal_db(
            &vault,
            "providers",
            "meta",
            &[&provider.id, "codex"],
            &serde_json::to_string(&meta).map_err(|e| AppError::Database(e.to_string()))?,
        )?;
        tx.execute(
            "UPDATE providers SET meta=?1 WHERE id=?2 AND app_type='codex'",
            params![encrypted, provider.id],
        )?;
        tx.commit()?;
        Ok(provider)
    }

    pub(crate) fn with_qoder_automatic_admission<T>(
        &self,
        lease: &Lease,
        dispatch: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let vault = self.secrets.read()?;
        let mut conn = lock_conn!(self.conn);
        // Independent connections use the same SQLite writer barrier. This
        // transaction performs no write and ends before any network wait.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = Self::get_all_providers_on_connection(&tx, &vault, "codex")?;
        if self.secrets.lifecycle_epoch() != lease.epoch
            || rows
                .get(&lease.provider_id)
                .is_none_or(|provider| !lease.matches(provider))
            || rows
                .values()
                .filter(|provider| {
                    automatic::active(provider).is_some_and(|policy| {
                        policy.subject.as_deref() == Some(lease.subject.as_str())
                    })
                })
                .count()
                != 1
        {
            return Err(automatic::denied());
        }
        self.secrets.ensure_available()?;
        dispatch()
    }
}
