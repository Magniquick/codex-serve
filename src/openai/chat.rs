use crate::error::ApiError;
use codex_core::{ContentItem, JsonSchema, Prompt, ResponsesApiTool, ResponseItem, ToolSpec};
use codex_protocol::models::FunctionCallOutputPayload;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use tracing::{info, warn};

use super::sanitize_json_schema;

#[derive(Debug, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub tools: Vec<RequestTool>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct ChatMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct ChatToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub function: Option<ChatToolFunction>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct ChatToolFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct RequestTool {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub function: Option<RequestToolFunction>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct RequestToolFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub strict: Option<bool>,
    #[serde(default)]
    pub parameters: Option<Value>,
}

#[derive(Debug)]
pub struct PromptPayload {
    pub model: String,
    pub prompt: Prompt,
    pub first_user_message: Option<String>,
}

impl ChatCompletionRequest {
    pub fn into_prompt(self) -> Result<PromptPayload, ApiError> {
        if self.messages.is_empty() {
            return Err(ApiError::bad_request("Request must include messages: []"));
        }

        let model = normalize_model(self.model);
        let mut prompt = Prompt::default();
        let mut first_user = None;
        for message in self.messages {
            let role = normalize_role(&message.role);

            if role == "tool" {
                if let Some(output_item) = convert_tool_output(&message) {
                    prompt.input.push(output_item);
                }
                continue;
            }

            if role == "assistant" {
                let tool_call_items = convert_assistant_tool_calls(message.tool_calls.as_ref());
                prompt.input.extend(tool_call_items);
            }

            let content = convert_content(&role, message.content)?;
            if first_user.is_none() && role == "user" {
                first_user = first_text(&content);
            }

            if content.is_empty() {
                continue;
            }

            prompt.input.push(ResponseItem::Message {
                id: None,
                role,
                content,
            });
        }

        if let Some(specs) = convert_function_tools(&self.tools)? {
            log_function_tools(&specs);
            prompt.tools.extend(specs);
        }

        if let Some(enabled) = self.parallel_tool_calls {
            prompt.parallel_tool_calls = enabled;
        }

        Ok(PromptPayload {
            model,
            prompt,
            first_user_message: first_user,
        })
    }
}

fn normalize_model(model: String) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        "gpt-5".to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_role(role: &str) -> String {
    let trimmed = role.trim();
    if trimmed.is_empty() {
        return "user".to_string();
    }
    match trimmed.to_ascii_lowercase().as_str() {
        // The Codex backend rejects role=system. Translate it into the
        // developer stream as described in reference/blog.
        "system" => "developer".to_string(),
        other => other.to_string(),
    }
}

fn convert_content(role: &str, value: Value) -> Result<Vec<ContentItem>, ApiError> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![content_item_for_role(role, text)]),
        Value::Array(items) => {
            let mut content_items = Vec::with_capacity(items.len());
            for item in items {
                content_items.push(convert_content_item(role, item)?);
            }
            Ok(content_items)
        }
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return Ok(vec![content_item_for_role(role, text.to_string())]);
            }
            if let Some(value) = map.get("type").and_then(Value::as_str) {
                match value {
                    "text" | "input_text" => {
                        let text = map
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| ApiError::bad_request("text block missing `text`"))?;
                        Ok(vec![content_item_for_role(role, text.to_string())])
                    }
                    "image_url" | "input_image" => {
                        let url = extract_image_url(&map)?;
                        Ok(vec![ContentItem::InputImage { image_url: url }])
                    }
                    other => Err(ApiError::bad_request(format!(
                        "Unsupported content type `{other}`"
                    ))),
                }
            } else {
                Err(ApiError::bad_request(
                    "Message content object must include `type`",
                ))
            }
        }
        _ => Err(ApiError::bad_request(
            "Message content must be text or a structured content array",
        )),
    }
}

fn convert_content_item(role: &str, value: Value) -> Result<ContentItem, ApiError> {
    match value {
        Value::String(text) => Ok(content_item_for_role(role, text)),
        Value::Object(map) => {
            let ctype = map
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::bad_request("Content item missing `type`"))?;
            match ctype {
                "text" | "input_text" => {
                    let text = map
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ApiError::bad_request("text block missing `text`"))?;
                    Ok(content_item_for_role(role, text.to_string()))
                }
                "image_url" | "input_image" => {
                    let url = extract_image_url(&map)?;
                    Ok(ContentItem::InputImage { image_url: url })
                }
                other => Err(ApiError::bad_request(format!(
                    "Unsupported content type `{other}`"
                ))),
            }
        }
        _ => Err(ApiError::bad_request(
            "Content items must be strings or structured objects",
        )),
    }
}

fn content_item_for_role(role: &str, text: impl Into<String>) -> ContentItem {
    let text = text.into();
    if role == "assistant" {
        ContentItem::OutputText { text }
    } else {
        ContentItem::InputText { text }
    }
}

fn extract_image_url(map: &Map<String, Value>) -> Result<String, ApiError> {
    if let Some(url) = map.get("image_url").and_then(Value::as_str) {
        return Ok(url.to_string());
    }
    if let Some(url_obj) = map.get("image_url").and_then(Value::as_object)
        && let Some(url) = url_obj.get("url").and_then(Value::as_str)
    {
        return Ok(url.to_string());
    }
    Err(ApiError::bad_request("image content requires `image_url`"))
}

fn first_text(content: &[ContentItem]) -> Option<String> {
    content.iter().find_map(|item| match item {
        ContentItem::InputText { text } => Some(text.clone()),
        _ => None,
    })
}

fn convert_assistant_tool_calls(calls: Option<&Vec<ChatToolCall>>) -> Vec<ResponseItem> {
    let mut items = Vec::new();
    if let Some(list) = calls {
        for tc in list {
            let call_type = tc.r#type.as_deref().unwrap_or("function");
            if !call_type.eq_ignore_ascii_case("function") {
                continue;
            }
            let Some(function) = tc.function.as_ref() else {
                continue;
            };
            let Some(name) = function
                .name
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let arguments = function
                .arguments
                .clone()
                .unwrap_or_else(|| "{}".to_string());
            let call_id = tc
                .id
                .clone()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_else(|| format!("call_{}", items.len()));
            items.push(ResponseItem::FunctionCall {
                id: None,
                name,
                arguments,
                call_id,
            });
        }
    }
    items
}

fn convert_tool_output(message: &ChatMessage) -> Option<ResponseItem> {
    let call_id = message.tool_call_id.as_deref()?;
    let content = match &message.content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    Some(ResponseItem::FunctionCallOutput {
        call_id: call_id.to_string(),
        output: FunctionCallOutputPayload {
            content,
            success: Some(true),
            content_items: None,
        },
    })
}

fn convert_function_tools(tools: &[RequestTool]) -> Result<Option<Vec<ToolSpec>>, ApiError> {
    let mut specs = Vec::new();
    for tool in tools {
        if !tool.kind.eq_ignore_ascii_case("function") {
            continue;
        }
        let Some(function) = tool.function.as_ref() else {
            continue;
        };
        let Some(name) = function
            .name
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let description = function.description.as_ref().and_then(|d| {
            let trimmed = d.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
        let mut parameters_value = normalize_tool_schema(function.parameters.clone());
        sanitize_json_schema(&mut parameters_value);
        let parameters: JsonSchema = match serde_json::from_value(parameters_value.clone()) {
            Ok(schema) => schema,
            Err(source) => {
                warn!(
                    tool = %name,
                    error = %source,
                    schema = %parameters_value,
                    "invalid tool schema; falling back to empty object"
                );
                JsonSchema::Object {
                    properties: BTreeMap::new(),
                    required: None,
                    additional_properties: None,
                }
            }
        };
        specs.push(ToolSpec::Function(ResponsesApiTool {
            name,
            description: description.unwrap_or_default(),
            strict: function.strict.unwrap_or(false),
            parameters,
        }));
    }

    if specs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(specs))
    }
}

fn normalize_tool_schema(parameters: Option<Value>) -> Value {
    match parameters {
        Some(Value::Object(mut map)) => {
            map.entry("type".to_string())
                .or_insert_with(|| Value::String("object".to_string()));
            map.entry("properties".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            Value::Object(map)
        }
        _ => json!({
            "type": "object",
            "properties": {}
        }),
    }
}

fn log_function_tools(specs: &[ToolSpec]) {
    if !tools_verbose_logging_enabled() || specs.is_empty() {
        return;
    }
    let payload: Vec<Value> = specs
        .iter()
        .map(|spec| match spec {
            ToolSpec::Function(tool) => json!({
                "name": tool.name,
                "description": tool.description,
                "strict": tool.strict,
                "parameters": tool.parameters,
            }),
            ToolSpec::Freeform(tool) => json!({
                "name": tool.name,
                "description": tool.description,
                "strict": false,
                "parameters": null,
            }),
            ToolSpec::LocalShell {} => json!({
                "name": "local_shell",
                "description": null,
                "strict": false,
                "parameters": null,
            }),
            ToolSpec::WebSearch {} => json!({
                "name": "web_search",
                "description": null,
                "strict": false,
                "parameters": null,
            }),
        })
        .collect();
    match serde_json::to_string(&payload) {
        Ok(serialized) => {
            info!(event = "chat.tools", payload = %serialized, "registered function tools")
        }
        Err(err) => info!("chat.tools serialization failed: {err}"),
    }
}

fn tools_verbose_logging_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        matches!(
            std::env::var("CODEX_SERVE_VERBOSE")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_message(value: Value) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: value,
                ..Default::default()
            }],
            stream: false,
            tools: Vec::new(),
            parallel_tool_calls: None,
        }
    }

    #[test]
    fn converts_string_message() {
        let payload = user_message(Value::String("hello".into()))
            .into_prompt()
            .expect("conversion should succeed");
        assert_eq!(payload.model, "gpt-5");
        assert_eq!(payload.first_user_message.as_deref(), Some("hello"));
        assert_eq!(payload.prompt.input.len(), 1);
        match &payload.prompt.input[0] {
            ResponseItem::Message { role, content, .. } => {
                assert_eq!(role, "user");
                assert_eq!(
                    content,
                    &vec![ContentItem::InputText {
                        text: "hello".into()
                    }]
                );
            }
            other => panic!("unexpected response item: {other:?}"),
        }
    }

    #[test]
    fn converts_structured_array_message() {
        let value = serde_json::json!([
            {"type": "text", "text": "hi"},
            {"type": "image_url", "image_url": "https://example.com/image.png"}
        ]);
        let payload = user_message(value)
            .into_prompt()
            .expect("conversion should succeed");
        assert_eq!(payload.prompt.input.len(), 1);
        match &payload.prompt.input[0] {
            ResponseItem::Message { content, .. } => {
                assert_eq!(content.len(), 2);
            }
            _ => panic!("expected message item"),
        }
    }

    #[test]
    fn rejects_invalid_content() {
        let result = user_message(Value::Number(42.into())).into_prompt();
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn system_messages_become_developer() {
        let payload = ChatCompletionRequest {
            model: "".to_string(),
            messages: vec![ChatMessage {
                role: "system".to_string(),
                content: Value::String("act like a pelican".into()),
                ..Default::default()
            }],
            stream: false,
            tools: Vec::new(),
            parallel_tool_calls: None,
        };
        let prompt = payload.into_prompt().expect("conversion should succeed");
        match &prompt.prompt.input[0] {
            ResponseItem::Message { role, .. } => assert_eq!(role, "developer"),
            other => panic!("expected developer message, got {other:?}"),
        }
    }

    #[test]
    fn assistant_messages_use_output_text() {
        let payload = ChatCompletionRequest {
            model: "".to_string(),
            messages: vec![ChatMessage {
                role: "assistant".to_string(),
                content: Value::String("done".into()),
                ..Default::default()
            }],
            stream: false,
            tools: Vec::new(),
            parallel_tool_calls: None,
        };
        let prompt = payload.into_prompt().expect("conversion should succeed");
        match &prompt.prompt.input[0] {
            ResponseItem::Message { content, .. } => match &content[0] {
                ContentItem::OutputText { text } => assert_eq!(text, "done"),
                other => panic!("expected output text, got {other:?}"),
            },
            other => panic!("expected assistant message, got {other:?}"),
        }
    }

    #[test]
    fn convert_function_tools_handles_anyof_schemas() {
        let tools = vec![RequestTool {
            kind: "function".to_string(),
            function: Some(RequestToolFunction {
                name: Some("edit_notebook_file".to_string()),
                description: None,
                strict: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "newCode": {
                            "anyOf": [
                                {"type": "string"},
                                {"type": "array", "items": {"type": "string"}}
                            ]
                        }
                    }
                })),
            }),
        }];
        let specs = convert_function_tools(&tools)
            .expect("conversion should succeed")
            .expect("tool definitions should exist");
        assert_eq!(specs.len(), 1);
        match &specs[0] {
            ToolSpec::Function(tool) => {
                let schema_value = serde_json::to_value(&tool.parameters).expect("json schema");
                assert_eq!(
                    schema_value["properties"]["newCode"]["type"],
                    Value::String("string".into())
                );
            }
            other => panic!("expected function tool, got {other:?}"),
        }
    }
}
