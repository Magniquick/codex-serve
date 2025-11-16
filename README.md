# Codex-Serve

Codex Serve is a tiny bridge that lets any OpenAI-compatible client talk to the Codex CLI that’s already lounging on your machine. It keeps your prompts local, speaks the `/v1/chat/completions` dialect fluently, and sprinkles in just enough whimsy to make devops feel less grumpy.

## Highlights
- **OpenAI surface, Codex brain.** Accepts regular Chat Completions payloads (messages, tools, streaming) and forwards them to Codex via the `codex-rs` workspace.
- **Self-hosted comfort.** No extra auth layer—if `codex login` works on the box, Codex Serve can reuse that session.
- **Observability built-in.** `tracing` spans, structured errors that mirror OpenAI’s schema, and optional verbose payload logging when you want to snoop on every token.
- **Extensible routing.** Besides `/v1/chat/completions`, the server exposes `/v1/models`, `/healthz`, a tiny `/api/version`, and a couple of compatibility shims used by local testing tools.

## Architecture snapshot
1. **Tokio + Axum core.** The binary binds to `CODEX_SERVE_ADDR` (default `127.0.0.1:8000`) and wires routes through `tower-http` middleware for logging and CORS.
2. **Codex Adapter.** OpenAI-flavored requests are converted into `codex_core::Prompt`s, then executed via `SharedChatExecutor`, which in turn talks to the Codex CLI backend over the local IPC transport.
3. **Streaming fan-out.** `ResponseEvent`s from Codex are folded into OpenAI JSON chunks, surfaced as standard SSE events when `stream: true` is requested, or accumulated into a single JSON payload otherwise.
4. **State + Auth.** `AppState` calls into the Codex auth subsystem; if the CLI isn’t logged in, the HTTP handler returns a friendly `401` with an OpenAI-style error body.
5. **Tracing sprinkles.** Every request lives inside a span, errors are serialized into `{ "error": { ... } }`, and optional verbose logs reveal inputs/outputs for debugging.

## Endpoints
- `POST /v1/chat/completions` – main entry point; supports streaming and tool calls.
- `GET /v1/models` – lists Codex model IDs derived from `codex-core` presets (toggle reasoning variants with `CODEX_SERVE_EXPOSE_REASONING_MODELS`).
- `GET /healthz` – returns readiness plus whether Codex auth is available.
- `GET /api/version`, `GET /api/tags`, `POST /api/show` – small compatibility helpers mirrored from the Codex CLI ChatMock tooling.

## Getting started
1. **Prereqs**
   - Rust 1.79+ (edition 2024).
   - Codex CLI installed, authenticated, and running (`codex login`).
2. **Boot the server**

```bash
git clone https://github.com/magniquick/codex-serve # or local path
cd codex-serve
cargo run
```

3. **Poke it**

```bash
curl -s http://127.0.0.1:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
        "model": "gpt-5.1-codex-mini",
        "messages": [
          {"role": "system", "content": "be concise"},
          {"role": "user", "content": "tell me a cozy pun"}
        ]
      }'                             
```

If Codex is logged in, you’ll receive a valid OpenAI-style completion like -
```json
{"id":"resp_id","object":"chat.completion","created":"redacted","model":"gpt-5.1-codex-mini","choices":[{"index":0,"message":{"role":"assistant","content":"Cozy pun: “I’m feeling so woolly today—guess it’s time to knit some warm fuzzy feelings!”"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2371,"completion_tokens":29,"total_tokens":2401}}
```
If not, you (should) be gently nudged toward `codex login`.

## Configuration knobs 
> Will be moved into command line flags later.

| Variable | Default | Purpose |
| --- | --- | --- |
| `CODEX_SERVE_ADDR` | `127.0.0.1:8000` | Address the server binds to (set to `0.0.0.0:8080` to accept LAN traffic). |
| `CODEX_SERVE_VERBOSE` | unset | When `true/1/yes`, echo every request, response, and streamed chunk via `tracing` logs. |
| `CODEX_SERVE_EXPOSE_REASONING_MODELS` | unset | When enabled, include reasoning-tier Codex models in `/v1/models`. |
| `RUST_LOG` | `info` | Usual `tracing_subscriber` filter; handy for enabling `debug` on specific modules. |

## Observability & errors
- All handlers emit structured logs; set the logging env vars to see per-route spans.
- Errors follow `{ "error": { "message", "type", ... } }` so upstream OpenAI SDKs can parse them without special cases.
- `GET /healthz` is the simplest smoke test for readiness and auth.

## Testing
- `cargo test` exercises the request/response adapters, the fake executor, and the auth gating logic.
- `cargo test -- --ignored` runs the optional end-to-end test that expects a real Codex session.
- Integration suites (under `tests/`) spin up the Axum server on an ephemeral port and hit the public endpoints using `reqwest`.

## Roadmap
1. **Complete adapter parity.** Finish wiring `ModelClient` + `ResponseStream` so streaming matches Codex CLI behavior byte-for-byte.
2. **Tooling polish.** Support tool choice hints, reasoning tokens, and structured tool call echoes found in the latest OpenAI schema.
3. **Conversation reuse.** Introduce persistent IDs backed by `ConversationManager` for multi-turn sessions.
4. **Docs & deployment.** Package systemd examples, Dockerfile snippets, and a troubleshooting section for common auth hiccups.

## Contributing
Pull requests are welcome! Please:
- Add tests (or update snapshots) for any API change.
- Keep logging human-friendly and consistent with the OpenAI schema.
- Note any behavior differences from OpenAI’s API so we can document them.
- open issues _with enough context_ in case models act weird.

PS: ASCII art wanted UwU
