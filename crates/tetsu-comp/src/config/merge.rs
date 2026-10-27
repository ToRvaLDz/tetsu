//! The three layers merged: the nearest layer wins, key by key.

use std::collections::BTreeMap;

use serde_json::Value as Json;
use tetsu_bus::config::{ConfigEntry, ConfigEvent, Origin};
use toml::Value;

use super::keys::{self, KEYS};

/// Dotted key to value: what one layer sets.
pub type Values = BTreeMap<String, Value>;

/// The merged config: one entry per known key.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    entries: Vec<ConfigEntry>,
    values: Values,
    user_keys: Vec<String>,
}

fn json(value: &Value) -> Json {
    serde_json::to_value(value).unwrap_or(Json::Null)
}

/// Merges the layers. The built-in values sit under `defaults`; a managed
/// value of a key that runs commands is ignored.
pub fn merge(defaults: &Values, managed: &Values, user: &Values) -> View {
    let builtin = keys::builtin();
    let mut entries = Vec::with_capacity(KEYS.len());
    let mut values = Values::new();
    let mut user_keys = Vec::new();
    for key in KEYS {
        let name = key.name;
        let default = defaults.get(name).or_else(|| builtin.get(name));
        let managed = managed.get(name).filter(|_| !key.runs_commands);
        let user = user.get(name);
        let (effective, origin) = match (user, managed, default) {
            (Some(v), _, _) => (Some(v), Origin::User),
            (None, Some(v), _) => (Some(v), Origin::Managed),
            (None, None, v) => (v, Origin::Default),
        };
        let Some(effective) = effective else {
            continue;
        };
        if user.is_some() {
            user_keys.push(name.to_owned());
        }
        values.insert(name.to_owned(), effective.clone());
        entries.push(ConfigEntry {
            key: name.to_owned(),
            value: json(effective),
            origin,
            default_value: default.map(json),
            managed_value: managed.map(json),
            user_value: user.map(json),
            writable: key.writable,
            method: key.method.map(str::to_owned),
        });
    }
    user_keys.sort();
    View {
        entries,
        values,
        user_keys,
    }
}

impl View {
    /// Every key, in registry order.
    pub fn entries(&self) -> &[ConfigEntry] {
        &self.entries
    }

    /// One key.
    pub fn entry(&self, key: &str) -> Option<&ConfigEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    /// The effective value of every key.
    pub fn values(&self) -> Values {
        self.values.clone()
    }

    /// The keys the user file sets, sorted.
    pub fn user_keys(&self) -> &[String] {
        &self.user_keys
    }
}

/// A `changed` for each key whose entry differs.
pub fn diff(old: &View, new: &View) -> Vec<ConfigEvent> {
    new.entries
        .iter()
        .filter(|entry| old.entry(&entry.key) != Some(entry))
        .cloned()
        .map(ConfigEvent::changed)
        .collect()
}
