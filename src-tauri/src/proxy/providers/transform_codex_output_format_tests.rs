//! Wire-level regressions: exercise the real Responses request converters.
use super::transform_codex_anthropic::responses_request_to_anthropic;
use super::transform_codex_chat::responses_to_chat_completions;
use crate::proxy::error::ProxyError;
use serde_json::{json, Value};

fn request(format: Value) -> Value {
    json!({"model": "example-model", "input": "Return JSON", "text": {"format": format}})
}

fn schema_format() -> Value {
    json!({
        "type": "json_schema", "name": "answer_v1", "description": "",
        "schema": {
            "type": "object", "additionalProperties": false,
            "properties": {
                "items": {"type": "array", "items": {"$ref": "#/$defs/item"}},
                "status": {"enum": ["ok", "unknown"]}
            },
            "required": ["items", "status"],
            "$defs": {"item": {"type": "object", "properties": {"value": {"type": ["string", "null"]}},
                "required": ["value"], "additionalProperties": false}}
        }
    })
}

#[test]
fn maps_text_and_json_object_formats_to_chat_wire() {
    for format in [json!({"type": "text"}), json!({"type": "json_object"})] {
        let result = responses_to_chat_completions(request(format.clone())).unwrap();
        assert_eq!(result["response_format"], format);
        assert!(result.get("text").is_none());
    }
}

#[test]
fn nests_json_schema_without_changing_nested_schema_or_optional_values() {
    for strict in [
        Some(json!(true)),
        Some(json!(false)),
        Some(Value::Null),
        None,
    ] {
        let mut format = schema_format();
        if let Some(strict) = strict {
            format["strict"] = strict;
        }
        let input = request(format.clone());
        let before = input.clone();
        let result = responses_to_chat_completions(input.clone()).unwrap();
        let expected_schema = json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "items": {"type": "array", "items": {"$ref": "#/$defs/item"}},
                "status": {"enum": ["ok", "unknown"]}
            },
            "required": ["items", "status"],
            "$defs": {"item": {"type": "object", "properties": {"value": {"type": ["string", "null"]}},
                "required": ["value"], "additionalProperties": false}}
        });
        assert_eq!(result["response_format"]["type"], "json_schema");
        let mapped = &result["response_format"]["json_schema"];
        assert_eq!(mapped["name"], "answer_v1");
        assert_eq!(mapped["schema"], expected_schema);
        assert_eq!(mapped["description"], "");
        assert_eq!(mapped.get("strict"), format.get("strict"));
        assert!(mapped.get("type").is_none());
        assert_eq!(input, before);
    }
}

#[test]
fn absent_format_preserves_legacy_chat_response_format_exactly() {
    // The legacy extension is intentionally opaque, including provider extensions.
    for format in [
        json!({"type":"json_object"}),
        json!({"type":"json_schema", "json_schema": {
            "name":"legacy", "schema":{"type":"object"}, "strict":false
        }}),
        json!({"type":"vendor_format", "vendor_option":true}),
        Value::Null,
    ] {
        for text in [None, Some(json!({}))] {
            let mut input = json!({"input":"Return JSON", "response_format":format});
            if let Some(text) = text {
                input["text"] = text;
            }
            let result = responses_to_chat_completions(input).unwrap();
            assert_eq!(result.get("response_format"), Some(&format));
        }
    }
    for input in [
        json!({"input":"hello"}),
        json!({"input":"hello", "text":{}}),
    ] {
        let result = responses_to_chat_completions(input).unwrap();
        assert!(result.get("response_format").is_none());
    }
}

#[test]
fn rejects_malformed_or_unknown_formats_instead_of_dropping_constraints() {
    let invalid = [
        Value::Null,
        json!(false),
        json!("json_object"),
        json!([]),
        json!({}),
        json!({"type": null}),
        json!({"type": 42}),
        json!({"type":"xml"}),
        json!({"type":"json_object", "schema":{"type":"object"}}),
        json!({"type":"text", "strict":true}),
        json!({"type":"json_schema", "schema":{}}),
        json!({"type":"json_schema", "name":"answer"}),
        json!({"type":"json_schema", "name":"", "schema":{}}),
        json!({"type":"json_schema", "name":"invalid name", "schema":{}}),
        json!({"type":"json_schema", "name":"x".repeat(65), "schema":{}}),
        json!({"type":"json_schema", "name":"answer", "schema":null}),
        json!({"type":"json_schema", "name":"answer", "schema":[]}),
        json!({"type":"json_schema", "name":"answer", "schema":true}),
        json!({"type":"json_schema", "name":"answer", "schema":{}, "strict":"true"}),
        json!({"type":"json_schema", "name":"answer", "schema":{}, "description":7}),
        json!({"type":"json_schema", "name":"answer", "schema":{}, "future_constraint":true}),
    ];
    for format in invalid {
        let result = responses_to_chat_completions(request(format.clone()));
        assert!(
            matches!(result, Err(ProxyError::TransformError(_))),
            "accepted {format}: {result:?}"
        );
    }
    for text in [Value::Null, json!(false), json!([]), json!("invalid")] {
        let result = responses_to_chat_completions(json!({"input":"hello", "text":text}));
        assert!(matches!(result, Err(ProxyError::TransformError(_))));
    }
}

#[test]
fn rejects_conflicting_legacy_and_responses_formats() {
    let mut input = request(json!({"type":"json_object"}));
    input["response_format"] = json!({"type":"text"});
    assert!(matches!(
        responses_to_chat_completions(input),
        Err(ProxyError::TransformError(_))
    ));
    let mut input = request(schema_format());
    input["response_format"] = json!({"type":"json_object"});
    assert!(matches!(
        responses_to_chat_completions(input),
        Err(ProxyError::TransformError(_))
    ));
}

#[test]
fn accepts_equivalent_legacy_and_responses_formats() {
    let mut input = request(json!({"type":"json_object"}));
    input["response_format"] = json!({"type":"json_object"});
    let result = responses_to_chat_completions(input).unwrap();
    assert_eq!(result["response_format"], json!({"type":"json_object"}));
}

#[test]
fn structured_output_keeps_tools_and_original_context() {
    let mut input = request(
        json!({"type":"json_schema", "name":"answer", "schema":{"type":"object"}, "strict":true}),
    );
    input["instructions"] = json!("Keep this instruction exactly.");
    input["input"] = json!([
        {"role":"user", "content":[{"type":"input_text", "text":"Original question"}]},
        {"type":"function_call", "call_id":"call_1", "name":"lookup", "arguments":"{\"key\":\"x\"}"},
        {"type":"function_call_output", "call_id":"call_1", "output":"Original result"}
    ]);
    input["tools"] = json!([{"type":"function", "name":"lookup", "description":"Look up an item.", "parameters":{"type":"object", "properties":{"key":{"type":"string"}}}, "strict":false}]);
    input["tool_choice"] = json!("auto");
    input["parallel_tool_calls"] = json!(false);
    let before = input.clone();
    let result = responses_to_chat_completions(input.clone()).unwrap();
    assert_eq!(
        result["response_format"],
        json!({"type":"json_schema", "json_schema":{"name":"answer", "schema":{"type":"object"}, "strict":true}})
    );
    assert_eq!(
        result["tools"],
        json!([{"type":"function", "function":{"name":"lookup", "description":"Look up an item.", "parameters":{"type":"object", "properties":{"key":{"type":"string"}}}, "strict":false}}])
    );
    assert_eq!(result["tool_choice"], "auto");
    assert_eq!(result["parallel_tool_calls"], false);
    assert_eq!(
        result["messages"][0],
        json!({"role":"system", "content":"Keep this instruction exactly."})
    );
    assert_eq!(result["messages"][1]["content"], "Original question");
    assert_eq!(
        result["messages"][2]["tool_calls"][0]["function"]["name"],
        "lookup"
    );
    assert_eq!(result["messages"][3]["content"], "Original result");
    assert_eq!(input, before);
}

#[test]
fn anthropic_rejects_structured_constraints_without_lossless_support() {
    for format in [
        json!({"type":"json_object"}),
        schema_format(),
        json!({"type":"unknown"}),
        Value::Null,
    ] {
        let result = responses_request_to_anthropic(request(format), 1024);
        assert!(matches!(result, Err(ProxyError::TransformError(_))));
    }
    for format in [
        json!({"type":"json_object"}),
        json!({"type":"json_schema", "json_schema":{"name":"answer", "schema":{}}}),
    ] {
        let result = responses_request_to_anthropic(
            json!({"input":"Return JSON", "response_format":format}),
            1024,
        );
        assert!(matches!(result, Err(ProxyError::TransformError(_))));
    }
}

#[test]
fn anthropic_default_text_behavior_remains_usable() {
    for input in [
        json!({"input":"hello"}),
        request(json!({"type":"text"})),
        json!({"input":"hello", "response_format":{"type":"text"}}),
    ] {
        let result = responses_request_to_anthropic(input, 1024).unwrap();
        assert_eq!(result["messages"][0]["role"], "user");
        assert!(result.get("response_format").is_none());
    }
}

#[test]
fn legacy_null_is_unconstrained_for_anthropic_and_preserved_for_chat() {
    let input = json!({"model":"example-model", "input":"hello", "response_format":null});
    let chat = responses_to_chat_completions(input.clone()).unwrap();
    assert_eq!(chat.get("response_format"), Some(&Value::Null));
    let anthropic = responses_request_to_anthropic(input, 1024).unwrap();
    assert_eq!(
        anthropic["messages"][0]["content"],
        json!([{"type":"text", "text":"hello"}])
    );
    assert!(anthropic.get("response_format").is_none());
}

#[test]
fn legacy_null_does_not_conflict_with_explicit_text() {
    let mut input = request(json!({"type":"text"}));
    input["response_format"] = Value::Null;
    let chat = responses_to_chat_completions(input.clone()).unwrap();
    assert_eq!(chat["response_format"], json!({"type":"text"}));
    let anthropic = responses_request_to_anthropic(input, 1024).unwrap();
    assert_eq!(anthropic["messages"][0]["role"], "user");
}

#[test]
fn legacy_null_keeps_modern_json_constraints() {
    for (format, expected) in [
        (json!({"type":"json_object"}), json!({"type":"json_object"})),
        (
            json!({"type":"json_schema", "name":"answer", "schema":{"type":"object"}, "strict":true}),
            json!({"type":"json_schema", "json_schema":{"name":"answer", "schema":{"type":"object"}, "strict":true}}),
        ),
    ] {
        let mut input = request(format);
        input["response_format"] = Value::Null;
        let chat = responses_to_chat_completions(input.clone()).unwrap();
        assert_eq!(chat["response_format"], expected);
        let Err(ProxyError::TransformError(message)) = responses_request_to_anthropic(input, 1024)
        else {
            panic!("Anthropic must still reject the modern structured constraint");
        };
        assert!(message.contains("not supported losslessly"), "{message}");
    }
    // Legacy null is absence of a legacy constraint, not permission to accept
    // a malformed modern text configuration.
    let malformed = json!({"input":"hello", "response_format":null, "text":null});
    assert!(matches!(
        responses_to_chat_completions(malformed.clone()),
        Err(ProxyError::TransformError(_))
    ));
    assert!(matches!(
        responses_request_to_anthropic(malformed, 1024),
        Err(ProxyError::TransformError(_))
    ));
}
