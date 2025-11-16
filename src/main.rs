use std::env;

use anyhow::Context;
use codex_serve::server;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, filter::Directive};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let addr = env::var("CODEX_SERVE_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".to_string());
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
