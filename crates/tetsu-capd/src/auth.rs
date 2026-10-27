//! Who may call what, decided from the connecting peer's credentials only
//! (#28).
//!
//! `Check` is for Tetsu daemons: tetsu-tx as root in its system unit, and
//! the session daemons of the same user. `Set` is for anyone but the agent.
//! Everything else is open to whoever can reach the 0600 socket.

use tetsu_bus::peer::{Peer, unit_name};

/// The cgroup of the system daemon that calls `Check`.
pub const TX_CGROUP: &str = "/system.slice/tetsu-tx.service";
/// The session units that may call `Check`.
pub const SESSION_UNITS: [&str; 3] = [
    "tetsu-comp.service",
    "tetsu-notifyd.service",
    "tetsu-secrets.service",
];

/// A method with its own rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    /// `Check`.
    Check,
    /// `Set`.
    Set,
}

/// Whether `peer` may make the call. `own_uid` is the user capd runs as.
pub fn allowed(peer: &Peer, own_uid: u32, call: Call) -> bool {
    let _ = (peer, own_uid, call);
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(uid: u32, cgroup: &str, agent: bool) -> Peer {
        Peer {
            uid,
            gid: uid,
            pid: 4242,
            cgroup: cgroup.to_owned(),
            agent,
            comm: "x".into(),
        }
    }

    const OWN: u32 = 1000;

    fn session(unit: &str) -> String {
        format!("/user.slice/user-1000.slice/user@1000.service/session.slice/{unit}")
    }

    #[test]
    fn tetsu_tx_is_allowed_as_root_in_its_system_unit() {
        assert!(allowed(&peer(0, TX_CGROUP, false), OWN, Call::Check));
    }

    #[test]
    fn tetsu_tx_is_not_allowed_as_a_user_or_from_another_cgroup() {
        assert!(!allowed(&peer(1000, TX_CGROUP, false), OWN, Call::Check));
        assert!(!allowed(
            &peer(
                0,
                "/user.slice/user-0.slice/user@0.service/tetsu-tx.service",
                false
            ),
            OWN,
            Call::Check
        ));
        assert!(!allowed(
            &peer(0, "/system.slice/other.service", false),
            OWN,
            Call::Check
        ));
        assert!(!allowed(
            &peer(0, "/system.slice/tetsu-tx.service/child.scope", false),
            OWN,
            Call::Check
        ));
    }

    #[test]
    fn session_daemons_of_the_same_user_are_allowed() {
        for unit in SESSION_UNITS {
            assert!(
                allowed(&peer(OWN, &session(unit), false), OWN, Call::Check),
                "{unit}"
            );
        }
    }

    #[test]
    fn a_session_daemon_of_another_uid_is_denied() {
        let other =
            "/user.slice/user-1001.slice/user@1001.service/session.slice/tetsu-comp.service";
        assert!(!allowed(&peer(1001, other, false), OWN, Call::Check));
        // The right uid but the cgroup of someone else's manager.
        assert!(!allowed(&peer(OWN, other, false), OWN, Call::Check));
    }

    #[test]
    fn an_agent_peer_is_denied_even_if_it_looks_like_a_session_daemon() {
        let cgroup = session("tetsu-comp.service");
        assert!(!allowed(&peer(OWN, &cgroup, true), OWN, Call::Check));
    }

    #[test]
    fn other_units_and_a_plain_shell_are_denied() {
        assert!(!allowed(
            &peer(OWN, &session("tetsu-other.service"), false),
            OWN,
            Call::Check
        ));
        assert!(!allowed(
            &peer(
                OWN,
                "/user.slice/user-1000.slice/user@1000.service/app.slice/foot.scope",
                false
            ),
            OWN,
            Call::Check
        ));
        assert!(!allowed(
            &peer(OWN, "/user.slice/user-1000.slice/session-2.scope", false),
            OWN,
            Call::Check
        ));
        // A child scope under a daemon's unit is not the daemon.
        let child = format!("{}/run-1.scope", session("tetsu-comp.service"));
        assert!(!allowed(&peer(OWN, &child, false), OWN, Call::Check));
    }

    #[test]
    fn set_is_denied_for_the_agent_and_allowed_for_anyone_else() {
        let shell = "/user.slice/user-1000.slice/session-2.scope";
        assert!(allowed(&peer(OWN, shell, false), OWN, Call::Set));
        assert!(allowed(
            &peer(OWN, &session("tetsu-comp.service"), false),
            OWN,
            Call::Set
        ));
        let agent =
            "/user.slice/user-1000.slice/user@1000.service/tetsu-agent.slice/tetsu-agent.service";
        assert!(!allowed(&peer(OWN, agent, true), OWN, Call::Set));
    }
}
