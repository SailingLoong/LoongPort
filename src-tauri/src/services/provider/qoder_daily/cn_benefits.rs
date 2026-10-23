//! Explicit CN free-benefit batch runner. Uses the original encrypted auth owner,
//! policy barrier, and durable campaign ledger; never generates device identity.
//! Wire evidence: MIT magpie-community/plugins 08ff18a43853ccf23b30fc6dd39b3f10591331ee,
//! packages/qoder/index.mjs:2024-2055. No daily-key, machine, or 100-credit inference.
use super::{
    automatic,
    cn_campaigns::FailureCode,
    http::{CredentialSource, PatDeclaration},
};
use crate::{
    database::{self as ledger, CnOperation, CnTarget, Database},
    error::AppError,
    services::provider::qoder_http_auth::{self as owner, RequestSnapshot},
};
use serde::Serialize;
#[path = "cn_benefits_contract.rs"]
mod wire;
#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RunResult {
    Completed {
        provider_id: String,
        credential_generation: String,
        activities: Vec<ActivityResult>,
    },
    Failed {
        code: FailureCode,
    },
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivityResult {
    pub(crate) campaign_id: String,
    pub(crate) outcome: ActivityOutcome,
    pub(crate) reason: Option<&'static str>,
    pub(crate) benefit_amount: Option<f64>,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ActivityOutcome {
    ClaimedDeltaUnknown,
    ObservedClaimed,
    Unknown,
    Skipped,
}
struct Transport {
    client: reqwest::Client,
    url: reqwest::Url,
}
impl Transport {
    fn production() -> Result<Self, FailureCode> {
        Ok(Self {
            client: crate::services::provider::qoder_auth_http::client_builder()
                .build()
                .map_err(|_| FailureCode::NetworkError)?,
            url: reqwest::Url::parse("https://openapi.qoder.com.cn/sash/api/v1/me/campaigns")
                .map_err(|_| FailureCode::NetworkError)?,
        })
    }
    #[cfg(test)]
    fn synthetic(url: reqwest::Url) -> Self {
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        Self {
            client: crate::services::provider::qoder_auth_http::client_builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
            url,
        }
    }
}
pub(crate) async fn run(
    db: &Database,
    declaration: PatDeclaration,
    lease: Option<automatic::Lease>,
) -> RunResult {
    let transport = match Transport::production() {
        Ok(t) => t,
        Err(code) => return RunResult::Failed { code },
    };
    let source = match declaration.source {
        CredentialSource::ChinaQoderPat => owner::SourceKind::ChinaQoderPat,
        CredentialSource::ChinaQoderDevice => owner::SourceKind::ChinaQoderDevice,
        _ => {
            return RunResult::Failed {
                code: FailureCode::InvalidSource,
            }
        }
    };
    let auth = match owner::transport::FixedRegionTransport::for_source(source) {
        Ok(t) => t,
        Err(_) => {
            return RunResult::Failed {
                code: FailureCode::OwnerUnavailable,
            }
        }
    };
    run_using(db, declaration, lease, &transport, &auth, now_ms).await
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(-1)
}
fn fail(error: AppError) -> FailureCode {
    super::cn_campaigns::owner_failure(&error)
}
fn parse_error(error: wire::ParseError) -> FailureCode {
    match error {
        wire::ParseError::VerificationRequired => FailureCode::VerificationRequired,
        wire::ParseError::InvalidResponse => FailureCode::InvalidResponse,
        wire::ParseError::IdentityMismatch => FailureCode::IdentityMismatch,
        wire::ParseError::ServiceRejected => FailureCode::ServiceRejected,
        wire::ParseError::ResponseTooLarge => FailureCode::ResponseTooLarge,
    }
}
struct Listed {
    items: Vec<wire::Activity>,
    server_seconds: Option<i64>,
    observed: std::time::Instant,
}
impl Listed {
    fn window_reason(&self, target: &CnTarget) -> Option<&'static str> {
        if target.start_at.is_none() && target.end_at.is_none() {
            return None;
        }
        let elapsed = self.observed.elapsed();
        let Some(now) = self
            .server_seconds
            .and_then(|n| n.checked_add(i64::try_from(elapsed.as_secs()).ok()?))
        else {
            return Some("clock_unverified");
        };
        if elapsed > std::time::Duration::from_secs(300) {
            return Some("clock_unverified");
        }
        if target.start_at.is_some_and(|start| now < start) {
            Some("campaign_not_started")
        } else if target
            .end_at
            .is_some_and(|end| now.saturating_add(1) >= end)
        {
            Some("campaign_ended")
        } else {
            None
        }
    }
}
impl Transport {
    fn request(
        &self,
        snapshot: &RequestSnapshot,
        target: Option<&CnTarget>,
    ) -> Result<reqwest::Request, FailureCode> {
        let mut url = self.url.clone();
        let mut builder = if let Some(target) = target {
            // Opaque IDs are one encoded path segment. Never URL-join caller data.
            if !target.valid() {
                return Err(FailureCode::InvalidResponse);
            }
            url.path_segments_mut()
                .map_err(|_| FailureCode::InvalidResponse)?
                .push(&target.campaign_id)
                .push("claim");
            self.client.post(url).body("{}")
        } else {
            self.client.get(url)
        };
        builder = builder
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("Cosy-ClientType", "10")
            .header(reqwest::header::USER_AGENT, "Qoder")
            .bearer_auth(snapshot.access_token());
        builder.build().map_err(|_| FailureCode::NetworkError)
    }
}
async fn body(
    mut response: reqwest::Response,
    expected: &reqwest::Url,
) -> Result<(Vec<u8>, Option<i64>, std::time::Instant), FailureCode> {
    if response.url() != expected {
        return Err(FailureCode::RedirectRejected);
    }
    match response.status().as_u16() {
        200 => {}
        300..=399 => return Err(FailureCode::RedirectRejected),
        401 => return Err(FailureCode::AuthenticationRequired),
        403 => return Err(FailureCode::VerificationRequired),
        429 => return Err(FailureCode::RateLimited),
        _ => return Err(FailureCode::HttpError),
    }
    let observed = std::time::Instant::now();
    let date = response
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
        .map(|d| d.timestamp());
    if response
        .content_length()
        .is_some_and(|n| n > wire::MAX_BYTES as u64)
    {
        return Err(FailureCode::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| FailureCode::NetworkError)?
    {
        if chunk.len() > wire::MAX_BYTES.saturating_sub(bytes.len()) {
            return Err(FailureCode::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk)
    }
    Ok((bytes, date, observed))
}
fn observed(
    item: &wire::Activity,
    outcome: ActivityOutcome,
    reason: Option<&'static str>,
) -> ActivityResult {
    ActivityResult {
        campaign_id: item.target.campaign_id.clone(),
        outcome,
        reason,
        benefit_amount: item.amount,
    }
}
fn advanced(old: &CnTarget, new: &CnTarget) -> bool {
    matches!((old.start_at,old.end_at,new.start_at,new.end_at),(Some(_),Some(end),Some(start),Some(_)) if start>=end)
}
const ACCOUNT_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);
async fn run_using(
    db: &Database,
    declaration: PatDeclaration,
    lease: Option<automatic::Lease>,
    transport: &Transport,
    auth: &dyn owner::AuthTransport,
    now: impl Fn() -> i64,
) -> RunResult {
    run_budget(db, declaration, lease, transport, auth, now, ACCOUNT_BUDGET).await
}
async fn run_budget(
    db: &Database,
    declaration: PatDeclaration,
    lease: Option<automatic::Lease>,
    transport: &Transport,
    auth: &dyn owner::AuthTransport,
    now: impl Fn() -> i64,
    budget: std::time::Duration,
) -> RunResult {
    tokio::time::timeout(
        budget,
        run_inner(db, declaration, lease, transport, auth, now),
    )
    .await
    .unwrap_or(RunResult::Failed {
        code: FailureCode::Timeout,
    })
}
async fn run_inner(
    db: &Database,
    declaration: PatDeclaration,
    lease: Option<automatic::Lease>,
    transport: &Transport,
    auth: &dyn owner::AuthTransport,
    now: impl Fn() -> i64,
) -> RunResult {
    let result: Result<RunResult, FailureCode> = async {
        if !matches!(
            declaration.source,
            CredentialSource::ChinaQoderPat | CredentialSource::ChinaQoderDevice
        ) {
            return Err(FailureCode::InvalidSource);
        }
        // Capture the exact policy revision BEFORE any await; refresh cannot absorb revocation.
        let start_epoch = db.secrets.lifecycle_epoch();
        let provider = super::initialize_provider(db, &declaration.provider_id).map_err(fail)?;
        if lease
            .as_ref()
            .is_some_and(|lease| !lease.matches(&provider) || lease.epoch != start_epoch)
        {
            return Err(FailureCode::OwnerChanged);
        }
        let (source, _) = owner::SourceBinding::from_provider(&provider).map_err(fail)?;
        source.admit(&declaration).map_err(fail)?;
        let snapshot = owner::acquire(db, &provider, auth, &now)
            .await
            .map_err(|_| FailureCode::OwnerUnavailable)?;
        if snapshot.stamp.vault_epoch != start_epoch {
            return Err(FailureCode::OwnerChanged);
        }
        owner::check_selected_provider(&provider, &snapshot).map_err(fail)?;
        if snapshot.region() != "cn"
            || snapshot.stamp.source.origin != "https://openapi.qoder.com.cn"
        {
            return Err(FailureCode::InvalidSource);
        }
        let lease = match lease {
            Some(lease) => lease,
            None => automatic::Lease::for_cn_manual(&provider, &snapshot).map_err(fail)?,
        };
        let admission = automatic::Admission { db, lease };
        let identity = super::contract::Identity {
            provider_id: provider.id.clone(),
            uid: snapshot.uid().into(),
            region: super::contract::Region::China,
        };
        // The receipt/marker must predate this GET's first poll, not merely its
        // response. Concurrent runs cannot give an older in-flight GET eligibility.
        let mut before_listing = Vec::new();
        let request = transport.request(&snapshot, None)?;
        let url = request.url().clone();
        let listing_result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let response = admission
                .send_cn(
                    &snapshot,
                    &now,
                    |conn| {
                        before_listing = ledger::read_qoder_cn_operations_on(conn, &identity)?;
                        Ok(())
                    },
                    || transport.client.execute(request),
                )
                .await
                .map_err(fail)?;
            body(response, &url).await
        })
        .await
        .unwrap_or(Err(FailureCode::Timeout));
        admission
            .dispatch_cn(&snapshot, now(), |_| Ok(()))
            .map_err(fail)?;
        let (bytes, date, headers_observed) = listing_result?;
        let listing = Listed {
            items: wire::listing(&bytes, snapshot.uid()).map_err(parse_error)?,
            server_seconds: date,
            observed: headers_observed,
        };
        let previous = admission
            .dispatch_cn(&snapshot, now(), |conn| {
                ledger::read_qoder_cn_operations_on(conn, &identity)
            })
            .map_err(fail)?;
        let mut activities = Vec::new();
        for old in previous
            .iter()
            .filter(|o| o.outcome == super::contract::Outcome::Unknown)
        {
            // Recovery preserves original provider/generation/target. Missing or changed
            // metadata never turns an uncertain send into authorization for a new POST.
            let current = listing
                .items
                .iter()
                .find(|i| i.target.campaign_id == old.target.campaign_id);
            let confirmed = before_listing.contains(old)
                && current.is_some_and(|i| {
                    i.target == old.target
                        && i.skip_reason.is_none()
                        && i.status == wire::Status::Claimed
                });
            if confirmed {
                admission
                    .dispatch_cn(&snapshot, now(), |conn| {
                        ledger::reconcile_qoder_cn_operation_on(conn, old)
                    })
                    .map_err(fail)?;
            }
            activities.push(ActivityResult {
                campaign_id: old.target.campaign_id.clone(),
                outcome: if confirmed {
                    ActivityOutcome::ObservedClaimed
                } else {
                    ActivityOutcome::Unknown
                },
                reason: if confirmed {
                    Some("reconciled_no_new_round")
                } else if current.is_some_and(|i| i.target == old.target) {
                    Some("previous_unknown")
                } else {
                    Some("reconciliation_mismatch")
                },
                benefit_amount: current.and_then(|i| i.amount),
            });
        }
        for item in &listing.items {
            admission
                .dispatch_cn(&snapshot, now(), |_| Ok(()))
                .map_err(fail)?;
            let old = previous
                .iter()
                .find(|o| o.target.campaign_id == item.target.campaign_id);
            if old.is_some_and(|o| o.outcome == super::contract::Outcome::Unknown) {
                continue;
            }
            if let Some(reason) = item.skip_reason {
                activities.push(observed(item, ActivityOutcome::Skipped, Some(reason)));
                continue;
            }
            if let Some(old) = old {
                let eligible_before_get = before_listing.contains(old);
                // Only a POST-confirmed receipt may learn a later independent
                // CLAIMED listing. GET reconciliation cannot create this marker.
                if old.target == item.target
                    && item.status == wire::Status::Claimed
                    && old.receipt_confirmed
                    && eligible_before_get
                    && old.identity == identity
                    && old.generation == snapshot.stamp.source.credential_generation
                {
                    admission
                        .dispatch_cn(&snapshot, now(), |conn| {
                            ledger::observe_qoder_cn_claimed_on(conn, old)
                        })
                        .map_err(fail)?;
                    activities.push(observed(item, ActivityOutcome::ObservedClaimed, None));
                    continue;
                }
                let observed_rearm = eligible_before_get
                    && old.receipt_confirmed
                    && old.listing_claimed
                    && old.target == item.target
                    && old.identity == identity
                    && old.generation == snapshot.stamp.source.credential_generation;
                if !advanced(&old.target, &item.target) && !observed_rearm {
                    activities.push(observed(
                        item,
                        if item.status == wire::Status::Claimed && old.target == item.target {
                            ActivityOutcome::ObservedClaimed
                        } else {
                            ActivityOutcome::Skipped
                        },
                        Some(
                            if old.identity != identity
                                || old.generation != snapshot.stamp.source.credential_generation
                            {
                                "prior_owner_changed"
                            } else if old.receipt_confirmed {
                                "same_campaign_no_new_window"
                            } else {
                                "reconciled_no_new_round"
                            },
                        ),
                    ));
                    continue;
                }
            }
            if item.status == wire::Status::Claimed {
                activities.push(observed(item, ActivityOutcome::ObservedClaimed, None));
                continue;
            }
            if item.status != wire::Status::Claimable {
                activities.push(observed(
                    item,
                    ActivityOutcome::Skipped,
                    Some("not_claimable"),
                ));
                continue;
            }
            if let Some(reason) = listing.window_reason(&item.target) {
                activities.push(observed(item, ActivityOutcome::Skipped, Some(reason)));
                continue;
            }
            let request = transport.request(&snapshot, Some(&item.target))?;
            let url = request.url().clone();
            let operation = CnOperation {
                id: uuid::Uuid::new_v4().to_string(),
                generation: snapshot.stamp.source.credential_generation.clone(),
                identity: identity.clone(),
                target: item.target.clone(),
                outcome: super::contract::Outcome::Unknown,
                receipt_confirmed: false,
                listing_claimed: false,
            };
            // Durable intent COMMIT precedes construction/first poll of the POST future.
            // A process crash here is intentionally GET-only recovery, not a retry ticket.
            let reserved = admission
                .dispatch_cn(&snapshot, now(), |conn| {
                    if listing.window_reason(&item.target).is_some() {
                        return Ok(false);
                    };
                    let current = ledger::read_qoder_cn_operations_on(conn, &identity)?
                        .into_iter()
                        .find(|o| o.target.campaign_id == item.target.campaign_id);
                    if current.as_ref() != old {
                        return Ok(false);
                    }
                    match ledger::reserve_qoder_cn_operation_on(conn, &operation) {
                        Ok(_) => Ok(true),
                        Err(AppError::Config(code)) if code == "qoder.window_not_advanced" => {
                            Ok(false)
                        }
                        Err(error) => Err(error),
                    }
                })
                .map_err(fail)?;
            if !reserved {
                activities.push(observed(
                    item,
                    ActivityOutcome::Skipped,
                    Some("same_campaign_no_new_window"),
                ));
                continue;
            }
            let receipt = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                let response = admission
                    .send_cn(
                        &snapshot,
                        &now,
                        |conn| {
                            if listing.window_reason(&item.target).is_some()
                                || !ledger::read_qoder_cn_operations_on(conn, &identity)?
                                    .iter()
                                    .any(|o| o == &operation)
                            {
                                return Err(automatic::denied());
                            }
                            Ok(())
                        },
                        || transport.client.execute(request),
                    )
                    .await
                    .map_err(fail)?;
                let (bytes, _, _) = body(response, &url).await?;
                wire::claimed(&bytes, snapshot.uid(), &item.target).map_err(parse_error)
            })
            .await
            .unwrap_or(Err(FailureCode::Timeout));
            // Both durable result publication and the next item require the original
            // snapshot + source + policy revision + Vault epoch after all network awaits.
            admission
                .dispatch_cn(&snapshot, now(), |_| Ok(()))
                .map_err(fail)?;
            if matches!(receipt, Ok(true)) {
                admission
                    .dispatch_cn(&snapshot, now(), |conn| {
                        ledger::finish_qoder_cn_operation_on(
                            conn,
                            &operation,
                            super::contract::Outcome::ClaimedDeltaUnknown,
                        )
                    })
                    .map_err(fail)?;
                activities.push(observed(item, ActivityOutcome::ClaimedDeltaUnknown, None));
            } else {
                activities.push(observed(
                    item,
                    ActivityOutcome::Unknown,
                    Some("claim_outcome_unknown"),
                ));
                if let Err(
                    code @ (FailureCode::AuthenticationRequired
                    | FailureCode::VerificationRequired
                    | FailureCode::RateLimited
                    | FailureCode::IdentityMismatch),
                ) = receipt
                {
                    return Err(code);
                }
            }
        }
        admission
            .dispatch_cn(&snapshot, now(), |_| Ok(()))
            .map_err(fail)?;
        Ok(RunResult::Completed {
            provider_id: provider.id,
            credential_generation: snapshot.stamp.source.credential_generation.clone(),
            activities,
        })
    }
    .await;
    result.unwrap_or_else(|code| RunResult::Failed { code })
}
#[cfg(test)]
#[path = "cn_benefits_tests.rs"]
mod tests;
