use super::*;
use serde_json::json;

fn request(scope: &str) -> SaveRequest {
    serde_json::from_value(json!({
        "id": "33333333-3333-4333-8333-333333333333",
        "scope": scope,
        "providerId": "tier",
        "draftDigest": "a".repeat(64),
        "revision": "b".repeat(64)
    }))
    .expect("the original save receipt must accept the narrow model settings scope")
}

#[test]
fn model_settings_scope_roundtrips_in_the_original_receipt() {
    let request = request("providerModelSettings");
    request.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&request).unwrap()["scope"],
        "providerModelSettings"
    );
}

#[test]
fn model_settings_scope_requires_an_exact_nonempty_provider_id() {
    let mut request = request("providerModelSettings");
    request.provider_id = None;
    assert!(request.validate().is_err());
    request.provider_id = Some(" ".into());
    assert!(request.validate().is_err());
}

#[test]
fn model_settings_receipt_cannot_equal_a_full_provider_edit() {
    let narrow = request("providerModelSettings");
    let full = request("provider");
    assert_ne!(narrow, full);
}

#[test]
fn old_provider_receipt_keeps_its_original_wire_image() {
    let request = request("provider");
    request.validate().unwrap();
    let value = serde_json::to_value(&request).unwrap();
    assert!(value.get("scope").is_none());
    assert_eq!(
        serde_json::from_value::<SaveRequest>(value).unwrap(),
        request
    );
}
