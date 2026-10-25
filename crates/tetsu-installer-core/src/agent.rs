//! Agent backend step (decision #24): skippable, no default. API backends
//! need a key; CLI backends only record the choice.

use crate::error::InstallError;
use crate::frontend::{Frontend, choose_from};
use crate::secret::Secret;

const MIN_KEY_LEN: usize = 8;

/// Providers reached with an API key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiProvider {
    Anthropic,
    OpenAi,
    Google,
}

impl ApiProvider {
    pub const ALL: [Self; 3] = [Self::Anthropic, Self::OpenAi, Self::Google];

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Google => "Google",
        }
    }
}

/// Subscription tools driven through their own CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliTool {
    ClaudeCode,
    Codex,
    Antigravity,
}

impl CliTool {
    pub const ALL: [Self; 3] = [Self::ClaudeCode, Self::Codex, Self::Antigravity];

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity",
        }
    }
}

/// The chosen agent backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentBackend {
    Api { provider: ApiProvider, key: Secret },
    Cli(CliTool),
}

impl AgentBackend {
    pub fn label(&self) -> String {
        match self {
            Self::Api { provider, .. } => format!("{} API", provider.label()),
            Self::Cli(tool) => format!("{} CLI", tool.label()),
        }
    }
}

fn ask_key(fe: &mut dyn Frontend, provider: ApiProvider) -> Result<Secret, InstallError> {
    loop {
        let key = fe.ask_secret(&format!("{} API key", provider.label()))?;
        let key = key.trim();
        if key.len() >= MIN_KEY_LEN && !key.chars().any(char::is_whitespace) {
            return Ok(Secret::new(key));
        }
        fe.message("That does not look like an API key.");
    }
}

/// Ask for a backend; `None` when skipped.
pub fn run(fe: &mut dyn Frontend) -> Result<Option<AgentBackend>, InstallError> {
    let mut labels: Vec<String> = ApiProvider::ALL
        .iter()
        .map(|p| format!("{} (API key)", p.label()))
        .collect();
    labels.extend(CliTool::ALL.iter().map(|t| format!("{} (CLI)", t.label())));
    labels.push("Skip: no agent backend for now".to_owned());
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let i = choose_from(fe, "Agent backend", &refs)?;
    let api = ApiProvider::ALL.len();
    Ok(if i < api {
        let provider = ApiProvider::ALL[i];
        Some(AgentBackend::Api {
            provider,
            key: ask_key(fe, provider)?,
        })
    } else if i < api + CliTool::ALL.len() {
        Some(AgentBackend::Cli(CliTool::ALL[i - api]))
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Script;

    #[test]
    fn skip_is_available_and_nothing_is_preselected() {
        let mut fe = Script::new().pick(6);
        assert_eq!(run(&mut fe).unwrap(), None);
        assert_eq!(fe.options[0].len(), 7);
        assert!(fe.options[0][6].starts_with("Skip"));
    }

    #[test]
    fn api_backend_asks_the_key() {
        let mut fe = Script::new().pick(0).secret("x").secret(" sk-ant-123456 ");
        let b = run(&mut fe).unwrap().unwrap();
        assert_eq!(
            b,
            AgentBackend::Api {
                provider: ApiProvider::Anthropic,
                key: Secret::new("sk-ant-123456")
            }
        );
        assert_eq!(b.label(), "Anthropic API");
        assert!(!format!("{b:?}").contains("sk-ant"));
        fe.assert_done();
    }

    #[test]
    fn cli_backend_records_the_choice_only() {
        for (i, tool) in CliTool::ALL.into_iter().enumerate() {
            let mut fe = Script::new().pick(3 + i);
            assert_eq!(run(&mut fe).unwrap(), Some(AgentBackend::Cli(tool)));
            fe.assert_done();
        }
    }
}
