//! Single unreleased execution contract. Durable bindings deliberately omit input and output.
use super::bindings::Scope;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) const MAX_PARALLEL_ANALYSES: usize = 8;

/// Each item is independent; the first item's model also performs the summary.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AnalysisInput {
    pub input: String,
    pub model: String,
}

pub(crate) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
fn denied() -> AppError {
    AppError::Config("paperclip.invalid_task".into())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Budget {
    pub mode: String,
    pub currency: String,
    pub limit_micros: u64,
}
impl Budget {
    pub(crate) fn valid(&self) -> bool {
        self.mode == "estimated"
            && matches!(self.currency.as_str(), "USD" | "CNY")
            && (1..=MAX_SAFE_INTEGER).contains(&self.limit_micros)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Task {
    pub version: u8,
    pub input: String,
    pub model: String,
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Constraints {
    pub model: String,
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}
impl Constraints {
    fn valid(&self) -> bool {
        !self.model.is_empty()
            && self.model.len() <= 128
            && self
                .model
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.:/".contains(&c))
            && (1..=4096).contains(&self.max_output_tokens)
            && (100..=30_000).contains(&self.timeout_ms)
            && self.budget.as_ref().is_none_or(Budget::valid)
    }
}
#[cfg(any(unix, test))]
impl Task {
    pub(crate) fn constraints(&self) -> Constraints {
        Constraints {
            model: self.model.clone(),
            max_output_tokens: self.max_output_tokens,
            timeout_ms: self.timeout_ms,
            budget: self.budget.clone(),
        }
    }
    pub(crate) fn valid(&self) -> bool {
        self.version == 3
            && !self.input.trim().is_empty()
            && self.input.len() <= 8192
            && self.constraints().valid()
    }
    pub(crate) fn fingerprint(&self) -> Result<String, AppError> {
        if !self.valid() {
            return Err(denied());
        }
        Ok(digest(
            b"paperclip-execution-v3\0",
            &serde_json::to_vec(self).map_err(|_| denied())?,
        ))
    }
}
pub(crate) fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(bytes);
    hex::encode(h.finalize())
}
pub(crate) fn valid_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn binding_parts(
    s: &Scope,
    app: &str,
    fingerprint: &str,
    constraints: &Constraints,
) -> Result<Value, AppError> {
    if !s.valid() || !super::bindings::id(app) || !valid_digest(fingerprint) || !constraints.valid()
    {
        return Err(denied());
    }
    let mut b = serde_json::to_value(s).map_err(|_| denied())?;
    b["version"] = json!(3);
    b["taskFingerprint"] = json!(fingerprint);
    b["constraints"] = serde_json::to_value(constraints).map_err(|_| denied())?;
    let data = serde_json::to_vec(&(app, &b)).map_err(|_| denied())?;
    b["bindingId"] = json!(digest(b"paperclip-binding-v3\0", &data));
    Ok(b)
}
#[cfg(any(unix, test))]
pub(crate) fn binding(s: &Scope, app: &str, task: &Task) -> Result<Value, AppError> {
    binding_parts(s, app, &task.fingerprint()?, &task.constraints())
}
pub(crate) fn validate_binding(b: &Value, s: &Scope, app: &str) -> Result<Constraints, AppError> {
    let constraints: Constraints =
        serde_json::from_value(b["constraints"].clone()).map_err(|_| denied())?;
    if binding_parts(
        s,
        app,
        b["taskFingerprint"].as_str().ok_or_else(denied)?,
        &constraints,
    )? != *b
    {
        return Err(denied());
    }
    Ok(constraints)
}
#[cfg(any(unix, test))]
pub(crate) fn scope_in_binding(b: &Value) -> Result<Scope, AppError> {
    let mut v = b.clone();
    let m = v.as_object_mut().ok_or_else(denied)?;
    for k in ["version", "bindingId", "taskFingerprint", "constraints"] {
        m.remove(k);
    }
    serde_json::from_value(v).map_err(|_| denied())
}
#[cfg(any(unix, test))]
pub(crate) fn task_for_start(
    b: &Value,
    s: &Scope,
    app: &str,
    task: Task,
) -> Result<Task, AppError> {
    if binding(s, app, &task)? != *b {
        return Err(denied());
    }
    Ok(task)
}
#[cfg(any(unix, test))]
pub(crate) fn outcome(b: &Value, status: &str) -> Value {
    json!({"bindingId":b["bindingId"],"status":status,"providerWorkStarted":null,"providerStopped":false,"output":null,"outputDigest":null,"usage":null,"reason":null,"outputUnavailable":false})
}
/// Persist only references and digests; response content remains in the worker/Paperclip.
#[cfg(any(unix, test))]
pub(crate) fn durable_result(result: &Value) -> Value {
    let mut value = result.clone();
    value.as_object_mut().unwrap().remove("output");
    value.as_object_mut().unwrap().remove("outputUnavailable");
    value
}
#[cfg(any(unix, test))]
pub(crate) fn recovered_result(receipt: &Value) -> Value {
    let mut value = receipt.clone();
    value["output"] = Value::Null;
    value["outputUnavailable"] = json!(!value["outputDigest"].is_null());
    value
}
pub(crate) fn validate_receipt(b: &Value, r: &Value) -> Result<(), AppError> {
    let status = r["status"].as_str().ok_or_else(denied)?;
    if !["completed", "failed", "unknown"].contains(&status) || r["bindingId"] != b["bindingId"] {
        return Err(denied());
    }
    let object = r.as_object().ok_or_else(denied)?;
    let expected = [
        "bindingId",
        "status",
        "providerWorkStarted",
        "providerStopped",
        "outputDigest",
        "usage",
        "reason",
    ];
    if object.len() != expected.len() || expected.iter().any(|k| !object.contains_key(*k)) {
        return Err(denied());
    }
    if !r["outputDigest"].is_null() && !r["outputDigest"].as_str().is_some_and(valid_digest) {
        return Err(denied());
    }
    if r["providerStopped"] != json!(false)
        || !(r["providerWorkStarted"].is_null() || r["providerWorkStarted"].is_boolean())
    {
        return Err(denied());
    }
    if !r["reason"].is_null()
        && !r["reason"].as_str().is_some_and(|s| {
            s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(denied());
    }
    if !r["usage"].is_null() {
        let u = r["usage"].as_object().ok_or_else(denied)?;
        if u.len() != 3
            || !u
                .get("requestId")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty() && s.len() <= 256)
            || !u
                .get("digest")
                .and_then(Value::as_str)
                .is_some_and(valid_digest)
            || u.get("currency") != Some(&json!("USD"))
        {
            return Err(denied());
        }
    }
    if status == "completed"
        && ((b["constraints"].get("budget").is_some() && r["usage"].is_null())
            || r["outputDigest"].is_null()
            || r["providerWorkStarted"] != json!(true)
            || !r["reason"].is_null())
    {
        return Err(denied());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn task() -> Task {
        Task {
            version: 3,
            input: "synthetic request".into(),
            model: "fixture-model".into(),
            max_output_tokens: 128,
            timeout_ms: 1000,
            budget: Some(Budget {
                mode: "estimated".into(),
                currency: "USD".into(),
                limit_micros: 1000,
            }),
        }
    }
    fn scope() -> Scope {
        Scope {
            company_id: "company".into(),
            task_id: "task".into(),
            agent_id: "agent".into(),
            run_id: "run".into(),
            resource_id: "pool".into(),
            owner_id: "owner".into(),
            snapshot_revision: "revision".into(),
        }
    }
    #[test]
    fn accepts_bounded_task_without_budget_v3() {
        assert!(task().valid());
        let mut old = task();
        old.version = 2;
        assert!(!old.valid());
        let mut v = serde_json::to_value(task()).unwrap();
        v.as_object_mut().unwrap().remove("budget");
        assert!(serde_json::from_value::<Task>(v).unwrap().valid());
    }
    #[test]
    fn rejects_null_missing_wrong_currency_and_unsafe_budget() {
        let mut t = task();
        for currency in ["EUR", ""] {
            t.budget.as_mut().unwrap().currency = currency.into();
            assert!(!t.valid());
        }
        t = task();
        t.budget.as_mut().unwrap().limit_micros = MAX_SAFE_INTEGER + 1;
        assert!(!t.valid());
        t = task();
        t.budget.as_mut().unwrap().mode = "hard".into();
        assert!(!t.valid());
    }
    #[test]
    fn binding_never_contains_original_task_text() {
        let t = task();
        let b = binding(&scope(), "codex", &t).unwrap();
        assert!(!b.to_string().contains(&t.input));
        assert!(b.get("task").is_none());
        assert!(task_for_start(&b, &scope(), "codex", t).is_ok());
    }
    #[test]
    fn input_budget_model_and_hash_shaped_input_cannot_reuse_binding() {
        let t = task();
        let b = binding(&scope(), "codex", &t).unwrap();
        for n in 0..4 {
            let mut changed = t.clone();
            match n {
                0 => changed.input.push('!'),
                1 => changed.budget.as_mut().unwrap().limit_micros += 1,
                2 => changed.model.push('x'),
                _ => changed.input = b["taskFingerprint"].as_str().unwrap().into(),
            };
            assert!(task_for_start(&b, &scope(), "codex", changed).is_err());
        }
    }
    #[test]
    fn paperclip_v3_adapter_fingerprint_and_output_digest_vectors_match() {
        let t:Task=serde_json::from_value(json!({"version":3,"input":"Synthetic only","model":"synthetic-model","maxOutputTokens":32,"timeoutMs":2000,"budget":{"mode":"estimated","currency":"USD","limitMicros":1000000}})).unwrap();
        assert_eq!(
            t.fingerprint().unwrap(),
            "3edca8c64843b93c8c6db128f44b88d43a6aae1ed8fb3dbc4f731b9f945321e5"
        );
        assert_eq!(
            digest(b"paperclip-output-v3\0", b"synthetic"),
            "7818035915e35e29a63808b3427d34371ebda4c8a9e3813dafa229c5748af496"
        );
    }
}
