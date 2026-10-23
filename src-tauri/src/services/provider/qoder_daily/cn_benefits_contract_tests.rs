use super::*;
use serde_json::json;
fn row(id: &str) -> Value {
    json!({"campaignId":id,"actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMABLE"})
}
fn list(rows: Vec<Value>) -> Result<Vec<Activity>, ParseError> {
    listing(
        &serde_json::to_vec(&json!({"campaigns":rows})).unwrap(),
        "owner",
    )
}
fn target() -> CnTarget {
    CnTarget {
        campaign_id: "c1".into(),
        campaign_key: None,
        start_at: None,
        end_at: None,
    }
}
#[test]
fn minimal_upstream_fixture_needs_no_free_price_key_window_amount_or_uid() {
    let got = list(vec![row("c1")]).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target, target());
    assert_eq!(got[0].status, Status::Claimable);
    assert_eq!(got[0].skip_reason, None);
    assert_eq!(got[0].amount, None);
}
#[test]
fn explicit_claim_receipt_without_echo_is_bound_to_original_request() {
    for body in [
        json!({"status":"CLAIMED"}),
        json!({"data":{"status":"CLAIMED"}}),
    ] {
        assert_eq!(
            claimed(&serde_json::to_vec(&body).unwrap(), "owner", &target()),
            Ok(true)
        );
    }
}
#[test]
fn each_bad_activity_isolated_without_hiding_good_siblings() {
    let got = list(vec![row("c1"), json!(null), row("c2")]).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].skip_reason, None);
    assert_eq!(got[1].skip_reason, Some("invalid_activity"));
    assert_eq!(got[2].skip_reason, None);
}
#[test]
fn ordinary_opaque_ids_are_supported_without_path_assumptions() {
    for id in ["benefit:2026", "a.b", "活动", "campaign/one", "a%2Fb"] {
        let got = list(vec![row(id)]).unwrap();
        assert_eq!(got[0].target.campaign_id, id);
        assert_eq!(got[0].skip_reason, None);
    }
}
#[test]
fn duplicate_ids_and_dot_segments_never_claim() {
    let got = list(vec![row("c1"), row("c1"), row("."), row("..")]).unwrap();
    assert!(got.iter().all(|r| r.skip_reason.is_some()));
}
#[test]
fn known_extra_requirements_are_skipped_without_following_urls() {
    for patch in [
        json!({"purchaseRequired":true}),
        json!({"subscriptionRequired":true}),
        json!({"creditsCost":1}),
        json!({"termsAcceptanceRequired":true}),
        json!({"requiredAchievementKey":"invite"}),
        json!({"requiredAchievementKey":"invite","achievementCompleted":false}),
        json!({"unavailableReason":"purchase first"}),
    ] {
        let mut r = row("c1");
        r.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(list(vec![r]).unwrap()[0].skip_reason.is_some());
    }
    let mut r = row("c1");
    r["requiredAchievementKey"] = json!("invite");
    r["achievementCompleted"] = json!(true);
    r["detailUrl"] = json!("https://evil.invalid/task");
    assert_eq!(list(vec![r]).unwrap()[0].skip_reason, None);
}
#[test]
fn optional_fields_bind_actual_seconds_and_do_not_infer_daily_rounds() {
    let mut r = row("c1");
    r["campaignKey"] = json!("some_key");
    r["startAt"] = json!(1700000000_i64);
    r["endAt"] = json!(1800000000_i64);
    r["benefit"] = json!({"amount":12.5});
    let got = list(vec![r]).unwrap();
    assert_eq!(got[0].target.start_at, Some(1700000000));
    assert_eq!(got[0].target.end_at, Some(1800000000));
    assert_eq!(got[0].target.campaign_key.as_deref(), Some("some_key"));
    assert_eq!(got[0].amount, Some(12.5));
}
#[test]
fn present_uid_must_match_at_each_supported_envelope() {
    for body in [
        json!({"uid":"other","campaigns":[]}),
        json!({"campaigns":[{"campaignId":"c1","actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMABLE","uid":"other"}]}),
    ] {
        let got = listing(&serde_json::to_vec(&body).unwrap(), "owner");
        assert!(got.is_err() || got.unwrap()[0].skip_reason == Some("identity_mismatch"));
    }
    for body in [
        json!({"status":"CLAIMED","uid":"other"}),
        json!({"data":{"status":"CLAIMED","uid":"other"}}),
    ] {
        assert_eq!(
            claimed(&serde_json::to_vec(&body).unwrap(), "owner", &target()),
            Err(ParseError::IdentityMismatch)
        );
    }
}
#[test]
fn explicit_wrong_receipt_target_or_failed_envelope_cannot_confirm() {
    for body in [
        json!({"status":"CLAIMED","campaignId":"other"}),
        json!({"status":"CLAIMED","success":false}),
        json!({"status":"CLAIMED","code":500}),
        json!({"data":{"status":"CLAIMED"},"error":"sensitive"}),
    ] {
        assert_ne!(
            claimed(&serde_json::to_vec(&body).unwrap(), "owner", &target()),
            Ok(true)
        );
    }
}
#[test]
fn unknown_claim_status_never_implies_claimed() {
    for body in [
        json!({}),
        json!({"status":"CLAIMABLE"}),
        json!({"status":"SUCCESS"}),
        json!({"claimed":true}),
    ] {
        assert_ne!(
            claimed(&serde_json::to_vec(&body).unwrap(), "owner", &target()),
            Ok(true)
        );
    }
}
#[test]
fn malformed_key_and_interval_are_skips_not_absent_metadata() {
    for patch in [
        json!({"campaignKey":""}),
        json!({"startAt":"today"}),
        json!({"startAt":100,"endAt":99}),
    ] {
        let mut r = row("c1");
        r.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(list(vec![r]).unwrap()[0].skip_reason.is_some());
    }
}
#[test]
fn listing_error_and_bounds_are_not_empty_success() {
    for body in [
        json!({"code":401,"campaigns":[]}),
        json!({"data":{"campaigns":[]}}),
        json!({"campaigns":vec![row("c1");101]}),
    ] {
        assert!(listing(&serde_json::to_vec(&body).unwrap(), "owner").is_err());
    }
}
#[test]
fn explicit_null_optional_metadata_allows_first_claim() {
    let mut r = row("c1");
    r["campaignKey"] = Value::Null;
    r["startAt"] = Value::Null;
    r["endAt"] = Value::Null;
    let got = list(vec![r]).unwrap();
    assert_eq!(got[0].skip_reason, None);
    assert_eq!(got[0].target, target());
}
#[test]
fn receipt_may_enrich_unknown_metadata_but_not_conflict_with_known_values() {
    let rich =
        json!({"status":"CLAIMED","campaignId":"c1","campaignKey":"key","startAt":100,"endAt":200});
    assert_eq!(
        claimed(&serde_json::to_vec(&rich).unwrap(), "owner", &target()),
        Ok(true)
    );
    let mut known = target();
    known.campaign_key = Some("different".into());
    assert_ne!(
        claimed(&serde_json::to_vec(&rich).unwrap(), "owner", &known),
        Ok(true)
    );
}
#[test]
fn explicit_unknown_cost_or_condition_types_fail_closed_without_requiring_fields() {
    for patch in [
        json!({"purchaseRequired":"yes"}),
        json!({"subscriptionRequired":1}),
        json!({"termsAcceptanceRequired":{}}),
        json!({"creditsCost":"10"}),
        json!({"creditsCost":-1}),
    ] {
        let mut r = row("c1");
        r.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(
            list(vec![r]).unwrap()[0].skip_reason,
            Some("additional_condition")
        );
    }
    let mut r = row("c1");
    r["purchaseRequired"] = json!(false);
    r["subscriptionRequired"] = Value::Null;
    r["termsAcceptanceRequired"] = json!(false);
    r["creditsCost"] = json!(0);
    assert_eq!(list(vec![r]).unwrap()[0].skip_reason, None);
}
#[test]
fn explicit_business_security_stops_are_not_generic_activity_errors() {
    for value in [
        "SAME_PERSON_ALREADY_CLAIMED",
        "BLOCKED",
        "FORBIDDEN",
        "CHALLENGE_REQUIRED",
    ] {
        for body in [json!({"status":value}), json!({"data":{"code":value}})] {
            assert_eq!(
                claimed(&serde_json::to_vec(&body).unwrap(), "owner", &target()),
                Err(ParseError::VerificationRequired)
            );
        }
    }
}
