use super::{checkin::*, credits::*};
use serde_json::json;
const NOW: i64 = 1_800_000_000_000;
fn response(key: &str, items: serde_json::Value) -> serde_json::Value {
    json!({"code":0,"data":{key:items,"TotalCount":items.as_array().unwrap().len()}})
}
#[test]
fn balances_preserve_unknown_and_real_zero() {
    let summary = response(
        "Packages",
        json!([{"PackageCode":"a"},{"PackageCode":"b","CycleCapacityRemainPrecise":0}]),
    );
    let c = normalize(
        &summary,
        &response("Accounts", json!([])),
        &response("Accounts", json!([])),
        NOW,
    );
    assert_eq!(c.packages.len(), 2);
    assert_eq!(c.packages[0].remaining, None);
    assert_eq!(c.packages[1].remaining, Some(0.0));
    assert_eq!(c.total_remaining, None);
}
#[test]
fn resource_instances_are_not_deduplicated_by_package_code() {
    let details = response(
        "Accounts",
        json!([
            {"ResourceId":"r1","PackageCode":"a","CapacityRemainPrecise":"25.5","DeductionEndTime":NOW+1000},
            {"ResourceId":"r2","PackageCode":"a","CapacitySizePrecise":50,"CapacityUsedPrecise":10,"DeductionEndTime":NOW+2000},
            {"ResourceId":"expired","PackageCode":"a","CapacityRemainPrecise":900,"DeductionEndTime":NOW-1}
        ]),
    );
    let c = normalize(
        &response(
            "Packages",
            json!([{"PackageCode":"a","CycleCapacityRemainPrecise":65.5}]),
        ),
        &details,
        &response("Accounts", json!([])),
        NOW,
    );
    assert_eq!(c.total_remaining, Some(65.5));
    assert_eq!(c.nearest_expiry, Some(NOW + 1000));
    assert_eq!(c.packages.len(), 3);
    assert_eq!(c.updated_at, Some(NOW));
}
#[test]
fn failed_or_truncated_resource_branch_does_not_produce_total() {
    let empty = response("Accounts", json!([]));
    let summary = response("Packages", json!([]));
    assert_eq!(
        normalize(
            &summary,
            &empty,
            &json!({"code":401,"message":"secret-canary"}),
            NOW
        )
        .total_remaining,
        None
    );
    assert_eq!(
        normalize(&json!({}), &json!({}), &json!({}), NOW).total_remaining,
        None
    );
    let partial =
        json!({"code":0,"data":{"Accounts":[{"CapacityRemainPrecise":5}],"TotalCount":3}});
    assert_eq!(
        normalize(&summary, &partial, &empty, NOW).total_remaining,
        None
    );
    assert_eq!(
        normalize(&summary, &empty, &empty, NOW).total_remaining,
        Some(0.0)
    );
}
#[test]
fn malformed_negative_nonfinite_and_slice_capacity_are_unknown() {
    let details = response(
        "Accounts",
        json!([
            {"ResourceId":"a","CapacityRemainPrecise":"NaN"},
            {"ResourceId":"b","CapacityRemainPrecise":-1},
            {"ResourceId":"c","CapacityType":4,"CapacityRemainPrecise":800}
        ]),
    );
    let c = normalize(
        &response("Packages", json!([])),
        &details,
        &response("Accounts", json!([])),
        NOW,
    );
    assert_eq!(c.packages.len(), 2);
    assert!(c.packages.iter().all(|p| p.remaining.is_none()));
    assert_eq!(c.total_remaining, None);
}
#[test]
fn cn_dates_do_not_depend_on_host_timezone() {
    let details = response(
        "Accounts",
        json!([{"ResourceId":"a","CapacityRemainPrecise":5,"DeductionEndTime":"2027-01-15 08:00:00"}]),
    );
    let c = normalize(
        &response("Packages", json!([])),
        &details,
        &response("Accounts", json!([])),
        NOW,
    );
    assert_eq!(c.packages[0].expire_at, Some(1_799_971_200_000));
}
#[test]
fn checkin_state_requires_explicit_authoritative_fields() {
    assert_eq!(
        status(&json!({"code":0,"data":{}})),
        ClaimState::Unconfirmed
    );
    assert_eq!(
        status(&json!({"code":0,"data":{"active":true,"today_checked_in":false}})),
        ClaimState::Available
    );
    assert_eq!(
        status(&json!({"code":0,"data":{"active":false,"today_checked_in":false}})),
        ClaimState::Unavailable
    );
    assert_eq!(
        status(&json!({"code":0,"data":{"today_checked_in":true}})),
        ClaimState::AlreadyClaimed
    );
    assert_eq!(
        status(&json!({"code":10085})),
        ClaimState::NeedsVerification
    );
    assert_eq!(
        status(&json!({"code":401,"message":"secret-canary"})),
        ClaimState::NeedsVerification
    );
    assert_eq!(receipt(&json!({"code":1001})), ClaimState::AlreadyClaimed);
    assert_eq!(receipt(&json!({"code":1002})), ClaimState::Unavailable);
    assert_eq!(receipt(&json!({"code":1003})), ClaimState::Unavailable);
}
#[test]
fn claimed_requires_status_and_actual_credit_readback() {
    assert_eq!(
        reconcile(
            ClaimState::Claimed,
            ClaimState::AlreadyClaimed,
            Some(10.0),
            Some(30.0),
            Some(20.0)
        ),
        (ClaimState::Claimed, Some(20.0))
    );
    assert_eq!(
        reconcile(
            ClaimState::Claimed,
            ClaimState::AlreadyClaimed,
            Some(10.0),
            Some(10.0),
            Some(20.0)
        ),
        (ClaimState::Unconfirmed, None)
    );
    assert_eq!(
        reconcile(
            ClaimState::Claimed,
            ClaimState::AlreadyClaimed,
            None,
            Some(30.0),
            Some(20.0)
        ),
        (ClaimState::Unconfirmed, None)
    );
    assert_eq!(
        reconcile(
            ClaimState::Claimed,
            ClaimState::Available,
            Some(10.0),
            Some(30.0),
            Some(20.0)
        ),
        (ClaimState::Unconfirmed, None)
    );
    assert_eq!(
        reconcile(
            ClaimState::Unconfirmed,
            ClaimState::AlreadyClaimed,
            Some(10.0),
            Some(30.0),
            None
        ),
        (ClaimState::Unconfirmed, None)
    );
    assert_eq!(
        reconcile(
            ClaimState::AlreadyClaimed,
            ClaimState::AlreadyClaimed,
            Some(10.0),
            Some(30.0),
            None
        ),
        (ClaimState::AlreadyClaimed, None)
    );
}
#[test]
fn workbuddy_owned_file_uses_existing_vault_and_rejects_plaintext() {
    use crate::secrets::{owned_file::OwnedFile, VaultContext};
    let file = OwnedFile::registered("workbuddy_accounts.json")
        .expect("WorkBuddy must be in the existing registry");
    assert!(!file.allows_legacy_plaintext());
    let vault = VaultContext::generate().unwrap();
    let sealed = file.encode(&vault, b"SYNTHETIC_TOKEN_CANARY").unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("SYNTHETIC_TOKEN_CANARY"));
    assert_eq!(
        &*file.decode(&vault, &sealed).unwrap(),
        b"SYNTHETIC_TOKEN_CANARY"
    );
    assert!(file.decode(&vault, b"{}").is_err());
    assert!(OwnedFile::registered("zcode_account_profiles.json")
        .unwrap()
        .decode(&vault, &sealed)
        .is_err());
}
#[test]
fn official_transport_is_cn_only_and_never_places_credentials_in_urls() {
    use super::{http::*, model::*, protocol::Endpoint};
    let account = Account {
        uid: "synthetic-uid".into(),
        domain: Domain::CodeBuddy,
        label: "Synthetic".into(),
        token: "SYNTHETIC_TOKEN_CANARY".into(),
    };
    for e in [
        Endpoint::Summary,
        Endpoint::Paid,
        Endpoint::Free,
        Endpoint::Status,
        Endpoint::Claim,
    ] {
        let url = request_url(e, Some(&account), None).unwrap();
        assert_eq!(url.host_str(), Some("www.codebuddy.cn"));
        assert!(!url.as_str().contains("SYNTHETIC_TOKEN_CANARY"));
        assert_eq!(url.path(), e.path());
        assert!(request_url(e, None, None).is_err());
    }
    let url = request_url(Endpoint::AuthState, None, None).unwrap();
    assert!(url
        .query_pairs()
        .any(|(k, v)| k == "platform" && v == "workbuddy"));
    assert!(request_url(Endpoint::AuthToken, None, None).is_err());
    let url = request_url(Endpoint::AuthToken, None, Some("synthetic&injection=value")).unwrap();
    assert_eq!(url.query_pairs().count(), 1);
    assert_eq!(body(Endpoint::Claim, NOW), json!({}));
    assert_eq!(body(Endpoint::Free, NOW)["PageSize"], 200);
    assert_eq!(
        body(Endpoint::Free, NOW)["SlicePeriodStartTime"],
        "2027-01-15 00:00:00"
    );
}
#[test]
fn request_codes_are_real_package_codes_from_pinned_upstream() {
    use super::{http::body, protocol::Endpoint};
    assert_eq!(
        body(Endpoint::Paid, NOW)["PackageCodes"][0],
        "TCACA_code_002_AkiJS3ZHF5"
    );
    assert_eq!(
        body(Endpoint::Free, NOW)["PackageCodes"][0],
        "TCACA_code_008_cfWoLwvjU4"
    );
}
#[test]
fn deepest_nested_resource_count_must_be_complete() {
    let partial = json!({"code":0,"data":{"data":{"Response":{"Data":{"Accounts":[{"ResourceId":"a","CapacityRemainPrecise":5}],"TotalCount":2}}}}});
    assert_eq!(
        normalize(
            &response("Packages", json!([])),
            &partial,
            &response("Accounts", json!([])),
            NOW
        )
        .total_remaining,
        None
    );
}
#[test]
fn duplicated_resource_id_is_counted_once_and_conflicts_stay_unknown() {
    let item = json!({"ResourceId":"a","PackageCode":"p","CapacityRemainPrecise":5});
    let paid = response("Accounts", json!([item]));
    let free = response("Accounts", json!([item]));
    let c = normalize(&response("Packages", json!([])), &paid, &free, NOW);
    assert_eq!(c.total_remaining, Some(5.0));
    assert_eq!(c.packages.len(), 1);
    let conflicting = response(
        "Accounts",
        json!([{"ResourceId":"a","PackageCode":"p","CapacityRemainPrecise":9}]),
    );
    assert_eq!(
        normalize(&response("Packages", json!([])), &paid, &conflicting, NOW).total_remaining,
        None
    );
}
#[test]
fn far_future_deduction_placeholder_uses_actual_cycle_expiry() {
    let paid = response(
        "Accounts",
        json!([{"ResourceId":"a","CapacityRemainPrecise":5,"DeductionEndTime":"2049-01-01 00:00:00","CycleEndTime":NOW+5000}]),
    );
    assert_eq!(
        normalize(
            &response("Packages", json!([])),
            &paid,
            &response("Accounts", json!([])),
            NOW
        )
        .nearest_expiry,
        Some(NOW + 5000)
    );
}

#[test]
fn frontend_commands_and_claim_states_match_backend_contract() {
    let api = include_str!("../../../src/lib/api/workbuddy.ts");
    let commands = include_str!("../commands/workbuddy.rs");
    let registration = include_str!("../lib.rs");
    for name in [
        "list_workbuddy_accounts",
        "refresh_workbuddy_account",
        "refresh_all_workbuddy_accounts",
        "claim_workbuddy_today",
        "begin_workbuddy_authorization",
        "finish_workbuddy_authorization",
    ] {
        assert!(api.contains(&format!("\"{name}\"")));
        assert!(commands.contains(&format!("fn {name}(")));
        assert!(registration.contains(&format!("commands::{name}")));
    }
    for state in [
        ClaimState::Unconfirmed,
        ClaimState::Available,
        ClaimState::Claimed,
        ClaimState::AlreadyClaimed,
        ClaimState::Unavailable,
        ClaimState::NeedsVerification,
    ] {
        assert!(api.contains(&serde_json::to_string(&state).unwrap()));
    }
}
