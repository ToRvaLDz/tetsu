//! `org.tetsu.cap.v1` on the facade server.

use std::rc::Rc;

use tetsu_bus::peer::Peer;
use tetsu_bus::server::{Handler, Request, Response};

use crate::ask::NotifyChannel;
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::engine::Engine;

/// How `Check` callers are authorised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckAuth {
    /// By unit and uid, see `auth`.
    Units,
    /// Everyone; for tests only.
    #[doc(hidden)]
    AllowAll,
}

/// Serves `org.tetsu.cap.v1` and the `org.varlink.service` basics.
pub struct CapService<N, C, A> {
    engine: Rc<Engine<N, C, A>>,
    own_uid: u32,
    check_auth: CheckAuth,
}

impl<N, C, A> CapService<N, C, A> {
    /// A service over an engine, run by the user `own_uid`.
    pub fn new(engine: Rc<Engine<N, C, A>>, own_uid: u32) -> Self {
        Self {
            engine,
            own_uid,
            check_auth: CheckAuth::Units,
        }
    }

    /// A service that lets any peer call `Check`. For tests; `main` never
    /// calls it.
    #[doc(hidden)]
    pub fn allow_all_checks(engine: Rc<Engine<N, C, A>>, own_uid: u32) -> Self {
        Self {
            check_auth: CheckAuth::AllowAll,
            ..Self::new(engine, own_uid)
        }
    }

    /// The engine.
    pub fn engine(&self) -> &Rc<Engine<N, C, A>> {
        &self.engine
    }
}

impl<N, C, A> Handler for CapService<N, C, A>
where
    N: NotifyChannel + 'static,
    C: Clock + 'static,
    A: AuditSink + 'static,
{
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        let _ = (peer, request, self.own_uid, self.check_auth);
        unimplemented!()
    }
}
