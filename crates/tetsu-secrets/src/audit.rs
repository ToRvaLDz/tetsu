//! The audit trail: one journal entry per access, over the native
//! journald protocol.
//!
//! An entry holds ids and names, never a value: [`AuditEntry`] has no
//! field that could carry one. Entries are tagged
//! `SYSLOG_IDENTIFIER=tetsu-secrets` and carry a fixed `MESSAGE_ID` per
//! action, so `journalctl --user -t tetsu-secrets` and
//! `MESSAGE_ID=<id>` find them.

use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

use crate::caller::Caller;

/// Where journald listens for native-protocol datagrams.
pub const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";
/// The syslog identifier of every entry.
pub const IDENTIFIER: &str = "tetsu-secrets";

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A secret was added.
    Add,
    /// A secret was removed.
    Remove,
    /// A secret's value was replaced.
    Rotate,
    /// The value went into a proxied request.
    Use,
    /// The value went into a spawned tool's environment.
    Spawn,
    /// A request was refused.
    Denied,
    /// The vault was opened with its key.
    Unlock,
    /// The secrets were listed.
    List,
}

impl Action {
    /// Every action.
    pub const ALL: [Action; 8] = [
        Self::Add,
        Self::Remove,
        Self::Rotate,
        Self::Use,
        Self::Spawn,
        Self::Denied,
        Self::Unlock,
        Self::List,
    ];

    /// The word in `TETSU_ACTION`.
    pub fn as_str(self) -> &'static str {
        todo!("{self:?}")
    }

    /// The fixed `MESSAGE_ID`, 32 lowercase hex digits.
    pub fn message_id(self) -> &'static str {
        todo!("{self:?}")
    }
}

/// How it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It happened.
    Ok,
    /// It was tried and failed.
    Failed,
    /// It was refused.
    Denied,
}

impl Outcome {
    /// The word in `TETSU_OUTCOME`.
    pub fn as_str(self) -> &'static str {
        todo!("{self:?}")
    }
}

/// One audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// What happened.
    pub action: Action,
    /// How it ended.
    pub outcome: Outcome,
    /// Who asked.
    pub caller: Caller,
    /// A spawn: the tool id.
    pub tool_id: Option<String>,
    /// The secret concerned.
    pub secret_id: Option<String>,
    /// A proxied use: the upstream host.
    pub upstream: Option<String>,
    /// A proxied use: the HTTP status the upstream answered.
    pub http_status: Option<u16>,
    /// Why, for a refusal or a failure. Never a value.
    pub reason: Option<String>,
}

impl AuditEntry {
    /// An entry with no details.
    pub fn new(action: Action, outcome: Outcome, caller: Caller) -> Self {
        Self {
            action,
            outcome,
            caller,
            tool_id: None,
            secret_id: None,
            upstream: None,
            http_status: None,
            reason: None,
        }
    }
}

/// The datagram for `entry`, in the journald native protocol.
pub fn encode(entry: &AuditEntry) -> Vec<u8> {
    todo!("{entry:?}")
}

/// Where audit entries go.
pub trait AuditSink {
    /// Records one entry; an error means it is not in the journal.
    fn record(&self, entry: &AuditEntry) -> io::Result<()>;
}

/// Writes entries to the journal socket.
#[derive(Debug, Clone)]
pub struct JournalSink {
    socket: PathBuf,
}

impl JournalSink {
    /// A sink sending to the datagram socket at `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }
}

impl AuditSink for JournalSink {
    fn record(&self, entry: &AuditEntry) -> io::Result<()> {
        todo!("{:?} {entry:?}", self.socket)
    }
}

#[cfg(test)]
mod tests;
