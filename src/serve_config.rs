use std::{fmt, str::FromStr, sync::OnceLock};

#[derive(Clone, Copy, Debug)]
pub struct ServeConfig {
    pub verbose: bool,
    pub expose_reasoning_models: bool,
    pub web_search_request: Option<bool>,
    pub developer_prompt_mode: DeveloperPromptMode,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            verbose: false,
            expose_reasoning_models: false,
            web_search_request: None,
            developer_prompt_mode: DeveloperPromptMode::Default,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DeveloperPromptMode {
    Disabled,
    #[default]
    Default,
    Override,
}

impl DeveloperPromptMode {
    fn as_str(self) -> &'static str {
        match self {
            DeveloperPromptMode::Disabled => "none",
            DeveloperPromptMode::Default => "default",
            DeveloperPromptMode::Override => "override",
        }
    }
}

impl fmt::Display for DeveloperPromptMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeveloperPromptMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(DeveloperPromptMode::Disabled),
            "default" => Ok(DeveloperPromptMode::Default),
            "override" => Ok(DeveloperPromptMode::Override),
            other => Err(format!(
                "invalid developer prompt mode `{other}` (expected none/default/override)"
            )),
        }
    }
}

static GLOBAL_CONFIG: OnceLock<ServeConfig> = OnceLock::new();

/// Sets the global configuration for the running server. This should be called once at startup.
pub fn configure(config: ServeConfig) {
    GLOBAL_CONFIG
        .set(config)
        .expect("codex serve config already initialized");
}

/// Returns true if verbose logging was requested.
pub fn verbose_logging_enabled() -> bool {
    GLOBAL_CONFIG.get().is_some_and(|cfg| cfg.verbose)
}

/// Returns true if the reasoning model variants should be exposed.
pub fn expose_reasoning_models() -> bool {
    GLOBAL_CONFIG
        .get()
        .is_some_and(|cfg| cfg.expose_reasoning_models)
}

/// Returns the override for forcing web search requests (if any).
pub fn web_search_request_override() -> Option<bool> {
    GLOBAL_CONFIG.get().and_then(|cfg| cfg.web_search_request)
}

pub fn developer_prompt_mode() -> DeveloperPromptMode {
    GLOBAL_CONFIG
        .get()
        .map(|cfg| cfg.developer_prompt_mode)
        .unwrap_or_default()
}
