use super::*;
use serde_json::json;
#[test]
fn preview_keeps_unknown_units_and_orders_official_priority() {
    let plans=preview(&json!({"code":0,"data":{"plans":[
  {"plan_id":"low","priority":1,"entitlements":[]},
  {"planId":"high","priority":2,"entitlements":[{"meter":"model_usage","unitType":"token","showName":"Tokens"}]}]}})).unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].id, "high");
    assert_eq!(plans[0].units, None);
}
#[test]
fn malformed_preview_cannot_mean_no_claimable_plans() {
    assert!(preview(&json!({"code":0,"data":{}})).is_err());
    assert!(preview(&json!({"data":{"plans":[]}})).is_err());
}

fn claimable() -> Record {
    Record {
        status: Status::Claimable,
        plan_id: Some("plan".into()),
        ..Record::default()
    }
}
#[test]
fn durable_pending_prevents_double_click_and_restart_retry() {
    let mut r = claimable();
    assert!(r.may_submit());
    r.mark_submitted();
    assert!(!r.may_submit());
    let restored: Record = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(restored.status, Status::ResultPending);
    assert!(!restored.may_submit());
}
#[test]
fn ineligible_and_not_due_are_not_retried() {
    for code in [1004, 1005] {
        let mut r = claimable();
        r.mark_submitted();
        r.apply_reply(&json!({"code":code,"data":{"plan":{"ends_at":123}}}));
        assert_eq!(r.status, Status::NoClaim);
        assert!(!r.may_submit());
        assert_eq!(r.ends_at, Some(123));
    }
}
#[test]
fn captcha_rejection_returns_to_human_and_auth_failure_is_distinct() {
    let mut r = claimable();
    r.mark_submitted();
    r.apply_reply(&json!({"code":3007,"message":"credential-canary"}));
    assert_eq!(r.status, Status::VerificationRequired);
    assert!(!serde_json::to_string(&r)
        .unwrap()
        .contains("credential-canary"));
    r.apply_reply(&json!({"code":401}));
    assert_eq!(r.status, Status::LoginExpired);
}
#[test]
fn ambiguous_post_keeps_pending_until_new_active_plan_is_proven() {
    let mut r = claimable();
    r.prior_plan_ids = vec!["old".into()];
    r.mark_submitted();
    r.apply_reply(&json!({"message":"unreadable"}));
    assert_eq!(r.status, Status::ResultPending);
    r.reconcile(&json!({"plans":[{"plan_id":"plan","user_plan_id":"old","status":"active"}]}));
    assert_eq!(r.status, Status::ResultPending);
    r.reconcile(&json!({"plans":[{"plan_id":"plan","user_plan_id":"new","status":"active","starts_at":40,"ends_at":80}]}));
    assert_eq!(r.status, Status::Claimed);
    assert_eq!(r.ends_at, Some(80));
}

#[test]
fn successful_or_already_claimed_reply_blocks_another_post() {
    for code in [0, 1003] {
        let mut r = claimable();
        r.mark_submitted();
        r.apply_reply(&json!({"code":code}));
        assert_eq!(r.status, Status::Claimed);
        assert!(!r.may_submit());
    }
}
#[test]
fn empty_preview_is_valid_but_invalid_plan_id_is_not() {
    assert!(preview(&json!({"code":0,"data":{"plans":[]}}))
        .unwrap()
        .is_empty());
    assert!(preview(&json!({"code":0,"data":{"plans":[{"plan_id":""}]}})).is_err());
}

#[test]
fn captcha_webview_cannot_invoke_account_or_vault_commands() {
    assert!(!allowed_command(
        "zcode-claim-captcha",
        "get_zcode_account_library"
    ));
    assert!(!allowed_command(
        "zcode-claim-captcha",
        "switch_zcode_saved_account"
    ));
    assert!(allowed_command(
        "zcode-claim-captcha",
        "submit_zcode_claim_captcha"
    ));
    assert!(allowed_command("main", "get_zcode_account_library"));
}

#[test]
fn preflight_failure_cannot_erase_an_uncertain_post() {
    let mut r = claimable();
    r.prior_plan_ids = vec!["previous".into()];
    r.mark_submitted();
    r.preflight_failed("appVersionUnknown");
    assert_eq!(r.status, Status::ResultPending);
    assert_eq!(r.plan_id.as_deref(), Some("plan"));
    assert_eq!(r.prior_plan_ids, vec!["previous"]);
    assert!(!r.may_submit());
    r.preflight_failed("loginExpired");
    assert_eq!(r.status, Status::ResultPending);
}

#[test]
fn captcha_webview_cannot_invoke_workbuddy_commands() {
    for command in [
        "list_workbuddy_accounts",
        "refresh_workbuddy_account",
        "refresh_all_workbuddy_accounts",
        "claim_workbuddy_today",
        "begin_workbuddy_authorization",
        "finish_workbuddy_authorization",
    ] {
        assert!(!allowed_command("zcode-claim-captcha", command));
        assert!(allowed_command("main", command));
    }
}
