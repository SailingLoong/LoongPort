use super::*;
use serde_json::json;
#[test]
fn free_review_numeric_wire_underflow_is_unknown() {
    let model: ModelEntry = serde_json::from_str(
        r#"{"id":"vendor/model:free","pricing":{"prompt":1e-1000,"completion":0}}"#,
    )
    .unwrap();
    let row = describe_model(model, "https://unknown.invalid/models");
    assert_eq!(row.is_free, None);
    assert_eq!(row.pricing, None);
}
#[tokio::test]
#[serial_test::serial]
async fn free_review_slug_response_keeps_pricing() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(|| async {
        axum::Json(
            json!({"models":[{"slug":"glm-4.7-flash","pricing":{"prompt":"0","completion":"0"}}]}),
        )
    });
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = fetch_models(
        &format!("http://{address}"),
        "synthetic-key",
        false,
        None,
        None,
        None,
        None,
    )
    .await;
    server.abort();
    let wire = serde_json::to_value(result.unwrap()).unwrap();
    assert_eq!(wire[0]["id"], "glm-4.7-flash");
    assert_eq!(wire[0]["pricing"]["prompt"], "0");
    assert_eq!(wire[0]["isFree"], true);
}
fn describe(id: &str, price: Option<serde_json::Value>, source: &str) -> FetchedModel {
    let mut value = json!({"id":id,"owned_by":"synthetic"});
    if let Some(price) = price {
        value["pricing"] = price;
    }
    describe_model(serde_json::from_value(value).unwrap(), source)
}
#[test]
fn free_ids_require_the_actual_official_source_or_explicit_zero_prices() {
    for id in ["openrouter/free", "vendor/model:free"] {
        assert_eq!(
            describe(id, None, "https://openrouter.ai/api/v1/models").is_free,
            Some(true)
        );
        assert_eq!(
            describe(id, None, "https://openrouter.ai.evil.invalid/models").is_free,
            None
        );
        assert_eq!(
            describe(id, None, "http://openrouter.ai/models").is_free,
            None
        );
    }
    for id in ["glm-4.7-flash", "glm-4.5-flash"] {
        assert_eq!(
            describe(id, None, "https://open.bigmodel.cn/api/paas/v4/models").is_free,
            Some(true)
        );
        assert_eq!(
            describe(id, None, "https://unknown.invalid/models").is_free,
            None
        );
    }
}
#[test]
fn opencode_free_catalog_metadata_requires_the_exact_source_and_variant() {
    for model in crate::vendor::opencode::FREE_CHAT_MODELS {
        assert_eq!(
            describe(model, None, "https://opencode.ai/inference/v1/models").is_free,
            Some(true)
        );
        assert_eq!(
            describe(model, None, "https://unknown.invalid/models").is_free,
            None
        );
    }
    for model in ["glm-5.3-flash", "big-pickle", "unknown-free"] {
        assert_eq!(
            describe(model, None, "https://opencode.ai/inference/v1/models").is_free,
            None
        );
    }
    assert_eq!(
        describe(
            "glm-5.3-flash-free",
            Some(json!({"prompt":"0","completion":"1"})),
            "https://opencode.ai/inference/v1/models"
        )
        .is_free,
        Some(false)
    );
}

#[test]
fn all_advertised_charge_components_must_be_known_zero() {
    for (price, expected) in [
        (
            json!({"prompt":"0","completion":"0.000","request":"0e-2"}),
            Some(true),
        ),
        (json!({"prompt":0,"completion":0}), Some(true)),
        (
            json!({"prompt":"0","completion":"0","request":"0.1"}),
            Some(false),
        ),
        (
            json!({"prompt":"0","completion":"0","image":"1e-1000"}),
            Some(false),
        ),
        (json!({"prompt":"0"}), None),
        (
            json!({"prompt":"0","completion":"0","unknown":"unavailable"}),
            None,
        ),
        (json!({"prompt":"0","completion":"NaN"}), None),
        (json!({"prompt":"0","completion":"-1"}), None),
        (json!({"prompt":"0","completion":""}), None),
        (json!([]), None),
    ] {
        assert_eq!(
            describe(
                "vendor/model:free",
                Some(price.clone()),
                "https://openrouter.ai/api/v1/models"
            )
            .is_free,
            expected,
            "{price}"
        );
    }
}
#[test]
fn pricing_survives_the_wire_and_absent_metadata_does_not_invent_free() {
    let row = describe(
        "vendor/model",
        Some(json!({"prompt":"0.001","completion":"0.002"})),
        "https://openrouter.ai/api/v1/models",
    );
    let wire = serde_json::to_value(row).unwrap();
    assert_eq!(wire["pricing"]["prompt"], "0.001");
    assert_eq!(wire["isFree"], false);
    assert!(describe("ordinary", None, "https://unknown.invalid/models")
        .is_free
        .is_none());
    assert_eq!(
        describe(
            "ordinary/model",
            Some(json!({"prompt":"0","completion":"0"})),
            "https://openrouter.ai/api/v1/models"
        )
        .is_free,
        None
    );
    let frontend = include_str!("../../../src/lib/api/model-fetch.ts");
    assert!(frontend.contains("pricing?: Record<string, string> | null"));
    assert!(frontend.contains("isFree?: boolean | null"));
}
