use anyhow::{anyhow, Result};

/// Temporary placeholder for the HTTP server we are about to build.
pub struct TestServer {
    base_url: String,
}

impl TestServer {
    /// Spawn the Codex Serve HTTP server on an ephemeral port for integration tests.
    pub async fn spawn() -> Result<Self> {
        Err(anyhow!(
            "Codex Serve HTTP surface is not implemented yet"
        ))
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}
