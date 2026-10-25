//! The answer file: a TOML file that answers any subset of the questions.
//! Whatever is missing is asked. It is meant for unattended tests, so its
//! rules are strict:
//!
//! - the destructive confirmation is the exact `by-id` name of the disk,
//!   never a boolean;
//! - the user password is a `crypt` hash, never plain text;
//! - the recovery key is generated and printed, never read or written;
//! - unknown keys are an error;
//! - the disk passphrase and the API key are plain text here, which is why
//!   the installer warns about the file and never copies it to the target.

use serde::Deserialize;
use tetsu_installer_core::agent::{ApiProvider, CliTool};
use tetsu_installer_core::keyboard::{validate_locale, validate_timezone};
use tetsu_installer_core::secret::Secret;
use tetsu_installer_core::unlock::{UnlockLevel, strength};
use tetsu_installer_core::user::{Password, validate_full_name, validate_username};

/// Why an answer file was refused.
#[derive(Debug, thiserror::Error)]
#[error("answer file: {0}")]
pub struct AnswerFileError(String);

fn bad(msg: impl Into<String>) -> AnswerFileError {
    AnswerFileError(msg.into())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    keyboard: RawKeyboard,
    #[serde(default)]
    disk: RawDisk,
    #[serde(default)]
    unlock: RawUnlock,
    #[serde(default)]
    user: RawUser,
    #[serde(default)]
    agent: RawAgent,
    #[serde(default)]
    install: RawInstall,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKeyboard {
    layout: Option<String>,
    locale: Option<String>,
    timezone: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDisk {
    confirm_by_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUnlock {
    level: Option<String>,
    passphrase: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUser {
    name: Option<String>,
    full_name: Option<String>,
    password_hash: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgent {
    backend: Option<String>,
    provider: Option<String>,
    tool: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInstall {
    #[serde(default)]
    confirm: bool,
}

/// `[keyboard]`: the layout, with optional overrides of what it implies.
#[derive(Debug, PartialEq, Eq)]
pub struct KeyboardAnswer {
    pub layout: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
}

/// The agent backend as the file states it.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentAnswer {
    None,
    /// The key may be missing; it is then asked.
    Api {
        provider: ApiProvider,
        key: Option<Secret>,
    },
    Cli(CliTool),
}

/// A validated answer file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AnswerFile {
    pub keyboard: Option<KeyboardAnswer>,
    /// File name of the disk's `/dev/disk/by-id` link.
    pub disk_by_id: Option<String>,
    pub level: Option<UnlockLevel>,
    pub passphrase: Option<Secret>,
    pub username: Option<String>,
    pub full_name: Option<String>,
    pub password: Option<Password>,
    pub agent: Option<AgentAnswer>,
    /// Skip the summary confirmation when nothing had to be asked.
    pub confirm: bool,
}

impl AnswerFile {
    /// Parse and validate `text`. Errors never repeat a value from the file.
    pub fn parse(text: &str) -> Result<Self, AnswerFileError> {
        let table: toml::Table = text.parse().map_err(|e| syntax_error(text, &e))?;
        refuse_forbidden(&table, "")?;
        let raw: Raw = toml::from_str(text).map_err(|e| syntax_error(text, &e))?;
        Self::resolve(raw)
    }

    fn resolve(raw: Raw) -> Result<Self, AnswerFileError> {
        let k = raw.keyboard;
        let keyboard = match (k.layout, &k.locale, &k.timezone) {
            (Some(layout), _, _) => Some(KeyboardAnswer {
                layout: layout_name(layout)?,
                locale: k.locale.map(check_locale).transpose()?,
                timezone: k.timezone.map(check_timezone).transpose()?,
            }),
            (None, None, None) => None,
            (None, _, _) => return Err(bad("keyboard.layout is required with locale or timezone")),
        };
        let u = raw.user;
        Ok(Self {
            keyboard,
            disk_by_id: raw.disk.confirm_by_id.map(by_id_name).transpose()?,
            level: raw.unlock.level.map(level).transpose()?,
            passphrase: raw.unlock.passphrase.map(passphrase).transpose()?,
            username: u
                .name
                .map(|n| {
                    validate_username(&n)
                        .map(|()| n)
                        .map_err(|m| bad(format!("user.name: {m}")))
                })
                .transpose()?,
            full_name: u
                .full_name
                .map(|n| {
                    validate_full_name(&n)
                        .map(|()| n)
                        .map_err(|m| bad(format!("user.full_name: {m}")))
                })
                .transpose()?,
            password: u
                .password_hash
                .map(|h| {
                    Password::from_hash(&h).map_err(|m| bad(format!("user.password_hash: {m}")))
                })
                .transpose()?,
            agent: agent(raw.agent)?,
            confirm: raw.install.confirm,
        })
    }
}

/// Parse error with the line number and the parser's message, but not the
/// source line: that may hold a secret.
fn syntax_error(text: &str, e: &toml::de::Error) -> AnswerFileError {
    let line = e
        .span()
        .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
    match line {
        Some(n) => bad(format!("line {n}: {}", e.message())),
        None => bad(e.message().to_owned()),
    }
}

/// Keys that must never be in the file, whatever the section.
fn refuse_forbidden(table: &toml::Table, section: &str) -> Result<(), AnswerFileError> {
    for (key, value) in table {
        let path = format!("{section}{key}");
        if key.to_ascii_lowercase().contains("recovery") {
            return Err(bad(format!(
                "{path}: the recovery key is generated during the install and printed on the console; it is never read from a file"
            )));
        }
        if path == "user.password" {
            return Err(bad(
                "user.password: plain text is not accepted; give user.password_hash (a crypt hash, e.g. from `openssl passwd -6`)",
            ));
        }
        if let toml::Value::Table(inner) = value {
            refuse_forbidden(inner, &format!("{path}."))?;
        }
    }
    Ok(())
}

fn layout_name(layout: String) -> Result<String, AnswerFileError> {
    let ok = !layout.is_empty()
        && layout
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    ok.then_some(layout)
        .ok_or_else(|| bad("keyboard.layout: use a layout code such as it, us or de"))
}

fn check_locale(locale: String) -> Result<String, AnswerFileError> {
    validate_locale(&locale)
        .map(|()| locale)
        .map_err(|m| bad(format!("keyboard.locale: {m}")))
}

fn check_timezone(tz: String) -> Result<String, AnswerFileError> {
    validate_timezone(&tz)
        .map(|()| tz)
        .map_err(|m| bad(format!("keyboard.timezone: {m}")))
}

/// The file name of a by-id link: no path, so it cannot point elsewhere.
fn by_id_name(id: String) -> Result<String, AnswerFileError> {
    let ok = !id.is_empty() && id.chars().all(|c| c.is_ascii_graphic() && c != '/');
    ok.then_some(id).ok_or_else(|| {
        bad("disk.confirm_by_id: give the exact name of the link in /dev/disk/by-id, without a path")
    })
}

fn level(id: String) -> Result<UnlockLevel, AnswerFileError> {
    UnlockLevel::ALL
        .into_iter()
        .find(|l| l.id() == id)
        .ok_or_else(|| bad("unlock.level: use paranoid, comfort or balanced"))
}

fn passphrase(p: String) -> Result<Secret, AnswerFileError> {
    strength::check(&p)
        .map(|()| Secret::new(p))
        .map_err(|m| bad(format!("unlock.passphrase: {m}")))
}

fn agent(a: RawAgent) -> Result<Option<AgentAnswer>, AnswerFileError> {
    let Some(backend) = a.backend else {
        if a.provider.is_some() || a.tool.is_some() || a.key.is_some() {
            return Err(bad("agent.backend is required with provider, tool or key"));
        }
        return Ok(None);
    };
    let only = |what: &str, present: bool| {
        if present {
            Err(bad(format!(
                "agent.{what} does not apply to backend {backend}"
            )))
        } else {
            Ok(())
        }
    };
    match backend.as_str() {
        "none" => {
            only("provider", a.provider.is_some())?;
            only("tool", a.tool.is_some())?;
            only("key", a.key.is_some())?;
            Ok(Some(AgentAnswer::None))
        }
        "api" => {
            only("tool", a.tool.is_some())?;
            let provider = a
                .provider
                .ok_or_else(|| bad("agent.provider is required for backend api"))?;
            let provider = ApiProvider::ALL
                .into_iter()
                .find(|p| p.id() == provider)
                .ok_or_else(|| bad("agent.provider: use anthropic, openai or google"))?;
            Ok(Some(AgentAnswer::Api {
                provider,
                key: a.key.map(|k| Secret::new(k.trim())),
            }))
        }
        "cli" => {
            only("provider", a.provider.is_some())?;
            only("key", a.key.is_some())?;
            let tool = a
                .tool
                .ok_or_else(|| bad("agent.tool is required for backend cli"))?;
            let tool = CliTool::ALL
                .into_iter()
                .find(|t| t.id() == tool)
                .ok_or_else(|| bad("agent.tool: use claude-code, codex or antigravity"))?;
            Ok(Some(AgentAnswer::Cli(tool)))
        }
        _ => Err(bad("agent.backend: use api, cli or none")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
[keyboard]
layout = "it"
locale = "it_IT.UTF-8"
timezone = "Europe/Rome"

[disk]
confirm_by_id = "virtio-tetsu0"

[unlock]
level = "balanced"
passphrase = "Correct-Horse-9-Battery"

[user]
name = "marco"
full_name = "Marco M"
password_hash = "$6$salt$abcdef"

[agent]
backend = "api"
provider = "anthropic"
key = "sk-test-12345678"

[install]
confirm = true
"#;

    fn err(text: &str) -> String {
        AnswerFile::parse(text).unwrap_err().to_string()
    }

    #[test]
    fn a_full_file_is_read() {
        let f = AnswerFile::parse(FULL).unwrap();
        assert_eq!(f.keyboard.unwrap().layout, "it");
        assert_eq!(f.disk_by_id.as_deref(), Some("virtio-tetsu0"));
        assert_eq!(f.level, Some(UnlockLevel::Balanced));
        assert_eq!(f.username.as_deref(), Some("marco"));
        assert_eq!(f.password, Some(Password::Hash("$6$salt$abcdef".into())));
        assert!(matches!(
            f.agent,
            Some(AgentAnswer::Api {
                provider: ApiProvider::Anthropic,
                key: Some(_)
            })
        ));
        assert!(f.confirm);
    }

    #[test]
    fn any_subset_is_fine_and_an_empty_file_answers_nothing() {
        assert_eq!(AnswerFile::parse("").unwrap(), AnswerFile::default());
        let f = AnswerFile::parse("[unlock]\nlevel = \"comfort\"\n").unwrap();
        assert_eq!(f.level, Some(UnlockLevel::Comfort));
        assert_eq!(f.passphrase, None);
        let f = AnswerFile::parse("[agent]\nbackend = \"none\"\n").unwrap();
        assert_eq!(f.agent, Some(AgentAnswer::None));
    }

    #[test]
    fn unknown_keys_and_sections_are_errors() {
        assert!(err("[disk]\nwipe = true\n").contains("unknown field"));
        assert!(err("[network]\nssid = \"x\"\n").contains("unknown field"));
        assert!(err("verbose = true\n").contains("unknown field"));
    }

    #[test]
    fn the_disk_confirmation_is_a_by_id_name_not_a_boolean() {
        assert!(err("[disk]\nconfirm_by_id = true\n").contains("invalid type"));
        assert!(err("[disk]\nconfirm_by_id = \"\"\n").contains("confirm_by_id"));
        assert!(err("[disk]\nconfirm_by_id = \"/dev/sda\"\n").contains("without a path"));
    }

    #[test]
    fn the_password_is_a_hash_only() {
        assert!(err("[user]\npassword = \"hunter2hunter2\"\n").contains("password_hash"));
        assert!(err("[user]\npassword_hash = \"hunter2\"\n").contains("not a crypt hash"));
    }

    #[test]
    fn a_recovery_key_is_refused_anywhere() {
        for text in [
            "[unlock]\nrecovery_key = \"cccc\"\n",
            "[recovery]\nkey = \"cccc\"\n",
            "[disk]\nRecovery-Key = \"cccc\"\n",
        ] {
            assert!(err(text).contains("never read from a file"), "{text}");
        }
    }

    #[test]
    fn values_are_validated_with_the_rules_of_the_questions() {
        assert!(err("[unlock]\nlevel = \"loose\"\n").contains("unlock.level"));
        assert!(err("[unlock]\npassphrase = \"short\"\n").contains("unlock.passphrase"));
        assert!(err("[user]\nname = \"root\"\n").contains("user.name"));
        assert!(err("[keyboard]\nlayout = \"it\"\ntimezone = \"Rome\"\n").contains("timezone"));
        assert!(err("[keyboard]\ntimezone = \"Europe/Rome\"\n").contains("layout is required"));
    }

    #[test]
    fn agent_keys_must_fit_the_backend() {
        assert!(err("[agent]\nprovider = \"openai\"\n").contains("backend is required"));
        assert!(err("[agent]\nbackend = \"api\"\n").contains("provider is required"));
        assert!(err("[agent]\nbackend = \"api\"\nprovider = \"x\"\n").contains("provider:"));
        assert!(
            err("[agent]\nbackend = \"cli\"\ntool = \"codex\"\nkey = \"k\"\n")
                .contains("does not apply")
        );
        assert!(err("[agent]\nbackend = \"cli\"\n").contains("tool is required"));
        let f = AnswerFile::parse("[agent]\nbackend = \"cli\"\ntool = \"codex\"\n").unwrap();
        assert_eq!(f.agent, Some(AgentAnswer::Cli(CliTool::Codex)));
        let f = AnswerFile::parse("[agent]\nbackend = \"api\"\nprovider = \"google\"\n").unwrap();
        assert!(matches!(f.agent, Some(AgentAnswer::Api { key: None, .. })));
    }

    #[test]
    fn syntax_errors_name_the_line_but_never_show_it() {
        let text = "[unlock]\npassphrase = \"Correct-Horse-9-Battery\n";
        let msg = err(text);
        assert!(msg.contains("line 2"), "{msg}");
        assert!(!msg.contains("Correct-Horse"), "{msg}");
    }
}
