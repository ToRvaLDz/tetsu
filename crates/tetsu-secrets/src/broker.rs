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

use serde_json::json;
use tetsu_bus::secrets::{SecretEvent, SecretInfo};
use tetsu_bus::server::StreamItem;
use tetsu_tx::serve::convert::rfc3339_ms;
use tokio::sync::mpsc;

use crate::audit::{Action, AuditEntry, AuditSink, Outcome};
use crate::caller::Caller;
use crate::model::{Host, KindSpec, SecretId, SecretMeta, ToolId};
use crate::store::{Store, StoreError};
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

const AGENT_DENIED: &str = "the agent unit may not change secrets";
const HOST_DENIED: &str = "the secret is not bound to this host";
const TOOL_DENIED: &str = "the secret is not allowed for this tool";

impl Broker {
    /// A broker over an open vault.
    pub fn new(store: Store, sink: Rc<dyn AuditSink>, clock: Rc<dyn Clock>) -> Self {
        Self {
            store,
            sink,
            clock,
            state: RefCell::new(State::default()),
        }
    }

    /// Writes an audit entry; without it the operation does not happen.
    fn audit(&self, entry: &AuditEntry) -> Result<(), BrokerError> {
        self.sink
            .record(entry)
            .map_err(|_| BrokerError::Unavailable)
    }

    /// Records an operation that was tried and failed in the vault. The
    /// operation is already refused, so a journal error changes nothing.
    fn audit_failure(&self, mut entry: AuditEntry, reason: &str) {
        entry.outcome = Outcome::Failed;
        entry.reason = Some(reason.to_owned());
        let _ = self.sink.record(&entry);
    }

    /// The info of a secret: its binding and the in-memory counters.
    fn info(&self, meta: &SecretMeta) -> SecretInfo {
        let usage = self.state.borrow().usage.get(&meta.id).copied();
        let (kind, header_name) = meta.kind.to_wire();
        SecretInfo {
            id: meta.id.to_string(),
            kind,
            header_name,
            host: meta.host.to_string(),
            allowed_tools: meta.allowed_tools.iter().map(ToString::to_string).collect(),
            created_at: rfc3339_ms(meta.created_at),
            rotated_at: meta.rotated_at.map(rfc3339_ms),
            last_used_at: usage.and_then(|u| u.last).map(rfc3339_ms),
            use_count: usage.map_or(0, |u| i64::try_from(u.count).unwrap_or(i64::MAX)),
        }
    }

    /// Sends an event to every watcher still listening.
    fn emit(&self, event: &SecretEvent) {
        let item = StreamItem::Reply(json!({ "event": event }));
        self.state
            .borrow_mut()
            .watchers
            .retain(|w| w.send(item.clone()).is_ok());
    }

    /// Refuses the agent a write, and audits the refusal.
    fn refuse_agent(&self, caller: &Caller, id: &SecretId) -> Result<(), BrokerError> {
        if !caller.agent {
            return Ok(());
        }
        let mut entry = AuditEntry::new(Action::Denied, Outcome::Denied, caller.clone());
        entry.secret_id = Some(id.to_string());
        entry.reason = Some(AGENT_DENIED.to_owned());
        let _ = self.sink.record(&entry);
        Err(BrokerError::Denied(AGENT_DENIED))
    }

    async fn metas(&self, entry: &AuditEntry) -> Result<Vec<SecretMeta>, BrokerError> {
        self.store.list().await.map_err(|e| {
            self.audit_failure(entry.clone(), &e.to_string());
            BrokerError::Unavailable
        })
    }

    /// The secrets' bindings and counters.
    pub async fn list(&self, caller: &Caller) -> Result<Vec<SecretInfo>, BrokerError> {
        let entry = AuditEntry::new(Action::List, Outcome::Ok, caller.clone());
        self.audit(&entry)?;
        let metas = self.metas(&entry).await?;
        Ok(metas.iter().map(|m| self.info(m)).collect())
    }

    /// A stream of events: `initial` first.
    pub async fn watch(
        &self,
        caller: &Caller,
    ) -> Result<mpsc::UnboundedReceiver<StreamItem>, BrokerError> {
        let mut entry = AuditEntry::new(Action::List, Outcome::Ok, caller.clone());
        entry.reason = Some("watch".to_owned());
        self.audit(&entry)?;
        let metas = self.metas(&entry).await?;
        let initial = SecretEvent::initial(metas.iter().map(|m| self.info(m)).collect());
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(StreamItem::Reply(json!({ "event": initial })));
        self.state.borrow_mut().watchers.push(tx);
        Ok(rx)
    }

    /// Checks that `id` is (not) taken, auditing a refusal.
    async fn expect_exists(
        &self,
        entry: &AuditEntry,
        id: &SecretId,
        wanted: bool,
    ) -> Result<(), BrokerError> {
        let exists = self.store.contains(id).await.map_err(|e| {
            self.audit_failure(entry.clone(), &e.to_string());
            BrokerError::Unavailable
        })?;
        if exists == wanted {
            return Ok(());
        }
        let reason = if exists { "exists" } else { "no such secret" };
        let mut refused = entry.clone();
        refused.outcome = Outcome::Failed;
        refused.reason = Some(reason.to_owned());
        self.audit(&refused)?;
        Err(if exists {
            BrokerError::Exists(id.clone())
        } else {
            BrokerError::NotFound(id.clone())
        })
    }

    fn entry_for(action: Action, caller: &Caller, id: &SecretId) -> AuditEntry {
        let mut entry = AuditEntry::new(action, Outcome::Ok, caller.clone());
        entry.secret_id = Some(id.to_string());
        entry
    }

    /// Stores a new secret.
    pub async fn add(&self, caller: &Caller, new: NewSecret) -> Result<SecretInfo, BrokerError> {
        self.refuse_agent(caller, &new.id)?;
        let entry = Self::entry_for(Action::Add, caller, &new.id);
        self.expect_exists(&entry, &new.id, false).await?;
        self.audit(&entry)?;
        let meta = SecretMeta {
            id: new.id,
            kind: new.kind,
            host: new.host,
            allowed_tools: new.allowed_tools,
            created_at: self.clock.now_secs(),
            rotated_at: None,
        };
        if let Err(e) = self.store.add(&meta, &new.value).await {
            self.audit_failure(entry, &e.to_string());
            return Err(match e {
                StoreError::Exists(id) => BrokerError::Exists(id),
                _ => BrokerError::Unavailable,
            });
        }
        let info = self.info(&meta);
        self.emit(&SecretEvent::added(info.clone()));
        Ok(info)
    }

    /// Replaces the value of a secret.
    pub async fn rotate(
        &self,
        caller: &Caller,
        id: &SecretId,
        value: Secret,
    ) -> Result<SecretInfo, BrokerError> {
        self.refuse_agent(caller, id)?;
        let entry = Self::entry_for(Action::Rotate, caller, id);
        self.expect_exists(&entry, id, true).await?;
        self.audit(&entry)?;
        let meta = match self.store.rotate(id, &value, self.clock.now_secs()).await {
            Ok(meta) => meta,
            Err(e) => {
                self.audit_failure(entry, &e.to_string());
                return Err(match e {
                    StoreError::NotFound(id) => BrokerError::NotFound(id),
                    _ => BrokerError::Unavailable,
                });
            }
        };
        let info = self.info(&meta);
        self.emit(&SecretEvent::changed(info.clone()));
        Ok(info)
    }

    /// Deletes a secret.
    pub async fn remove(&self, caller: &Caller, id: &SecretId) -> Result<(), BrokerError> {
        self.refuse_agent(caller, id)?;
        let entry = Self::entry_for(Action::Remove, caller, id);
        self.expect_exists(&entry, id, true).await?;
        self.audit(&entry)?;
        if let Err(e) = self.store.remove(id).await {
            self.audit_failure(entry, &e.to_string());
            return Err(match e {
                StoreError::NotFound(id) => BrokerError::NotFound(id),
                _ => BrokerError::Unavailable,
            });
        }
        self.state.borrow_mut().usage.remove(id);
        self.emit(&SecretEvent::removed(id.as_str()));
        Ok(())
    }

    /// Releases a value for a use bound to where it is allowed to go.
    pub async fn use_secret(
        &self,
        caller: &Caller,
        id: &SecretId,
        usage: Use,
    ) -> Result<Secret, BrokerError> {
        let (action, upstream, tool_id) = match &usage {
            Use::Proxy { upstream } => (Action::Use, Some(upstream.clone()), None),
            Use::Spawn { tool_id } => (Action::Spawn, None, Some(tool_id.clone())),
        };
        let mut entry = Self::entry_for(action, caller, id);
        entry.upstream = upstream.clone();
        entry.tool_id = tool_id.clone();
        let metas = self.metas(&entry).await?;
        let Some(meta) = metas.into_iter().find(|m| &m.id == id) else {
            let mut refused = entry;
            refused.outcome = Outcome::Failed;
            refused.reason = Some("no such secret".to_owned());
            self.audit(&refused)?;
            return Err(BrokerError::NotFound(id.clone()));
        };
        let bound = match (&usage, &meta) {
            (Use::Proxy { upstream }, m) if upstream == m.host.as_str() => None,
            (Use::Proxy { .. }, _) => Some(HOST_DENIED),
            (Use::Spawn { tool_id }, m)
                if m.allowed_tools.iter().any(|t| t.as_str() == tool_id) =>
            {
                None
            }
            (Use::Spawn { .. }, _) => Some(TOOL_DENIED),
        };
        if let Some(reason) = bound {
            let mut denied = entry;
            denied.action = Action::Denied;
            denied.outcome = Outcome::Denied;
            denied.reason = Some(reason.to_owned());
            self.audit(&denied)?;
            return Err(BrokerError::Denied(reason));
        }
        self.audit(&entry)?;
        let value = match self.store.value(id).await {
            Ok(value) => value,
            Err(e) => {
                self.audit_failure(entry, &e.to_string());
                return Err(BrokerError::Unavailable);
            }
        };
        let now = self.clock.now_secs();
        {
            let mut state = self.state.borrow_mut();
            let used = state.usage.entry(id.clone()).or_insert(Usage {
                count: 0,
                last: None,
            });
            used.count += 1;
            used.last = Some(now);
        }
        self.emit(&SecretEvent::changed(self.info(&meta)));
        Ok(value)
    }
}

#[cfg(test)]
mod tests;
