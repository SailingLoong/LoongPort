//! Per-account consent and periodic orchestration reuse the original account owner.
use super::*;
impl<T: Transport> Engine<'_, T> {
    pub async fn set_automatic(
        &self,
        id: &str,
        generation: u64,
        revision: u64,
        policy_version: u32,
        enabled: bool,
    ) -> Result<AccountView, Failure> {
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let b = store::binding(self.session)?;
        let mut rows = store::load(self.session, &b)?;
        let i = rows
            .iter()
            .position(|r| r.account.id(&b.vault_id) == id)
            .ok_or(Failure::UnsupportedContext)?;
        let row = &mut rows[i];
        if row.generation != generation
            || row.auto_claim_revision != revision
            || policy_version != AUTO_CLAIM_POLICY_VERSION
            || (enabled && !row.can_set_automatic())
        {
            return Err(Failure::ExpiredAuthorization);
        }
        row.auto_claim_revision = row
            .auto_claim_revision
            .checked_add(1)
            .ok_or(Failure::StorageUnavailable)?;
        row.auto_claim_enabled = enabled;
        row.auto_claim_policy_version = policy_version;
        row.auto_claim_credential = if enabled {
            Some(row.credential_binding())
        } else {
            None
        };
        self.persist(&b, rows, i)
    }
    pub async fn automatic_tick(&self) -> Result<usize, Failure> {
        let rows = self.list().await?;
        let mut failures = 0;
        for row in rows.into_iter().filter(|r| {
            r.auto_claim_enabled && r.can_set_automatic && !r.auto_claim_blocked && r.can_claim
        }) {
            // The final dispatch barrier rechecks this exact identity and policy.
            match self
                .claim_with_policy(&row.id, Some((row.generation, row.auto_claim_revision)))
                .await
            {
                // Read failures can be represented as an unconfirmed account,
                // rather than an Err. Do not call that a successful check.
                Ok(account)
                    if matches!(
                        account.claim_state,
                        ClaimState::Unconfirmed | ClaimState::NeedsVerification
                    ) =>
                {
                    failures += 1
                }
                Err(_) => failures += 1,
                Ok(_) => {}
            }
        }
        Ok(failures)
    }
}
