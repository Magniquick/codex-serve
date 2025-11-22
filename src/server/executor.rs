use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use codex_app_server_protocol::AuthMode;
use codex_core::{
    ModelClient, Prompt, ResponseEvent, ResponseItem, ResponseStream,
    auth::{AuthManager, CodexAuth},
    compact::content_items_to_text,
    config::{Config, ConfigOverrides},
    protocol::SessionSource,
};
use codex_otel::otel_event_manager::OtelEventManager;
use codex_protocol::ConversationId;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use toml::Value as TomlValue;
use tracing::{error, warn};

use crate::{
    error::ApiError,
    openai::chat::PromptPayload,
    prompt::{ensure_web_search_tool, inject_developer_prompt},
    serve_config::developer_prompt_mode,
    server::response::{AssistantReasoning, ChatCompletionResponse, ToolCall, Usage},
};

pub type SharedChatExecutor = Arc<dyn ChatExecutor + Send + Sync>;

/// Streaming response returned by the real executor.
pub struct StreamingHandle {
    pub response_model: String,
    pub stream: ResponseStream,
}

/// Executes Codex prompts either to completion or as an SSE stream.
#[async_trait]
pub trait ChatExecutor {
    async fn complete(&self, payload: PromptPayload) -> Result<ChatCompletionResponse, ApiError>;

    async fn stream(&self, payload: PromptPayload) -> Result<StreamingHandle, ApiError>;
}

/// In-memory executor used by the test harness.
pub struct MockChatExecutor;

impl MockChatExecutor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ChatExecutor for MockChatExecutor {
    async fn complete(&self, payload: PromptPayload) -> Result<ChatCompletionResponse, ApiError> {
        let reply = payload
            .first_user_message
            .as_deref()
            .map(|text| format!("Hi there! You said: {}", text.trim()))
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "Hi there! How can I help you today?".to_string());
        Ok(ChatCompletionResponse::stub(payload.model, reply))
    }

    async fn stream(&self, _payload: PromptPayload) -> Result<StreamingHandle, ApiError> {
        Err(ApiError::bad_request(
            "Streaming is not available in test mode",
        ))
    }
}

/// Production executor backed by `codex-core::ModelClient`.
pub struct RealChatExecutor {
    config: Arc<Config>,
    auth_manager: Arc<AuthManager>,
    config_cache: RwLock<HashMap<String, Arc<Config>>>,
    cli_overrides: Vec<(String, TomlValue)>,
}

impl RealChatExecutor {
    pub fn new(
        config: Arc<Config>,
        auth_manager: Arc<AuthManager>,
        cli_overrides: Vec<(String, TomlValue)>,
    ) -> Self {
        Self {
            config,
            auth_manager,
            config_cache: RwLock::new(HashMap::new()),
            cli_overrides,
        }
    }

    async fn config_for_model(&self, requested: &str) -> Result<Arc<Config>, ApiError> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err(ApiError::bad_request("model must be provided"));
        }

        if requested == self.config.model {
            return Ok(Arc::clone(&self.config));
        }

        if let Some(existing) = self.config_cache.read().await.get(requested) {
            return Ok(Arc::clone(existing));
        }

        let overrides = ConfigOverrides {
            model: Some(requested.to_string()),
            ..ConfigOverrides::default()
        };

        let config = Config::load_with_cli_overrides(self.cli_overrides.clone(), overrides)
            .await
            .map_err(|_| {
                ApiError::bad_request(format!(
                    "model `{requested}` is not configured for Codex Serve. \
                     Use `codex config set model {requested}` to enable it."
                ))
            })?;
        let config = Arc::new(config);

        let mut cache = self.config_cache.write().await;
        cache.insert(requested.to_string(), Arc::clone(&config));
        Ok(config)
    }

    fn auth_snapshot(&self) -> Option<CodexAuth> {
        self.auth_manager.auth()
    }
}

#[async_trait]
impl ChatExecutor for RealChatExecutor {
    async fn complete(&self, payload: PromptPayload) -> Result<ChatCompletionResponse, ApiError> {
        let handle = self.stream(payload).await?;
        aggregate_response_stream(handle).await
    }

    async fn stream(&self, payload: PromptPayload) -> Result<StreamingHandle, ApiError> {
        let config = self.config_for_model(&payload.model).await?;

        let PromptPayload {
            model,
            mut prompt,
            system_prompt,
            ..
        } = payload;

        let has_web_search = ensure_web_search_tool(&mut prompt, config.tools_web_search_request);
        let prompt_mode = developer_prompt_mode();
        inject_developer_prompt(
            &mut prompt,
            has_web_search,
            system_prompt.as_deref(),
            prompt_mode,
        );

        let conversation_id = ConversationId::default();
        let auth_snapshot = self.auth_snapshot();
        let (account_id, auth_mode): (Option<String>, Option<AuthMode>) = match auth_snapshot {
            Some(auth) => (auth.get_account_id(), Some(auth.mode)),
            None => (None, None),
        };

        let otel = OtelEventManager::new(
            conversation_id,
            config.model.as_str(),
            config.model_family.slug.as_str(),
            account_id,
            None,
            auth_mode,
            false,
            "codex-serve".to_string(),
        );

        let client = ModelClient::new(
            Arc::clone(&config),
            Some(Arc::clone(&self.auth_manager)),
            otel,
            config.model_provider.clone(),
            config.model_reasoning_effort,
            config.model_reasoning_summary,
            conversation_id,
            SessionSource::Exec,
        );

        let stream = client.stream(&prompt).await.map_err(|err| {
            error!(
                model = config.model.as_str(),
                prompt = %prompt_debug_snapshot(&prompt),
                "Codex upstream error: {err}"
            );
            ApiError::internal(format!("Codex request failed: {err}"))
        })?;

        Ok(StreamingHandle {
            response_model: model,
            stream,
        })
    }
}

async fn aggregate_response_stream(
    mut handle: StreamingHandle,
) -> Result<ChatCompletionResponse, ApiError> {
    let mut streamed_text = String::new();
    let mut final_text: Option<String> = None;
    let mut response_id: Option<String> = None;
    let mut usage = Usage::default();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut tool_call_indices: HashMap<String, usize> = HashMap::new();
    let mut reasoning_summary_parts: BTreeMap<i64, String> = BTreeMap::new();

    while let Some(event) = handle.stream.next().await {
        let event =
            event.map_err(|err| ApiError::internal(format!("Codex stream error: {err}")))?;
        match event {
            ResponseEvent::OutputTextDelta(delta) => streamed_text.push_str(&delta),
            ResponseEvent::OutputItemAdded(item) | ResponseEvent::OutputItemDone(item) => {
                if matches!(item, ResponseItem::Reasoning { .. }) {
                    continue;
                }
                if let Some(text) = assistant_text_from_item(item.clone()) {
                    final_text = Some(text);
                }
                if let Some(call) = super::tool_call_from_item(&item) {
                    if let Some(idx) = tool_call_indices.get(&call.id) {
                        if let Some(existing) = tool_calls.get_mut(*idx) {
                            *existing = call;
                        }
                    } else {
                        let idx = tool_calls.len();
                        tool_call_indices.insert(call.id.clone(), idx);
                        tool_calls.push(call);
                    }
                }
            }
            ResponseEvent::ReasoningSummaryDelta {
                delta,
                summary_index,
            } => {
                reasoning_summary_parts
                    .entry(summary_index)
                    .or_default()
                    .push_str(&delta);
            }
            ResponseEvent::ReasoningSummaryPartAdded { summary_index } => {
                reasoning_summary_parts
                    .entry(summary_index)
                    .or_default();
            }
            ResponseEvent::Completed {
                response_id: rid,
                token_usage,
            } => {
                response_id = Some(rid);
                if let Some(tokens) = token_usage {
                    usage = Usage::from(tokens);
                }
                break;
            }
            ResponseEvent::RateLimits(_) | ResponseEvent::Created => {}
            other => {
                warn!("Unhandled Codex response event in aggregation: {other:?}");
            }
        }
    }

    let response_id = response_id.unwrap_or_else(|| "resp_local".to_string());
    let mut content = final_text.or_else(|| {
        if streamed_text.trim().is_empty() {
            None
        } else {
            Some(streamed_text)
        }
    });
    // ensure we don't return empty string content
    if content.as_ref().is_some_and(|text| text.trim().is_empty()) {
        content = None;
    }

    let finish_reason = if !tool_calls.is_empty() {
        "tool_calls"
    } else {
        "stop"
    };
    let reasoning_summary = reasoning_summary_parts.into_values()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();
    let reasoning = AssistantReasoning::from_summary_parts(reasoning_summary);

    Ok(ChatCompletionResponse::with_metadata(
        handle.response_model,
        content,
        tool_calls,
        finish_reason,
        response_id,
        usage,
        reasoning,
    ))
}

fn assistant_text_from_item(item: ResponseItem) -> Option<String> {
    match item {
        ResponseItem::Message { role, content, .. } if role == "assistant" => {
            content_items_to_text(&content)
        }
        _ => None,
    }
}

fn prompt_debug_snapshot(prompt: &Prompt) -> Value {
    let input = serde_json::to_value(&prompt.input)
        .unwrap_or_else(|_| json!("<failed to serialize prompt input>"));
    json!({
        "input": input,
        "base_instructions_override": prompt.base_instructions_override,
    })
}
