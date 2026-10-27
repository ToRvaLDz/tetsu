//! Who is asking: the facts of a peer that the audit journal records.

use std::fs;

use tetsu_bus::peer::{Peer, unit_name};

/// A caller of the vault, as the audit trail names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The systemd unit the process runs in, when its cgroup names one.
    pub unit: Option<String>,
    /// The process id.
    pub pid: i32,
    /// The executable, when `/proc` says.
    pub exe: Option<String>,
    /// The process name.
    pub comm: String,
    /// Whether the process is the agent or one of its children.
    pub agent: bool,
}

impl Caller {
    /// The caller behind a connection, from its socket credentials.
    pub fn from_peer(peer: &Peer) -> Self {
        Self {
            unit: unit_name(&peer.cgroup).map(str::to_owned),
            pid: peer.pid,
            exe: exe_of(peer.pid),
            comm: peer.comm.clone(),
            agent: peer.agent,
        }
    }

    /// The broker itself, for what it does on its own (the staged-key
    /// import, the unlock).
    pub fn this_process() -> Self {
        let pid = i32::try_from(std::process::id()).unwrap_or(0);
        Self {
            unit: None,
            pid,
            exe: exe_of(pid),
            comm: "tetsu-secrets".to_owned(),
            agent: false,
        }
    }

    /// A short name for messages: the unit, else the process name.
    pub fn display_name(&self) -> &str {
        self.unit.as_deref().unwrap_or(&self.comm)
    }
}

fn exe_of(pid: i32) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
