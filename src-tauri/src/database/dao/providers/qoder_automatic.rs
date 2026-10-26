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
        self.save_qoder_automatic_bound(request, subject, epoch, None)
    }

    pub(crate) fn save_qoder_automatic_bound(
        &self,
        request: &Request,
        subject: Option<String>,
        epoch: u64,
        snapshot: Option<&crate::services::provider::qoder_http_auth::RequestSnapshot>,
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
                automatic::realm(other) == automatic::realm(&provider)
                    && automatic::active(other).is_some_and(|policy| policy.subject == subject)
            })
        {
            return Err(AppError::Config(
                "qoder.automatic_subject_already_enabled".into(),
            ));
        }
        let version = automatic::policy_version(&provider).ok_or_else(automatic::denied)?;
        if request.enabled {
            let (source, _) =
                crate::services::provider::qoder_http_auth::SourceBinding::from_provider(
                    &provider,
                )?;
            if request.source != Some(source.source.declaration()) {
                return Err(automatic::denied());
            }
            if version == automatic::CN_POLICY_VERSION {
                let snapshot = snapshot.ok_or_else(automatic::denied)?;
                crate::database::dao::qoder_http_auth::snapshot_matches_on(
                    &tx,
                    &vault,
                    epoch,
                    &provider,
                    snapshot,
                    Some(automatic::now_ms()),
                )?;
                if subject.as_deref() != Some(snapshot.uid()) {
                    return Err(automatic::denied());
                }
            }
        }
        let state = provider
            .meta
            .as_mut()
            .and_then(|m| m.qoder_campaign_state.as_mut())
            .ok_or_else(automatic::denied)?;
        if self.secrets.lifecycle_epoch() != epoch
            || state.generation != request.credential_generation
            || state.automatic.revision != request.expected_revision
            || request.policy_version != version
            || request.expected_revision >= automatic::MAX_REVISION
            || (request.enabled && subject.as_ref().is_none_or(|uid| uid.trim().is_empty()))
        {
            return Err(automatic::denied());
        }
        self.secrets.ensure_available()?;
        state.automatic.revision += 1;
        state.automatic.enabled = request.enabled;
        state.automatic.policy_version = version;
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

    pub(crate) fn revoke_qoder_automatic_on(
        conn: &rusqlite::Connection,
        vault: &StorageContext,
        provider: &Provider,
    ) -> Result<(), AppError> {
        let Some(mut meta) = provider.meta.clone() else {
            return Ok(());
        };
        let Some(state) = meta.qoder_campaign_state.as_mut() else {
            return Ok(());
        };
        state.automatic.revoke();
        meta.custom_endpoints.clear();
        let encrypted = seal_db(
            vault,
            "providers",
            "meta",
            &[&provider.id, "codex"],
            &serde_json::to_string(&meta).map_err(|e| AppError::Database(e.to_string()))?,
        )?;
        conn.execute(
            "UPDATE providers SET meta=?1 WHERE id=?2 AND app_type='codex'",
            params![encrypted, provider.id],
        )?;
        Ok(())
    }

    pub(crate) fn with_qoder_automatic_admission<T>(
        &self,
        lease: &Lease,
        dispatch: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        if lease.region != crate::services::provider::qoder_daily::contract::Region::International
            || !lease.requires_opt_in()
        {
            return Err(automatic::denied());
        }
        self.with_qoder_policy_admission(lease, None, || 0, |_| dispatch())
    }

    pub(crate) fn with_qoder_cn_admission<T>(
        &self,
        lease: &Lease,
        snapshot: &crate::services::provider::qoder_http_auth::RequestSnapshot,
        now: impl FnOnce() -> i64,
        dispatch: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        if lease.region != crate::services::provider::qoder_daily::contract::Region::China {
            return Err(automatic::denied());
        }
        self.with_qoder_policy_admission(lease, Some(snapshot), now, dispatch)
    }

    fn with_qoder_policy_admission<T>(
        &self,
        lease: &Lease,
        snapshot: Option<&crate::services::provider::qoder_http_auth::RequestSnapshot>,
        now: impl FnOnce() -> i64,
        dispatch: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let vault = self.secrets.read()?;
        let mut conn = lock_conn!(self.conn);
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = Self::get_all_providers_on_connection(&tx, &vault, "codex")?;
        let provider = rows.get(&lease.provider_id).ok_or_else(automatic::denied)?;
        if self.secrets.lifecycle_epoch() != lease.epoch
            || !lease.matches(provider)
            || (lease.requires_opt_in()
                && rows
                    .values()
                    .filter(|provider| {
                        automatic::realm(provider) == Some(lease.region)
                            && automatic::active(provider).is_some_and(|policy| {
                                policy.subject.as_deref() == Some(lease.subject.as_str())
                            })
                    })
                    .count()
                    != 1)
        {
            return Err(automatic::denied());
        }
        if let Some(snapshot) = snapshot {
            crate::database::dao::qoder_http_auth::snapshot_matches_on(
                &tx,
                &vault,
                lease.epoch,
                provider,
                snapshot,
                Some(now()),
            )?;
        }
        self.secrets.ensure_available()?;
        let result = dispatch(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}
