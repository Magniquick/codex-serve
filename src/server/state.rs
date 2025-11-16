use std::sync::Arc;

use anyhow::{Context, Result};
use codex_core::{
    auth::{AuthCredentialsStoreMode, AuthManager},
    config::{Config, ConfigOverrides, find_codex_home},
};

use crate::error::ApiError;

use super::executor::{MockChatExecutor, RealChatExecutor, SharedChatExecutor};

/// Shared application state for the Axum router.
#[derive(Clone)]
pub struct AppState {
    auth: AuthController,
    engine: SharedChatExecutor,
}

impl AppState {
    /// Loads the Codex configuration and constructs the backing executor.
    pub async fn initialize() -> Result<Self> {
        let codex_home = find_codex_home()
            .context("could not determine Codex home directory (run `codex` once)")?;
        let auth_manager =
            AuthManager::shared(codex_home.clone(), true, AuthCredentialsStoreMode::File);

        let config =
            Config::load_with_cli_overrides(Vec::new(), ConfigOverrides::default()).await?;
        let config = Arc::new(config);

        let engine = Arc::new(RealChatExecutor::new(
            Arc::clone(&config),
            Arc::clone(&auth_manager),
        ));

        Ok(Self {
            auth: AuthController::Real(auth_manager),
            engine,
        })
    }

    /// Test-only constructor that avoids hitting the real Codex CLI.
    pub fn insecure_mock(authenticated: bool) -> Self {
        Self {
            auth: AuthController::Mock { authenticated },
            engine: Arc::new(MockChatExecutor::new()),
        }
    }

    pub fn ensure_authenticated(&self) -> Result<(), ApiError> {
        if self.auth.is_authenticated() {
            Ok(())
        } else {
            Err(ApiError::unauthorized(
                "Codex Serve requires an active Codex login. \
                 Run `codex login` (or sign in via the Codex CLI) and try again.",
            ))
        }
    }

    pub fn engine(&self) -> SharedChatExecutor {
        Arc::clone(&self.engine)
    }

    pub fn auth(&self) -> &AuthController {
        &self.auth
    }
}

#[derive(Clone)]
pub enum AuthController {
    Real(Arc<AuthManager>),
    Mock { authenticated: bool },
}

impl AuthController {
    pub fn is_authenticated(&self) -> bool {
        match self {
            Self::Real(manager) => manager.auth().is_some(),
            Self::Mock { authenticated } => *authenticated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn app_state_is_send_sync() {
        assert_send::<AppState>();
        assert_sync::<AppState>();
    }
}
