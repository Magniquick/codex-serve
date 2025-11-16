use std::time::{SystemTime, UNIX_EPOCH};

use codex_core::protocol::TokenUsage;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    id: String,
    object: &'static str,
    created: i64,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct Choice {
    index: usize,
    message: AssistantMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct AssistantMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: &'static str,
    pub function: ToolCallFunction,
}

#[derive(Debug, Serialize, Clone)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// Token accounting compatible with OpenAI responses.
#[derive(Debug, Serialize, Default, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl From<TokenUsage> for Usage {
    fn from(value: TokenUsage) -> Self {
        fn clamp(v: i64) -> u32 {
            if v <= 0 { 0 } else { v as u32 }
        }

        Self {
            prompt_tokens: clamp(value.input_tokens + value.cached_input_tokens),
            completion_tokens: clamp(value.output_tokens + value.reasoning_output_tokens),
            total_tokens: clamp(value.total_tokens),
        }
    }
}

impl ChatCompletionResponse {
    pub fn stub(model: String, content: String) -> Self {
        Self::with_metadata(
            model,
            Some(content),
            Vec::new(),
            "stop",
            "resp_stub".to_string(),
            Usage::default(),
        )
    }

    pub fn with_metadata(
        model: String,
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
        finish_reason: &'static str,
        response_id: String,
        usage: Usage,
    ) -> Self {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();

        Self {
            id: response_id,
            object: "chat.completion",
            created,
            model,
            choices: vec![Choice {
                index: 0,
                finish_reason: finish_reason.to_string(),
                message: AssistantMessage {
                    role: "assistant",
                    content,
                    tool_calls,
                },
            }],
            usage,
        }
    }
}

impl ToolCall {
    pub fn new(id: String, name: String, arguments: String) -> Self {
        Self {
            id,
            call_type: "function",
            function: ToolCallFunction { name, arguments },
        }
    }
}
