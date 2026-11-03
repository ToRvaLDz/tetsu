//! The audit trail: native journald protocol, one datagram per entry.
//!
//! Values that hold a newline use the binary-safe length framing, so
//! untrusted text (arguments, the agent's reason) cannot add fields. A
//! failed write is an error the caller turns into a refusal.

use std::cell::{Cell, RefCell};
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use crate::intent::truncate;

/// The journal's native socket.
pub const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";
/// The `SYSLOG_IDENTIFIER` of every entry.
pub const IDENTIFIER: &str = "tetsu-capd";
/// Bytes of `TETSU_ARGS` kept.
pub const MAX_ARGS: usize = 4096;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A `Check` was decided.
    Check,
    /// The user was asked.
    Ask,
    /// The user approved.
    Approved,
    /// A call was denied by the user, or not decided.
    Denied,
    /// A grant was changed.
    Set,
    /// An ask was held back by a suppression.
    Suppressed,
}

impl Event {
    /// The fixed `MESSAGE_ID`, generated once with `systemd-id128 new`.
    pub fn message_id(self) -> &'static str {
        match self {
            Self::Check => "a8e00ec20cf94ae1b084275879e087f3",
            Self::Ask => "0c93ac66e468413085c727147fa6e6f3",
            Self::Approved => "72d2d069b1fb4df78c277def0b9de595",
            Self::Denied => "f019c8a2f4dc44c49d36ede619f40874",
            Self::Set => "b396d11fd3634da6be8cd49c2f5304c4",
            Self::Suppressed => "9bb5d2fd85eb473e8ef64e28d70395c4",
        }
    }

    /// A short name, for the message line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Ask => "ask",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Set => "set",
            Self::Suppressed => "suppressed",
        }
    }
}

/// One journal entry. Optional fields that are `None` are left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// What happened.
    pub event: Event,
    /// `TETSU_METHOD`.
    pub method: Option<String>,
    /// `TETSU_CLASS`.
    pub class: Option<String>,
    /// `TETSU_SCOPE`.
    pub scope: Option<String>,
    /// `TETSU_STATE`.
    pub state: Option<String>,
    /// `TETSU_LAYER`.
    pub layer: Option<String>,
    /// `TETSU_DECISION`.
    pub decision: Option<String>,
    /// `TETSU_DENIED_BY`.
    pub denied_by: Option<String>,
    /// `TETSU_CANCELLED`: the caller withdrew the call.
    pub cancelled: bool,
    /// `TETSU_PEER_PID`.
    pub peer_pid: Option<i64>,
    /// `TETSU_PEER_UNIT`.
    pub peer_unit: Option<String>,
    /// `TETSU_CALLER_UNIT`: the daemon that asked.
    pub caller_unit: Option<String>,
    /// `TETSU_ARGS`: JSON, cut to 4 KiB.
    pub args: Option<String>,
    /// `TETSU_REASON`: untrusted.
    pub reason: Option<String>,
    /// Further fields, for tests.
    pub extra: Vec<(String, String)>,
}

impl AuditEntry {
    /// An entry with only its event.
    pub fn new(event: Event) -> Self {
        Self {
            event,
            method: None,
            class: None,
            scope: None,
            state: None,
            layer: None,
            decision: None,
            denied_by: None,
            cancelled: false,
            peer_pid: None,
            peer_unit: None,
            caller_unit: None,
            args: None,
            reason: None,
            extra: Vec::new(),
        }
    }

    /// The datagram in the native journal protocol.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let event = self.event;
        field(&mut out, "SYSLOG_IDENTIFIER", IDENTIFIER);
        field(&mut out, "MESSAGE_ID", event.message_id());
        field(&mut out, "PRIORITY", "6");
        let message = match &self.class {
            Some(class) => format!("tetsu-capd: {} {class}", event.name()),
            None => format!("tetsu-capd: {}", event.name()),
        };
        field(&mut out, "MESSAGE", &message);
        let text = [
            ("TETSU_METHOD", &self.method),
            ("TETSU_CLASS", &self.class),
            ("TETSU_SCOPE", &self.scope),
            ("TETSU_STATE", &self.state),
            ("TETSU_LAYER", &self.layer),
            ("TETSU_DECISION", &self.decision),
            ("TETSU_DENIED_BY", &self.denied_by),
        ];
        for (key, value) in text {
            if let Some(value) = value {
                field(&mut out, key, value);
            }
        }
        if self.cancelled {
            field(&mut out, "TETSU_CANCELLED", "true");
        }
        if let Some(pid) = self.peer_pid {
            field(&mut out, "TETSU_PEER_PID", &pid.to_string());
        }
        let more = [
            ("TETSU_PEER_UNIT", &self.peer_unit),
            ("TETSU_CALLER_UNIT", &self.caller_unit),
        ];
        for (key, value) in more {
            if let Some(value) = value {
                field(&mut out, key, value);
            }
        }
        if let Some(args) = &self.args {
            field(&mut out, "TETSU_ARGS", &truncate(args, MAX_ARGS));
        }
        if let Some(reason) = &self.reason {
            field(&mut out, "TETSU_REASON", reason);
        }
        for (key, value) in &self.extra {
            field(&mut out, key, value);
        }
        out
    }
}

/// Appends one field. A value with a newline is length-framed.
fn field(out: &mut Vec<u8>, key: &str, value: &str) {
    out.extend_from_slice(key.as_bytes());
    if value.contains('\n') {
        out.push(b'\n');
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    } else {
        out.push(b'=');
    }
    out.extend_from_slice(value.as_bytes());
    out.push(b'\n');
}

/// Where entries go.
pub trait AuditSink {
    /// Writes one entry.
    fn record(&self, entry: &AuditEntry) -> io::Result<()>;
}

/// The journal, over a datagram socket.
#[derive(Debug)]
pub struct JournalSink {
    socket: UnixDatagram,
    path: PathBuf,
}

impl JournalSink {
    /// A sink sending to the journal socket at `path`.
    pub fn new(path: &Path) -> io::Result<Self> {
        Ok(Self {
            socket: UnixDatagram::unbound()?,
            path: path.to_owned(),
        })
    }
}

impl AuditSink for JournalSink {
    fn record(&self, entry: &AuditEntry) -> io::Result<()> {
        self.socket.send_to(&entry.encode(), &self.path).map(drop)
    }
}

/// A sink that keeps the entries, and can be told to fail.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct RecordingSink {
    /// The entries written so far.
    pub entries: RefCell<Vec<AuditEntry>>,
    /// While true, every write fails.
    pub fail: Cell<bool>,
}

impl AuditSink for RecordingSink {
    fn record(&self, entry: &AuditEntry) -> io::Result<()> {
        if self.fail.get() {
            return Err(io::Error::other("the journal is down"));
        }
        self.entries.borrow_mut().push(entry.clone());
        Ok(())
    }
}

impl<T: AuditSink + ?Sized> AuditSink for std::rc::Rc<T> {
    fn record(&self, entry: &AuditEntry) -> io::Result<()> {
        (**self).record(entry)
    }
}
