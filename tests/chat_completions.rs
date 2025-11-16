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

    assert_eq!(body.get("object").and_then(Value::as_str), Some("chat.completion"));
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
