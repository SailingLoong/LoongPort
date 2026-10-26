//! Adapted from workbuddy-switch credits.rs (MIT); see docs/licenses/workbuddy-switch-MIT.txt.
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Package {
    pub id: String,
    pub name: String,
    pub remaining: Option<f64>,
    pub expire_at: Option<i64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Credits {
    pub total_remaining: Option<f64>,
    pub nearest_expiry: Option<i64>,
    pub updated_at: Option<i64>,
    pub packages: Vec<Package>,
    /// Sampling generation, never inferred from a later account view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expiry_observations: Vec<ExpiryObservation>,
}

impl Credits {
    pub(super) fn for_display(&self) -> Self {
        let mut display = self.clone();
        display.expiry_observations.clear();
        display.observed_generation = None;
        display
    }
}

fn value_at_path<'a>(mut value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for key in path {
        value = value.get(*key)?;
    }
    Some(value)
}
fn first_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key))
}
pub(crate) fn number(value: Option<&Value>) -> Option<f64> {
    let n = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    (n.is_finite() && n >= 0.0).then_some(n)
}
fn first_number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| number(value.get(*key)))
}
fn timestamp(value: Option<&Value>) -> Option<i64> {
    let v = value?;
    if let Some(n) = number(Some(v)) {
        let n = if n < 10_000_000_000.0 { n * 1000.0 } else { n };
        return (n > 0.0 && n < i64::MAX as f64).then_some(n.round() as i64);
    }
    let s = v.as_str()?;
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    // The CN billing contract's timezone-less dates use China standard time,
    // irrespective of the engine host's timezone.
    use chrono::TimeZone;
    let zone = chrono::FixedOffset::east_opt(8 * 3600)?;
    let t = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(23, 59, 59)
        })?;
    zone.from_local_datetime(&t)
        .single()
        .map(|t| t.timestamp_millis())
}
fn remaining(raw: &Value) -> Option<f64> {
    let remain = [
        "CapacityRemainPrecise",
        "CapacityRemain",
        "CycleCapacityRemainPrecise",
        "CycleCapacityRemain",
        "CycleRemainCapacity",
    ];
    if remain.iter().any(|k| raw.get(k).is_some()) {
        return first_number(raw, &remain);
    }
    let total = first_number(
        raw,
        &[
            "CapacitySizePrecise",
            "CapacitySize",
            "CycleCapacitySizePrecise",
            "CycleCapacitySize",
            "CycleTotalCapacity",
        ],
    )?;
    let used = first_number(
        raw,
        &[
            "CapacityUsedPrecise",
            "CapacityUsed",
            "CycleCapacityUsedPrecise",
            "CycleCapacityUsed",
            "CycleUsedCapacity",
        ],
    )?;
    (used <= total).then_some(total - used)
}
fn container<'a>(response: &'a Value, field: &str) -> Option<&'a Value> {
    if !super::checkin::success(response) {
        return None;
    }
    for path in [
        vec!["data"],
        vec!["data", "data"],
        vec!["data", "Response", "Data"],
        vec!["data", "data", "Response", "Data"],
    ] {
        let Some(container) = value_at_path(response, &path) else {
            continue;
        };
        if container
            .get(field)
            .or_else(|| container.get(field.to_ascii_lowercase()))
            .is_some_and(Value::is_array)
        {
            return Some(container);
        }
    }
    None
}
fn rows<'a>(response: &'a Value, field: &str) -> Option<&'a Vec<Value>> {
    let container = container(response, field)?;
    container
        .get(field)
        .or_else(|| container.get(field.to_ascii_lowercase()))?
        .as_array()
}
fn complete(response: &Value, field: &str) -> bool {
    let Some(container) = container(response, field) else {
        return false;
    };
    let rows = rows(response, field).unwrap();
    if let Some(count) = first_value(container, &["TotalCount", "totalCount"]) {
        return number(Some(count)).is_some_and(|n| n == rows.len() as f64);
    }
    rows.len() < 200
}
fn package(raw: &Value, index: usize) -> Package {
    let code = first_value(raw, &["PackageCode", "packageCode"])
        .and_then(Value::as_str)
        .unwrap_or("");
    let id = first_value(raw, &["ResourceId", "resourceId"])
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{code}:{index}"));
    let expire_at = timestamp(first_value(
        raw,
        &[
            "DeductionEndTime",
            "deductionEndTime",
            "ExpiredTime",
            "expiredTime",
        ],
    ))
    .or_else(|| timestamp(first_value(raw, &["CycleEndTime", "cycleEndTime"])));
    let cycle_end = timestamp(first_value(raw, &["CycleEndTime", "cycleEndTime"]));
    // Pinned upstream documents DeductionEndTime as a long-lived placeholder
    // when the actual capacity expires at the much earlier cycle boundary.
    let expire_at = match (expire_at, cycle_end) {
        (Some(deduction), Some(cycle))
            if deduction.saturating_sub(cycle) > 365 * 24 * 3600 * 1000 =>
        {
            Some(cycle)
        }
        (expiry, _) => expiry,
    };
    Package {
        id,
        name: first_value(raw, &["PackageName", "packageName"])
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect(),
        remaining: remaining(raw),
        expire_at,
    }
}
pub(crate) fn normalize(summary: &Value, paid: &Value, free: &Value, now: i64) -> Credits {
    let summary_rows = rows(summary, "Packages");
    let paid_rows = rows(paid, "Accounts");
    let free_rows = rows(free, "Accounts");
    if summary_rows.is_none() && paid_rows.is_none() && free_rows.is_none() {
        return Credits::default();
    }
    // Detail resource instances supersede summary aggregates for the same package;
    // ResourceId (rather than PackageCode) keeps separately purchased packs distinct.
    let details: Vec<&Value> = paid_rows
        .into_iter()
        .chain(free_rows)
        .flatten()
        .filter(|v| number(v.get("CapacityType")) != Some(4.0))
        .collect();
    let mut seen = std::collections::HashMap::new();
    let mut conflict = false;
    let details: Vec<&Value> = details
        .into_iter()
        .filter(|raw| {
            let Some(id) = first_value(raw, &["ResourceId", "resourceId"])
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                return true;
            };
            match seen.insert(id, *raw) {
                Some(previous) => {
                    conflict |= previous != *raw;
                    false
                }
                None => true,
            }
        })
        .collect();
    let codes: std::collections::HashSet<&str> = details
        .iter()
        .filter_map(|v| first_value(v, &["PackageCode", "packageCode"]).and_then(Value::as_str))
        .collect();
    let mut raw = details;
    raw.extend(summary_rows.into_iter().flatten().filter(|v| {
        number(v.get("CapacityType")) != Some(4.0)
            && !first_value(v, &["PackageCode", "packageCode"])
                .and_then(Value::as_str)
                .is_some_and(|c| codes.contains(c))
    }));
    let packages: Vec<Package> = raw
        .into_iter()
        .enumerate()
        .map(|(i, r)| package(r, i))
        .collect();
    let all_complete = !conflict
        && complete(summary, "Packages")
        && complete(paid, "Accounts")
        && complete(free, "Accounts");
    let active = packages
        .iter()
        .filter(|p| p.expire_at.is_none_or(|expiry| expiry > now));
    let total_remaining = all_complete
        .then(|| active.clone().map(|p| p.remaining).sum::<Option<f64>>())
        .flatten()
        .filter(|n| n.is_finite());
    let nearest_expiry = active
        .filter(|p| p.remaining.is_some_and(|n| n > 0.0))
        .filter_map(|p| p.expire_at)
        .min();
    Credits {
        total_remaining,
        nearest_expiry,
        updated_at: Some(now),
        packages,
        observed_generation: None,
        expiry_observations: expiry_observations(summary, paid, free),
    }
}

#[cfg(test)]
mod expiry_observation_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn raw_expiry_sources_survive_display_heuristics_and_conflicting_details() {
        let summary = json!({"code":0,"data":{"Packages":[{"PackageCode":"p","CapacityRemain":9,"ExpiredTime":"2026-10-29"}],"TotalCount":1}});
        let paid = json!({"code":0,"data":{"Accounts":[{"PackageCode":"p","ResourceId":"r","CapacityRemain":9,"DeductionEndTime":"2049-01-01","CycleEndTime":"2026-10-25"}],"TotalCount":1}});
        let free = json!({"code":0,"data":{"Accounts":[{"PackageCode":"p","ResourceId":"r","CapacityRemain":8,"DeductionEndTime":"2049-01-01","CycleEndTime":"2026-10-25"}],"TotalCount":1}});
        let value = serde_json::to_value(normalize(&summary, &paid, &free, 100)).unwrap();
        let observations = value["expiryObservations"]
            .as_array()
            .expect("raw owner observations");
        assert_eq!(observations.len(), 3);
        assert_eq!(observations[0]["source"], "summary");
        assert_eq!(observations[1]["source"], "paid_detail");
        assert_eq!(
            observations[1]["deadlines"]["DeductionEndTime"],
            "2049-01-01"
        );
        assert_eq!(observations[1]["deadlines"]["CycleEndTime"], "2026-10-25");
        assert_eq!(observations[1]["conflicting"], true);
        assert_eq!(observations[2]["conflicting"], true);
        assert_eq!(value["observedGeneration"], Value::Null);
    }
}

/// Protocol fields: magpie-community/plugins 8cdf2d2 packages/workbuddy/index.mjs.
/// Only consumption yes/no is projected; rates are never compared or converted.
pub(super) fn model_consumes_credits(data: &Value, model: &str) -> Option<bool> {
    let allowed = data.get("agents")?.as_array()?.iter().any(|a| {
        a.get("name").and_then(Value::as_str) == Some("cli")
            && a.get("models")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(model)))
    });
    if !allowed {
        return None;
    }
    // Even a positive base rate can be free during a promotion. Unknown target,
    // schedule or discount semantics must not manufacture a consumption proof.
    for promo in data.get("modelPromotions")?.as_array()? {
        if promo.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let ids = promo.get("modelIds")?.as_array()?;
        if ids
            .iter()
            .any(|id| id.as_str().is_none_or(|id| id == model))
        {
            return None;
        }
    }
    let rates: Vec<_> = data
        .get("models")?
        .as_array()?
        .iter()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(model))
        .map(|row| row.get("credits"))
        .collect();
    let raw = *rates.first()?;
    if rates.iter().any(|other| *other != raw) {
        return None;
    }
    let text = match raw? {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let text = text
        .strip_prefix('x')
        .or_else(|| text.strip_prefix('X'))
        .unwrap_or(&text);
    // Decimal lexical positivity avoids f64 underflow, negative zero and pricing.
    if text.is_empty()
        || text.chars().filter(|c| *c == '.').count() > 1
        || !text.chars().all(|c| c.is_ascii_digit() || c == '.')
        || !text.chars().any(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(text.bytes().any(|c| (b'1'..=b'9').contains(&c)))
}
#[cfg(test)]
mod model_credit_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exact_cli_model_credit_field_distinguishes_positive_free_and_unlisted() {
        let d = json!({"agents":[{"name":"cli","models":["paid","free"]}],
            "models":[{"id":"paid","credits":"x0.03"},{"id":"free","credits":"x0.00"}],"modelPromotions":[]});
        assert_eq!(model_consumes_credits(&d, "paid"), Some(true));
        assert_eq!(model_consumes_credits(&d, "free"), Some(false));
        assert_eq!(model_consumes_credits(&d, "alias"), None);
    }
    #[test]
    fn promotions_missing_rate_or_duplicate_conflict_cannot_prove_consumption() {
        let base = json!({"agents":[{"name":"cli","models":["m"]}],"models":[{"id":"m","credits":"x0.03"}],"modelPromotions":[]});
        let mut d = base.clone();
        d["modelPromotions"] = json!([{"kind":"discount","modelIds":["m"],"enabled":true,"schedule":{"daily":{}},"discount":{"discountedCredits":"x0"}}]);
        assert_eq!(model_consumes_credits(&d, "m"), None);
        let mut d = base.clone();
        d["models"] = json!([{"id":"m","credits":"x0.03"},{"id":"m","credits":"x0.00"}]);
        assert_eq!(model_consumes_credits(&d, "m"), None);
        let mut d = base.clone();
        d["models"][0]["credits"] = Value::Null;
        assert_eq!(model_consumes_credits(&d, "m"), None);
        let mut d = base;
        d.as_object_mut().unwrap().remove("modelPromotions");
        assert_eq!(model_consumes_credits(&d, "m"), None);
    }
}

/// Raw meter fields are independent of legacy display normalization. In
/// particular CycleEndTime never replaces a long DeductionEndTime here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExpiryObservation {
    pub source: String,
    pub resource_id: Option<String>,
    pub package_code: Option<String>,
    pub capacity_unit: Option<Value>,
    pub capacity_type: Option<Value>,
    pub remaining: Option<f64>,
    pub balance_fields: serde_json::Map<String, Value>,
    pub deadlines: serde_json::Map<String, Value>,
    pub conflicting: bool,
}
fn balance_fields(raw: &Value) -> serde_json::Map<String, Value> {
    [
        "CapacityRemainPrecise",
        "CapacityRemain",
        "CycleCapacityRemainPrecise",
        "CycleCapacityRemain",
        "CycleRemainCapacity",
    ]
    .into_iter()
    .filter_map(|key| raw.get(key).map(|v| (key.into(), v.clone())))
    .collect()
}
fn observed_remaining(raw: &Value) -> Option<f64> {
    let fields = balance_fields(raw);
    let mut remaining = None;
    for value in fields.values() {
        let amount = number(Some(value))?;
        if remaining.is_some_and(|previous| previous != amount) {
            return None;
        }
        remaining = Some(amount);
    }
    remaining
}
fn expiry_observations(summary: &Value, paid: &Value, free: &Value) -> Vec<ExpiryObservation> {
    let mut out: Vec<_> = [
        ("summary", summary, "Packages"),
        ("paid_detail", paid, "Accounts"),
        ("free_detail", free, "Accounts"),
    ]
    .into_iter()
    .flat_map(|(source, response, field)| {
        rows(response, field).into_iter().flatten().map(move |raw| {
            let string = |keys: &[&str]| {
                first_value(raw, keys)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            let mut deadlines = serde_json::Map::new();
            for (key, lower) in [
                ("DeductionEndTime", "deductionEndTime"),
                ("ExpiredTime", "expiredTime"),
                ("CycleEndTime", "cycleEndTime"),
            ] {
                if let Some(value) = first_value(raw, &[key, lower]) {
                    deadlines.insert(key.into(), value.clone());
                }
            }
            ExpiryObservation {
                source: source.into(),
                resource_id: string(&["ResourceId", "resourceId"]),
                package_code: string(&["PackageCode", "packageCode"]),
                capacity_unit: first_value(raw, &["CapacityUnit", "capacityUnit"]).cloned(),
                capacity_type: first_value(raw, &["CapacityType", "capacityType"]).cloned(),
                remaining: observed_remaining(raw),
                balance_fields: balance_fields(raw),
                deadlines,
                conflicting: false,
            }
        })
    })
    .collect();
    let conflicts: Vec<_> = out
        .iter()
        .map(|row| {
            row.resource_id.is_some()
                && out.iter().any(|other| {
                    other.resource_id == row.resource_id
                        && other.source != "summary"
                        && row.source != "summary"
                        && (other.package_code != row.package_code
                            || other.balance_fields != row.balance_fields
                            || other.deadlines != row.deadlines
                            || other.capacity_unit != row.capacity_unit
                            || other.capacity_type != row.capacity_type)
                })
        })
        .collect();
    for (row, conflict) in out.iter_mut().zip(conflicts) {
        row.conflicting = conflict;
    }
    out
}

/// Cached only by the existing authenticated model discovery owner. These are
/// raw product-config fields, not user-editable provider model declarations.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CreditModelSnapshot {
    pub observed_at_ms: i64,
    pub generation: u64,
    pub credential_binding: String,
    pub grant_id: String,
    pub credential_version: u64,
    pub data: Value,
}
impl CreditModelSnapshot {
    pub fn facts(data: &Value) -> Value {
        let mut facts = serde_json::Map::new();
        for key in ["agents", "models", "modelPromotions"] {
            if let Some(rows) = data.get(key).and_then(Value::as_array) {
                let keys: &[&str] = match key {
                    "agents" => &["name", "models"],
                    "models" => &["id", "credits"],
                    _ => &["kind", "modelIds", "enabled", "schedule", "discount"],
                };
                facts.insert(
                    key.into(),
                    Value::Array(
                        rows.iter()
                            .map(|row| {
                                Value::Object(
                                    keys.iter()
                                        .filter_map(|key| {
                                            row.get(*key).map(|v| ((*key).into(), v.clone()))
                                        })
                                        .collect(),
                                )
                            })
                            .collect(),
                    ),
                );
            }
        }
        Value::Object(facts)
    }
}

/// Public WorkBuddy Credits contract: individual credit packs are consumed by
/// earliest expiry; monthly grants do not carry. We accept only directly dated
/// credit resources here. A lone cycle boundary is not enough to identify a
/// monthly grant, and conflicting direct/cycle deadlines stay unknown.
fn direct_expiry(row: &ExpiryObservation) -> Option<i64> {
    fn calendar(raw: &Value) -> Option<i64> {
        let text = raw.as_str()?;
        if text.trim().parse::<f64>().is_ok() {
            return None;
        }
        timestamp(Some(raw))
    }
    let mut at = None;
    for key in ["DeductionEndTime", "ExpiredTime"] {
        if let Some(raw) = row.deadlines.get(key) {
            if raw.as_str().is_some_and(|value| value.trim().is_empty()) {
                continue;
            }
            // Native DeductionEndTime is milliseconds (Magpie endOf contract),
            // never seconds inferred from magnitude. Require a representable
            // integer; fractions, exponents and overflow remain unknown.
            let parsed = if key == "DeductionEndTime" {
                match raw {
                    Value::Number(n) => n.as_i64(),
                    Value::String(text) => text.parse::<i64>().ok().or_else(|| calendar(raw)),
                    _ => None,
                }
            } else {
                calendar(raw)
            }?;
            if parsed <= 0 || at.is_some_and(|at| at != parsed) {
                return None;
            }
            at = Some(parsed);
        }
    }
    let at = at?;
    if let Some(cycle) = row
        .deadlines
        .get("CycleEndTime")
        .filter(|raw| !raw.as_str().is_some_and(|value| value.trim().is_empty()))
    {
        if calendar(cycle)? != at {
            return None;
        }
    }
    Some(at)
}

fn credit_unit(row: &ExpiryObservation, all: &[ExpiryObservation]) -> bool {
    let mut units = Vec::new();
    if let Some(unit) = row.capacity_unit.as_ref() {
        units.push(unit);
    }
    if let Some(code) = row.package_code.as_ref() {
        units.extend(
            all.iter()
                .filter(|r| r.source == "summary" && r.package_code.as_ref() == Some(code))
                .filter_map(|r| r.capacity_unit.as_ref()),
        );
    }
    !units.is_empty() && units.iter().all(|v| v.as_str() == Some("credits"))
}

pub(crate) async fn expiry_candidates(
    session: &crate::secrets::session::SecretSession,
    account_id: &str,
    final_model: &str,
    now: i64,
) -> Result<Vec<crate::relay::entitlement_expiry::Candidate>, super::model::Failure> {
    let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
    let binding = super::store::binding(session)?;
    let rows = super::store::load(session, &binding)?;
    let row = rows
        .iter()
        .find(|r| r.account.id(binding.store_id()) == account_id)
        .ok_or(super::model::Failure::ExpiredAuthorization)?;
    Ok(project(row, account_id, final_model, now))
}
fn project(
    row: &super::model::SavedAccount,
    account_id: &str,
    final_model: &str,
    now: i64,
) -> Vec<crate::relay::entitlement_expiry::Candidate> {
    use crate::relay::entitlement_expiry::*;
    let models = row.credit_models.as_ref().filter(|snapshot| {
        snapshot.generation == row.generation
            && snapshot.observed_at_ms >= 0
            && snapshot.observed_at_ms <= now
            && snapshot.credential_binding == row.credential_binding()
            && row
                .inference
                .as_ref()
                .is_some_and(|grant| grant.matches_credit_snapshot(snapshot))
    });
    let consumption =
        models.and_then(|snapshot| model_consumes_credits(&snapshot.data, final_model));
    row.credits
        .expiry_observations
        .iter()
        .enumerate()
        .map(|(index, bucket)| {
            let owned = row.account.domain == super::model::Domain::WorkBuddy
                && row.credits.observed_generation == Some(row.generation)
                && row
                    .credits
                    .updated_at
                    .is_some_and(|at| at >= 0 && at <= now);
            let detail = bucket.source != "summary"
                && bucket.resource_id.is_some()
                && number(bucket.capacity_type.as_ref()) != Some(4.0)
                && bucket.package_code.as_deref() != Some("TCACA_code_001_PqouKr6QWV")
                && bucket.package_code.as_ref().is_some_and(|code| {
                    row.credits.expiry_observations.iter().any(|summary| {
                        summary.source == "summary" && summary.package_code.as_ref() == Some(code)
                    })
                });
            let unit = credit_unit(bucket, &row.credits.expiry_observations);
            Candidate {
                account_id: account_id.into(),
                final_model: final_model.into(),
                bucket_id: bucket
                    .resource_id
                    .clone()
                    .unwrap_or_else(|| format!("unresolved:{index}")),
                evidence: Evidence {
                    source: format!("workbuddy:{}", bucket.source),
                    trusted: owned && detail && !bucket.conflicting,
                    observed_at_ms: row
                        .credits
                        .updated_at
                        .zip(models.map(|m| m.observed_at_ms))
                        .map(|(a, b)| a.min(b)),
                    complete: false,
                    applicable_models: consumption.map(|consumes| {
                        if consumes && unit {
                            vec![final_model.into()]
                        } else {
                            vec![]
                        }
                    }),
                    remaining: bucket.remaining,
                    unit: unit.then(|| "workbuddy:credits".into()),
                    expiry: direct_expiry(bucket)
                        .map(Expiry::ConsumableEndsAtMs)
                        .unwrap_or(Expiry::Unknown),
                    loss_events: vec![],
                    remaining_precision: AmountPrecision::Exact,
                    concurrent_limits: None,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod expiry_review_tests {
    use super::*;
    use serde_json::json;
    fn observation(raw: Value) -> ExpiryObservation {
        let details = json!({"code":0,"data":{"Accounts":[raw]}});
        normalize(&json!({}), &details, &json!({}), 100)
            .expiry_observations
            .remove(0)
    }
    #[test]
    fn expiry_review_no_cross_family_balance_or_invalid_precise_fallback() {
        for raw in [
            json!({"CapacitySize":10,"CycleCapacityUsed":2}),
            json!({"CapacityRemainPrecise":"invalid","CapacityRemain":42}),
            json!({"CapacityRemain":42,"CycleCapacityRemain":0}),
        ] {
            assert_eq!(observation(raw).remaining, None);
        }
    }
    #[test]
    fn expiry_review_deduction_epoch_has_fixed_millisecond_units() {
        assert_eq!(
            direct_expiry(&observation(json!({"DeductionEndTime":"1800000000"}))),
            Some(1_800_000_000)
        );
        assert_eq!(
            direct_expiry(&observation(
                json!({"DeductionEndTime":1_800_000_000_000_i64})
            )),
            Some(1_800_000_000_000)
        );
        for value in [
            json!("1.8e12"),
            json!("1800000000.5"),
            json!("9223372036854775808"),
        ] {
            assert_eq!(
                direct_expiry(&observation(json!({"DeductionEndTime":value}))),
                None
            );
        }
    }
    #[test]
    fn expiry_review_internal_raw_observations_never_escape_account_view() {
        let mut row = super::super::model::SavedAccount::new(super::super::model::Account {
            uid: "synthetic".into(),
            domain: super::super::model::Domain::WorkBuddy,
            label: "synthetic".into(),
            token: "synthetic-token".into(),
        });
        row.credits.expiry_observations.push(observation(
            json!({"DeductionEndTime":{"raw":"PRIVATE-RAW-CANARY"}}),
        ));
        row.credits.observed_generation = Some(17);
        let displayed = serde_json::to_string(&row.view("synthetic-owner", "2026-10-24")).unwrap();
        assert!(!displayed.contains("PRIVATE-RAW-CANARY"));
        assert!(!displayed.contains("expiryObservations"));
        assert!(serde_json::to_string(&row)
            .unwrap()
            .contains("PRIVATE-RAW-CANARY"));
    }
}
#[cfg(test)]
mod expiry_empty_fields_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn empty_optional_calendar_fields_do_not_erase_direct_millisecond_expiry() {
        let raw = json!({"code":0,"data":{"Accounts":[{"ResourceId":"r","DeductionEndTime":1_800_000_000_000_i64,"ExpiredTime":"","CycleEndTime":""}]}});
        let facts = normalize(&json!({}), &raw, &json!({}), 1000);
        assert_eq!(
            direct_expiry(&facts.expiry_observations[0]),
            Some(1_800_000_000_000)
        );
    }
}
