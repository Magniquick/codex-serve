use codex_core::{ContentItem, Prompt, ResponseItem, ToolSpec};

use crate::serve_config::DeveloperPromptMode;

pub const CODEX_SERVE_PROMPT_MARKER: &str = "Codex Serve compatibility mode";

/// Ensures the prompt includes the Codex web search tool when allowed.
pub fn ensure_web_search_tool(prompt: &mut Prompt, allow_web_search: bool) -> bool {
    let mut has_web_search = prompt
        .tools
        .iter()
        .any(|tool| matches!(tool, ToolSpec::WebSearch {}));

    if allow_web_search && !has_web_search {
        prompt.tools.push(ToolSpec::WebSearch {});
        has_web_search = true;
    }

    has_web_search
}

/// Injects Codex Serve's developer prompt based on the configured mode.
pub fn inject_developer_prompt(
    prompt: &mut Prompt,
    has_web_search: bool,
    system_prompt: Option<&str>,
    mode: DeveloperPromptMode,
) {
    match mode {
        DeveloperPromptMode::Disabled => return,
        DeveloperPromptMode::Default if system_prompt.is_some() => return,
        _ => {}
    }

    if has_existing_codex_serve_message(prompt) {
        return;
    }

    let original_system = match mode {
        DeveloperPromptMode::Override => system_prompt.and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }),
        DeveloperPromptMode::Disabled | DeveloperPromptMode::Default => None,
    };

    let text = build_developer_prompt_text(has_web_search, original_system);

    prompt.input.insert(
        0,
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText { text }],
        },
    );
}

fn build_developer_prompt_text(has_web_search: bool, original_system: Option<&str>) -> String {
    let mut lines = vec![
        "This compatibility shim cannot run shells, edit files, or inspect your workspace.",
        "Never claim you executed commands or edits—describe what the user should run instead and wait for their results.",
    ];

    if has_web_search {
        lines.push("You may invoke the `web_search` tool when you truly need new information.");
    } else {
        lines.push("No tools are available for this conversation.");
    }

    let mut text = format!(
        "{CODEX_SERVE_PROMPT_MARKER}:\n{}",
        lines
            .into_iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if let Some(original) = original_system {
        text.push_str("\n\nThe original system message follows:\n");
        text.push_str(original);
    }

    text
}

fn has_existing_codex_serve_message(prompt: &Prompt) -> bool {
    prompt.input.iter().any(|item| match item {
        ResponseItem::Message { role, content, .. } if role == "developer" => {
            content.iter().any(|entry| {
                matches!(
                    entry,
                    ContentItem::InputText { text } if text.contains(CODEX_SERVE_PROMPT_MARKER)
                )
            })
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_web_search_tool_inserts_when_allowed() {
        let mut prompt = Prompt::default();
        assert!(!ensure_web_search_tool(&mut prompt, false));
        assert!(prompt.tools.is_empty());

        assert!(ensure_web_search_tool(&mut prompt, true));
        assert!(matches!(prompt.tools.as_slice(), [ToolSpec::WebSearch {}]));
    }

    #[test]
    fn ensure_web_search_tool_no_duplicates() {
        let mut prompt = Prompt {
            tools: vec![ToolSpec::WebSearch {}],
            ..Default::default()
        };
        assert!(ensure_web_search_tool(&mut prompt, true));
        assert_eq!(prompt.tools.len(), 1);
    }

    #[test]
    fn default_mode_skips_when_system_prompt_present() {
        let mut prompt = Prompt::default();
        inject_developer_prompt(
            &mut prompt,
            false,
            Some("custom"),
            DeveloperPromptMode::Default,
        );
        assert!(prompt.input.is_empty());
    }

    #[test]
    fn default_mode_injects_when_missing_system_prompt() {
        let mut prompt = Prompt::default();
        inject_developer_prompt(&mut prompt, false, None, DeveloperPromptMode::Default);
        assert_eq!(prompt.input.len(), 1);
        assert!(matches!(prompt.input[0], ResponseItem::Message { .. }));
    }

    #[test]
    fn override_mode_includes_original_text() {
        let mut prompt = Prompt::default();
        inject_developer_prompt(
            &mut prompt,
            true,
            Some("keep this"),
            DeveloperPromptMode::Override,
        );
        let ResponseItem::Message { content, .. } = &prompt.input[0] else {
            panic!("expected developer message");
        };
        match &content[0] {
            ContentItem::InputText { text } => assert!(text.contains("keep this")),
            other => panic!("unexpected content: {other:?}"),
        }
    }

    #[test]
    fn disabled_mode_never_injects() {
        let mut prompt = Prompt::default();
        inject_developer_prompt(&mut prompt, false, None, DeveloperPromptMode::Disabled);
        assert!(prompt.input.is_empty());
    }
}
