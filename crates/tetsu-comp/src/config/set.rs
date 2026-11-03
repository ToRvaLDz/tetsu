//! `SetManaged`: the pure part. Validates a batch and gives the managed
//! layer it would leave; nothing is written here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as Json, json};
use tetsu_bus::server::ErrorReply;
use toml::Value;

use super::keys;
use super::merge::{Values, View};

const NS: &str = "org.tetsu.comp.config.v1";

/// Why a batch is rejected as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetError {
    /// No config key has this name.
    UnknownKey(String),
    /// The key is not writable through `SetManaged`.
    KeyNotWritable {
        /// The key.
        key: String,
        /// The method that writes it; none when only the user file does.
        method: Option<String>,
    },
    /// The value does not fit the key.
    InvalidValue {
        /// The key.
        key: String,
        /// What is wrong.
        reason: String,
    },
    /// The user file sets these keys, so the managed values would not
    /// take effect.
    OverriddenByUser(Vec<String>),
}

impl SetError {
    /// The Varlink error reply.
    pub fn reply(&self) -> ErrorReply {
        let (name, parameters) = match self {
            Self::UnknownKey(key) => ("UnknownKey", json!({ "key": key })),
            Self::KeyNotWritable { key, method } => {
                ("KeyNotWritable", json!({ "key": key, "method": method }))
            }
            Self::InvalidValue { key, reason } => {
                ("InvalidValue", json!({ "key": key, "reason": reason }))
            }
            Self::OverriddenByUser(keys) => ("OverriddenByUser", json!({ "keys": keys })),
        };
        ErrorReply::new(&format!("{NS}.{name}"), parameters)
    }
}

/// `StateWriteFailed (reason)`.
pub fn state_write_failed(reason: &str) -> ErrorReply {
    ErrorReply::new(
        &format!("{NS}.StateWriteFailed"),
        json!({ "reason": reason }),
    )
}

/// JSON to TOML. TOML has no null, and its integers are 64-bit signed.
fn to_toml(value: &Json) -> Result<Value, String> {
    Ok(match value {
        Json::Null => return Err("null is not a value".to_owned()),
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => Value::Integer(i),
            (None, Some(f)) if n.is_f64() => Value::Float(f),
            _ => return Err(format!("{n} is out of range")),
        },
        Json::String(s) => Value::String(s.clone()),
        Json::Array(items) => Value::Array(items.iter().map(to_toml).collect::<Result<_, _>>()?),
        Json::Object(map) => Value::Table(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), to_toml(v)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

fn writable(key: &str) -> Result<(), SetError> {
    let def = keys::lookup(key).ok_or_else(|| SetError::UnknownKey(key.to_owned()))?;
    if def.writable {
        Ok(())
    } else {
        Err(SetError::KeyNotWritable {
            key: key.to_owned(),
            method: def.method.map(str::to_owned),
        })
    }
}

fn invalid(key: &str, reason: String) -> SetError {
    SetError::InvalidValue {
        key: key.to_owned(),
        reason,
    }
}

/// The managed layer after the batch: `managed` with `values` set and
/// `remove` taken out. The checks run in this order, and the first bad key
/// rejects everything: unknown key, key not writable, invalid value (a key
/// both set and removed included), key set by the user file.
pub fn plan(
    view: &View,
    managed: &Values,
    values: &BTreeMap<String, Json>,
    remove: &[String],
) -> Result<Values, SetError> {
    let named = || values.keys().chain(remove);
    for key in named() {
        if keys::lookup(key).is_none() {
            return Err(SetError::UnknownKey(key.clone()));
        }
    }
    for key in named() {
        writable(key)?;
    }
    let mut converted = Values::new();
    for (key, json) in values {
        if remove.contains(key) {
            return Err(invalid(key, "both set and removed".to_owned()));
        }
        let value = to_toml(json).map_err(|reason| invalid(key, reason))?;
        keys::validate(key, &value).map_err(|(_, e)| invalid(key, e.to_string()))?;
        converted.insert(key.clone(), value);
    }
    let user: BTreeSet<&String> = view.user_keys().iter().collect();
    let overridden: Vec<String> = converted
        .keys()
        .filter(|key| user.contains(key))
        .cloned()
        .collect();
    if !overridden.is_empty() {
        return Err(SetError::OverriddenByUser(overridden));
    }
    let mut next = managed.clone();
    for key in remove {
        next.remove(key);
    }
    next.extend(converted);
    Ok(next)
}
