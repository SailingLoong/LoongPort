use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Historical protocol observations, not an actionable model entitlement.
/// A model_name is an upstream display label, not a verified final model ID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct QuotaRow {
    pub model_name: Option<String>,
    pub fields: BTreeMap<String, serde_json::Number>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum QuotaSource {
    PersonalWorkspaceOAuthCodingPlanRemains,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct QuotaObservation {
    pub source: QuotaSource,
    pub account_id: String,
    pub region: super::catalog::Region,
    pub real_user_id: String,
    pub auth_generation: u64,
    pub credential_version: u64,
    /// Captured before the existing read sequence, never refreshed on failure.
    pub observed_at_ms: i64,
    pub rows: Vec<QuotaRow>,
}

impl QuotaObservation {
    pub(super) fn matches_account(&self, account: &super::catalog::Account) -> bool {
        self.account_id == account.id
            && self.region == account.region
            && self.real_user_id == account.real_user_id
            && self.auth_generation == account.auth_generation
            && self.credential_version == account.credential_version
    }
}

fn observation_rows(body: &Value) -> Option<Vec<QuotaRow>> {
    let rows = body.get("model_remains")?.as_array()?;
    if rows.is_empty() || rows.len() > 128 {
        return None;
    }
    rows.iter()
        .map(|row| {
            row.as_object()?;
            let model_name = match row.get("model_name") {
                Some(Value::String(name))
                    if name.len() <= 256 && !name.chars().any(char::is_control) =>
                {
                    Some(name.clone())
                }
                None | Some(Value::Null) => None,
                _ => return None,
            };
            // Preserve numeric protocol values without rounding, deriving or
            // adding concurrent windows. Never retain the response, group or token.
            let mut fields = BTreeMap::new();
            for key in [
                "current_interval_status",
                "current_interval_remaining_percent",
                "current_interval_usage_count",
                "current_interval_total_count",
                "end_time",
                "current_weekly_status",
                "current_weekly_remaining_percent",
                "current_weekly_usage_count",
                "current_weekly_total_count",
                "weekly_end_time",
            ] {
                match row.get(key) {
                    Some(Value::Number(number)) => {
                        fields.insert(key.to_owned(), number.clone());
                    }
                    None | Some(Value::Null) => {}
                    _ => return None,
                }
            }
            if fields.is_empty() {
                return None;
            }
            Some(QuotaRow { model_name, fields })
        })
        .collect()
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuotaWindow {
    pub remaining_percent: Option<f64>,
    pub reset_at_ms: Option<f64>,
    pub unlimited: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanQuota {
    pub five_hour: QuotaWindow,
    pub weekly: QuotaWindow,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum QuotaState {
    Available,
    NotSubscribed,
    #[default]
    Unavailable,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entitlement {
    pub remaining_credits: Option<String>,
    pub has_token_plan: Option<bool>,
    pub token_plan_expires_at: Option<i64>,
    pub quota_state: QuotaState,
    pub quota: Option<PlanQuota>,
}
/// Protocol fields from the MIT MiniMax Code pinned matrix-account-client.ts.
/// Daily sign-in awards are deliberately not interpreted as remaining credits.
pub(super) fn project(body: &Value) -> Entitlement {
    let field = |key: &str| {
        body.get(key)
            .filter(|v| !v.is_null())
            .or_else(|| body.get("data").and_then(|v| v.get(key)))
    };
    let summary = field("op_credit_summary").and_then(|v| v.get("total_remaining_amount"));
    Entitlement {
        remaining_credits: summary
            .and_then(amount)
            .or_else(|| field("opcredit_balance").and_then(amount)),
        has_token_plan: field("has_token_plan").and_then(Value::as_bool),
        token_plan_expires_at: field("token_plan_expires_at")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0),
        ..Entitlement::default()
    }
}
fn amount(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    // Keep an exact bounded decimal string, never round a balance through f64.
    let mut dots = 0;
    if text.is_empty()
        || text.len() > 64
        || !text.bytes().all(|b| {
            if b == b'.' {
                dots += 1;
                dots <= 1
            } else {
                b.is_ascii_digit()
            }
        })
        || text.starts_with('.')
        || text.ends_with('.')
    {
        return None;
    }
    Some(text)
}
/// Pinned official client uses the first object row and treats usage_count as
/// remaining (not consumed). A missing percentage is unknown, never zero.
pub(super) fn project_quota(body: &Value) -> Option<PlanQuota> {
    let primary = body
        .get("model_remains")?
        .as_array()?
        .iter()
        .find(|v| v.is_object())?;
    Some(PlanQuota {
        five_hour: quota_window(primary, "current_interval", "end_time"),
        weekly: quota_window(primary, "current_weekly", "weekly_end_time"),
    })
}
fn quota_window(value: &Value, prefix: &str, reset_key: &str) -> QuotaWindow {
    let number = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    };
    let unlimited = number(&format!("{prefix}_status")) == Some(3.0);
    let remaining = number(&format!("{prefix}_remaining_percent")).or_else(|| {
        let total = number(&format!("{prefix}_total_count")).filter(|v| *v > 0.0)?;
        let remaining = number(&format!("{prefix}_usage_count"))?;
        Some(remaining / total * 100.0)
    });
    QuotaWindow {
        remaining_percent: if unlimited {
            None
        } else {
            remaining.map(|v| v.clamp(0.0, 100.0).round())
        },
        reset_at_ms: number(reset_key).filter(|v| *v > 0.0),
        unlimited,
    }
}
// OWNER_READ_BOUNDARY
use super::{
    catalog::{self, AccountView, Failure, Snapshot},
    http::{Endpoint, Request, Response, Transport},
};
use crate::secrets::session::SecretSession;
fn successful(response: &Response) -> Result<(), Failure> {
    if !(200..300).contains(&response.status) {
        return Err(Failure::NetworkUnavailable);
    }
    if !response.body.is_object() {
        return Err(Failure::InvalidResponse);
    }
    for pointer in ["/statusInfo/code", "/base_resp/status_code"] {
        if let Some(v) = response.body.pointer(pointer) {
            if v.as_i64() != Some(0) {
                return Err(Failure::InvalidResponse);
            }
        }
    }
    Ok(())
}
fn current(d: &mut catalog::Document, snapshot: &Snapshot) -> Result<(), Failure> {
    let account = catalog::matched(d, snapshot)?;
    account.usable()?;
    if account.credential_version != snapshot.account.credential_version {
        return Err(Failure::StaleIdentity);
    }
    Ok(())
}
async fn query<T: Transport>(
    session: &SecretSession,
    transport: &T,
    snapshot: &Snapshot,
    endpoint: Endpoint,
    body: Value,
) -> Result<Response, Failure> {
    let response = catalog::dispatch(
        session,
        &snapshot.binding,
        transport,
        Request {
            endpoint,
            region: snapshot.account.region,
            access_token: Some(
                snapshot
                    .account
                    .credentials
                    .as_ref()
                    .ok_or(Failure::NeedsLogin)?
                    .access_token
                    .clone(),
            ),
            real_user_id: Some(snapshot.account.real_user_id.clone()),
            body: Some(body),
        },
        |d, _| current(d, snapshot),
    )
    .await?;
    catalog::access(session, Some(&snapshot.binding), |d, _| {
        current(d, snapshot)?;
        Ok(((), false))
    })
    .await?;
    successful(&response)?;
    Ok(response)
}
/// Informational read only, on an explicitly refreshed original account. No
/// credential refresh, claim, plan purchase, or routing eligibility. Only a
/// bounded quota observation is saved in the existing encrypted account owner.
pub(crate) async fn read<T: Transport>(
    session: &SecretSession,
    transport: &T,
    expected: &AccountView,
) -> Result<Entitlement, Failure> {
    let snapshot = catalog::snapshot(session, &expected.id).await?;
    let observed_at_ms = catalog::now_ms();
    let mut observed_rows = None;
    if snapshot.account.auth_generation != expected.auth_generation
        || snapshot.account.credential_version != expected.credential_version
        || snapshot.account.real_user_id != expected.real_user_id
        || snapshot.account.region != expected.region
    {
        return Err(Failure::StaleIdentity);
    }
    let extra = query(
        session,
        transport,
        &snapshot,
        Endpoint::UserExtra,
        serde_json::json!({}),
    )
    .await;
    let mut personal_group_id = None;
    let mut result = match extra {
        Err(_) => project(
            &query(
                session,
                transport,
                &snapshot,
                Endpoint::Membership,
                serde_json::json!({}),
            )
            .await?
            .body,
        ),
        Ok(extra) => {
            let workspaces = extra
                .body
                .get("workspaces")
                .or_else(|| extra.body.pointer("/data/workspaces"))
                .and_then(Value::as_array);
            let personal = workspaces.and_then(|items| {
                items.iter().find(|v| {
                    v.get("workspace_type").and_then(Value::as_i64) == Some(0)
                        && v.get("workspace_id").is_some_and(|id| {
                            id.as_u64().is_some()
                                || id
                                    .as_str()
                                    .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256)
                        })
                })
            });
            if let Some(personal) = personal {
                let id = personal.get("workspace_id").filter(|v| {
                    v.as_u64().is_some()
                        || v.as_str()
                            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256)
                });
                if let Some(id) = id {
                    // Only the verified personal workspace can scope OAuth
                    // quota reads. Never adopt a scoped/fallback membership's
                    // group or a team workspace; do not expose this in the DTO.
                    personal_group_id = personal
                        .get("op_group_id")
                        .filter(|v| !v.is_null())
                        .or_else(|| personal.pointer("/data/op_group_id"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|v| {
                            !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control)
                        })
                        .map(str::to_owned);
                    let base = project(personal);
                    match query(
                        session,
                        transport,
                        &snapshot,
                        Endpoint::Membership,
                        serde_json::json!({"workspace_id":id}),
                    )
                    .await
                    {
                        Ok(scoped) => {
                            let scoped = project(&scoped.body);
                            Entitlement {
                                remaining_credits: scoped
                                    .remaining_credits
                                    .or(base.remaining_credits),
                                has_token_plan: if base.has_token_plan == Some(true)
                                    || scoped.has_token_plan == Some(true)
                                {
                                    Some(true)
                                } else {
                                    scoped.has_token_plan.or(base.has_token_plan)
                                },
                                token_plan_expires_at: scoped
                                    .token_plan_expires_at
                                    .or(base.token_plan_expires_at),
                                ..Entitlement::default()
                            }
                        }
                        Err(_) => base,
                    }
                } else {
                    Entitlement::default()
                }
            } else {
                Entitlement::default()
            }
        }
    };
    if result.has_token_plan == Some(false) {
        result.quota_state = QuotaState::NotSubscribed;
    } else if let Some(group) = personal_group_id {
        if let Ok(response) = query(
            session,
            transport,
            &snapshot,
            Endpoint::PlanQuota,
            serde_json::json!({"op_group_id":group}),
        )
        .await
        {
            observed_rows = observation_rows(&response.body);
            result.quota = project_quota(&response.body);
            if result.quota.is_some() {
                result.quota_state = QuotaState::Available;
            }
        }
    }
    // Optional quota errors never erase credits, but account changes cannot
    // publish even the earlier membership snapshot.
    catalog::access(session, Some(&snapshot.binding), |d, _| {
        current(d, &snapshot)?;
        let account = catalog::matched(d, &snapshot)?;
        let dirty = if let Some(rows) = observed_rows {
            // A late request cannot replace a newer observation for this owner.
            // Failed reads retain history without manufacturing a fresh timestamp.
            let newer = account.quota_observation.as_ref().is_some_and(|old| {
                old.matches_account(account) && old.observed_at_ms >= observed_at_ms
            });
            if newer {
                false
            } else {
                account.quota_observation = Some(QuotaObservation {
                    source: QuotaSource::PersonalWorkspaceOAuthCodingPlanRemains,
                    account_id: account.id.clone(),
                    region: account.region,
                    real_user_id: account.real_user_id.clone(),
                    auth_generation: snapshot.account.auth_generation,
                    credential_version: snapshot.account.credential_version,
                    observed_at_ms,
                    rows,
                });
                true
            }
        } else {
            false
        };
        Ok((result, dirty))
    })
    .await
}

#[cfg(test)]
#[path = "entitlement_tests.rs"]
mod tests;
