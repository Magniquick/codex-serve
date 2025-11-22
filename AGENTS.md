# Repository Guidelines

## Project Structure & Module Organization
The `src/` tree contains the binary entrypoint (`main.rs`), the shared library (`lib.rs`), Axum routing code in `src/server/`, OpenAI schema adapters in `src/openai/`, and runtime knobs inside `src/serve_config.rs`. `tests/` hosts end-to-end suites such as `tests/chat_completions.rs`, `reference/` stores the vendored Codex CLI docs/assets, and `codex/` pins the upstream workspace subcrates.

## Build, Test, and Development Commands
- `cargo check` — fast type-check to validate incremental edits.
- `cargo fmt` — enforce the canonical Rust formatting before committing.
- `cargo clippy --all-targets --all-features` — lint async/HTTP code paths; treat warnings as errors locally.
- `cargo run -- --addr 0.0.0.0:8080 --verbose` — launch the bridge for manual testing with custom bindings.
- `cargo test` / `cargo test -- --ignored` — run the unit/integration suite; the ignored group expects a logged-in Codex CLI.

## Coding Style & Naming Conventions
Follow Rust 2024 defaults: four-space indents, `snake_case` modules/functions, `PascalCase` types, and `SCREAMING_SNAKE_CASE` constants. Keep new HTTP handlers in focused modules under `src/server/` so extractors stay lean. Always run `cargo fmt` + `cargo clippy`; prefer `?` over `unwrap`, propagate errors via `anyhow::Result`, and log actionable context with `tracing` spans.

## Testing Guidelines
Unit tests should sit beside the module they cover using `#[cfg(test)]`, while cross-route checks belong in `tests/` using the existing `TestServer` helpers. Name tests after the user-visible behavior (`handles_streaming_chunks`, `rejects_missing_auth`). Gate Codex-dependent tests with `#[ignore]`, note the `codex login` prerequisite, and use `RUST_LOG=debug cargo test` when you need extra tracing.

## Commit & Pull Request Guidelines
Commits follow short, imperative subjects (see `init`, `version 1 !`), with optional bodies for multi-module changes. Each PR should explain the user-facing impact, note follow-up tasks, link related issues, and include updated `curl`/CLI examples when changing endpoints. Verify `cargo fmt`, `cargo clippy`, and the full test suite, call out any skipped ignored tests, and refresh README/config tables for new flags.

## Security & Configuration Tips
Never commit credentials—use the CLI flags (`--addr`, `--verbose`, `--expose-reasoning-models`, `--web-search-request`) or `RUST_LOG` to tweak behavior. Default bindings are loopback-only; switch to `0.0.0.0` only when the network is trusted. When logging verbosely, scrub payloads before sharing traces and ensure the Codex CLI session remains on a secured machine.
