# Codex Serve: Agent Design Notes

## Goal
Expose a local, OpenAI-compatible `/v1/chat/completions` endpoint that proxies every request to the Codex CLI backend. Users must already have `codex` installed, authenticated, and running on the target machine; this project simply wraps the existing Codex capabilities in an HTTP surface that other tools or scripts can call.

## Architecture Overview
- **HTTP server:** `tokio` runtime with `axum` + `tower-http` for routing, middlewares, and CORS. Primary routes: `POST /v1/chat/completions`, `GET /v1/models`, `GET /healthz`.
- **Codex integration:** Depend directly on the `codex/codex-rs` workspace (via `codex-core`, `codex-protocol`, etc.) for auth, model metadata, and request execution. A thin `CodexAdapter` layer converts OpenAI-shaped requests into `codex_core::Prompt` objects and streams `ResponseEvent`s back into OpenAI-compatible JSON.
- **State:** Short-lived per-request conversations at first; evolve toward re-usable conversation IDs backed by `codex_core::ConversationManager`.
- **Observability:** `tracing` spans per request, structured errors that mirror OpenAI’s `{ "error": { ... } }` schema.

## Dependencies
- `axum`, `tokio`, `tower-http` for the server.
- `serde`, `serde_json`, `uuid`, `chrono`, `thiserror` for API shapes and plumbing.
- `reqwest` and `tokio::test` for integration tests.
- Codex crates: `codex-core`, `codex-protocol`, `codex-backend-openapi-models`, etc., pulled via path dependencies to the submodule.

## Testing Strategy
1. **TDD driver:** Integration test that boots the server on an ephemeral port, sends the sample `curl` payload from the prompt, and asserts we return a valid OpenAI-style response (mocked Codex adapter for determinism).
2. **Unit coverage:** Request translation (messages → prompts), response translation (`ResponseEvent`s → OpenAI JSON), and auth gating (surface friendly 401 when Codex login is missing).
3. **Optional end-to-end:** Ignored test that exercises a real Codex session for manual verification once `codex login` is complete.

## Milestones
1. Scaffold binary crate + stub endpoints + passing curl-style test.
2. Implement Codex adapter using `ModelClient` and `ResponseStream`.
3. Reach OpenAI compatibility parity (streaming, tool calls, reasoning knobs), reusing ChatMock behaviors found under `./reference`.
4. Add conversation reuse, better error reporting, and deployment docs.
