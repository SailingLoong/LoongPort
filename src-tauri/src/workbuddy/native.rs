//! Same-machine native auth snapshot core. No IO, process, IPC, or token conversion.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

const MAX_NATIVE_AUTH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeContext {
    pub product: String,
    pub region: String,
    pub realm: String,
    /// Caller-verified local user/install/key-context binding, not a token.
    pub source: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NativeError {
    InvalidDocument,
    UnsupportedContext,
    ContextMismatch,
    UnsupportedHistory,
    ConflictingAlias,
    OutputTooLarge,
}

pub(crate) struct NativeSnapshot {
    context: NativeContext,
    identity: String,
    bytes: Zeroizing<Vec<u8>>,
}

/// Restricted pure four-block preparation; no native write or activation claim.
// Remove when the native snapshot switch consumer is connected.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare(
    current: &NativeSnapshot,
    target: &NativeSnapshot,
) -> Result<Zeroizing<Vec<u8>>, NativeError> {
    if current.context != target.context {
        return Err(NativeError::ContextMismatch);
    }
    let mut root = SecretDocument(
        serde_json::from_slice(&current.bytes).map_err(|_| NativeError::InvalidDocument)?,
    );
    let target_doc = SecretDocument(
        serde_json::from_slice(&target.bytes).map_err(|_| NativeError::InvalidDocument)?,
    );
    let root_map = root.0.as_object_mut().ok_or(NativeError::InvalidDocument)?;
    let target_account = target_doc
        .0
        .get("account")
        .ok_or(NativeError::InvalidDocument)?;
    let target_auth = target_doc
        .0
        .get("auth")
        .ok_or(NativeError::InvalidDocument)?;
    if root_map
        .get("auth")
        .and_then(|a| a.get("domain"))
        .and_then(Value::as_str)
        != Some(current.context.realm.as_str())
        || target_auth.get("domain").and_then(Value::as_str) != Some(current.context.realm.as_str())
    {
        return Err(NativeError::ContextMismatch);
    }
    if root_map
        .get("uid")
        .is_some_and(|v| v.as_str() != Some(target.identity.as_str()))
    {
        return Err(NativeError::ConflictingAlias);
    }
    let accounts = root_map.get("accounts");
    let all = root_map.get("allAccounts");
    if matches!((accounts,all),(Some(a),Some(b)) if a!=b) {
        return Err(NativeError::UnsupportedHistory);
    }
    let mut rows_doc = SecretDocument(
        all.or(accounts)
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![])),
    );
    let rows = rows_doc
        .0
        .as_array_mut()
        .ok_or(NativeError::UnsupportedHistory)?;
    let realm = &current.context.realm;
    if !history_domain_matches(target_account, realm) {
        return Err(NativeError::UnsupportedHistory);
    }
    let mut keys = std::collections::BTreeSet::new();
    let mut found = None;
    for (i, row) in rows.iter().enumerate() {
        let key = history_key(row).ok_or(NativeError::UnsupportedHistory)?;
        if !keys.insert(key) || !history_domain_matches(row, realm) {
            return Err(NativeError::UnsupportedHistory);
        }
        if key == target.identity {
            if row.get("domain").and_then(Value::as_str) != Some(realm.as_str())
                && row != target_account
            {
                return Err(NativeError::UnsupportedHistory);
            }
            found = Some(i);
        }
    }
    // Discard borrowed keys before modifying the rows they point into.
    drop(keys);
    if let Some(i) = found {
        drop(SecretDocument(std::mem::replace(
            &mut rows[i],
            target_account.clone(),
        )));
    } else {
        rows.push(target_account.clone());
    }
    replace_secret(root_map, "account", target_account.clone());
    replace_secret(root_map, "auth", target_auth.clone());
    replace_secret(root_map, "accounts", rows_doc.0.clone());
    replace_secret(root_map, "allAccounts", rows_doc.0.clone());
    let output =
        Zeroizing::new(serde_json::to_vec(&root.0).map_err(|_| NativeError::InvalidDocument)?);
    if output.len() > MAX_NATIVE_AUTH_BYTES {
        return Err(NativeError::OutputTooLarge);
    }
    Ok(output)
}

fn history_key(row: &Value) -> Option<&str> {
    row.get("uid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            row.get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
}
fn history_domain_matches(row: &Value, realm: &str) -> bool {
    row.as_object()
        .is_some_and(|o| o.get("domain").is_none_or(|v| v.as_str() == Some(realm)))
}
fn replace_secret(root: &mut serde_json::Map<String, Value>, key: &str, value: Value) {
    if let Some(old) = root.insert(key.to_owned(), value) {
        drop(SecretDocument(old));
    }
}

// Validation discards scalar values and rejects duplicate object keys recursively.
// This is intentionally not a new native document serializer.
struct UniqueJson;
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueJson, A::Error> {
                while a.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut keys = std::collections::BTreeSet::new();
                while let Some(key) = a.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    a.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
        }
        d.deserialize_any(Visitor)
    }
}

// Parsed secret strings are scrubbed as well as the retained original byte image.
// This does not promise erasure of allocator/parser-internal temporary copies.
struct SecretDocument(Value);
impl Drop for SecretDocument {
    fn drop(&mut self) {
        fn scrub(v: &mut Value) {
            match v {
                Value::String(s) => s.zeroize(),
                Value::Array(a) => a.iter_mut().for_each(scrub),
                Value::Object(o) => {
                    for (mut key, mut value) in std::mem::take(o) {
                        key.zeroize();
                        scrub(&mut value);
                    }
                }
                _ => (),
            }
        }
        scrub(&mut self.0);
    }
}

fn text(v: &Value) -> Option<&str> {
    v.as_str()
        .filter(|s| !s.trim().is_empty() && !s.chars().any(char::is_control))
}
// Presence only, NOT validity, decryptability, or activation readiness. Preserve
// the legacy upstream object-envelope admission without interpreting its crypto.
// WorkBuddy 5.9.3 additionally uses exact standard/asym-v1 wrappers containing
// canonical base64. Only that outer format is checked; original bytes are kept.
fn native_access_present(v: &Value) -> bool {
    text(v).is_some()
        || v.as_object().is_some_and(|o| {
            if o.contains_key("$wbEncrypted") && o.get("envelope").is_some_and(Value::is_object) {
                return true;
            }
            if o.get("$wbEncrypted").and_then(Value::as_u64) != Some(1)
                || !(o.len() == 2
                    || (o.len() == 3 && o.get("scheme").and_then(Value::as_str) == Some("asym-v1")))
            {
                return false;
            }
            let Some(envelope) = o.get("envelope").and_then(Value::as_str) else {
                return false;
            };
            let Ok(decoded) = STANDARD.decode(envelope).map(Zeroizing::new) else {
                return false;
            };
            !decoded.is_empty() && STANDARD.encode(decoded.as_slice()) == envelope
        })
}

// Remove when official-session snapshot capture has a runtime consumer.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn capture(
    bytes: &[u8],
    context: &NativeContext,
) -> Result<NativeSnapshot, NativeError> {
    let identity = validate_stored_identity(bytes, context)?;
    Ok(NativeSnapshot {
        context: context.clone(),
        identity,
        bytes: Zeroizing::new(bytes.to_vec()),
    })
}

/// Shared structural validation only. A stored caller label is not official
/// provenance and this function cannot mint the NativeSnapshot consumer type.
pub(super) fn validate_stored_identity(
    bytes: &[u8],
    context: &NativeContext,
) -> Result<String, NativeError> {
    if context.product != "workbuddy"
        || context.region != "cn"
        || !matches!(
            context.realm.as_str(),
            "www.workbuddy.cn" | "www.codebuddy.cn"
        )
        || context.source.trim().is_empty()
        || context.source.chars().any(char::is_control)
    {
        return Err(NativeError::UnsupportedContext);
    }
    validated_identity(bytes, context.realm.as_str())
}

// Shared strict schema validator; it does not mint a source/install binding.
fn validated_identity(bytes: &[u8], realm: &str) -> Result<String, NativeError> {
    if bytes.len() > MAX_NATIVE_AUTH_BYTES {
        return Err(NativeError::InvalidDocument);
    }
    serde_json::from_slice::<UniqueJson>(bytes).map_err(|_| NativeError::InvalidDocument)?;
    let document =
        SecretDocument(serde_json::from_slice(bytes).map_err(|_| NativeError::InvalidDocument)?);
    let root = document.0.as_object().ok_or(NativeError::InvalidDocument)?;
    let account = root
        .get("account")
        .and_then(Value::as_object)
        .ok_or(NativeError::InvalidDocument)?;
    let auth = root
        .get("auth")
        .and_then(Value::as_object)
        .ok_or(NativeError::InvalidDocument)?;
    let identity = account
        .get("uid")
        .and_then(text)
        .ok_or(NativeError::InvalidDocument)?;
    // The upstream account builder includes this current enterprise-state field;
    // a personal type label does not override an active OneID enterprise context.
    if account
        .get("isCurrentOneIdEnterprise")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(NativeError::UnsupportedContext);
    }
    if !matches!(
        account.get("type").and_then(Value::as_str),
        Some("personal" | "individual")
    ) || [root, account, auth].iter().any(|o| {
        ["enterpriseId", "enterprise_id"].iter().any(|key| {
            o.get(*key)
                .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
        })
    }) {
        return Err(NativeError::UnsupportedContext);
    }
    if auth.get("domain").and_then(Value::as_str) != Some(realm) {
        return Err(NativeError::ContextMismatch);
    }
    // Upstream import checks these root aliases before the nested active blocks.
    // Do not label an ambiguous native capture as either account/realm.
    if root
        .get("uid")
        .is_some_and(|v| v.as_str() != Some(identity))
        || root
            .get("domain")
            .is_some_and(|v| v.as_str() != Some(realm))
    {
        return Err(NativeError::InvalidDocument);
    }
    if !auth.get("accessToken").is_some_and(native_access_present)
        || ["accounts", "allAccounts"]
            .iter()
            .any(|key| root.get(*key).is_some_and(|v| !v.is_array()))
    {
        return Err(NativeError::InvalidDocument);
    }
    Ok(identity.to_owned())
}

/// Observation is deliberately not NativeSnapshot: known file location does not
/// authenticate a machine/install/key context, and cannot authorize prepare/apply.
#[cfg(any(test, feature = "gui"))]
pub(super) fn observe(bytes: &[u8]) -> Result<&'static str, NativeError> {
    if bytes.len() > MAX_NATIVE_AUTH_BYTES {
        return Err(NativeError::InvalidDocument);
    }
    serde_json::from_slice::<UniqueJson>(bytes).map_err(|_| NativeError::InvalidDocument)?;
    let document =
        SecretDocument(serde_json::from_slice(bytes).map_err(|_| NativeError::InvalidDocument)?);
    let realm = match document
        .0
        .get("auth")
        .and_then(|a| a.get("domain"))
        .and_then(Value::as_str)
    {
        Some("www.workbuddy.cn") => "www.workbuddy.cn",
        Some("www.codebuddy.cn") => "www.codebuddy.cn",
        Some(_) => return Err(NativeError::UnsupportedContext),
        None => return Err(NativeError::InvalidDocument),
    };
    // The identity is required for original strict validation, never returned.
    let mut identity = validated_identity(bytes, realm)?;
    identity.zeroize();
    Ok(realm)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic values; shape comes from upstream auth_file.rs write/import methods,
    // commit 68468c17b3997d7eed965f1ae1eceb4992aa1efe, not any user's credentials.
    const A: &[u8] = br#"{
      "account":{"uid":"synthetic-a","type":"personal","nickname":"A"},
      "auth":{"accessToken":"synthetic-access","refreshToken":"synthetic-refresh","domain":"www.workbuddy.cn","tokenType":"Bearer"},
      "accounts":[{"uid":"synthetic-a"}],"allAccounts":[{"uid":"synthetic-a"}],
      "future":{"preserve":"exact whitespace and original bytes"}
    }"#;
    const B: &[u8] = br#"{"account":{"uid":"synthetic-b","type":"personal","nickname":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":"synthetic"}}},"auth":{"accessToken":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":"synthetic-access"}},"refreshToken":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":"synthetic-refresh"}},"domain":"www.workbuddy.cn"},"accounts":[],"allAccounts":[]}"#;

    fn context() -> NativeContext {
        NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "synthetic-machine-user-install".into(),
        }
    }

    #[test]
    fn captures_entire_native_bytes_without_constructing_a_replacement() {
        let a = capture(A, &context()).unwrap();
        let b = capture(B, &context()).unwrap();
        assert_eq!(a.identity, "synthetic-a");
        assert_eq!(b.identity, "synthetic-b");
        assert_eq!(a.bytes.as_slice(), A);
        assert_eq!(b.bytes.as_slice(), B);
        assert_eq!(a.bytes.as_slice(), A);
    }

    #[test]
    fn both_cn_realms_are_distinct_and_never_normalized() {
        let bytes = String::from_utf8(A.to_vec())
            .unwrap()
            .replace("www.workbuddy.cn", "www.codebuddy.cn");
        assert!(matches!(
            capture(bytes.as_bytes(), &context()),
            Err(NativeError::ContextMismatch)
        ));
        let mut cb = context();
        cb.realm = "www.codebuddy.cn".into();
        let saved = capture(bytes.as_bytes(), &cb).unwrap();
        assert_ne!(saved.context.realm, context().realm);
        assert_eq!(saved.bytes.as_slice(), bytes.as_bytes());
    }

    #[test]
    fn rejects_product_region_source_and_realm_changes() {
        for (field, value) in [
            ("product", "codebuddy"),
            ("region", "ai"),
            ("realm", "www.workbuddy.ai"),
        ] {
            let mut other = context();
            match field {
                "product" => other.product = value.into(),
                "region" => other.region = value.into(),
                "realm" => other.realm = value.into(),
                _ => other.source = value.into(),
            }
            assert!(capture(A, &other).is_err());
        }
        let mut empty = context();
        empty.source.clear();
        assert!(capture(A, &empty).is_err());
    }

    #[test]
    fn rejects_api_rows_and_missing_or_encrypted_identity() {
        for bytes in [
            br#"{"uid":"a","domain":"www.workbuddy.cn","token":"synthetic"}"#.as_slice(),
            br#"{}"#,
            br#"[]"#,
            br#"null"#,
        ] {
            assert!(capture(bytes, &context()).is_err());
        }
        for replacement in ["\"\"", "null", "{\"$wbEncrypted\":1,\"envelope\":{}}"] {
            let bytes =
                String::from_utf8(A.to_vec())
                    .unwrap()
                    .replacen("\"synthetic-a\"", replacement, 1);
            assert!(capture(bytes.as_bytes(), &context()).is_err());
        }
    }

    #[test]
    fn rejects_empty_or_invalid_token_without_coercing_envelopes() {
        for replacement in [
            "\"\"",
            "\"   \"",
            "null",
            "123",
            "{}",
            "{\"$wbEncrypted\":1}",
        ] {
            let bytes = String::from_utf8(A.to_vec())
                .unwrap()
                .replace("\"synthetic-access\"", replacement);
            assert!(capture(bytes.as_bytes(), &context()).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_keys_at_every_depth_and_trailing_data() {
        for (old, new) in [
            ("\"uid\":", "\"uid\":\"other\",\"uid\":"),
            ("\"auth\":", "\"auth\":{},\"auth\":"),
            ("\"preserve\":", "\"preserve\":null,\"preserve\":"),
        ] {
            let bytes = String::from_utf8(A.to_vec()).unwrap().replacen(old, new, 1);
            assert!(capture(bytes.as_bytes(), &context()).is_err());
        }
        let mut trailing = A.to_vec();
        trailing.extend_from_slice(b"{}");
        assert!(capture(&trailing, &context()).is_err());
    }

    #[test]
    fn refuses_enterprise_profiles() {
        for (old, new) in [
            ("\"personal\"", "\"enterprise\""),
            (
                "\"nickname\":\"A\"",
                "\"nickname\":\"A\",\"enterpriseId\":\"corp\"",
            ),
        ] {
            let bytes = String::from_utf8(A.to_vec()).unwrap().replacen(old, new, 1);
            assert!(capture(bytes.as_bytes(), &context()).is_err());
        }
    }

    #[test]
    fn personal_label_cannot_override_current_oneid_enterprise_state() {
        let bytes = String::from_utf8(A.to_vec()).unwrap().replacen(
            "\"nickname\":\"A\"",
            "\"nickname\":\"A\",\"isCurrentOneIdEnterprise\":true",
            1,
        );
        assert!(matches!(
            capture(bytes.as_bytes(), &context()),
            Err(NativeError::UnsupportedContext)
        ));
    }

    #[test]
    fn root_identity_and_realm_cannot_disagree_with_active_blocks() {
        for prefix in [
            r#""uid":"another-account","#,
            r#""domain":"www.codebuddy.cn","#,
        ] {
            let mut bytes = A.to_vec();
            bytes.splice(1..1, prefix.bytes());
            assert!(capture(&bytes, &context()).is_err());
        }
    }

    #[test]
    fn rejects_malformed_oversized_and_deep_input() {
        assert!(capture(b"{", &context()).is_err());
        assert!(capture(&vec![b' '; MAX_NATIVE_AUTH_BYTES + 1], &context()).is_err());
        let nested = format!("{}0{}", "[".repeat(140), "]".repeat(140));
        assert!(capture(nested.as_bytes(), &context()).is_err());
    }
    fn snapshot(value: &Value) -> NativeSnapshot {
        capture(&serde_json::to_vec(value).unwrap(), &context()).unwrap()
    }
    fn value(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }
    fn prepared(current: &Value, target: &Value) -> Value {
        value(&prepare(&snapshot(current), &snapshot(target)).unwrap())
    }
    fn lists(document: &mut Value, rows: Value) {
        document["accounts"] = rows.clone();
        document["allAccounts"] = rows;
    }

    #[test]
    fn prepare_appends_target_preserving_current_root_and_other_rows() {
        let mut a = value(A);
        let mut b = value(B);
        lists(
            &mut a,
            serde_json::json!([{"uid":"synthetic-a","extra":"keep"},{"id":"synthetic-c","opaque":{"x":1}}]),
        );
        b["targetOnlyRoot"] = serde_json::json!("do not replay");
        b["auth"]["opaque"] = serde_json::json!({"retain":"target-only"});
        let out = prepared(&a, &b);
        assert_eq!(out["account"], b["account"]);
        assert_eq!(out["auth"], b["auth"]);
        assert_eq!(out["future"], a["future"]);
        assert!(out.get("targetOnlyRoot").is_none());
        assert_eq!(out["accounts"][0], a["accounts"][0]);
        assert_eq!(out["accounts"][1], a["accounts"][1]);
        assert_eq!(out["accounts"][2], b["account"]);
        assert_eq!(out["accounts"], out["allAccounts"]);
        assert_eq!(out["accounts"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn prepare_replaces_only_explicit_same_realm_target_row_in_place() {
        let mut a = value(A);
        let b = value(B);
        lists(
            &mut a,
            serde_json::json!([{"uid":"synthetic-c"},{"uid":"synthetic-b","domain":"www.workbuddy.cn","old":"old target only"},{"uid":"synthetic-a"}]),
        );
        let out = prepared(&a, &b);
        assert_eq!(out["accounts"][0], a["accounts"][0]);
        assert_eq!(out["accounts"][1], b["account"]);
        assert_eq!(out["accounts"][2], a["accounts"][2]);
        assert_eq!(out["accounts"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn prepare_uses_uid_before_id_and_empty_uid_id_fallback() {
        let mut a = value(A);
        let b = value(B);
        lists(
            &mut a,
            serde_json::json!([{"uid":"different","id":"synthetic-b"},{"uid":"","id":"synthetic-b","domain":"www.workbuddy.cn"}]),
        );
        let out = prepared(&a, &b);
        assert_eq!(out["accounts"][0], a["accounts"][0]);
        assert_eq!(out["accounts"][1], b["account"]);
        assert_eq!(out["accounts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn prepare_missing_realm_target_requires_exact_existing_row() {
        let mut a = value(A);
        let b = value(B);
        lists(&mut a, serde_json::json!([b["account"].clone()]));
        let out = prepared(&a, &b);
        assert_eq!(out["accounts"], a["accounts"]);
        a["accounts"][0]["old"] = serde_json::json!(true);
        a["allAccounts"] = a["accounts"].clone();
        assert!(prepare(&snapshot(&a), &snapshot(&b)).is_err());
    }

    #[test]
    fn prepare_rejects_divergent_duplicate_and_unidentified_history() {
        let b = snapshot(&value(B));
        let mut divergent = value(A);
        divergent["accounts"] = serde_json::json!([]);
        assert!(prepare(&snapshot(&divergent), &b).is_err());
        for rows in [
            serde_json::json!([{"uid":"a"},{"id":"a"}]),
            serde_json::json!([{}]),
            serde_json::json!([null]),
            serde_json::json!([{"uid":"","id":""}]),
        ] {
            let mut a = value(A);
            lists(&mut a, rows);
            assert!(prepare(&snapshot(&a), &b).is_err());
        }
    }

    #[test]
    fn prepare_rejects_foreign_and_invalid_history_or_target_domains() {
        for domain in [
            serde_json::json!("www.codebuddy.cn"),
            serde_json::json!(null),
            serde_json::json!(42),
        ] {
            let mut a = value(A);
            let mut b = value(B);
            lists(
                &mut a,
                serde_json::json!([{"uid":"other","domain":domain.clone()}]),
            );
            assert!(prepare(&snapshot(&a), &snapshot(&b)).is_err());
            b["account"]["domain"] = domain;
            assert!(prepare(&snapshot(&value(A)), &snapshot(&b)).is_err());
        }
    }

    #[test]
    fn prepare_single_or_absent_history_never_replays_target_history() {
        let mut b = value(B);
        lists(&mut b, serde_json::json!([{"uid":"stale-only"}]));
        for absent in ["accounts", "allAccounts"] {
            let mut a = value(A);
            a.as_object_mut().unwrap().remove(absent);
            let out = prepared(&a, &b);
            assert_eq!(
                out["accounts"],
                serde_json::json!([{"uid":"synthetic-a"},b["account"].clone()])
            );
            assert_eq!(out["accounts"], out["allAccounts"]);
        }
        let mut a = value(A);
        a.as_object_mut().unwrap().remove("accounts");
        a.as_object_mut().unwrap().remove("allAccounts");
        assert_eq!(
            prepared(&a, &b)["accounts"],
            serde_json::json!([b["account"].clone()])
        );
    }

    #[test]
    fn prepare_rejects_context_changes_and_conflicting_root_uid() {
        let a = snapshot(&value(A));
        let mut other = context();
        other.source = "other-machine".into();
        let b = capture(B, &other).unwrap();
        assert!(prepare(&a, &b).is_err());
        let mut cb = context();
        cb.realm = "www.codebuddy.cn".into();
        let bytes = String::from_utf8(B.to_vec())
            .unwrap()
            .replace("www.workbuddy.cn", "www.codebuddy.cn");
        assert!(prepare(&a, &capture(bytes.as_bytes(), &cb).unwrap()).is_err());
        let mut alias = value(A);
        alias["uid"] = serde_json::json!("synthetic-a");
        assert!(prepare(&snapshot(&alias), &snapshot(&value(B))).is_err());
    }

    #[test]
    fn prepare_rejects_output_over_bound_without_changing_input_snapshots() {
        let mut a = value(A);
        let mut b = value(B);
        a["largeShared"] = serde_json::json!("x".repeat(MAX_NATIVE_AUTH_BYTES / 2 + 100));
        b["auth"]["largeTarget"] = serde_json::json!("y".repeat(MAX_NATIVE_AUTH_BYTES / 2 + 100));
        let a = snapshot(&a);
        let b = snapshot(&b);
        let before_a = a.bytes.to_vec();
        let before_b = b.bytes.to_vec();
        assert!(matches!(prepare(&a, &b), Err(NativeError::OutputTooLarge)));
        assert_eq!(a.bytes.as_slice(), before_a);
        assert_eq!(b.bytes.as_slice(), before_b);
    }
    #[test]
    fn official_593_string_envelopes_are_opaque_and_preserve_exact_bytes() {
        for wrapper in [
            r#"{"$wbEncrypted":1,"envelope":"b3BhcXVlLWNpcGhlcnRleHQ="}"#,
            r#"{"$wbEncrypted":1,"scheme":"asym-v1","envelope":"b3BhcXVlLWNpcGhlcnRleHQ="}"#,
        ] {
            // Synthetic bytes test format presence only, NOT a valid crypto envelope.
            let bytes = String::from_utf8(A.to_vec())
                .unwrap()
                .replace("\"synthetic-access\"", wrapper);
            let snapshot = capture(bytes.as_bytes(), &context()).unwrap();
            assert_eq!(snapshot.bytes.as_slice(), bytes.as_bytes());
            assert_eq!(observe(bytes.as_bytes()).unwrap(), "www.workbuddy.cn");
        }
    }
    #[test]
    fn official_string_wrapper_rejects_noncanonical_or_unknown_shapes() {
        for wrapper in [
            r#"{"$wbEncrypted":0,"envelope":"YQ=="}"#,
            r#"{"$wbEncrypted":true,"envelope":"YQ=="}"#,
            r#"{"$wbEncrypted":null,"envelope":"YQ=="}"#,
            r#"{"$wbEncrypted":1,"envelope":""}"#,
            r#"{"$wbEncrypted":1,"envelope":" "}"#,
            r#"{"$wbEncrypted":1,"envelope":"YQ"}"#,
            r#"{"$wbEncrypted":1,"envelope":"YR=="}"#,
            r#"{"$wbEncrypted":1,"envelope":"YQ==\n"}"#,
            r#"{"$wbEncrypted":1,"envelope":"-w=="}"#,
            r#"{"$wbEncrypted":1,"envelope":"YQ==","extra":"unknown"}"#,
            r#"{"$wbEncrypted":1,"scheme":"unknown","envelope":"YQ=="}"#,
            r#"{"$wbEncrypted":1,"scheme":null,"envelope":"YQ=="}"#,
            r#"{"$wbEncrypted":1,"scheme":"asym-v1","envelope":"YQ==","extra":true}"#,
        ] {
            let bytes = String::from_utf8(A.to_vec())
                .unwrap()
                .replace("\"synthetic-access\"", wrapper);
            assert!(capture(bytes.as_bytes(), &context()).is_err());
            assert!(observe(bytes.as_bytes()).is_err());
        }
    }
}
