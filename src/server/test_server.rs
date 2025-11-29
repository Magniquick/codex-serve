use anyhow::Result;
use tokio::{
    net::TcpListener,
    sync::oneshot,
    task::{self, JoinHandle},
};

use codex_app_server_protocol::AuthMode;

use super::{router, state::AppState};

/// Helper for integration tests to run the server in the background.
pub struct TestServer {
    base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl TestServer {
    pub async fn spawn() -> Result<Self> {
        Self::spawn_with_state(AppState::insecure_mock(true)).await
    }

    pub async fn spawn_unauthenticated() -> Result<Self> {
        Self::spawn_with_state(AppState::insecure_mock(false)).await
    }

    pub async fn spawn_with_auth_mode(
        authenticated: bool,
        auth_mode: Option<AuthMode>,
    ) -> Result<Self> {
        let state = AppState::insecure_mock_with_mode(authenticated, auth_mode);
        Self::spawn_with_state(state).await
    }

    pub async fn spawn_with_state(state: AppState) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let server = axum::serve(listener, router(state)).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });

        let task = task::spawn(async move {
            if let Err(err) = server.await {
                eprintln!("codex-serve test server error: {err}");
            }
        });

        Ok(Self {
            base_url: format!("http://{}", addr),
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        self.task.abort();
    }
}
