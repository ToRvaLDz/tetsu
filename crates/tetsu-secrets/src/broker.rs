//! The broker: the vault behind its rules.
//!
//! Every operation is audited before it acts, and refused when the audit
//! cannot be written ("every access is in the journal"). Writes are
//! refused to the agent. A secret is used only where it is bound: the
//! exact host for a proxied request, a listed tool id for a spawn. A value
//! leaves only through [`Broker::use_secret`], to in-process code; no call
//! of the bus returns one.
//!
//! Usage counters live in memory, since the broker started; the journal is
//! the durable record.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tetsu_bus::secrets::SecretInfo;
use tetsu_bus::server::StreamItem;
use tokio::sync::mpsc;

use crate::audit::AuditSink;
use crate::caller::Caller;
use crate::model::{Host, KindSpec, SecretId, ToolId};
use crate::store::Store;
use crate::vault::Secret;

/// The time source, so tests control it.
pub trait Clock {
    /// Unix seconds.
    fn now_secs(&self) -> u64;
}

/// The wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
}

/// Where a value is about to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Use {
    /// Into a proxied request to this host.
    Proxy {
        /// The upstream host (`host[:port]`).
        upstream: String,
    },
    /// Into the environment of this tool.
    Spawn {
        /// The tool id.
        tool_id: String,
    },
}

/// A secret to store.
#[derive(Debug, Clone)]
pub struct NewSecret {
    /// The name.
    pub id: SecretId,
    /// How the value is presented.
    pub kind: KindSpec,
    /// The only host it may go to.
    pub host: Host,
    /// Tool ids that may receive it.
    pub allowed_tools: Vec<ToolId>,
    /// The value.
    pub value: Secret,
}

/// Why an operation was refused. No variant carries a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrokerError {
    /// No secret has this id.
    #[error("no secret `{0}`")]
    NotFound(SecretId),
    /// A secret has this id already.
    #[error("a secret `{0}` exists already")]
    Exists(SecretId),
    /// The caller may not do this.
    #[error("denied: {0}")]
    Denied(&'static str),
    /// The vault or the audit journal cannot be written; nothing changed.
    #[error("the vault or the audit journal is not available")]
    Unavailable,
}

/// The vault, its audit sink and its watchers.
pub struct Broker {
    store: Store,
    sink: Rc<dyn AuditSink>,
    clock: Rc<dyn Clock>,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    usage: HashMap<SecretId, Usage>,
    watchers: Vec<mpsc::UnboundedSender<StreamItem>>,
}

#[derive(Clone, Copy)]
struct Usage {
    count: u64,
    last: Option<u64>,
}

impl Broker {
    /// A broker over an open vault.
    pub fn new(store: Store, sink: Rc<dyn AuditSink>, clock: Rc<dyn Clock>) -> Self {
        let _ = (&store, &sink, &clock);
        Self {
            store,
            sink,
            clock,
            state: RefCell::new(State::default()),
        }
    }

    /// The secrets' bindings and counters.
    pub async fn list(&self, caller: &Caller) -> Result<Vec<SecretInfo>, BrokerError> {
        todo!("{caller:?}")
    }

    /// A stream of events: `initial` first.
    pub async fn watch(
        &self,
        caller: &Caller,
    ) -> Result<mpsc::UnboundedReceiver<StreamItem>, BrokerError> {
        todo!("{caller:?}")
    }

    /// Stores a new secret.
    pub async fn add(&self, caller: &Caller, new: NewSecret) -> Result<SecretInfo, BrokerError> {
        todo!("{caller:?} {}", new.id)
    }

    /// Replaces the value of a secret.
    pub async fn rotate(
        &self,
        caller: &Caller,
        id: &SecretId,
        value: Secret,
    ) -> Result<SecretInfo, BrokerError> {
        todo!("{caller:?} {id} {}", value.expose().len())
    }

    /// Deletes a secret.
    pub async fn remove(&self, caller: &Caller, id: &SecretId) -> Result<(), BrokerError> {
        todo!("{caller:?} {id}")
    }

    /// Releases a value for a use bound to where it is allowed to go.
    pub async fn use_secret(
        &self,
        caller: &Caller,
        id: &SecretId,
        usage: Use,
    ) -> Result<Secret, BrokerError> {
        todo!("{caller:?} {id} {usage:?}")
    }
}

#[cfg(test)]
mod tests;
