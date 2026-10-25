//! Peer identity, per the bus access policy (#28): uid and gid from
//! `SO_PEERCRED`, the cgroup from `SO_PEERPIDFD`, resolved once at connect.
//! Anything that cannot be read fails closed.

use std::fs;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

/// The slice every agent process lives under, the agent unit and its
/// transient children alike.
const AGENT_SLICE: &str = "tetsu-agent.slice";

/// Who is on the other end of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Unix user id.
    pub uid: u32,
    /// Primary Unix group id.
    pub gid: u32,
    /// Process id, pinned by the pidfd at connect.
    pub pid: i32,
    /// The unified cgroup path of the process.
    pub cgroup: String,
    /// Whether the process is the agent or one of its children.
    pub agent: bool,
    /// The process name (`comm`), for display only.
    pub comm: String,
}

impl Peer {
    /// Resolves a peer from its socket credentials. The cgroup is read
    /// through `/proc/<pid>` and then re-checked against the pidfd, so a
    /// recycled pid is never mistaken for the peer.
    pub fn resolve(uid: u32, gid: u32, pid: i32, pidfd: BorrowedFd<'_>) -> io::Result<Self> {
        let pinned = pidfd_pid(pidfd)?;
        if pinned != pid {
            return Err(denied("the pidfd does not match the peer pid"));
        }
        let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
        let cgroup = parse_proc_cgroup(&cgroup).ok_or_else(|| denied("no unified cgroup"))?;
        let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim_end().to_owned())
            .unwrap_or_default();
        // The process may have exited and its pid been reused while we read.
        if pidfd_pid(pidfd)? != pid {
            return Err(denied("the peer exited during identification"));
        }
        let agent = is_agent_cgroup(&cgroup);
        Ok(Self {
            uid,
            gid,
            pid,
            cgroup,
            agent,
            comm,
        })
    }

    /// A name for the sender, derived from the credentials only: the unit
    /// when the cgroup names one, else the process name.
    pub fn sender_name(&self) -> String {
        match unit_name(&self.cgroup) {
            Some(unit) if !unit.starts_with("user@") => unit.to_owned(),
            _ if !self.comm.is_empty() => self.comm.clone(),
            _ => format!("pid {}", self.pid),
        }
    }
}

fn denied(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, why.to_owned())
}

fn pidfd_pid(pidfd: BorrowedFd<'_>) -> io::Result<i32> {
    let info = fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd()))?;
    parse_fdinfo_pid(&info).ok_or_else(|| denied("the peer process is gone"))
}

/// Whether a cgroup path belongs to the agent: any segment is the agent
/// slice or a slice nested in it. Matching anywhere is deliberately strict,
/// since an agent peer only loses rights.
pub fn is_agent_cgroup(path: &str) -> bool {
    path.split('/').any(|segment| segment == AGENT_SLICE)
}

/// The unified (cgroup v2) path from `/proc/<pid>/cgroup`, if absolute.
pub fn parse_proc_cgroup(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .filter(|path| path.starts_with('/'))
        .map(str::to_owned)
}

/// The pid a pidfd refers to, from its `fdinfo`; `None` once it exited.
pub fn parse_fdinfo_pid(content: &str) -> Option<i32> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("Pid:"))
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|pid| *pid > 0)
}

/// The innermost `.service` or `.scope` unit of a cgroup path.
pub fn unit_name(path: &str) -> Option<&str> {
    path.rsplit('/')
        .find(|segment| segment.ends_with(".service") || segment.ends_with(".scope"))
}
