use super::state::{
    decode, AppLiveState, LiveState, Mode, ModeState, PendingFile, StateDecodeError,
};
use serde_json::json;

#[test]
fn absent_mode_is_unknown_and_does_not_infer_direct_or_a_route() {
    let state = AppLiveState::default();
    assert_eq!(state.mode_state().mode, None);
    assert!(!state.mode_state().is_proxy());
    assert!(!state.mode_state().routes_to("p1"));
    let proxy = ModeState {
        mode: Some(Mode::Proxy),
        proxy_route: Some("p1".into()),
        ..Default::default()
    };
    assert!(proxy.routes_to("p1"));
    assert!(!proxy.routes_to("p2"));
    let direct = ModeState {
        mode: Some(Mode::Direct),
        ..proxy
    };
    assert!(!direct.routes_to("p1"));
}

#[test]
fn decode_rejects_invalid_or_unversioned_bytes_without_disclosing_values() {
    for bytes in [
        b"not json".as_slice(),
        b"null",
        b"{}",
        b"{\"version\":1,\"apps\":{\"codex\":{\"mode\":\"secret-canary\"}}}",
        b"{\"version\":1,\"version\":1,\"apps\":{}}",
    ] {
        let error = decode(bytes).unwrap_err();
        assert_eq!(error, StateDecodeError::InvalidData);
        assert!(!error.to_string().contains("secret-canary"));
    }
    assert_eq!(
        decode(br#"{"version":1,"apps":{}}"#).unwrap(),
        LiveState::default()
    );
}

#[test]
fn decode_rejects_future_or_old_state_versions() {
    for version in [0, 2, u32::MAX] {
        let bytes = serde_json::to_vec(&json!({"version":version,"apps":{}})).unwrap();
        assert_eq!(
            decode(&bytes),
            Err(StateDecodeError::UnsupportedVersion(version))
        );
    }
}

#[test]
fn absent_pending_privacy_remains_unknown_not_public() {
    let value =
        json!({"path":"/synthetic/client","pre":null,"planned":"abc","staged":"/synthetic/tmp"});
    let missing: PendingFile = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(missing.private, None);
    assert!(serde_json::to_value(&missing)
        .unwrap()
        .get("private")
        .is_none());
    for private in [false, true] {
        let mut explicit = value.clone();
        explicit["private"] = json!(private);
        let file: PendingFile = serde_json::from_value(explicit.clone()).unwrap();
        assert_eq!(file.private, Some(private));
        assert_eq!(serde_json::to_value(file).unwrap(), explicit);
    }
}

#[test]
fn nested_unknown_fields_survive_mode_and_pending_roundtrip() {
    let contract = json!({"version":1,"key":"digest","exclusive":{"field":"value"},"futureContract":{"keep":true}});
    let mode = json!({"mode":"proxy","attached":true,"proxy_route":"p1","contract":contract,"futureMode":[1,2]});
    let value = json!({
        "version":1, "futureRoot":true,
        "apps":{"codex":{
            "mode":"proxy", "attached":true, "proxy_route":"p1", "contract":contract,
            "written":{"tables":["table"],"futureWritten":true},
            "stack":{"enabled":true,"members":["p1"],"keys":{"key":"p1"},"futureStack":true},
            "futureApp":{"keep":"yes"},
            "pending":{
                "op":"future-op", "published":true, "futurePending":{"keep":true},
                "files":[{"path":"/synthetic/client","pre":null,"planned":"abc","staged":"/synthetic/tmp","private":true,"futureFile":"keep"}],
                "target":{"pointer":"p1","state":mode,"futureTarget":true}
            }
        }}
    });
    let state = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        state.apps["codex"].pending.as_ref().unwrap().op,
        "future-op"
    );
    assert_eq!(serde_json::to_value(state).unwrap(), value);
}

#[test]
fn mode_projection_retains_unknown_device_fields() {
    let app: AppLiveState =
        serde_json::from_value(json!({"mode":"proxy","futureApp":"original"})).unwrap();
    assert_eq!(
        app.mode_state().extra.get("futureApp"),
        Some(&json!("original"))
    );
}

#[test]
fn mode_updates_refuse_unknown_fields_without_changing_app() {
    for (app_value, next_value) in [
        (
            json!({"mode":"proxy","futureApp":"keep"}),
            json!({"mode":"direct"}),
        ),
        (
            json!({"mode":"proxy"}),
            json!({"mode":"direct","pending":{"future":"keep"}}),
        ),
        (
            json!({"mode":"proxy"}),
            json!({"mode":"direct","contract":{"version":1,"key":"digest","futureContract":true}}),
        ),
        (
            json!({"mode":"proxy","contract":{"version":1,"key":"digest","futureContract":true}}),
            json!({"mode":"direct"}),
        ),
    ] {
        let mut app: AppLiveState = serde_json::from_value(app_value).unwrap();
        let before = app.clone();
        let next: ModeState = serde_json::from_value(next_value).unwrap();
        assert!(app.set_mode_state(next).is_err());
        assert_eq!(app, before);
    }
}

#[test]
fn known_mode_updates_preserve_other_owned_state() {
    let mut app: AppLiveState = serde_json::from_value(
        json!({"mode":"proxy","written":{"tables":["keep"]},"stack":{"members":["p1"]}}),
    )
    .unwrap();
    let before = app.clone();
    app.set_mode_state(ModeState {
        mode: Some(Mode::Direct),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(app.mode, Some(Mode::Direct));
    assert_eq!(app.written, before.written);
    assert_eq!(app.stack, before.stack);
}

#[test]
fn duplicate_object_keys_cannot_erase_pending_or_reassign_stack_owners() {
    for bytes in [
        br#"{"version":1,"apps":{"codex":{"mode":"proxy","pending":{"op":"switch","published":true,"files":[]}},"codex":{"mode":"direct"}}}"#.as_slice(),
        br#"{"version":1,"apps":{"codex":{"stack":{"keys":{"model":"p1","model":"p2"}}}}}"#,
        br#"{"version":1,"apps":{},"future":{"items":[{"keep":1,"keep":2}]}}"#,
        br#"{"version":1,"apps":{},"future":{"\u006bey":1,"key":2}}"#,
    ] {
        assert_eq!(decode(bytes), Err(StateDecodeError::InvalidData));
    }
}

#[test]
fn unique_key_validation_preserves_all_json_value_kinds() {
    let bytes = br#"{"version":1,"apps":{},"future":[null,true,false,"text",-3,18446744073709551615,1.25,{},[]]}"#;
    let decoded = decode(bytes).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::from_slice::<serde_json::Value>(bytes).unwrap()
    );
}

#[test]
fn mode_updates_refuse_unsupported_contract_versions_without_mutation() {
    for version in [0, 2, u32::MAX] {
        let contract = json!({"version":version,"key":"digest"});
        for (app_value, next_value) in [
            (
                json!({"mode":"proxy","contract":contract}),
                json!({"mode":"direct"}),
            ),
            (
                json!({"mode":"direct"}),
                json!({"mode":"proxy","contract":contract}),
            ),
        ] {
            let mut app: AppLiveState = serde_json::from_value(app_value).unwrap();
            let before = app.clone();
            let next: ModeState = serde_json::from_value(next_value).unwrap();
            assert!(app.set_mode_state(next).is_err());
            assert_eq!(app, before);
        }
    }
}
