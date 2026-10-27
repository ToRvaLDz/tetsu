//! The config side of the bus thread: the `SetManaged` command that goes
//! to the calloop loop, and the mirror of the config the reads serve.

use std::collections::BTreeMap;

use serde_json::Value;
use tetsu_bus::config::{ConfigEntry, ConfigEvent, ConfigKind, GetConfigReply};
use tetsu_bus::server::ErrorReply;
use tokio::sync::oneshot;

/// A `SetManaged` call, for the loop to run: it owns the layers.
#[derive(Debug)]
pub struct SetManaged {
    /// Key to the new managed value.
    pub values: BTreeMap<String, Value>,
    /// Keys whose managed value goes.
    pub remove: Vec<String>,
    /// Where the loop answers. Dropped without an answer: the compositor
    /// is going away.
    pub reply: oneshot::Sender<Result<(), ErrorReply>>,
}

/// The config as the events built it.
#[derive(Debug, Clone, Default)]
pub struct ConfigMirror {
    state: GetConfigReply,
}

impl ConfigMirror {
    /// Starts from a snapshot.
    pub fn new(state: GetConfigReply) -> Self {
        Self { state }
    }

    /// For `GetConfig`.
    pub fn reply(&self) -> GetConfigReply {
        self.state.clone()
    }

    /// The first event of a `WatchConfig`.
    pub fn initial(&self) -> ConfigEvent {
        ConfigEvent::initial(self.state.entries.clone(), self.state.diagnostics.clone())
    }

    /// Applies one event.
    pub fn apply(&mut self, event: &ConfigEvent) {
        match event.kind {
            ConfigKind::Initial => {
                self.state.entries = event.entries.clone().unwrap_or_default();
                self.state.diagnostics = event.diagnostics.clone().unwrap_or_default();
            }
            ConfigKind::Changed => {
                if let Some(entry) = &event.entry {
                    self.put(entry);
                }
            }
            ConfigKind::Removed => {
                if let Some(key) = &event.key {
                    self.state.entries.retain(|e| &e.key != key);
                }
            }
            ConfigKind::Diagnostics => {
                self.state.diagnostics = event.diagnostics.clone().unwrap_or_default();
            }
            ConfigKind::Unknown => {}
        }
    }

    fn put(&mut self, entry: &ConfigEntry) {
        match self.state.entries.iter_mut().find(|e| e.key == entry.key) {
            Some(slot) => *slot = entry.clone(),
            None => self.state.entries.push(entry.clone()),
        }
    }
}
