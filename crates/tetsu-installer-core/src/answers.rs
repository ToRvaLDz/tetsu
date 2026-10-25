//! Everything the installer collected, held in memory until the summary is
//! confirmed.

use crate::agent::AgentBackend;
use crate::disk::{DiskChoice, human_size};
use crate::keyboard::Locale;
use crate::unlock::Unlock;
use crate::user::{Password, UserAccount};

/// The steps that can be redone from the summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Keyboard,
    /// Language and timezone only.
    Regional,
    Disk,
    Unlock,
    User,
    Agent,
}

impl Step {
    pub const ALL: [Self; 6] = [
        Self::Keyboard,
        Self::Regional,
        Self::Disk,
        Self::Unlock,
        Self::User,
        Self::Agent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "Keyboard layout",
            Self::Regional => "Language and timezone",
            Self::Disk => "Disk",
            Self::Unlock => "Unlock level and passphrase",
            Self::User => "User",
            Self::Agent => "Agent backend",
        }
    }
}

/// All answers of a complete installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answers {
    pub locale: Locale,
    pub disk: DiskChoice,
    pub unlock: Unlock,
    pub user: UserAccount,
    /// `tetsu-<4 random chars>`; never asked.
    pub hostname: String,
    pub agent: Option<AgentBackend>,
}

impl Answers {
    /// One line per answer for the summary, without any secret.
    pub fn summary_lines(&self) -> Vec<String> {
        let l = &self.locale;
        let d = &self.disk;
        let model = d.model.as_deref().unwrap_or("unknown model");
        let wiped =
            d.os.as_deref()
                .map_or_else(String::new, |o| format!(", holds {o}"));
        let password = match self.user.password {
            Password::Plain(_) => "set",
            Password::Hash(_) => "set (hash provided)",
        };
        vec![
            format!("Keyboard:  {}", l.keyboard),
            format!("Language:  {}", l.language),
            format!("Timezone:  {} (hardware clock in UTC)", l.timezone),
            format!(
                "Disk:      {} ({model}, {}{wiped}), will be wiped",
                d.name,
                human_size(d.size)
            ),
            format!(
                "Unlock:    {:?}, passphrase set, recovery key not stored",
                self.unlock.level
            ),
            format!(
                "User:      {}{}, password {password}, root locked, member of wheel",
                self.user.username,
                self.user
                    .full_name
                    .as_deref()
                    .map_or_else(String::new, |n| format!(" ({n})")),
            ),
            format!("Hostname:  {}", self.hostname),
            format!(
                "Agent:     {}",
                self.agent
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), AgentBackend::label)
            ),
        ]
    }
}
