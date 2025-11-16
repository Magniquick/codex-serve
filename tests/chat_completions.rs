use codex_common::model_presets::builtin_model_presets;
use codex_serve::server::TestServer;
use reqwest::StatusCode;
use serde_json::Value;

fn sample_payload() -> Value {
    serde_json::json!({
        "model": "gpt-5",
        "messages": [
            {"role": "user", "content": "hello world"}
        ]
    })
}

fn extract_message_content(body: &Value) -> Option<String> {
    let choices = body.get("choices")?.as_array()?;
    let first = choices.first()?;
    let message = first.get("message")?.as_object()?;
    let role = message.get("role")?.as_str()?;
    if role != "assistant" {
        return None;
    }
    message.get("content")?.as_str().map(|s| s.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_completions_curl_example() {
    let server = TestServer::spawn()
        .await
        .expect("Codex Serve test server should start");

    let client = reqwest::Client::new();
    let url = format!("{}/v1/chat/completions", server.base_url());
    let response = client
        .post(url)
        .json(&sample_payload())
        .send()
        .await
        .expect("request should reach Codex Serve");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Codex Serve should return 200 OK for the sample curl payload"
    );

    let body: Value = response.json().await.expect("response must be JSON");

    assert_eq!(
        body.get("object").and_then(Value::as_str),
        Some("chat.completion")
    );
    assert_eq!(body.get("model").and_then(Value::as_str), Some("gpt-5"));
    assert!(
        body.get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("resp_")),
        "response id should resemble resp_*"
    );
    assert!(
        extract_message_content(&body)
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty()),
        "assistant reply text should be present"
    );

    let usage = body
        .get("usage")
        .and_then(Value::as_object)
        .expect("usage block should be present");
    for field in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        assert!(
            usage
                .get(field)
                .and_then(Value::as_i64)
                .is_some_and(|value| value >= 0),
            "usage field {field} should be a non-negative integer",
            field = field
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_version_matches_crate() {
    let server = TestServer::spawn()
        .await
        .expect("Codex Serve test server should start");

    let client = reqwest::Client::new();
    let url = format!("{}/api/version", server.base_url());
    let response = client
        .get(url)
        .send()
        .await
        .expect("request should reach Codex Serve");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "/api/version should respond with 200"
    );

    let body: Value = response.json().await.expect("response must be JSON");
    let expected = "0.12.10";
    assert_eq!(
        body.get("version").and_then(Value::as_str),
        Some(expected),
        "/api/version should expose crate version"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_tags_lists_models() {
    let server = TestServer::spawn()
        .await
        .expect("Codex Serve test server should start");

    let client = reqwest::Client::new();
    let url = format!("{}/api/tags", server.base_url());
    let response = client
        .get(url)
        .send()
        .await
        .expect("request should reach Codex Serve");

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.expect("response must be JSON");
    let models = body
        .get("models")
        .and_then(Value::as_array)
        .expect("models array should be present");
    let expected_names: Vec<String> = builtin_model_presets(None)
        .into_iter()
        .map(|preset| preset.model.to_string())
        .collect();
    let names: Vec<String> = models
        .iter()
        .filter_map(|value| value.get("name").and_then(Value::as_str))
        .map(|value| value.to_string())
        .collect();
    assert_eq!(names, expected_names);
    for entry in models {
        let details = entry
            .get("details")
            .and_then(Value::as_object)
            .expect("each entry should include metadata");
        assert_eq!(details.get("family").and_then(Value::as_str), Some("llama"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_show_returns_metadata() {
    let server = TestServer::spawn()
        .await
        .expect("Codex Serve test server should start");

    let client = reqwest::Client::new();
    let url = format!("{}/api/show", server.base_url());
    let response = client
        .post(url)
        .json(&serde_json::json!({"model": "gpt-5"}))
        .send()
        .await
        .expect("request should reach Codex Serve");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response must be JSON");
    assert_eq!(
        body.get("capabilities")
            .and_then(Value::as_array)
            .map(|values| values.len()),
        Some(4)
    );
    assert_eq!(
        body.get("model_info")
            .and_then(Value::as_object)
            .and_then(|info| info.get("general.architecture"))
            .and_then(Value::as_str),
        Some("llama")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_show_requires_model() {
    let server = TestServer::spawn()
        .await
        .expect("Codex Serve test server should start");

    let client = reqwest::Client::new();
    let url = format!("{}/api/show", server.base_url());
    let response = client
        .post(url)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("request should reach Codex Serve");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
