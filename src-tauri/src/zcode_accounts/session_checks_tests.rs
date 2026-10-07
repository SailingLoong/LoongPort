use super::super::core::{AccountIdentity, CredentialDocument, OAuthFamily};
use super::super::official::{OfficialRequest, OfficialResponse, TransportFuture};
use super::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use zeroize::Zeroizing;

struct Fixture {
    requests: Mutex<Vec<OfficialRequest>>,
    responses: Mutex<VecDeque<Result<OfficialResponse, OfficialError>>>,
    cancelled: Option<Arc<AtomicBool>>,
}
impl Fixture {
    fn new(responses: Vec<Result<OfficialResponse, OfficialError>>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
            cancelled: None,
        }
    }
}
impl OfficialTransport for Fixture {
    fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
        self.requests.lock().unwrap().push(request);
        if let Some(cancelled) = &self.cancelled {
            cancelled.store(true, Ordering::SeqCst);
        }
        Box::pin(async {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected request")
        })
    }
}
fn response(status: u16, data: serde_json::Value) -> Result<OfficialResponse, OfficialError> {
    Ok(OfficialResponse {
        status,
        body: Zeroizing::new(
            serde_json::to_vec(&serde_json::json!({"code":0,"data":data})).unwrap(),
        ),
    })
}
fn fixture(
    family: OAuthFamily,
    global: Option<&str>,
    cached: Option<&str>,
    coding: Option<&str>,
) -> (NativeCipher, AccountSnapshot) {
    let native = NativeCipher::new("synthetic-context", "synthetic-native-secret").unwrap();
    let identity = AccountIdentity::new("synthetic-context", family, "declared-profile").unwrap();
    let keys = identity.credential_keys();
    let provider = match family {
        OAuthFamily::Zai => "zai",
        OAuthFamily::BigModel => "bigmodel",
    };
    let values = [
        Some(provider),
        Some("business-secret-canary"),
        Some("refresh-secret-canary"),
        Some(r#"{"id":"declared-profile","username":"Declared","displayName":"Declared"}"#),
        global,
        coding,
        cached,
    ];
    let entries: BTreeMap<_, _> = keys
        .into_iter()
        .zip(values)
        .filter_map(|(key, value)| value.map(|value| (key, native.encrypt(value).unwrap())))
        .collect();
    let document = CredentialDocument::parse(&serde_json::to_vec(&entries).unwrap()).unwrap();
    let snapshot = native.inspect(&document).unwrap();
    (native, snapshot)
}
fn check() -> Result<(), OfficialError> {
    Ok(())
}

#[tokio::test]
async fn session_checks_business_rejection_does_not_gate_start_or_coding_requests() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        Some("cached-secret-canary"),
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(vec![
        response(401, serde_json::json!({})),
        response(
            200,
            serde_json::json!({"plans":[{"status":"active","plan_id":"zcode-v3-start-plan"}],"balances":[]}),
        ),
        response(
            200,
            serde_json::json!([{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]),
        ),
        response(
            200,
            serde_json::json!({"limits":[{"type":"TIME_LIMIT","remaining":0}]}),
        ),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap();
    let display = report.display();
    assert_eq!(display.business.check.state, CheckState::Unavailable);
    assert_eq!(
        display.business.check.reason,
        Some(CheckReason::AuthRejected)
    );
    assert_eq!(display.start.check.state, CheckState::Accepted);
    assert_eq!(display.start.entitlement, EntitlementState::Available);
    assert_eq!(display.coding.check.state, CheckState::Accepted);
    assert_eq!(display.coding.entitlement, EntitlementState::Available);
    assert_eq!(display.coding.limits[0].remaining, Some(0.0));
    let requests = client.transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[1].authorization.as_ref().unwrap().expose(),
        "Bearer jwt-secret-canary"
    );
    assert!(requests[1].url.ends_with("app_version=4.1.2"));
    assert_eq!(
        requests[2].authorization.as_ref().unwrap().expose(),
        "coding-secret-canary"
    );
}

fn successful_responses() -> Vec<Result<OfficialResponse, OfficialError>> {
    vec![
        response(
            200,
            serde_json::json!({"customerNumber":"different-official-owner"}),
        ),
        response(200, serde_json::json!({"plans":[],"balances":[]})),
        response(200, serde_json::json!([])),
        response(200, serde_json::json!({"limits":[]})),
    ]
}
#[tokio::test]
async fn session_checks_missing_jwt_and_real_version_never_block_business_or_key() {
    for version in [None, Some("4.1.2")] {
        let (native, snapshot) = fixture(
            OAuthFamily::BigModel,
            None,
            None,
            Some("coding-secret-canary"),
        );
        let client = OfficialClient::new(Fixture::new(vec![
            response(
                200,
                serde_json::json!({"customerNumber":"different-official-owner"}),
            ),
            response(200, serde_json::json!([])),
            response(200, serde_json::json!({"limits":[]})),
        ]));
        let report = check_session(&client, &native, &snapshot, version, 100, &check)
            .await
            .unwrap()
            .display();
        assert_eq!(report.business.check.state, CheckState::Accepted);
        assert_eq!(
            report.business.official_owner_id.as_deref(),
            Some("different-official-owner")
        );
        assert_eq!(
            report.start.check.reason,
            Some(CheckReason::MissingCredential)
        );
        assert_eq!(report.coding.check.state, CheckState::Accepted);
        assert_eq!(client.transport.requests.lock().unwrap().len(), 3);
    }
    let (native, snapshot) = fixture(OAuthFamily::Zai, Some("jwt-secret-canary"), None, None);
    let client = OfficialClient::new(Fixture::new(vec![response(200, serde_json::json!({}))]));
    let report = check_session(&client, &native, &snapshot, None, 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.start.check.state, CheckState::Unknown);
    assert_eq!(
        report.start.check.reason,
        Some(CheckReason::AppVersionUnknown)
    );
    assert_eq!(client.transport.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn session_checks_cached_jwt_only_fills_absence_not_rejection() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        for global in [None, Some("jwt-secret-canary")] {
            let (native, snapshot) = fixture(family, global, Some("cached-secret-canary"), None);
            let client = OfficialClient::new(Fixture::new(vec![
                response(200, serde_json::json!({})),
                response(401, serde_json::json!({})),
            ]));
            let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
                .await
                .unwrap()
                .display();
            assert_eq!(report.start.check.reason, Some(CheckReason::AuthRejected));
            assert_eq!(
                report.start.check.source,
                Some(if global.is_some() {
                    CredentialSource::GlobalStartJwt
                } else {
                    CredentialSource::AccountStartJwt
                })
            );
            let requests = client.transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[1].authorization.as_ref().unwrap().expose(),
                format!("Bearer {}", global.unwrap_or("cached-secret-canary"))
            );
        }
    }
}
#[tokio::test]
async fn session_checks_unknown_is_not_rejection_or_zero() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(vec![
        Err(OfficialError::Timeout),
        response(200, serde_json::json!({"plans":"malformed","balances":[]})),
        response(
            200,
            serde_json::json!([{"productId":"coding-pro","status":"VALID"}]),
        ),
        response(
            200,
            serde_json::json!({"limits":[{"type":"TIME_LIMIT","usage":7}]}),
        ),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.business.check.state, CheckState::Unknown);
    assert_eq!(report.business.check.reason, Some(CheckReason::Timeout));
    assert_eq!(report.start.check.state, CheckState::Unknown);
    assert_eq!(
        report.start.check.reason,
        Some(CheckReason::MalformedResponse)
    );
    assert_eq!(report.coding.subscription.state, CheckState::Unknown);
    assert_eq!(report.coding.entitlement, EntitlementState::Unknown);
    assert_eq!(report.coding.quota.state, CheckState::Accepted);
    assert_eq!(report.coding.limits[0].remaining, None);
}
#[tokio::test]
async fn session_checks_coding_entitlement_survives_quota_failure_and_ignores_unrelated_products() {
    let (native, snapshot) = fixture(
        OAuthFamily::BigModel,
        None,
        None,
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(vec![
        response(200, serde_json::json!({})),
        response(
            200,
            serde_json::json!([{"productId":"unrelated"},{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]),
        ),
        Err(OfficialError::Timeout),
    ]));
    let report = check_session(&client, &native, &snapshot, None, 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.coding.check.state, CheckState::Accepted);
    assert_eq!(report.coding.entitlement, EntitlementState::Available);
    assert_eq!(report.coding.quota.reason, Some(CheckReason::Timeout));
    assert!(report.coding.limits.is_empty());
}
#[tokio::test]
async fn session_checks_start_preserves_instances_units_windows_zero_and_unknown_without_aggregation(
) {
    let (native, snapshot) = fixture(OAuthFamily::Zai, Some("jwt-secret-canary"), None, None);
    let client = OfficialClient::new(Fixture::new(vec![
        response(200, serde_json::json!({})),
        response(
            200,
            serde_json::json!({"server_time":100,"plans":[{"user_plan_id":"instance-a","plan_id":"zcode-v3-start-plan","status":"active","ends_at":200},{"user_plan_id":"instance-b","plan_id":"zcode-v3-start-plan","status":"active","ends_at":250}],"balances":[{"bucket_id":"a","user_plan_id":"instance-a","unit_type":"tokens","remaining_units":"0","available_units":42,"period_start":10,"period_end":30,"expires_at":40},{"bucket_id":"b","user_plan_id":"instance-b","unit_type":"requests","total_units":100,"available_units":8}]}),
        ),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.start.entitlement, EntitlementState::Available);
    assert_eq!(report.start.plans[1].status.as_deref(), Some("active"));
    assert_eq!(report.start.buckets.len(), 2);
    let a = &report.start.buckets[0];
    let b = &report.start.buckets[1];
    assert_eq!(a.remaining_units, Some(0.0));
    assert_eq!(a.available_units, Some(42.0));
    assert_eq!(a.period_end_seconds, Some(30.0));
    assert_eq!(a.expires_at_seconds, Some(40.0));
    assert_eq!(b.remaining_units, None);
    assert_eq!(b.unit_type.as_deref(), Some("requests"));
    assert_ne!(a.user_plan_id, b.user_plan_id);
}
#[tokio::test]
async fn session_checks_cancellation_after_first_send_aborts_remaining_requests() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut transport = Fixture::new(successful_responses());
    transport.cancelled = Some(cancelled.clone());
    let client = OfficialClient::new(transport);
    let check = || {
        if cancelled.load(Ordering::SeqCst) {
            Err(OfficialError::Cancelled)
        } else {
            Ok(())
        }
    };
    assert!(matches!(
        check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check).await,
        Err(OfficialError::Cancelled)
    ));
    assert_eq!(client.transport.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn session_checks_persist_nonce_stable_bindings_and_clear_only_changed_credentials() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        Some("unused-cache"),
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(successful_responses()));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap();
    let encoded = serde_json::to_vec(&report).unwrap();
    let persisted: SessionCheckReport = serde_json::from_slice(&encoded).unwrap();
    let (_, new_nonce) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        Some("changed-unused-cache"),
        Some("coding-secret-canary"),
    );
    assert!(persisted.matches_business(&native, &new_nonce));
    assert!(persisted.matches_start(&native, &new_nonce));
    assert!(persisted.matches_coding(&native, &new_nonce));
    assert_eq!(
        persisted.retain_matching(&native, &new_nonce).display(),
        report.display()
    );
    let (_, changed_key) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("new-coding-secret"),
    );
    assert!(!persisted.matches_coding(&native, &changed_key));
    let retained = persisted.retain_matching(&native, &changed_key).display();
    assert_eq!(retained.business, report.display().business);
    assert_eq!(retained.start, report.display().start);
    assert_eq!(retained.coding.check.reason, Some(CheckReason::Unverified));
    assert_eq!(retained.coding.check.checked_at, None);
    assert!(retained.coding.limits.is_empty());
    let mut with_unknown = serde_json::to_value(&persisted).unwrap();
    with_unknown["rawResponse"] = serde_json::json!({});
    assert!(serde_json::from_value::<SessionCheckReport>(with_unknown).is_err());
}
#[tokio::test]
async fn session_checks_display_and_storage_never_echo_credentials_or_unrecognized_remote_data() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        Some("cached-secret-canary"),
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(vec![
        response(
            200,
            serde_json::json!({"customerNumber":"Bearer business-secret-canary","customerName":"refresh-secret-canary","debug":"raw-private-response"}),
        ),
        response(
            200,
            serde_json::json!({"plans":[{"status":"active","name":"jwt-secret-canary"}],"balances":[{"show_name":"cached-secret-canary","remaining_units":0}]}),
        ),
        response(
            200,
            serde_json::json!([{"productId":"coding-secret-canary","status":"VALID","inCurrentPeriod":true}]),
        ),
        response(
            200,
            serde_json::json!({"limits":[{"type":"TIME_LIMIT","usageDetails":[{"modelCode":"coding-secret-canary","displayName":"business-secret-canary"}]}]}),
        ),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap();
    let stored = String::from_utf8(serde_json::to_vec(&report).unwrap()).unwrap();
    let display = serde_json::to_string(&report.display()).unwrap();
    let debug = format!("{:?}", report.display());
    for secret in [
        "business-secret-canary",
        "refresh-secret-canary",
        "jwt-secret-canary",
        "cached-secret-canary",
        "coding-secret-canary",
        "raw-private-response",
    ] {
        assert!(!stored.contains(secret));
        assert!(!display.contains(secret));
        assert!(!debug.contains(secret));
    }
    assert!(!display.contains("bindings"));
}

#[tokio::test]
async fn session_checks_start_expiry_filters_only_the_expired_instance_and_rejects_malformed_dates()
{
    let (native, snapshot) = fixture(OAuthFamily::Zai, Some("jwt-secret-canary"), None, None);
    let client = OfficialClient::new(Fixture::new(vec![
        response(200, serde_json::json!({})),
        response(
            200,
            serde_json::json!({"server_time":1,"plans":[{"user_plan_id":"expired","plan_id":"zcode-v3-start-plan","status":"active","ends_at":90},{"user_plan_id":"active","plan_id":"zcode-v3-start-plan","status":"active","ends_at":200}],"balances":[{"bucket_id":"expired-bucket","user_plan_id":"expired","plan_id":"zcode-v3-start-plan","remaining_units":7},{"bucket_id":"active-bucket","user_plan_id":"active","plan_id":"zcode-v3-start-plan","remaining_units":9},{"bucket_id":"orphan-bucket","user_plan_id":"orphan","plan_id":"zcode-v3-start-plan","remaining_units":11}]}),
        ),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.start.buckets.len(), 2);
    assert_eq!(
        report.start.buckets[0].bucket_id.as_deref(),
        Some("active-bucket")
    );
    assert_eq!(
        report.start.buckets[1].bucket_id.as_deref(),
        Some("orphan-bucket")
    );
    let client = OfficialClient::new(Fixture::new(vec![
        response(200, serde_json::json!({})),
        response(
            200,
            serde_json::json!({"plans":[{"status":"active","ends_at":"not-a-date"}],"balances":[]}),
        ),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.start.check.state, CheckState::Unknown);
    assert_eq!(report.start.entitlement, EntitlementState::Unknown);
}

#[tokio::test]
async fn session_checks_all_request_boundaries_honor_cancellation_including_final_response() {
    use std::sync::atomic::AtomicUsize;
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    // One initial guard, then a before/after guard for each of the four sends.
    for cancel_at in 1..=10 {
        let client = OfficialClient::new(Fixture::new(successful_responses()));
        let calls = AtomicUsize::new(0);
        let check = || {
            if calls.fetch_add(1, Ordering::SeqCst) + 1 == cancel_at {
                Err(OfficialError::Cancelled)
            } else {
                Ok(())
            }
        };
        assert!(matches!(
            check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check).await,
            Err(OfficialError::Cancelled)
        ));
        assert_eq!(
            client.transport.requests.lock().unwrap().len(),
            ((cancel_at - 1) / 2).min(4)
        );
    }
}

#[tokio::test]
async fn session_checks_start_future_entitlements_and_numeric_corruption_remain_distinct() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(vec![
        response(200, serde_json::json!({})),
        response(
            200,
            serde_json::json!({"server_time":100,"plans":[{"status":"active","starts_at":120}],"balances":[{"remaining_units":"not-a-number"}]}),
        ),
        response(403, serde_json::json!({})),
        Err(OfficialError::Timeout),
    ]));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap()
        .display();
    assert_eq!(report.start.entitlement, EntitlementState::Pending);
    assert_eq!(report.start.effective_at_seconds, Some(120.0));
    assert_eq!(report.start.quota.state, CheckState::Unknown);
    assert!(report.start.buckets.is_empty());
    assert_eq!(report.coding.check.reason, Some(CheckReason::AuthRejected));
    assert_eq!(report.coding.quota.reason, Some(CheckReason::Timeout));
}

#[tokio::test]
async fn session_checks_business_and_jwt_replacement_clear_their_own_evidence() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let client = OfficialClient::new(Fixture::new(successful_responses()));
    let report = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
        .await
        .unwrap();
    for index in [1, 4] {
        let document = snapshot.scoped_document();
        let mut entries: BTreeMap<String, String> =
            serde_json::from_slice(&document.to_bytes().unwrap()).unwrap();
        entries.insert(
            snapshot.identity().credential_keys()[index].clone(),
            native.encrypt("replacement-secret").unwrap(),
        );
        let changed = native
            .inspect(&CredentialDocument::parse(&serde_json::to_vec(&entries).unwrap()).unwrap())
            .unwrap();
        let retained = report.retain_matching(&native, &changed).display();
        assert_eq!(retained.coding, report.display().coding);
        if index == 1 {
            assert!(!report.matches_business(&native, &changed));
            assert_eq!(
                retained.business.check.reason,
                Some(CheckReason::Unverified)
            );
            assert_eq!(retained.business.official_owner_id, None);
            assert_eq!(retained.start, report.display().start);
        } else {
            assert!(!report.matches_start(&native, &changed));
            assert_eq!(retained.start.check.reason, Some(CheckReason::Unverified));
            assert_eq!(retained.business, report.display().business);
        }
    }
}

#[tokio::test]
async fn session_checks_start_nonempty_official_model_labels_prevent_false_pending() {
    let (native, snapshot) = fixture(OAuthFamily::Zai, Some("jwt-secret-canary"), None, None);
    for balance in [
        serde_json::json!({"show_name":"Future official model","remaining_units":0}),
        serde_json::json!({"capabilities":["model:   "],"remaining_units":0}),
    ] {
        let expected = if balance.get("show_name").is_some() {
            EntitlementState::Available
        } else {
            EntitlementState::Pending
        };
        let client = OfficialClient::new(Fixture::new(vec![
            response(200, serde_json::json!({})),
            response(
                200,
                serde_json::json!({"server_time":100,"plans":[{"status":"active","starts_at":120}],"balances":[balance]}),
            ),
        ]));
        let display = check_session(&client, &native, &snapshot, Some("4.1.2"), 100, &check)
            .await
            .unwrap()
            .display();
        assert_eq!(display.start.entitlement, expected);
    }
}

#[tokio::test]
async fn session_checks_transient_refresh_retains_only_bound_acceptance_and_original_time() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let mut initial = successful_responses();
    initial[1] = response(
        200,
        serde_json::json!({"plans":[{"status":"active","ends_at":500}],"balances":[{"remaining_units":7}]}),
    );
    initial[2] = response(
        200,
        serde_json::json!([{"productId":"coding-plan","status":"VALID","inCurrentPeriod":true}]),
    );
    let previous = check_session(
        &OfficialClient::new(Fixture::new(initial)),
        &native,
        &snapshot,
        Some("4.1.2"),
        100,
        &check,
    )
    .await
    .unwrap();
    let current = check_session(
        &OfficialClient::new(Fixture::new(vec![
            Err(OfficialError::Timeout),
            Err(OfficialError::Transport),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
        ])),
        &native,
        &snapshot,
        Some("4.1.2"),
        200,
        &check,
    )
    .await
    .unwrap();
    let merged = current
        .preserve_previous_acceptance(&previous, &native, &snapshot)
        .display();
    for result in [
        &merged.business.check,
        &merged.start.check,
        &merged.coding.check,
        &merged.coding.subscription,
    ] {
        assert_eq!(result.state, CheckState::Accepted);
        assert_eq!(result.checked_at, Some(100));
        assert_eq!(result.latest_failure.as_ref().unwrap().checked_at, 200);
    }
    assert_eq!(
        merged.business.official_owner_id,
        previous.display().business.official_owner_id
    );
    assert_eq!(merged.start.entitlement, EntitlementState::Available);
    assert_eq!(merged.coding.entitlement, EntitlementState::Available);
    assert_eq!(merged.start.quota.state, CheckState::Unknown);
    assert_eq!(merged.start.quota.checked_at, Some(200));
    assert!(merged.start.buckets.is_empty());
    assert!(merged.coding.limits.is_empty());
    assert_eq!(merged.coding.quota.state, CheckState::Unknown);
    let (_, changed) = fixture(
        OAuthFamily::Zai,
        Some("replacement-jwt"),
        None,
        Some("replacement-key"),
    );
    let changed_current = check_session(
        &OfficialClient::new(Fixture::new(vec![
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
        ])),
        &native,
        &changed,
        Some("4.1.2"),
        200,
        &check,
    )
    .await
    .unwrap();
    let changed_merged = changed_current
        .preserve_previous_acceptance(&previous, &native, &changed)
        .display();
    assert_eq!(changed_merged.business.check.state, CheckState::Accepted);
    assert_eq!(changed_merged.start.check.state, CheckState::Unknown);
    assert_eq!(changed_merged.coding.check.state, CheckState::Unknown);
    let never_accepted = current
        .preserve_previous_acceptance(&current, &native, &snapshot)
        .display();
    assert_eq!(never_accepted.business.check.state, CheckState::Unknown);
    assert_eq!(never_accepted.start.check.state, CheckState::Unknown);
    assert_eq!(never_accepted.coding.check.state, CheckState::Unknown);
}

#[tokio::test]
async fn session_checks_rejections_and_expired_entitlements_never_reuse_previous_readiness() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let mut initial = successful_responses();
    initial[1] = response(
        200,
        serde_json::json!({"plans":[{"status":"active","ends_at":150}],"balances":[]}),
    );
    initial[2] = response(
        200,
        serde_json::json!([{"productId":"coding-plan","status":"VALID","inCurrentPeriod":true}]),
    );
    let previous = check_session(
        &OfficialClient::new(Fixture::new(initial)),
        &native,
        &snapshot,
        Some("4.1.2"),
        100,
        &check,
    )
    .await
    .unwrap();
    for error in [
        OfficialError::Unauthorized,
        OfficialError::BusinessRejected,
        OfficialError::InvalidResponse,
    ] {
        let current = check_session(
            &OfficialClient::new(Fixture::new(vec![
                Err(error),
                Err(error),
                Err(error),
                Err(error),
            ])),
            &native,
            &snapshot,
            Some("4.1.2"),
            200,
            &check,
        )
        .await
        .unwrap();
        let merged = current
            .preserve_previous_acceptance(&previous, &native, &snapshot)
            .display();
        assert_ne!(merged.business.check.state, CheckState::Accepted);
        assert_ne!(merged.start.check.state, CheckState::Accepted);
        assert_ne!(merged.coding.check.state, CheckState::Accepted);
    }
    let mut expired = successful_responses();
    expired[1] = response(
        200,
        serde_json::json!({"plans":[{"status":"expired","ends_at":150}],"balances":[]}),
    );
    expired[2] = response(
        200,
        serde_json::json!([{"productId":"coding-plan","status":"EXPIRED","inCurrentPeriod":false}]),
    );
    let current = check_session(
        &OfficialClient::new(Fixture::new(expired)),
        &native,
        &snapshot,
        Some("4.1.2"),
        200,
        &check,
    )
    .await
    .unwrap();
    let merged = current
        .preserve_previous_acceptance(&previous, &native, &snapshot)
        .display();
    assert_eq!(merged.start.entitlement, EntitlementState::Unavailable);
    assert_eq!(merged.coding.entitlement, EntitlementState::Unavailable);
    let timed_out = check_session(
        &OfficialClient::new(Fixture::new(vec![
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
        ])),
        &native,
        &snapshot,
        Some("4.1.2"),
        200,
        &check,
    )
    .await
    .unwrap();
    let merged = timed_out
        .preserve_previous_acceptance(&previous, &native, &snapshot)
        .display();
    assert_ne!(merged.start.entitlement, EntitlementState::Available);
}

#[tokio::test]
async fn session_checks_start_acceptance_is_not_reused_under_a_different_app_version_policy() {
    let (native, snapshot) = fixture(
        OAuthFamily::Zai,
        Some("jwt-secret-canary"),
        None,
        Some("coding-secret-canary"),
    );
    let previous = check_session(
        &OfficialClient::new(Fixture::new(successful_responses())),
        &native,
        &snapshot,
        Some("4.1.2"),
        100,
        &check,
    )
    .await
    .unwrap();
    let current = check_session(
        &OfficialClient::new(Fixture::new(vec![
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
            Err(OfficialError::Timeout),
        ])),
        &native,
        &snapshot,
        Some("4.1.3"),
        200,
        &check,
    )
    .await
    .unwrap();
    assert!(previous.start_checked_for("4.1.2"));
    assert!(!previous.start_checked_for("4.1.3"));
    assert!(!current.start_checked_for("4.1.3"));
    let merged = current
        .preserve_previous_acceptance(&previous, &native, &snapshot)
        .display();
    assert_eq!(merged.start.check.state, CheckState::Unknown);
    assert_eq!(merged.business.check.state, CheckState::Accepted);
    assert_eq!(merged.coding.check.state, CheckState::Accepted);
}
