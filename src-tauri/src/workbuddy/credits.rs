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
    }
}
