use serde_json::{json, Map, Value};

/// Sanitize a JSON Schema so it fits within the subset that codex-core accepts.
/// - Recursively ensures every nested schema object has a `type`.
/// - Infers sensible defaults for `object`/`array` schemas when structural hints exist.
/// - Normalizes boolean schemas to permissive string schemas.
pub(crate) fn sanitize_json_schema(value: &mut Value) {
    match value {
        Value::Bool(_) => {
            *value = json!({ "type": "string" });
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                sanitize_json_schema(item);
            }
        }
        Value::Object(map) => sanitize_object_schema(map),
        _ => {}
    }
}

fn sanitize_object_schema(map: &mut Map<String, Value>) {
    if let Some(Value::Object(props)) = map.get_mut("properties") {
        for value in props.values_mut() {
            sanitize_json_schema(value);
        }
    }
    if let Some(items) = map.get_mut("items") {
        sanitize_json_schema(items);
    }
    for key in ["oneOf", "anyOf", "allOf", "prefixItems"] {
        if let Some(value) = map.get_mut(key) {
            sanitize_json_schema(value);
        }
    }

    let mut schema_type = map
        .get("type")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    if schema_type.is_none() {
        if let Some(Value::Array(types)) = map.get("type") {
            for t in types {
                if let Some(candidate) = t.as_str() {
                    if matches!(
                        candidate,
                        "object" | "array" | "string" | "number" | "integer" | "boolean"
                    ) {
                        schema_type = Some(candidate.to_string());
                        break;
                    }
                }
            }
        }
    }

    if schema_type.is_none() {
        if map.contains_key("properties")
            || map.contains_key("required")
            || map.contains_key("additionalProperties")
        {
            schema_type = Some("object".to_string());
        } else if map.contains_key("items") || map.contains_key("prefixItems") {
            schema_type = Some("array".to_string());
        } else if map.contains_key("enum")
            || map.contains_key("const")
            || map.contains_key("format")
        {
            schema_type = Some("string".to_string());
        } else if map.contains_key("minimum")
            || map.contains_key("maximum")
            || map.contains_key("exclusiveMinimum")
            || map.contains_key("exclusiveMaximum")
            || map.contains_key("multipleOf")
        {
            schema_type = Some("number".to_string());
        }
    }

    let schema_type = schema_type.unwrap_or_else(|| "string".to_string());
    map.insert("type".to_string(), Value::String(schema_type.clone()));

    if schema_type == "object" {
        if !map.contains_key("properties") {
            map.insert("properties".to_string(), Value::Object(Map::new()));
        }
        if let Some(additional) = map.get_mut("additionalProperties") {
            if !additional.is_boolean() {
                sanitize_json_schema(additional);
            }
        }
    }

    if schema_type == "array" && !map.contains_key("items") {
        map.insert("items".to_string(), json!({ "type": "string" }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_missing_top_level_type() {
        let mut value = json!({ "properties": { "x": { "minimum": 0 } } });
        sanitize_json_schema(&mut value);
        assert_eq!(value["type"], Value::String("object".into()));
        assert_eq!(value["properties"]["x"]["type"], Value::String("number".into()));
    }

    #[test]
    fn coalesces_anyof_branch() {
        let mut value = json!({
            "properties": {
                "newCode": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "array", "items": { "type": "string" } }
                    ]
                }
            }
        });
        sanitize_json_schema(&mut value);
        assert_eq!(
            value["properties"]["newCode"]["type"],
            Value::String("string".into())
        );
    }
}
