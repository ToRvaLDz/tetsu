//! The state every connection shares: the running transaction, the failed
//! ones kept in memory and the watchers of `WatchTransactions`. Lives on
//! the `LocalSet`, so plain `RefCell`s do.

use std::cell::RefCell;
use std::collections::VecDeque;

use serde_json::json;
use tetsu_bus::server::StreamItem;
use tetsu_bus::tx::{Progress, Transaction, TxEvent, TxState};
use tokio::sync::mpsc::UnboundedSender;

/// How many failed transactions are kept for `ListTransactions`. They are
/// lost when the daemon restarts; committed ones come from the records.
pub const FAILED_KEPT: usize = 32;

/// The transaction holding the single slot. It is visible to watchers only
/// once the engine has begun.
struct Slot {
    id: String,
    transaction: Option<Transaction>,
}

#[derive(Default)]
struct State {
    slot: Option<Slot>,
    /// Oldest first.
    failed: VecDeque<Transaction>,
    watchers: Vec<UnboundedSender<StreamItem>>,
}

/// Shared state of the service.
#[derive(Default)]
pub struct Hub {
    state: RefCell<State>,
}

impl Hub {
    /// An empty hub.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the slot for transaction `id`; the id of the one holding it
    /// otherwise.
    pub fn reserve(&self, id: &str) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        match &state.slot {
            Some(slot) => Err(slot.id.clone()),
            None => {
                state.slot = Some(Slot {
                    id: id.to_owned(),
                    transaction: None,
                });
                Ok(())
            }
        }
    }

    /// Frees the slot of a transaction that failed before it began: no
    /// transaction existed, nobody is told.
    pub fn release(&self) {
        self.state.borrow_mut().slot = None;
    }

    /// The engine began: the transaction exists and `started` goes out.
    pub fn started(&self, transaction: Transaction) {
        let event = TxEvent::started(transaction.clone());
        let mut state = self.state.borrow_mut();
        if let Some(slot) = &mut state.slot {
            slot.transaction = Some(transaction);
        }
        broadcast(&mut state.watchers, &event);
    }

    /// The running transaction moved on.
    pub fn progress(&self, id: &str, progress: Progress) {
        let event = TxEvent::progress(id, progress);
        broadcast(&mut self.state.borrow_mut().watchers, &event);
    }

    /// The transaction ended: the slot is free, a failure is remembered and
    /// `finished` goes out.
    pub fn finished(&self, transaction: Transaction) {
        let event = TxEvent::finished(transaction.clone());
        let mut state = self.state.borrow_mut();
        state.slot = None;
        if transaction.state == TxState::Failed {
            state.failed.push_back(transaction);
            while state.failed.len() > FAILED_KEPT {
                state.failed.pop_front();
            }
        }
        broadcast(&mut state.watchers, &event);
    }

    /// Registers a watcher; it gets `initial` first, then every delta.
    pub fn watch(&self, watcher: UnboundedSender<StreamItem>) {
        let mut state = self.state.borrow_mut();
        let running: Vec<Transaction> = state
            .slot
            .iter()
            .filter_map(|slot| slot.transaction.clone())
            .collect();
        if send(&watcher, &TxEvent::initial(running)) {
            state.watchers.push(watcher);
        }
    }

    /// The running transaction, if it has begun, then the failed ones,
    /// newest first.
    pub fn known(&self) -> Vec<Transaction> {
        let state = self.state.borrow();
        let running = state
            .slot
            .iter()
            .filter_map(|slot| slot.transaction.clone());
        running.chain(state.failed.iter().rev().cloned()).collect()
    }
}

fn send(watcher: &UnboundedSender<StreamItem>, event: &TxEvent) -> bool {
    let value = json!({ "event": event });
    watcher.send(StreamItem::Reply(value)).is_ok()
}

/// Sends `event` to every watcher and forgets the ones that are gone.
fn broadcast(watchers: &mut Vec<UnboundedSender<StreamItem>>, event: &TxEvent) {
    watchers.retain(|watcher| send(watcher, event));
}
