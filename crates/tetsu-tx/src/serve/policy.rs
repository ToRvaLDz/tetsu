//! Who may call a write method.

use tetsu_bus::peer::Peer;

/// Decides each write call. Async, so a capability daemon or polkit can
/// answer later.
pub trait Policy: 'static {
    /// Whether `peer` may use the capability `class` (`system.update`...).
    fn allows(&self, peer: &Peer, class: &str) -> impl Future<Output = bool>;
}

/// The interim policy: only a root process outside the agent's slice.
/// Everything else is denied, as when the capability daemon is down.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalRoot;

impl Policy for LocalRoot {
    async fn allows(&self, peer: &Peer, class: &str) -> bool {
        // Every write class gets the same answer for now.
        let _ = class;
        peer.uid == 0 && !peer.agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(uid: u32, agent: bool) -> Peer {
        Peer {
            uid,
            gid: uid,
            pid: 1234,
            cgroup: if agent {
                "/user.slice/tetsu-agent.slice/tetsu-agent.service".into()
            } else {
                "/user.slice/session-1.scope".into()
            },
            agent,
            comm: "test".into(),
        }
    }

    fn allowed(peer: &Peer, class: &str) -> bool {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(LocalRoot.allows(peer, class))
    }

    #[test]
    fn root_outside_the_agent_may_write() {
        for class in ["system.update", "packages.install", "system.rollback"] {
            assert!(allowed(&peer(0, false), class), "{class}");
        }
    }

    #[test]
    fn a_root_agent_may_not_write() {
        assert!(!allowed(&peer(0, true), "system.update"));
    }

    #[test]
    fn an_ordinary_user_may_not_write() {
        assert!(!allowed(&peer(1000, false), "system.update"));
        assert!(!allowed(&peer(1000, true), "system.update"));
    }
}
