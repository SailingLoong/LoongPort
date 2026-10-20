use super::*;
use serde_json::json;
#[test]
fn absent_entitlement_is_unknown_not_zero() {
    let e = project(&json!({}));
    assert_eq!(e.remaining_credits, None);
    assert_eq!(e.has_token_plan, None);
}
#[test]
fn credits_and_plan_are_independent() {
    let e = project(
        &json!({"data":{"op_credit_summary":{"total_remaining_amount":"23.50"},"has_token_plan":false}}),
    );
    assert_eq!(e.remaining_credits.as_deref(), Some("23.50"));
    assert_eq!(e.has_token_plan, Some(false));
    let p = project(
        &json!({"has_token_plan":true,"token_plan_tier":"max","token_plan_expires_at":12345}),
    );
    assert_eq!(p.remaining_credits, None);
    assert_eq!(p.has_token_plan, Some(true));
    assert_eq!(p.token_plan_expires_at, Some(12345));
}
#[test]
fn zero_is_known_and_daily_claim_is_not_balance() {
    assert_eq!(
        project(&json!({"opcredit_balance":0}))
            .remaining_credits
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        project(&json!({"points":200,"creditAmount":200})).remaining_credits,
        None
    );
}
#[test]
fn invalid_credit_values_remain_unknown() {
    for v in [
        json!(-1),
        json!("NaN"),
        json!("-1"),
        json!(" 1 "),
        json!(true),
        json!("Bearer secret"),
        json!("1e99"),
    ] {
        assert_eq!(
            project(&json!({"opcredit_balance":v})).remaining_credits,
            None
        );
    }
}
#[test]
fn official_summary_precedes_legacy_balance() {
    assert_eq!(
        project(&json!({"op_credit_summary":{"total_remaining_amount":"7"},"opcredit_balance":12}))
            .remaining_credits
            .as_deref(),
        Some("7")
    );
}
#[test]
fn unrelated_workspace_values_are_not_inferred() {
    let e = project(
        &json!({"workspaces":[{"workspace_type":1,"has_token_plan":true,"opcredit_balance":99}]}),
    );
    assert_eq!(e.remaining_credits, None);
    assert_eq!(e.has_token_plan, None);
}

#[test]
fn explicit_null_root_uses_official_data_fallback() {
    let e = project(
        &json!({"opcredit_balance":null,"has_token_plan":null,"data":{"opcredit_balance":"4","has_token_plan":true}}),
    );
    assert_eq!(e.remaining_credits.as_deref(), Some("4"));
    assert_eq!(e.has_token_plan, Some(true));
}

#[test]
fn quota_zero_differs_from_unknown_and_reset_stays_milliseconds() {
    let q=project_quota(&json!({"model_remains":[{"current_interval_remaining_percent":0,"end_time":1790000000123_i64,"weekly_end_time":1790100000000_i64}]})).unwrap();
    assert_eq!(q.five_hour.remaining_percent, Some(0.0));
    assert_eq!(q.weekly.remaining_percent, None);
    assert_eq!(q.five_hour.reset_at_ms, Some(1790000000123.0));
}
#[test]
fn official_legacy_usage_count_is_remaining_not_consumed() {
    let q=project_quota(&json!({"model_remains":[{"current_interval_total_count":100,"current_interval_usage_count":80,"current_weekly_total_count":500,"current_weekly_usage_count":100}]})).unwrap();
    assert_eq!(q.five_hour.remaining_percent, Some(80.0));
    assert_eq!(q.weekly.remaining_percent, Some(20.0));
}
#[test]
fn explicit_percent_precedes_legacy_and_is_rounded_clamped() {
    let q=project_quota(&json!({"model_remains":[{"current_interval_remaining_percent":12.5,"current_interval_total_count":100,"current_interval_usage_count":90,"current_weekly_remaining_percent":110}]})).unwrap();
    assert_eq!(q.five_hour.remaining_percent, Some(13.0));
    assert_eq!(q.weekly.remaining_percent, Some(100.0));
}
#[test]
fn unlimited_has_no_fake_numeric_percentage() {
    let q=project_quota(&json!({"model_remains":[{"current_interval_status":3,"current_interval_remaining_percent":0,"current_weekly_remaining_percent":"90"}]})).unwrap();
    assert!(q.five_hour.unlimited);
    assert_eq!(q.five_hour.remaining_percent, None);
    assert_eq!(q.weekly.remaining_percent, None);
}
#[test]
fn missing_quota_rows_do_not_manufacture_available_zero() {
    for v in [
        json!({}),
        json!({"model_remains":[]}),
        json!({"model_remains":[null,1,"bad"]}),
    ] {
        assert_eq!(project_quota(&v), None);
    }
}
