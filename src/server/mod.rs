mod executor;
pub mod response;
mod state;
mod test_server;

use std::{collections::HashMap, convert::Infallible, sync::OnceLock};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures_util::StreamExt as FuturesStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};
use uuid::Uuid;

use codex_common::model_presets::builtin_model_presets;
use codex_core::{ResponseEvent, ResponseItem, compact::content_items_to_text};
use codex_protocol::models::WebSearchAction;

use crate::{error::ApiError, openai::chat::ChatCompletionRequest};
use executor::{SharedChatExecutor, StreamingHandle};
use response::{ToolCall, Usage};
use state::AppState;

pub use test_server::TestServer;

type SseStream = ReceiverStream<Result<Event, Infallible>>;

/// Build the Axum router that powers Codex Serve.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/version", get(api_version))
        .route("/api/tags", get(api_tags))
        .route("/api/show", post(api_show))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(axum::middleware::from_fn(log_requests))
        .with_state(state)
}

/// Run the HTTP server on the provided TCP listener until shutdown.
pub async fn serve(listener: TcpListener) -> Result<()> {
    let state = AppState::initialize()
        .await
        .context("failed to initialize Codex Serve state")?;
    serve_with_state(listener, state)
        .await
        .context("axum server error")
}

pub async fn serve_with_state(listener: TcpListener, state: AppState) -> Result<()> {
    axum::serve(listener, router(state))
        .await
        .context("axum server error")?;
    Ok(())
}

async fn chat_completions(
    State(state): State<AppState>,
    Json(payload): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    state.ensure_authenticated()?;
    log_verbose_json("chat.request", &payload);

    let stream_requested = payload.stream;
    let prompt_payload = payload.into_prompt()?;

    if stream_requested {
        if verbose_logging_enabled() {
            info!(
                model = %prompt_payload.model,
                "forwarding streaming chat request to Codex"
            );
        }
        let stream = stream_chat_response(state.engine(), prompt_payload).await?;
        return Ok(stream.into_response());
    }

    let response = state.engine().complete(prompt_payload).await?;
    log_verbose_json("chat.response", &response);
    Ok(Json(response).into_response())
}

#[derive(Debug, serde::Serialize)]
struct HealthzResponse {
    ok: bool,
    authenticated: bool,
    message: String,
}

async fn healthz(State(state): State<AppState>) -> Json<HealthzResponse> {
    let authenticated = state.auth().is_authenticated();
    let message = if authenticated {
        "Codex auth detected".to_string()
    } else {
        "Codex auth missing; run `codex login`".to_string()
    };
    Json(HealthzResponse {
        ok: true,
        authenticated,
        message,
    })
}

#[derive(Debug, serde::Serialize)]
struct ModelsResponse {
    object: &'static str,
    data: Vec<ModelEntry>,
}

#[derive(Debug, serde::Serialize)]
struct ModelEntry {
    id: String,
    object: &'static str,
}

async fn list_models() -> Json<ModelsResponse> {
    let data = codex_model_ids()
        .into_iter()
        .map(|id| ModelEntry {
            id,
            object: "model",
        })
        .collect();
    Json(ModelsResponse {
        object: "list",
        data,
    })
}

#[derive(Debug, serde::Serialize)]
struct VersionResponse {
    version: &'static str,
}

const CHATMOCK_VERSION: &str = "0.12.10";

async fn api_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: CHATMOCK_VERSION,
    })
}

#[derive(Debug, serde::Serialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelEntry>,
}

#[derive(Debug, serde::Serialize)]
struct OllamaModelEntry {
    name: String,
    model: String,
    modified_at: &'static str,
    size: u64,
    digest: &'static str,
    details: OllamaModelDetails,
}

#[derive(Debug, serde::Serialize, Clone, Copy)]
struct OllamaModelDetails {
    parent_model: &'static str,
    format: &'static str,
    family: &'static str,
    families: &'static [&'static str],
    parameter_size: &'static str,
    quantization_level: &'static str,
}

#[derive(Clone, Copy)]
struct OllamaModelMetadata {
    modified_at: &'static str,
    size: u64,
    digest: &'static str,
    details: OllamaModelDetails,
}

const OLLAMA_MODEL_METADATA: OllamaModelMetadata = OllamaModelMetadata {
    modified_at: "2023-10-01T00:00:00Z",
    size: 815_319_791,
    digest: "8648f39daa8fbf5b18c7b4e6a8fb4990c692751d49917417b8842ca5758e7ffc",
    details: OllamaModelDetails {
        parent_model: "",
        format: "gguf",
        family: "llama",
        families: &["llama"],
        parameter_size: "8.0B",
        quantization_level: "Q4_0",
    },
};

const OLLAMA_VARIANT_MODEL_IDS: &[&str] = &[
    "gpt-5-high",
    "gpt-5-medium",
    "gpt-5-low",
    "gpt-5-minimal",
    "gpt-5-codex-high",
    "gpt-5-codex-medium",
    "gpt-5-codex-low",
];

#[derive(Debug, Deserialize)]
struct OllamaShowRequest {
    model: Option<String>,
}

async fn api_tags() -> Json<OllamaTagsResponse> {
    let mut models = codex_model_ids();
    if expose_reasoning_variants() {
        models.extend(
            OLLAMA_VARIANT_MODEL_IDS
                .iter()
                .map(|model| (*model).to_string()),
        );
    }
    let entries = models
        .iter()
        .map(|model_id| build_ollama_entry(model_id))
        .collect();
    Json(OllamaTagsResponse { models: entries })
}

fn build_ollama_entry(model_id: &str) -> OllamaModelEntry {
    OllamaModelEntry {
        name: model_id.to_string(),
        model: model_id.to_string(),
        modified_at: OLLAMA_MODEL_METADATA.modified_at,
        size: OLLAMA_MODEL_METADATA.size,
        digest: OLLAMA_MODEL_METADATA.digest,
        details: OLLAMA_MODEL_METADATA.details,
    }
}

fn codex_model_ids() -> Vec<String> {
    builtin_model_presets(None)
        .into_iter()
        .map(|preset| preset.model.to_string())
        .collect()
}

fn expose_reasoning_variants() -> bool {
    matches!(
        std::env::var("CODEX_SERVE_EXPOSE_REASONING_MODELS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

fn log_verbose_json<T>(event: &str, value: &T)
where
    T: ?Sized + Serialize,
{
    if !verbose_logging_enabled() {
        return;
    }
    match serde_json::to_string(value) {
        Ok(serialized) => info!(event = event, payload = %serialized, "verbose emit"),
        Err(err) => warn!(event = event, "failed to serialize verbose payload: {err}"),
    }
}

fn log_verbose_stream_response(
    model: &str,
    response_id: &str,
    text: Option<String>,
    reasoning_summary: Option<String>,
    tool_calls: Vec<ToolCall>,
    usage: &Usage,
) {
    let payload = json!({
        "model": model,
        "response_id": response_id,
        "text": text,
        "reasoning_summary": reasoning_summary,
        "tool_calls": if tool_calls.is_empty() { Value::Null } else { serde_json::to_value(tool_calls).unwrap_or(Value::Null) },
        "usage": usage,
    });
    log_verbose_json("chat.stream.response", &payload);
}

fn verbose_logging_enabled() -> bool {
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

pub(super) fn tool_call_from_item(item: &ResponseItem) -> Option<ToolCall> {
    match item {
        ResponseItem::FunctionCall {
            call_id,
            name,
            arguments,
            ..
        } => Some(ToolCall::new(
            call_id.clone(),
            name.clone(),
            arguments.clone(),
        )),
        ResponseItem::CustomToolCall {
            call_id,
            name,
            input,
            ..
        } => Some(ToolCall::new(call_id.clone(), name.clone(), input.clone())),
        ResponseItem::WebSearchCall { id, action, .. } => {
            let call_id = id
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("ws_call_{}", Uuid::new_v4()));
            let arguments = web_search_arguments(action);
            Some(ToolCall::new(call_id, "web_search".to_string(), arguments))
        }
        _ => None,
    }
}

fn web_search_arguments(action: &WebSearchAction) -> String {
    match action {
        WebSearchAction::Search { query } => json!({ "query": query }).to_string(),
        WebSearchAction::Other => json!({}).to_string(),
    }
}

fn tool_call_delta_chunk(
    response_id: &str,
    created: i64,
    model: &str,
    call: &ToolCall,
    index: usize,
) -> Event {
    let payload = json!({
        "id": response_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": index,
                    "id": call.id,
                    "type": call.call_type,
                    "function": {
                        "name": call.function.name,
                        "arguments": call.function.arguments,
                    }
                }]
            },
            "finish_reason": Value::Null,
        }],
    });
    Event::default()
        .json_data(payload)
        .expect("serialize tool call chunk")
}

const OLLAMA_SHOW_MODELFILE: &str = r#"# Modelfile generated by "ollama show"
# To build a new Modelfile based on this one, replace the FROM line with:
# FROM llava:latest

FROM /models/blobs/sha256:placeholder
TEMPLATE """{{ .System }}
USER: {{ .Prompt }}
ASSISTANT: """
PARAMETER num_ctx 100000
PARAMETER stop "</s>"
PARAMETER stop "USER:"
PARAMETER stop "ASSISTANT:""#;

const OLLAMA_SHOW_PARAMETERS: &str = r#"num_keep 24
stop "<|start_header_id|>"
stop "<|end_header_id|>"
stop "<|eot_id|>""#;

const OLLAMA_SHOW_TEMPLATE: &str = r#"{{ if .System }}<|start_header_id|>system<|end_header_id|>

{{ .System }}<|eot_id|>{{ end }}{{ if .Prompt }}<|start_header_id|>user<|end_header_id|>

{{ .Prompt }}<|eot_id|>{{ end }}<|start_header_id|>assistant<|end_header_id|>

{{ .Response }}<|eot_id|>"#;

async fn api_show(Json(payload): Json<OllamaShowRequest>) -> Response {
    let model_valid = payload
        .model
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !model_valid {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Model not found" })),
        )
            .into_response();
    }

    Json(build_ollama_show_payload()).into_response()
}

fn build_ollama_show_payload() -> Value {
    let details = serde_json::to_value(&OLLAMA_MODEL_METADATA.details)
        .expect("static model details should serialize");
    json!({
        "modelfile": OLLAMA_SHOW_MODELFILE,
        "parameters": OLLAMA_SHOW_PARAMETERS,
        "template": OLLAMA_SHOW_TEMPLATE,
        "details": details,
        "model_info": {
            "general.architecture": "llama",
            "general.file_type": 2,
            "llama.context_length": 2000000,
        },
        "capabilities": ["completion", "vision", "tools", "thinking"],
    })
}

async fn stream_chat_response(
    executor: SharedChatExecutor,
    payload: crate::openai::chat::PromptPayload,
) -> Result<Sse<SseStream>, ApiError> {
    let handle = executor.stream(payload).await?;
    Ok(build_sse_stream(handle))
}

fn build_sse_stream(handle: StreamingHandle) -> Sse<SseStream> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);

    tokio::spawn(async move {
        if let Err(err) = forward_sse_events(handle, tx.clone()).await {
            warn!("streaming error: {err:?}");
        }
        let _ = tx.send(Ok(done_event())).await;
    });

    Sse::new(ReceiverStream::new(rx))
}

async fn forward_sse_events(
    handle: StreamingHandle,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) -> Result<(), ApiError> {
    let StreamingHandle {
        mut stream,
        response_model,
    } = handle;
    let created = current_timestamp();
    let mut response_id = "resp_stream".to_string();
    let mut sent_role = false;
    let mut usage = Usage::default();
    let verbose_enabled = verbose_logging_enabled();
    let mut verbose_text = verbose_enabled.then(String::new);
    let mut text_emitted = false;
    let mut text_deltas_since_last_message = false;
    let mut reasoning_summary = verbose_enabled.then(String::new);
    let mut streamed_tool_calls: Vec<ToolCall> = Vec::new();
    let mut tool_call_indices: HashMap<String, usize> = HashMap::new();
    let mut next_tool_index = 0usize;

    while let Some(event) = FuturesStreamExt::next(&mut stream).await {
        match event {
            Ok(ResponseEvent::OutputTextDelta(delta)) => {
                text_deltas_since_last_message = true;
                let mut delta_obj = Map::new();
                delta_obj.insert("content".to_string(), Value::String(delta.clone()));
                if !sent_role {
                    delta_obj.insert("role".to_string(), Value::String("assistant".to_string()));
                    sent_role = true;
                }
                if let Some(buffer) = verbose_text.as_mut() {
                    buffer.push_str(&delta);
                }
                text_emitted = true;
                let chunk = chunk_event(
                    &response_id,
                    created,
                    &response_model,
                    Value::Object(delta_obj),
                    None,
                    None,
                );
                if tx.send(Ok(chunk)).await.is_err() {
                    break;
                }
            }
            Ok(ResponseEvent::OutputItemAdded(item)) => {
                if matches!(item, ResponseItem::Message { .. }) {
                    continue;
                }
                if forward_tool_call_chunk(
                    &item,
                    &tx,
                    &response_id,
                    created,
                    &response_model,
                    &mut tool_call_indices,
                    &mut next_tool_index,
                    &mut streamed_tool_calls,
                    verbose_enabled,
                )
                .await
                {
                    break;
                }
            }
            Ok(ResponseEvent::OutputItemDone(item)) => {
                if let ResponseItem::Message { role, content, .. } = &item {
                    if role == "assistant" && !text_deltas_since_last_message {
                        if let Some(text) =
                            content_items_to_text(content).filter(|text| !text.trim().is_empty())
                        {
                            if let Some(buffer) = verbose_text.as_mut() {
                                buffer.push_str(&text);
                            }
                            text_emitted = true;
                            let mut delta_obj = Map::new();
                            if !sent_role {
                                delta_obj.insert(
                                    "role".to_string(),
                                    Value::String("assistant".to_string()),
                                );
                                sent_role = true;
                            }
                            delta_obj.insert("content".to_string(), Value::String(text));
                            let chunk = chunk_event(
                                &response_id,
                                created,
                                &response_model,
                                Value::Object(delta_obj),
                                None,
                                None,
                            );
                            if tx.send(Ok(chunk)).await.is_err() {
                                break;
                            }
                        }
                    }
                    text_deltas_since_last_message = false;
                    continue;
                }
                if forward_tool_call_chunk(
                    &item,
                    &tx,
                    &response_id,
                    created,
                    &response_model,
                    &mut tool_call_indices,
                    &mut next_tool_index,
                    &mut streamed_tool_calls,
                    verbose_enabled,
                )
                .await
                {
                    break;
                }
            }
            Ok(ResponseEvent::ReasoningSummaryDelta { delta, .. }) => {
                if let Some(buffer) = reasoning_summary.as_mut() {
                    buffer.push_str(&delta);
                }
            }
            Ok(ResponseEvent::ReasoningSummaryPartAdded { .. }) => {
                if let Some(buffer) = reasoning_summary.as_mut() {
                    if !buffer.is_empty() {
                        buffer.push('\n');
                    }
                }
            }
            Ok(ResponseEvent::Completed {
                response_id: rid,
                token_usage,
            }) => {
                response_id = rid;
                if let Some(tokens) = token_usage {
                    usage = Usage::from(tokens);
                }
                let finish_reason = if !streamed_tool_calls.is_empty() && !text_emitted {
                    Some("tool_calls")
                } else {
                    Some("stop")
                };
                let chunk = chunk_event(
                    &response_id,
                    created,
                    &response_model,
                    json!({}),
                    finish_reason,
                    Some(&usage),
                );
                let _ = tx.send(Ok(chunk)).await;
                let text_snapshot = verbose_text.take();
                let reasoning_snapshot = reasoning_summary.take();
                if text_snapshot.is_some()
                    || reasoning_snapshot.is_some()
                    || !streamed_tool_calls.is_empty()
                {
                    log_verbose_stream_response(
                        &response_model,
                        &response_id,
                        text_snapshot,
                        reasoning_snapshot,
                        streamed_tool_calls.clone(),
                        &usage,
                    );
                }
                break;
            }
            Ok(ResponseEvent::RateLimits(_)) | Ok(ResponseEvent::Created) => {}
            Ok(other) => {
                warn!("Unhandled Codex stream event: {other:?}");
            }
            Err(err) => {
                let chunk = chunk_event(
                    &response_id,
                    created,
                    &response_model,
                    json!({}),
                    Some("error"),
                    None,
                );
                let _ = tx.send(Ok(chunk)).await;
                error!("Codex stream error: {err:?}");
                break;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn forward_tool_call_chunk(
    item: &ResponseItem,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    response_id: &str,
    created: i64,
    response_model: &str,
    tool_call_indices: &mut HashMap<String, usize>,
    next_tool_index: &mut usize,
    streamed_tool_calls: &mut Vec<ToolCall>,
    verbose_enabled: bool,
) -> bool {
    if matches!(item, ResponseItem::Reasoning { .. }) {
        return false;
    }

    if let Some(call) = tool_call_from_item(item) {
        if !tool_call_indices.contains_key(&call.id) {
            tool_call_indices.insert(call.id.clone(), *next_tool_index);
            *next_tool_index += 1;
        }
        let index = *tool_call_indices
            .get(&call.id)
            .expect("tool index should exist");
        let chunk = tool_call_delta_chunk(response_id, created, response_model, &call, index);
        if tx.send(Ok(chunk)).await.is_err() {
            return true;
        }
        streamed_tool_calls.push(call);
    } else if verbose_enabled {
        warn!("Unhandled Codex output item in stream: {item:?}");
    }

    false
}

fn chunk_event(
    response_id: &str,
    created: i64,
    model: &str,
    delta: Value,
    finish_reason: Option<&str>,
    usage: Option<&Usage>,
) -> Event {
    let mut choice = json!({
        "index": 0,
        "delta": delta,
        "finish_reason": finish_reason,
    });
    if finish_reason.is_none() {
        choice["finish_reason"] = Value::Null;
    }

    let mut payload = json!({
        "id": response_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [choice],
    });

    if let Some(usage) = usage {
        payload["usage"] = json!({
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "total_tokens": usage.total_tokens,
        });
    }

    Event::default()
        .json_data(payload)
        .expect("serialize chunk")
}

fn done_event() -> Event {
    Event::default().data("[DONE]")
}

fn current_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

async fn log_requests(request: Request<Body>, next: Next) -> Result<Response, Infallible> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    let status = response.status();
    if status.is_success() {
        info!(
            method = %method,
            path = path,
            status = %status,
            "handled request"
        );
    } else {
        error!(
            method = %method,
            path = path,
            status = %status,
            "request failed"
        );
    }
    Ok(response)
}
