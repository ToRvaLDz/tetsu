//! The engine: decides every `Check`, keeps the counters and suppressions,
//! writes the audit trail and tells the watchers.
//!
//! Single-threaded: it lives on a `LocalSet`, its mutable state in
//! `RefCell`s that are never held across an await.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tetsu_bus::cap::{
    Capability, CapabilityEvent, CheckParams, CheckReply, DeniedBy, EventKind, Grant, IntentParams,
    Layer, SetParams, State,
};
use tokio::sync::mpsc;

use crate::ask::{self, AskRequest, NotifyChannel};
use crate::audit::{AuditEntry, AuditSink, Event};
use crate::batch::{self, Batcher};
use crate::catalog;
use crate::clock::{Clock, rfc3339};
use crate::decide::{Buttons, Outcome, Verdict, answer_effect, decide};
use crate::intent::Intents;
use crate::store::{Store, StoreError};

/// How long a denial holds back further asks.
pub const SUPPRESSION: Duration = Duration::from_secs(24 * 3600);

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

/// A class and, optionally, a scope.
type Key = (String, Option<String>);

#[derive(Debug, Default, Clone)]
struct Stats {
    asks: i64,
    approvals: i64,
    denials: i64,
    suppressions: i64,
    last_used: Option<SystemTime>,
}

#[derive(Default)]
struct Mutable {
    stats: BTreeMap<Key, Stats>,
    suppressed: BTreeMap<Key, SystemTime>,
    intents: Intents,
    batcher: Batcher,
    watchers: Vec<mpsc::UnboundedSender<CapabilityEvent>>,
}

/// The state of the daemon.
pub struct Engine<N, C, A> {
    notify: N,
    clock: C,
    audit: A,
    store: RefCell<Store>,
    state: RefCell<Mutable>,
    backoff: Vec<Duration>,
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Unset => "unset",
        State::Automatic => "automatic",
        State::Notify => "notify",
        State::Ask => "ask",
        State::Off => "off",
        State::Unknown => "unknown",
    }
}

fn layer_name(layer: Layer) -> &'static str {
    match layer {
        Layer::Image => "image",
        Layer::Managed => "managed",
        Layer::User => "user",
        Layer::Unknown => "unknown",
    }
}

fn denied_name(by: DeniedBy) -> &'static str {
    match by {
        DeniedBy::Off => "off",
        DeniedBy::User => "user",
        DeniedBy::Suppressed => "suppressed",
        DeniedBy::Unavailable | DeniedBy::Unknown => "unavailable",
    }
}

/// Withdrawn-ask bookkeeping: if the check future is dropped while the
/// user is being asked, the cancel is audited. The show stream goes with
/// the future, which withdraws the notification.
struct CancelGuard<'a, N: NotifyChannel, C: Clock, A: AuditSink> {
    engine: &'a Engine<N, C, A>,
    base: Option<AuditEntry>,
}

impl<N: NotifyChannel, C: Clock, A: AuditSink> Drop for CancelGuard<'_, N, C, A> {
    fn drop(&mut self) {
        let Some(base) = self.base.take() else { return };
        for event in [Event::Denied, Event::Check] {
            let mut entry = base.clone();
            entry.event = event;
            entry.decision = Some("deny".into());
            entry.denied_by = Some(denied_name(DeniedBy::Unavailable).into());
            entry.cancelled = true;
            self.engine.write_best_effort(&entry);
        }
    }
}

impl<N: NotifyChannel, C: Clock, A: AuditSink> Engine<N, C, A> {
    /// An engine over a store. `backoff` are the pauses before each
    /// re-show of an ask.
    pub fn new(notify: N, clock: C, audit: A, mut store: Store, backoff: Vec<Duration>) -> Self {
        if let Err(e) = store.refresh() {
            eprintln!("tetsu-capd: {e}");
        }
        Self {
            notify,
            clock,
            audit,
            store: RefCell::new(store),
            state: RefCell::new(Mutable::default()),
            backoff,
        }
    }

    fn write_best_effort(&self, entry: &AuditEntry) {
        if let Err(e) = self.audit.record(entry) {
            eprintln!("tetsu-capd: cannot write the audit trail: {e}");
        }
    }

    fn entry_for(&self, event: Event, caller_unit: &str, params: &CheckParams) -> AuditEntry {
        let mut entry = AuditEntry::new(event);
        entry.method = Some(params.method.clone());
        entry.peer_pid = Some(params.peer.pid);
        entry.peer_unit = Some(params.peer.unit.clone());
        entry.caller_unit = Some(caller_unit.to_owned());
        entry.args = serde_json::to_string(&params.args).ok();
        entry
    }

    /// Decides a call of the agent.
    pub async fn check(
        &self,
        caller_unit: &str,
        params: &CheckParams,
    ) -> Result<CheckReply, CapError> {
        let method = catalog::lookup(&params.method)
            .ok_or_else(|| CapError::UnknownMethod(params.method.clone()))?;
        let mut base = self.entry_for(Event::Check, caller_unit, params);
        let Some(class) = method.class else {
            return Ok(self.conclude(base, None, CheckReply::allow()));
        };
        let scope = method
            .scope_param
            .and_then(|p| params.args.get(p))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let key: Key = (class.to_owned(), scope.clone());
        base.class = Some(class.to_owned());
        base.scope = scope.clone();

        let refreshed = self.store.borrow_mut().refresh();
        if let Err(e) = refreshed {
            eprintln!("tetsu-capd: failing closed: {e}");
            return Ok(self.conclude(base, Some(key), CheckReply::deny(DeniedBy::Unavailable)));
        }
        let effective = self.store.borrow().effective(class, scope.as_deref());
        base.state = Some(state_name(effective.state).into());
        base.layer = effective.layer.map(|l| layer_name(l).into());

        let now = self.clock.now();
        let suppressed = self.state.borrow().suppressed.get(&key).copied();
        match decide(effective.state, method.destructive, suppressed, now) {
            Verdict::Deny(by) => {
                if by == DeniedBy::Suppressed {
                    self.state
                        .borrow_mut()
                        .stats
                        .entry(key.clone())
                        .or_default()
                        .suppressions += 1;
                    let mut entry = base.clone();
                    entry.event = Event::Suppressed;
                    self.write_best_effort(&entry);
                }
                Ok(self.conclude(base, Some(key), CheckReply::deny(by)))
            }
            Verdict::Allow { notify } => {
                if notify {
                    self.state
                        .borrow_mut()
                        .batcher
                        .record(class, method.method, now);
                }
                Ok(self.conclude(base, Some(key), CheckReply::allow()))
            }
            Verdict::Ask(buttons) => {
                let reply = self.ask(base, &key, params, buttons).await;
                Ok(reply)
            }
        }
    }

    /// Writes the `check` entry and tells the watchers. An allow whose
    /// entry cannot be written becomes a deny: no unaudited allow.
    fn conclude(&self, mut base: AuditEntry, key: Option<Key>, reply: CheckReply) -> CheckReply {
        base.event = Event::Check;
        base.decision = Some(
            match reply.decision {
                tetsu_bus::cap::Decision::Allow => "allow",
                tetsu_bus::cap::Decision::Deny => "deny",
            }
            .into(),
        );
        base.denied_by = reply.denied_by.map(|by| denied_name(by).into());
        let reply = match (reply.decision, self.audit.record(&base)) {
            (tetsu_bus::cap::Decision::Allow, Err(e)) => {
                eprintln!("tetsu-capd: cannot write the audit trail, denying: {e}");
                CheckReply::deny(DeniedBy::Unavailable)
            }
            (_, Err(e)) => {
                eprintln!("tetsu-capd: cannot write the audit trail: {e}");
                reply
            }
            (_, Ok(())) => reply,
        };
        if let Some(key) = key {
            if reply.decision == tetsu_bus::cap::Decision::Allow {
                let now = self.clock.now();
                self.state
                    .borrow_mut()
                    .stats
                    .entry(key.clone())
                    .or_default()
                    .last_used = Some(now);
            }
            self.notify_changed(&key);
        }
        reply
    }

    async fn ask(
        &self,
        base: AuditEntry,
        key: &Key,
        params: &CheckParams,
        buttons: Buttons,
    ) -> CheckReply {
        let class = key.0.as_str();
        let now = self.clock.now();
        let reason = self
            .state
            .borrow_mut()
            .intents
            .take(params.peer.pid, &params.method, now);
        let mut asked = base.clone();
        asked.event = Event::Ask;
        asked.reason = reason.clone();
        if let Err(e) = self.audit.record(&asked) {
            eprintln!("tetsu-capd: cannot write the audit trail, denying: {e}");
            return self.conclude(
                base,
                Some(key.clone()),
                CheckReply::deny(DeniedBy::Unavailable),
            );
        }
        self.state
            .borrow_mut()
            .stats
            .entry(key.clone())
            .or_default()
            .asks += 1;
        self.notify_changed(key);

        let show = ask::render(&AskRequest {
            method: &params.method,
            class,
            args: &params.args,
            reason: reason.as_deref(),
            buttons,
        });
        let mut guard = CancelGuard {
            engine: self,
            base: Some(base.clone()),
        };
        let answer = ask::run_ask(&self.notify, &show, &self.backoff, &mut |_| {}).await;
        guard.base = None;
        drop(guard);

        let mut base = base;
        base.reason = reason;
        self.apply_answer(base, key, buttons, &answer)
    }

    fn apply_answer(
        &self,
        base: AuditEntry,
        key: &Key,
        buttons: Buttons,
        answer: &ask::Answer,
    ) -> CheckReply {
        let now = self.clock.now();
        let outcome = answer_effect(buttons, answer);
        match outcome {
            Outcome::Allow | Outcome::AllowAndAlways => {
                self.state
                    .borrow_mut()
                    .stats
                    .entry(key.clone())
                    .or_default()
                    .approvals += 1;
                let mut approved = base.clone();
                approved.event = Event::Approved;
                self.write_best_effort(&approved);
                if outcome == Outcome::AllowAndAlways {
                    self.always_allow(&base, key);
                }
                self.conclude(base, Some(key.clone()), CheckReply::allow())
            }
            Outcome::DenyAndSuppress => {
                {
                    let mut state = self.state.borrow_mut();
                    state.stats.entry(key.clone()).or_default().denials += 1;
                    state.suppressed.insert(key.clone(), now + SUPPRESSION);
                }
                self.deny_entry(&base, DeniedBy::User);
                self.conclude(base, Some(key.clone()), CheckReply::deny(DeniedBy::User))
            }
            Outcome::DenyUnavailable => {
                self.deny_entry(&base, DeniedBy::Unavailable);
                self.conclude(
                    base,
                    Some(key.clone()),
                    CheckReply::deny(DeniedBy::Unavailable),
                )
            }
        }
    }

    fn deny_entry(&self, base: &AuditEntry, by: DeniedBy) {
        let mut denied = base.clone();
        denied.event = Event::Denied;
        denied.decision = Some("deny".into());
        denied.denied_by = Some(denied_name(by).into());
        self.write_best_effort(&denied);
    }

    /// *Always allow*: writes `notify` to the managed layer.
    fn always_allow(&self, base: &AuditEntry, key: &Key) {
        let result =
            self.store
                .borrow_mut()
                .set_managed(&key.0, key.1.as_deref(), Some(Grant::Notify));
        match result {
            Ok(()) => {
                let mut set = base.clone();
                set.event = Event::Set;
                set.state = Some("notify".into());
                set.layer = Some("managed".into());
                self.write_best_effort(&set);
            }
            Err(e) => eprintln!("tetsu-capd: cannot save the grant: {e}"),
        }
    }

    /// Sets or removes a managed value.
    pub fn set(&self, caller_unit: &str, params: &SetParams) -> Result<(), CapError> {
        let class = catalog::class(&params.class)
            .ok_or_else(|| CapError::UnknownClass(params.class.clone()))?;
        if params.scope.is_some() && class.scope_param.is_none() {
            return Err(CapError::InvalidParameter("scope"));
        }
        let scope = params.scope.as_deref();
        let store_error = |e: StoreError| match e {
            StoreError::OverriddenByUser => CapError::OverriddenByUser(params.class.clone()),
            StoreError::Malformed { .. } => {
                eprintln!("tetsu-capd: refusing a Set: {e}");
                CapError::InvalidParameter("class")
            }
            StoreError::Io { .. } => {
                eprintln!("tetsu-capd: cannot save the grant: {e}");
                CapError::Unavailable
            }
        };
        self.store
            .borrow_mut()
            .check_settable(class.name, scope)
            .map_err(store_error)?;
        let mut entry = AuditEntry::new(Event::Set);
        entry.class = Some(class.name.to_owned());
        entry.scope = params.scope.clone();
        entry.state = Some(params.state.map_or("removed", Grant::as_str).to_owned());
        entry.layer = Some("managed".into());
        entry.caller_unit = Some(caller_unit.to_owned());
        if let Err(e) = self.audit.record(&entry) {
            eprintln!("tetsu-capd: cannot write the audit trail, refusing a Set: {e}");
            return Err(CapError::Unavailable);
        }
        self.store
            .borrow_mut()
            .set_managed(class.name, scope, params.state)
            .map_err(store_error)?;
        self.notify_changed(&(class.name.to_owned(), params.scope.clone()));
        Ok(())
    }

    /// Records why the peer `pid` is about to call a method.
    pub fn intent(&self, pid: i64, params: &IntentParams) -> Result<(), CapError> {
        if catalog::lookup(&params.method).is_none() {
            return Err(CapError::UnknownMethod(params.method.clone()));
        }
        let now = self.clock.now();
        self.state
            .borrow_mut()
            .intents
            .record(pid, &params.method, &params.reason, now);
        Ok(())
    }

    fn capability(&self, key: &Key) -> Option<Capability> {
        let class = catalog::class(&key.0)?;
        let effective = self.store.borrow().effective(&key.0, key.1.as_deref());
        let state = self.state.borrow();
        let stats = state.stats.get(key).cloned().unwrap_or_default();
        let now = self.clock.now();
        let until = state.suppressed.get(key).filter(|until| now < **until);
        Some(Capability {
            class: key.0.clone(),
            scope: key.1.clone(),
            state: effective.state,
            layer: effective.layer,
            destructive: class.destructive,
            methods: class.methods.iter().map(|m| (*m).to_owned()).collect(),
            asks: stats.asks,
            approvals: stats.approvals,
            denials: stats.denials,
            suppressions: stats.suppressions,
            last_used_at: stats.last_used.map(rfc3339),
            suppressed_until_at: until.map(|t| rfc3339(*t)),
        })
    }

    /// Every capability: each class, then each scope that has a grant or
    /// a history.
    pub fn list(&self) -> Vec<Capability> {
        let mut keys: Vec<Key> = catalog::classes()
            .iter()
            .map(|c| (c.name.to_owned(), None))
            .collect();
        let mut scoped: Vec<Key> = self
            .store
            .borrow()
            .scoped_keys()
            .into_iter()
            .map(|(class, scope)| (class, Some(scope)))
            .collect();
        scoped.extend(
            self.state
                .borrow()
                .stats
                .keys()
                .filter(|(_, scope)| scope.is_some())
                .cloned(),
        );
        scoped.sort();
        scoped.dedup();
        keys.extend(scoped);
        keys.iter().filter_map(|key| self.capability(key)).collect()
    }

    /// The initial event and the stream of changes.
    pub fn watch(&self) -> (CapabilityEvent, mpsc::UnboundedReceiver<CapabilityEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state.borrow_mut().watchers.push(tx);
        let initial = CapabilityEvent {
            kind: EventKind::Initial,
            capabilities: Some(self.list()),
            capability: None,
        };
        (initial, rx)
    }

    fn notify_changed(&self, key: &Key) {
        let Some(capability) = self.capability(key) else {
            return;
        };
        let event = CapabilityEvent {
            kind: EventKind::Changed,
            capabilities: None,
            capability: Some(capability),
        };
        self.state
            .borrow_mut()
            .watchers
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Sends the grouped notices whose window has ended.
    pub async fn flush_notices(&self) {
        let now = self.clock.now();
        let due = self.state.borrow_mut().batcher.due(now);
        for notice in due {
            if let Err(e) = self.notify.notice(batch::show_params(&notice)).await {
                eprintln!("tetsu-capd: cannot show a notice: {e}");
            }
        }
    }
}
