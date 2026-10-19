use super::*;
use serde_json::json;
fn chat() -> Value {
    json!({"model":"synthetic-model","stream":true,"messages":[{"role":"system","content":"unchanged"},{"role":"user","content":"hello"}]})
}
#[test]
fn text_request_maps_wire_without_injecting_prompt() {
    let out = request(chat(), "synthetic-session").unwrap();
    assert_eq!(out["function"], "chat_v3");
    assert_eq!(out["config_name"], "synthetic-model");
    assert_eq!(out["session_id"], "synthetic-session");
    assert_eq!(
        out["messages"][0]["content"],
        json!([{"type":"text","text":"unchanged"}])
    );
    assert_eq!(out["messages"].as_array().unwrap().len(), 2);
}
#[test]
fn unsupported_capabilities_fail_before_dispatch() {
    for mutate in [
        json!({"tools":[{"type":"function","function":{"name":"foo"}}]}),
        json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.invalid/x"}}]}]}),
        json!({"messages":[{"role":"tool","content":"tool result"}]}),
        json!({"messages":[{"role":"assistant","content":"","tool_calls":[{}]}]}),
    ] {
        let mut body = chat();
        body.as_object_mut()
            .unwrap()
            .extend(mutate.as_object().unwrap().clone());
        assert!(request(body, "s").is_err());
    }
}
#[test]
fn structured_denial_wins_without_text_heuristics() {
    for code in [11001, 11004, 11140, 9074, 9095] {
        assert_eq!(
            denial(&json!({"code":code,"message":"secret must not echo"})),
            Some(403)
        );
    }
    assert_eq!(denial(&json!({"code":1005})), Some(429));
    assert_eq!(
        denial(&json!({"ResponseMetadata":{"Error":{"Code":"20101"}}})),
        Some(401)
    );
    assert_eq!(denial(&json!({"response":"quota token expired"})), None);
}
