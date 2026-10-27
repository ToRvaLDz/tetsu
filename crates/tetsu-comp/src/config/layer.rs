//! One layer of the config: the text of a TOML file in, its valid values
//! and the problems found out. A problem in one key never costs another.

use std::collections::BTreeMap;

use tetsu_bus::config::Diagnostic;
use toml::Value;
use toml::de::{DeTable, DeValue};

use super::keys::{self, TABLES};
use crate::outputs::limits;
use crate::settings::SettingsError;

/// What a layer's text gave.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerParse {
    /// Dotted key to value, only the keys that validated.
    pub values: BTreeMap<String, Value>,
    /// What was wrong, in file order.
    pub diagnostics: Vec<Diagnostic>,
    /// The text is not TOML at all: `values` is empty and says nothing
    /// about the file.
    pub broken: bool,
}

impl LayerParse {
    /// The first diagnostic as the strict startup error.
    pub fn first_error(&self) -> Option<&Diagnostic> {
        self.diagnostics.first()
    }

    fn syntax(&mut self, text: &str, at: usize, message: String) {
        self.broken = true;
        self.problem(text, at, None, message);
    }

    fn problem(&mut self, text: &str, at: usize, key: Option<&str>, message: String) {
        self.diagnostics.push(Diagnostic {
            line: Some(line_of(text, at)),
            key: key.map(str::to_owned),
            message,
        });
    }
}

/// The 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> i64 {
    let end = offset.min(text.len());
    let newlines = text.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count();
    i64::try_from(newlines).unwrap_or(i64::MAX - 1) + 1
}

/// Parses the text of a layer.
pub fn parse(text: &str) -> LayerParse {
    let mut out = LayerParse::default();
    let spans = match DeTable::parse(text) {
        Ok(spans) => spans,
        Err(e) => {
            let at = e.span().map_or(0, |span| span.start);
            out.syntax(text, at, e.message().to_owned());
            return out;
        }
    };
    let table: toml::Table = match toml::from_str(text) {
        Ok(table) => table,
        Err(e) => {
            let at = e.span().map_or(0, |span| span.start);
            out.syntax(text, at, e.message().to_owned());
            return out;
        }
    };
    for (name, node) in spans.get_ref() {
        let (name, at) = (name.get_ref().as_ref(), name.span().start);
        let Some(value) = table.get(name) else {
            continue;
        };
        if name == "window_rule" {
            window_rules(&mut out, text, value, node.get_ref());
        } else if name == "output" {
            outputs(&mut out, text, value, node.get_ref(), at);
        } else if TABLES.contains(&name) {
            subtable(&mut out, text, name, value, node.get_ref(), at);
        } else {
            let message = SettingsError::UnknownKey(name.to_owned()).to_string();
            out.problem(text, at, Some(name), message);
        }
    }
    out.diagnostics.sort_by_key(|d| d.line);
    out
}

fn window_rules(out: &mut LayerParse, text: &str, value: &Value, node: &DeValue<'_>) {
    match keys::validate("window_rule", value) {
        Ok(()) => {
            out.values.insert("window_rule".to_owned(), value.clone());
        }
        Err((rule, error)) => {
            let at = match (rule, node.as_array()) {
                (Some(i), Some(items)) => items.get(i).map_or(0, |item| item.span().start),
                _ => 0,
            };
            out.problem(text, at, Some("window_rule"), error.to_string());
        }
    }
}

fn subtable(
    out: &mut LayerParse,
    text: &str,
    name: &str,
    value: &Value,
    node: &DeValue<'_>,
    at: usize,
) {
    let (Some(entries), Some(nodes)) = (value.as_table(), node.as_table()) else {
        let message = format!("{name}: expected a table");
        out.problem(text, at, Some(name), message);
        return;
    };
    for (sub, _) in nodes {
        let (sub, at) = (sub.get_ref().as_ref(), sub.span().start);
        let key = format!("{name}.{sub}");
        let Some(value) = entries.get(sub) else {
            continue;
        };
        if keys::lookup(&key).is_none() {
            let message = SettingsError::UnknownKey(key.clone()).to_string();
            out.problem(text, at, Some(&key), message);
            continue;
        }
        match keys::validate(&key, value) {
            Ok(()) => {
                out.values.insert(key, value.clone());
            }
            Err((_, error)) => out.problem(text, at, Some(&key), error.to_string()),
        }
    }
}

/// `[output.<connector>]` tables: each field becomes the key
/// `output.<connector>.<field>`.
fn outputs(out: &mut LayerParse, text: &str, value: &Value, node: &DeValue<'_>, at: usize) {
    let (Some(entries), Some(nodes)) = (value.as_table(), node.as_table()) else {
        out.problem(text, at, Some("output"), "output: expected a table".into());
        return;
    };
    for (connector, connector_node) in nodes {
        let (connector, at) = (connector.get_ref().as_ref(), connector.span().start);
        let Some(value) = entries.get(connector) else {
            continue;
        };
        let prefix = format!("output.{connector}");
        if !limits::valid_connector(connector) {
            let message = format!("{prefix}: a connector name is 1 to 32 letters, digits, - or _");
            out.problem(text, at, Some(&prefix), message);
            continue;
        }
        let (Some(fields), Some(field_nodes)) =
            (value.as_table(), connector_node.get_ref().as_table())
        else {
            out.problem(
                text,
                at,
                Some(&prefix),
                format!("{prefix}: expected a table"),
            );
            continue;
        };
        for (field, _) in field_nodes {
            let (field, at) = (field.get_ref().as_ref(), field.span().start);
            let key = format!("{prefix}.{field}");
            let Some(value) = fields.get(field) else {
                continue;
            };
            if keys::lookup(&key).is_none() {
                let message = SettingsError::UnknownKey(key.clone()).to_string();
                out.problem(text, at, Some(&key), message);
                continue;
            }
            match keys::validate(&key, value) {
                Ok(()) => {
                    out.values.insert(key, value.clone());
                }
                Err((_, error)) => out.problem(text, at, Some(&key), error.to_string()),
            }
        }
    }
}
