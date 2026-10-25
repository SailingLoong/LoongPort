#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]

//! Synthetic upstream boundary; this is not real-account inference acceptance.
#[tokio::test]
async fn one_login_retains_one_complete_grant_with_separate_purpose_consent() {
    cc_switch_lib::integration_test_support::verify_workbuddy_shared_authorization("one_login")
        .await;
}
#[tokio::test]
async fn incomplete_login_cannot_fabricate_shared_credentials() {
    cc_switch_lib::integration_test_support::verify_workbuddy_shared_authorization(
        "incomplete_login",
    )
    .await;
}

macro_rules! fixture_case {
    ($name:ident, $case:expr) => {
        #[tokio::test]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_workbuddy_shared_authorization($case)
                .await;
        }
    };
}
fixture_case!(
    inference_revocation_preserves_claim_permission_and_credentials,
    "consent"
);
fixture_case!(
    account_reads_and_inference_share_one_refresh_owner,
    "refresh"
);
fixture_case!(
    purpose_consent_does_not_replay_an_unresolved_claim,
    "pending"
);
fixture_case!(
    legacy_access_only_account_is_not_moved_cross_domain,
    "legacy"
);
fixture_case!(
    checkin_only_refusal_does_not_invalidate_account_login,
    "checkin_refusal"
);

fixture_case!(final_claim_fence_reports_definitely_unsent, "final_fence");
fixture_case!(
    pending_browser_cannot_undo_explicit_inference_revocation,
    "late_revoke"
);
fixture_case!(
    supplemental_authorization_cannot_replace_another_identity,
    "wrong_identity"
);

fixture_case!(shared_grant_claim_repeated_click_never_replays, "claim");

fixture_case!(
    legacy_schema_and_same_owner_encrypted_restore_preserve_fences,
    "schema"
);
