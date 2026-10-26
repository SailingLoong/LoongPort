#![cfg(feature = "test-hooks")]
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_owned() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("owned").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_files() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("files").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_settings() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("settings").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_live() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("live").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_workbuddy() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("workbuddy").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_minimax() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("minimax").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_trae() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("trae").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_zcode_checkpoint() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("zcode_checkpoint").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_zcode_key_intent() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("zcode_key_intent").await;
}
#[tokio::test]
#[serial_test::serial]
async fn plaintext_owner_zcode_operation_log() {
    cc_switch_lib::integration_test_support::verify_credential_files_case("zcode_operation_log")
        .await;
}
