//! The engine: decides every `Check`, keeps the counters and suppressions,
//! writes the audit trail and tells the watchers.

use std::marker::PhantomData;
use std::time::Duration;

use tokio::sync::mpsc;

use tetsu_bus::cap::{
    Capability, CapabilityEvent, CheckParams, CheckReply, IntentParams, SetParams,
};

use crate::ask::NotifyChannel;
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::store::Store;

/// Why a call is refused with an error rather than a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapError {
    /// No method of the contract has this name.
    UnknownMethod(String),
    /// No capability has this class.
    UnknownClass(String),
    /// The user file sets this class.
    OverriddenByUser(String),
    /// A parameter is wrong, or a layer file is malformed.
    InvalidParameter(&'static str),
    /// The journal or the store could not be written.
    Unavailable,
}

/// The state of the daemon.
pub struct Engine<N, C, A> {
    _marker: PhantomData<(N, C, A)>,
}

impl<N: NotifyChannel, C: Clock, A: AuditSink> Engine<N, C, A> {
    /// An engine over a store.
    pub fn new(notify: N, clock: C, audit: A, store: Store, backoff: Vec<Duration>) -> Self {
        let _ = (notify, clock, audit, store, backoff);
        unimplemented!()
    }

    /// Decides a call of the agent.
    pub async fn check(
        &self,
        caller_unit: &str,
        params: &CheckParams,
    ) -> Result<CheckReply, CapError> {
        let _ = (caller_unit, params);
        unimplemented!()
    }

    /// Sets or removes a managed value.
    pub fn set(&self, caller_unit: &str, params: &SetParams) -> Result<(), CapError> {
        let _ = (caller_unit, params);
        unimplemented!()
    }

    /// Records why the peer `pid` is about to call a method.
    pub fn intent(&self, pid: i64, params: &IntentParams) -> Result<(), CapError> {
        let _ = (pid, params);
        unimplemented!()
    }

    /// Every capability.
    pub fn list(&self) -> Vec<Capability> {
        unimplemented!()
    }

    /// The initial event and the stream of changes.
    pub fn watch(&self) -> (CapabilityEvent, mpsc::UnboundedReceiver<CapabilityEvent>) {
        unimplemented!()
    }

    /// Sends the grouped notices whose window has ended.
    pub async fn flush_notices(&self) {
        unimplemented!()
    }
}
