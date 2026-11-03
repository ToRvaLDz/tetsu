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
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Rotate => "rotate",
            Self::Use => "use",
            Self::Spawn => "spawn",
            Self::Denied => "denied",
            Self::Unlock => "unlock",
            Self::List => "list",
        }
    }

    /// The fixed `MESSAGE_ID`, 32 lowercase hex digits.
    pub fn message_id(self) -> &'static str {
        match self {
            Self::Add => "bb176da85ff448caa1bf6e30e9009771",
            Self::Remove => "ad4bdb81f820452ea46ad6af6be9b375",
            Self::Rotate => "13c81a1765904ea8a69bb9f6c01121dc",
            Self::Use => "68c76ae3a6ca41bcb400a02577d6333a",
            Self::Spawn => "2d5ba796f6ba4dab8cc94ffff6286059",
            Self::Denied => "9c106d5d17a64daf8ea6720fee2c263d",
            Self::Unlock => "4eeae7f4e8d643ff954516bec096a73a",
            Self::List => "bdf1a7cce232451a84a06ea5cd4eb3f3",
        }
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
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Denied => "denied",
        }
    }

    /// The syslog priority: info, warning, error.
    fn priority(self) -> u8 {
        match self {
            Self::Ok => 6,
            Self::Denied => 4,
            Self::Failed => 3,
        }
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
    let mut out = Vec::with_capacity(512);
    let caller = &entry.caller;
    field(&mut out, "MESSAGE", &message(entry));
    field(&mut out, "MESSAGE_ID", entry.action.message_id());
    field(&mut out, "PRIORITY", &entry.outcome.priority().to_string());
    field(&mut out, "SYSLOG_IDENTIFIER", IDENTIFIER);
    field(&mut out, "TETSU_ACTION", entry.action.as_str());
    field(&mut out, "TETSU_OUTCOME", entry.outcome.as_str());
    optional(&mut out, "TETSU_CALLER_UNIT", caller.unit.as_deref());
    field(&mut out, "TETSU_CALLER_PID", &caller.pid.to_string());
    optional(&mut out, "TETSU_CALLER_EXE", caller.exe.as_deref());
    field(&mut out, "TETSU_CALLER_COMM", &caller.comm);
    optional(&mut out, "TETSU_TOOL_ID", entry.tool_id.as_deref());
    optional(&mut out, "TETSU_SECRET_ID", entry.secret_id.as_deref());
    optional(&mut out, "TETSU_UPSTREAM", entry.upstream.as_deref());
    let status = entry.http_status.map(|s| s.to_string());
    optional(&mut out, "TETSU_HTTP_STATUS", status.as_deref());
    optional(&mut out, "TETSU_REASON", entry.reason.as_deref());
    out
}

/// The human line: `add secret demo: ok (unit, pid 1)`.
fn message(entry: &AuditEntry) -> String {
    let what = match &entry.secret_id {
        Some(id) => format!("{} secret {id}", entry.action.as_str()),
        None => entry.action.as_str().to_owned(),
    };
    let caller = &entry.caller;
    format!(
        "{what}: {} ({}, pid {})",
        entry.outcome.as_str(),
        caller.display_name(),
        caller.pid
    )
}

fn optional(out: &mut Vec<u8>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        field(out, name, value);
    }
}

/// One field: `NAME=value\n`, or for a value with a newline `NAME\n`, the
/// length as 64 bits little-endian, the value and `\n`, so a value can
/// never start a field of its own.
fn field(out: &mut Vec<u8>, name: &str, value: &str) {
    out.extend_from_slice(name.as_bytes());
    if value.contains('\n') {
        out.push(b'\n');
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    } else {
        out.push(b'=');
    }
    out.extend_from_slice(value.as_bytes());
    out.push(b'\n');
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
        let socket = UnixDatagram::unbound()?;
        // Never block the broker's one thread on a full journal buffer: the
        // send then fails (`WouldBlock`) and the operation fails closed.
        socket.set_nonblocking(true)?;
        socket.send_to(&encode(entry), &self.socket).map(|_| ())
    }
}

#[cfg(test)]
mod tests;
