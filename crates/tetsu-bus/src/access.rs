//! The access policy of the facade (#28), as pure decisions.
//!
//! [`method_kind`] says which calls are gated, [`route`] says where a call
//! from a given peer goes, [`permission_denied`] names the refusal, and
//! [`log_line`] writes it down. Nothing here does I/O: [`crate::gate`]
//! applies the decisions.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use crate::peer::{Peer, is_agent_cgroup, unit_name};
use crate::server::ErrorReply;

/// The longest method name written to a log line, in characters.
const LOG_METHOD_LIMIT: usize = 200;
/// What a truncated method name ends with.
const LOG_TRUNCATED: &str = "...";
/// Method name prefixes of reads; the prefix must end a word, as in the
/// contract suite (`ListItems` is a read, `Listen` is not).
const READ_PREFIXES: [&str; 3] = ["List", "Get", "Watch"];
/// The interface of the errors every Varlink service answers.
const VARLINK_SERVICE: &str = "org.varlink.service";

/// Whether a call is gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// Open to any local peer: the Varlink service methods and the reads.
    Free,
    /// Everything else, unknown names included.
    Gated,
}

/// Which kind of service the gate stands in front of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// A per-user service: only its owner (and root) may call.
    Session,
    /// A system service: any local user may connect, each call is decided.
    System,
}

/// What the gate does with a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Straight to the server.
    Pass,
    /// Refused.
    Deny,
    /// Asked to `tetsu-capd` of this user.
    AskCapd {
        /// Whose capability service decides.
        uid: u32,
    },
    /// Asked to the system authority (polkit).
    AskAuthority,
}

/// Whether `method` (fully qualified) is free or gated. A name rule, not a
/// catalog: anything that is not recognised as a read is gated.
pub fn method_kind(method: &str) -> MethodKind {
    let Some((interface, name)) = method.rsplit_once('.') else {
        return MethodKind::Gated;
    };
    if interface == VARLINK_SERVICE {
        return MethodKind::Free;
    }
    let read = !interface.is_empty()
        && READ_PREFIXES.iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_uppercase))
        });
    if read {
        MethodKind::Free
    } else {
        MethodKind::Gated
    }
}

/// Where a call goes. `own_uid` is the owner of a session service (the
/// effective uid of the daemon); `in_wheel` is whether the peer's user is
/// in `wheel`, and matters only for an agent on a system service.
pub fn route(scope: Scope, peer: &Peer, kind: MethodKind, own_uid: u32, in_wheel: bool) -> Route {
    match scope {
        Scope::Session => {
            if peer.uid != own_uid && peer.uid != 0 {
                Route::Deny
            } else if kind == MethodKind::Free {
                Route::Pass
            } else if peer.agent {
                Route::AskCapd { uid: own_uid }
            } else {
                Route::Pass
            }
        }
        Scope::System => {
            if kind == MethodKind::Free {
                Route::Pass
            } else if peer.agent {
                if in_wheel {
                    Route::AskCapd { uid: peer.uid }
                } else {
                    Route::Deny
                }
            } else {
                Route::AskAuthority
            }
        }
    }
}

/// The refusal of `method`: `<interface>.PermissionDenied`, which every
/// Tetsu interface declares.
pub fn permission_denied(method: &str) -> ErrorReply {
    let interface = method
        .rsplit_once('.')
        .map(|(interface, _)| interface)
        .filter(|interface| !interface.is_empty())
        .unwrap_or(VARLINK_SERVICE);
    ErrorReply::new(&format!("{interface}.PermissionDenied"), json!({}))
}

/// One log line for a refusal. The method comes from the caller, and the
/// reason may quote what a server answered, so control characters are
/// dropped and a long method name is cut: the line cannot be forged or
/// inflated.
pub fn log_line(method: &str, peer: &Peer, why: &str) -> String {
    let method = clean(method);
    let method = if method.chars().count() > LOG_METHOD_LIMIT {
        let kept: String = method.chars().take(LOG_METHOD_LIMIT).collect();
        format!("{kept}{LOG_TRUNCATED}")
    } else {
        method
    };
    format!(
        "tetsu-bus: refused {method} from pid {} ({}): {}",
        peer.pid,
        clean(&peer.sender_name()),
        clean(why)
    )
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// The group whose members may use `tetsu-capd` as the system authority for
/// an agent (#28 item 5).
const WHEEL: &str = "wheel";

/// Where a user's capability service listens, in both scopes. Fixed: never
/// read from the environment or the registry.
pub fn capd_path(uid: u32) -> PathBuf {
    PathBuf::from(format!("/run/user/{uid}/tetsu/org.tetsu.cap.v1"))
}

/// Whether the user `uid` is in `wheel`: the primary group of the user is
/// `wheel`, or the name of the user is in its member list. `passwd` and
/// `group` are the text of the two files. Malformed lines are skipped; a
/// missing `wheel` or an unknown uid is not a membership.
pub fn in_wheel(passwd: &str, group: &str, uid: u32) -> bool {
    let Some((name, primary_gid)) = passwd.lines().find_map(|line| passwd_entry(line, uid)) else {
        return false;
    };
    let Some((wheel_gid, members)) = group.lines().find_map(wheel_entry) else {
        return false;
    };
    primary_gid == wheel_gid || members.contains(&name)
}

/// `name:x:uid:gid:...` -> (name, gid), when the line is for `uid`.
fn passwd_entry(line: &str, uid: u32) -> Option<(&str, u32)> {
    let mut fields = line.split(':');
    let name = fields.next().filter(|name| !name.is_empty())?;
    let _password = fields.next()?;
    let entry_uid: u32 = fields.next()?.parse().ok()?;
    let gid: u32 = fields.next()?.parse().ok()?;
    (entry_uid == uid).then_some((name, gid))
}

/// `wheel:x:gid:a,b` -> (gid, members).
fn wheel_entry(line: &str) -> Option<(u32, Vec<&str>)> {
    let mut fields = line.split(':');
    if fields.next()? != WHEEL {
        return None;
    }
    let _password = fields.next()?;
    let gid: u32 = fields.next()?.parse().ok()?;
    let members = fields.next()?;
    Some((gid, members.split(',').filter(|m| !m.is_empty()).collect()))
}

/// Where the gate learns who is in `wheel`.
pub trait Groups {
    /// Whether the user is in `wheel`. Anything unreadable is `false`.
    fn in_wheel(&self, uid: u32) -> bool;
}

/// `wheel` from `/etc/passwd` and `/etc/group` under a root, read on every
/// call. Accounts known only to other NSS sources are not members.
#[derive(Debug, Clone)]
pub struct EtcFiles {
    root: PathBuf,
}

impl EtcFiles {
    /// Reads `<root>/etc/passwd` and `<root>/etc/group`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.root.join("etc").join(name)).ok()
    }
}

impl Groups for EtcFiles {
    fn in_wheel(&self, uid: u32) -> bool {
        match (self.read("passwd"), self.read("group")) {
            (Some(passwd), Some(group)) => in_wheel(&passwd, &group, uid),
            _ => false,
        }
    }
}

/// Who must be on the other end of a connection the gate opens itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedServer {
    /// A service unit of a user, running under that user's manager.
    UserUnit {
        /// The user.
        uid: u32,
        /// The unit, e.g. `tetsu-capd.service`.
        unit: String,
    },
    /// Exactly this cgroup. For tests, where the server is the test itself.
    Cgroup(String),
}

impl ExpectedServer {
    /// Whether `server`, resolved from the connection, is the expected one.
    pub fn matches(&self, server: &Peer) -> bool {
        match self {
            Self::Cgroup(cgroup) => server.cgroup == *cgroup,
            Self::UserUnit { uid, unit } => {
                let manager = format!("/user.slice/user-{uid}.slice/user@{uid}.service/");
                server.uid == *uid
                    && !server.agent
                    && !is_agent_cgroup(&server.cgroup)
                    && server.cgroup.starts_with(&manager)
                    && !server
                        .cgroup
                        .split('/')
                        .any(|segment| segment == "." || segment == "..")
                    && unit_name(&server.cgroup) == Some(unit.as_str())
            }
        }
    }
}
