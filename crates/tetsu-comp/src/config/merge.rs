//! The three layers merged: the nearest layer wins, key by key.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Json;
use tetsu_bus::config::{ConfigEntry, ConfigEvent, Origin};
use toml::Value;

use super::keys::{self, KEYS, KeyDef};
use super::output_keys;

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
/// value of a key that runs commands is ignored. After the registry keys
/// come the `output.<connector>.<field>` keys some layer sets, sorted.
pub fn merge(defaults: &Values, managed: &Values, user: &Values) -> View {
    let builtin = keys::builtin();
    let mut view = View {
        entries: Vec::with_capacity(KEYS.len()),
        values: Values::new(),
        user_keys: Vec::new(),
    };
    for key in KEYS {
        let default = defaults.get(key.name).or_else(|| builtin.get(key.name));
        view.add(key.name, &key, default, managed, user);
    }
    let dynamic: BTreeSet<&String> = [defaults, managed, user]
        .into_iter()
        .flat_map(Values::keys)
        .filter(|name| output_keys::lookup(name).is_some())
        .collect();
    for name in dynamic {
        if let Some(def) = output_keys::lookup(name) {
            view.add(name, def, defaults.get(name), managed, user);
        }
    }
    view.user_keys.sort();
    view
}

impl View {
    /// Adds the entry of one key, when some layer gives it a value.
    fn add(
        &mut self,
        name: &str,
        key: &KeyDef,
        default: Option<&Value>,
        managed: &Values,
        user: &Values,
    ) {
        let managed = managed.get(name).filter(|_| !key.runs_commands);
        let user = user.get(name);
        let (effective, origin) = match (user, managed, default) {
            (Some(v), _, _) => (Some(v), Origin::User),
            (None, Some(v), _) => (Some(v), Origin::Managed),
            (None, None, v) => (v, Origin::Default),
        };
        let Some(effective) = effective else {
            return;
        };
        if user.is_some() {
            self.user_keys.push(name.to_owned());
        }
        self.values.insert(name.to_owned(), effective.clone());
        self.entries.push(ConfigEntry {
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

/// A `changed` for each entry that is new or differs, then a `removed`
/// (sorted) for each key that no layer sets any more.
pub fn diff(old: &View, new: &View) -> Vec<ConfigEvent> {
    let changed = new
        .entries
        .iter()
        .filter(|entry| old.entry(&entry.key) != Some(entry))
        .cloned()
        .map(ConfigEvent::changed);
    let mut gone: Vec<&str> = old
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .filter(|key| new.entry(key).is_none())
        .collect();
    gone.sort_unstable();
    changed
        .chain(gone.into_iter().map(ConfigEvent::removed))
        .collect()
}
