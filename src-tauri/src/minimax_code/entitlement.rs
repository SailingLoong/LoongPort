use serde::Serialize;
use serde_json::Value;
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entitlement {
    pub remaining_credits: Option<String>,
    pub has_token_plan: Option<bool>,
    pub token_plan_expires_at: Option<i64>,
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
    if catalog::matched(d, snapshot)?.credential_version != snapshot.account.credential_version {
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
/// credential refresh, claim, plan purchase, persistence, or routing eligibility.
pub(crate) async fn read<T: Transport>(
    session: &SecretSession,
    transport: &T,
    expected: &AccountView,
) -> Result<Entitlement, Failure> {
    let snapshot = catalog::snapshot(session, &expected.id).await?;
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
    let result = match extra {
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
    catalog::access(session, Some(&snapshot.binding), |d, _| {
        current(d, &snapshot)?;
        Ok((result, false))
    })
    .await
}

#[cfg(test)]
#[path = "entitlement_tests.rs"]
mod tests;
