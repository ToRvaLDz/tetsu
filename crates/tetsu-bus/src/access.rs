//! The access policy of the facade (#28), as pure decisions.
//!
//! [`method_kind`] says which calls are gated, [`route`] says where a call
//! from a given peer goes, [`permission_denied`] names the refusal, and
//! [`log_line`] writes it down. Nothing here does I/O: [`crate::gate`]
//! applies the decisions.

use serde_json::json;

use crate::peer::Peer;
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

/// One log line for a refusal. The method comes from the caller, so control
/// characters are dropped and a long name is cut: the line cannot be forged
/// or inflated.
pub fn log_line(method: &str, peer: &Peer, why: &str) -> String {
    let method = clean(method);
    let method = if method.chars().count() > LOG_METHOD_LIMIT {
        let kept: String = method.chars().take(LOG_METHOD_LIMIT).collect();
        format!("{kept}{LOG_TRUNCATED}")
    } else {
        method
    };
    format!(
        "tetsu-bus: refused {method} from pid {} ({}): {why}",
        peer.pid,
        clean(&peer.sender_name())
    )
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
