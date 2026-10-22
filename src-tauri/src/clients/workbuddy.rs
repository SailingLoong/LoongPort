use serde_json::Value;

pub fn parse(current: Option<&[u8]>) -> Result<Value, String> {
    let mut document = match current {
        None => Value::Array(Vec::new()),
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| "clients.invalid_json")?,
    };
    if let Some(object) = document.as_object_mut() {
        object
            .entry("models")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(available) = object.get("availableModels") {
            if !available
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
            {
                return Err("clients.invalid_models".into());
            }
        }
    }
    models(&document)?;
    Ok(document)
}

pub fn models(document: &Value) -> Result<&Vec<Value>, String> {
    let value = if document.is_object() {
        &document["models"]
    } else {
        document
    };
    value
        .as_array()
        .ok_or_else(|| "clients.invalid_models".into())
}

pub fn project(current: Option<&[u8]>, entries: &[Value]) -> Result<Vec<u8>, String> {
    let mut document = parse(current)?;
    let existing = models(&document)?;
    let mut output = existing.clone();
    for entry in entries {
        if let Some(index) = output
            .iter()
            .position(|item| item["id"] == entry["id"] && item["vendor"] == "LoongPort")
        {
            let object = output[index]
                .as_object_mut()
                .ok_or("clients.invalid_models")?;
            for (key, value) in entry.as_object().ok_or("clients.invalid_models")? {
                object.insert(key.clone(), value.clone());
            }
        } else {
            output.push(entry.clone());
        }
    }
    if document.is_object() {
        if let Some(available) = document
            .get_mut("availableModels")
            .and_then(Value::as_array_mut)
        {
            if !available.is_empty() {
                for entry in entries {
                    if !available.contains(&entry["id"]) {
                        available.push(entry["id"].clone());
                    }
                }
            }
        }
        document["models"] = Value::Array(output);
    } else {
        document = Value::Array(output);
    }
    // Preserve formatting and exact bytes for semantic no-ops.
    if let Some(bytes) = current {
        if serde_json::from_slice::<Value>(bytes).ok().as_ref() == Some(&document) {
            return Ok(bytes.to_vec());
        }
    }
    serde_json::to_vec_pretty(&document).map_err(|_| "clients.invalid_json".into())
}

/// Restore only entries whose complete written value still matches. A client
/// editing one of those entries is a conflict, not permission to delete it.
pub fn restore(
    current: &[u8],
    original: Option<&[u8]>,
    installed: &[Value],
) -> Result<Vec<u8>, String> {
    let mut document = parse(Some(current))?;
    let original_doc = parse(original)?;
    let before = models(&original_doc)?;
    let mut output = models(&document)?.clone();
    for entry in installed {
        let index = output
            .iter()
            .position(|value| value["id"] == entry["id"] && value["vendor"] == entry["vendor"])
            .ok_or("clients.configuration_changed")?;
        if &output[index] != entry {
            return Err("clients.configuration_changed".into());
        }
        if let Some(old) = before
            .iter()
            .find(|value| value["id"] == entry["id"] && value["vendor"] == entry["vendor"])
        {
            output[index] = old.clone();
        } else {
            output.remove(index);
        }
    }
    if document.is_object() {
        if let Some(available) = document
            .get_mut("availableModels")
            .and_then(Value::as_array_mut)
        {
            let old = original_doc
                .get("availableModels")
                .and_then(Value::as_array);
            available.retain(|value| {
                !installed.iter().any(|entry| &entry["id"] == value)
                    || old.is_some_and(|items| items.contains(value))
            });
        }
        document["models"] = Value::Array(output);
    } else {
        document = Value::Array(output);
    }
    serde_json::to_vec_pretty(&document).map_err(|_| "clients.invalid_json".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_array_and_object_user_content() {
        let entry = json!({"id":"model-a","vendor":"LoongPort","apiKey":"synthetic-key"});
        for original in [
            json!([{"id":"personal","extra":42}]),
            json!({"models":[{"id":"personal","extra":42}],"availableModels":["personal"],"unknown":{"keep":true}}),
        ] {
            let bytes = serde_json::to_vec(&original).unwrap();
            let result: Value =
                serde_json::from_slice(&project(Some(&bytes), &[entry.clone()]).unwrap()).unwrap();
            let models = if result.is_array() {
                &result
            } else {
                &result["models"]
            };
            assert_eq!(models[0], json!({"id":"personal","extra":42}));
            assert_eq!(models[1], entry);
            if result.is_object() {
                assert_eq!(result["unknown"], original["unknown"]);
                assert_eq!(result["availableModels"], json!(["personal", "model-a"]));
            }
            let again = project(
                Some(&serde_json::to_vec(&result).unwrap()),
                &[entry.clone()],
            )
            .unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&again).unwrap(), result);
        }
    }

    #[test]
    fn refuses_broken_json_and_incompatible_shapes() {
        for bytes in [
            b"{broken".as_slice(),
            b"null",
            b"{\"models\":false}",
            b"{\"availableModels\":42}",
        ] {
            assert!(project(Some(bytes), &[]).is_err());
        }
    }
}

pub struct WorkBuddy;
impl super::registry::Adapter for WorkBuddy {
    fn project(&self, current: Option<&[u8]>, entries: &[Value]) -> Result<Vec<u8>, String> {
        project(current, entries)
    }
    fn restore(
        &self,
        current: &[u8],
        original: Option<&[u8]>,
        installed: &[Value],
    ) -> Result<Vec<u8>, String> {
        restore(current, original, installed)
    }
    fn entries(&self, current: Option<&[u8]>) -> Result<Vec<Value>, String> {
        Ok(models(&parse(current)?)?.clone())
    }
    fn projection(&self, endpoint: &str, secret: &str, models: &[String]) -> Vec<Value> {
        models.iter().map(|model| serde_json::json!({"id":model,"name":model,"vendor":"LoongPort","url":endpoint,"apiKey":secret})).collect()
    }
}
