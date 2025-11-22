use anyhow::Context;
use clap::Parser;
use codex_serve::{
    serve_config::{DeveloperPromptMode, ServeConfig, configure},
    server,
};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, filter::Directive};

#[derive(Parser)]
#[command(
    name = "codex-serve",
    version,
    about = "A local HTTP proxy for the Codex CLI",
    long_about = "Run Codex Serve to proxy OpenAI-compatible requests into the Codex CLI engine."
)]
struct Cli {
    /// Address to bind the HTTP listener to
    #[arg(long, default_value = "127.0.0.1:8000")]
    addr: String,

    /// Emit verbose tool and response logging
    #[arg(long)]
    verbose: bool,

    /// Include reasoning model variants in the `/api/tags` list
    #[arg(long)]
    expose_reasoning_models: bool,

    /// Override the Codex `features.web_search_request` flag (true/false). [default: false]
    #[arg(long)]
    web_search_request: bool,

    /// Controls how Codex Serve injects its compatibility instructions:
    /// - `none`: never add the helper prompt.
    /// - `default`: add it only when the request lacks a system prompt.
    /// - `override`: always prepend it (the original system message is appended for transparency).
    #[arg(long, default_value_t = DeveloperPromptMode::Default)]
    developer_prompt_mode: DeveloperPromptMode,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    configure(ServeConfig {
        verbose: cli.verbose,
        expose_reasoning_models: cli.expose_reasoning_models,
        web_search_request: Some(cli.web_search_request),
        developer_prompt_mode: cli.developer_prompt_mode,
    });

    let addr = cli.addr;
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind Codex Serve listener on {addr}"))?;

    info!(%addr, "Codex Serve listening");
    server::serve(listener).await
}

fn init_tracing() {
    static SET_TRACING: std::sync::Once = std::sync::Once::new();
    SET_TRACING.call_once(|| {
        let mut filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let otel_directive: Directive = "codex_otel::otel_event_manager=warn"
            .parse()
            .expect("static directive should parse");
        filter = filter.add_directive(otel_directive);

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .without_time()
            .init();
    });
}
