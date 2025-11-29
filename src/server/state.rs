use std::sync::Arc;

use anyhow::{Context, Result};
use codex_app_server_protocol::AuthMode;
use codex_core::{
    auth::{AuthCredentialsStoreMode, AuthManager},
    config::{Config, ConfigOverrides, find_codex_home},
};

use crate::{error::ApiError, serve_config::web_search_request_override};

use super::executor::{MockChatExecutor, RealChatExecutor, SharedChatExecutor};
use toml::Value as TomlValue;

/// Shared application state for the Axum router.
#[derive(Clone)]
pub struct AppState {
    auth: AuthController,
    engine: SharedChatExecutor,
    web_search_enabled: bool,
}

impl AppState {
    /// Loads the Codex configuration and constructs the backing executor.
    pub async fn initialize() -> Result<Self> {
        let codex_home = find_codex_home()
            .context("could not determine Codex home directory (run `codex` once)")?;
        let auth_manager =
            AuthManager::shared(codex_home.clone(), true, AuthCredentialsStoreMode::File);

        let mut cli_overrides = Vec::new();
        if let Some(flag) = web_search_request_override() {
            cli_overrides.push((
                "features.web_search_request".to_string(),
                TomlValue::Boolean(flag),
            ));
        }
        let config =
            Config::load_with_cli_overrides(cli_overrides.clone(), ConfigOverrides::default())
                .await?;
        let web_search_enabled = config.tools_web_search_request;
        let config = Arc::new(config);

        let engine = Arc::new(RealChatExecutor::new(
            Arc::clone(&config),
            Arc::clone(&auth_manager),
            cli_overrides,
        ));

        Ok(Self {
            auth: AuthController::Real(auth_manager),
            engine,
            web_search_enabled,
        })
    }

    /// Test-only constructor that avoids hitting the real Codex CLI.
    pub fn insecure_mock(authenticated: bool) -> Self {
        Self::insecure_mock_with_mode(authenticated, None)
    }

    pub fn insecure_mock_with_mode(authenticated: bool, auth_mode: Option<AuthMode>) -> Self {
        Self {
            auth: AuthController::Mock {
                authenticated,
                mode: auth_mode,
            },
            engine: Arc::new(MockChatExecutor::new()),
            web_search_enabled: false,
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

    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.auth.auth_mode()
    }

    pub fn web_search_enabled(&self) -> bool {
        self.web_search_enabled
    }
}

#[derive(Clone)]
pub enum AuthController {
    Real(Arc<AuthManager>),
    Mock {
        authenticated: bool,
        mode: Option<AuthMode>,
    },
}

impl AuthController {
    pub fn is_authenticated(&self) -> bool {
        match self {
            Self::Real(manager) => manager.auth().is_some(),
            Self::Mock { authenticated, .. } => *authenticated,
        }
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        match self {
            Self::Real(manager) => manager.auth().map(|auth| auth.mode),
            Self::Mock {
                authenticated,
                mode,
            } => {
                if *authenticated {
                    *mode
                } else {
                    None
                }
            }
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
