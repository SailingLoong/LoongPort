//! Serialized routing request types shared by headless storage and GUI consumers.

/// Short-lived compatibility review. Its revision is a CAS image, not a
/// persistent completion receipt or permission to enter the native writer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyRoutingReview {
    pub draft_digest: String,
    pub revision: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "protocol", rename_all = "camelCase", deny_unknown_fields)]
pub enum PreparedRoutingRequest {
    Native {
        request: crate::mode::state::SaveRequest,
    },
    Legacy {
        review: LegacyRoutingReview,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_request_wire_format_remains_compatible() {
        let examples = [
            serde_json::json!({"protocol":"legacy","review":{"draftDigest":"digest","revision":"revision"}}),
            serde_json::json!({"protocol":"native","request":{"id":"synthetic-id","scope":"applicationRouting","draftDigest":"digest","revision":"revision"}}),
        ];
        for expected in examples {
            let decoded: PreparedRoutingRequest = serde_json::from_value(expected.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
        }
        assert!(serde_json::from_value::<PreparedRoutingRequest>(serde_json::json!({
            "protocol":"legacy", "review":{"draftDigest":"digest","revision":"revision","unexpected":true}
        })).is_err());
    }
}
