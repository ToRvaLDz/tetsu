//! What `tetsu comp config` prints.

use std::fmt::Write as _;

use tetsu_bus::config::{ConfigEntry, Diagnostic, GetConfigReply, Origin};

const HEADER: [&str; 4] = ["KEY", "VALUE", "ORIGIN", "WRITE"];
/// The columns before the last are padded to the widest cell.
const SEPARATOR: &str = "  ";

/// The effective value, in full, on one line.
fn value(entry: &ConfigEntry) -> String {
    serde_json::to_string(&entry.value).unwrap_or_else(|e| format!("<{e}>"))
}

/// The layer the value comes from, saying so when it hides a managed one.
fn origin(entry: &ConfigEntry) -> &'static str {
    match entry.origin {
        Origin::Default => "default",
        Origin::Managed => "managed",
        Origin::User if entry.managed_value.as_ref().is_some_and(|v| !v.is_null()) => {
            "user (overrides managed)"
        }
        Origin::User => "user",
    }
}

/// Whether `SetManaged` takes the key, else what writes it.
fn writable(entry: &ConfigEntry) -> String {
    if entry.writable {
        "yes".to_owned()
    } else {
        entry
            .method
            .clone()
            .unwrap_or_else(|| "user file only".to_owned())
    }
}

fn diagnostic(d: &Diagnostic) -> String {
    let mut line = String::from("  ");
    if let Some(n) = d.line {
        let _ = write!(line, "line {n}: ");
    }
    if let Some(key) = &d.key {
        let _ = write!(line, "{key}: ");
    }
    line.push_str(&d.message);
    line
}

/// The table of keys, then the diagnostics of the user file if any.
pub fn table(reply: &GetConfigReply) -> String {
    let mut rows: Vec<[String; 4]> = vec![HEADER.map(str::to_owned)];
    rows.extend(
        reply
            .entries
            .iter()
            .map(|e| [e.key.clone(), value(e), origin(e).to_owned(), writable(e)]),
    );
    let width = |column: usize| rows.iter().map(|r| r[column].chars().count()).max();
    let widths = [width(0), width(1), width(2), Some(0)].map(Option::unwrap_or_default);
    let mut out = String::new();
    for row in &rows {
        let mut line = String::new();
        for (cell, width) in row.iter().zip(widths) {
            let _ = write!(line, "{cell:<width$}{SEPARATOR}");
        }
        // The last column is not padded, and nothing trails.
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if !reply.diagnostics.is_empty() {
        out.push_str("\ndiagnostics of the user file:\n");
        for d in &reply.diagnostics {
            out.push_str(&diagnostic(d));
            out.push('\n');
        }
    }
    out
}

/// The reply in its wire shape, pretty-printed.
pub fn json(reply: &GetConfigReply) -> String {
    let mut text =
        serde_json::to_string_pretty(reply).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tetsu_bus::config::{ConfigEntry, Diagnostic, Origin};

    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn entry(
        key: &str,
        value: Value,
        origin: Origin,
        managed: Option<Value>,
        writable: bool,
        method: Option<&str>,
    ) -> ConfigEntry {
        ConfigEntry {
            key: key.into(),
            default_value: None,
            user_value: (origin == Origin::User).then(|| value.clone()),
            managed_value: managed,
            value,
            origin,
            writable,
            method: method.map(str::to_owned),
        }
    }

    fn diagnostic(line: Option<i64>, key: Option<&str>, message: &str) -> Diagnostic {
        Diagnostic {
            line,
            key: key.map(str::to_owned),
            message: message.into(),
        }
    }

    fn reply(diagnostics: Vec<Diagnostic>) -> GetConfigReply {
        GetConfigReply {
            entries: vec![
                entry(
                    "layout.gaps_outer",
                    json!(7),
                    Origin::User,
                    None,
                    true,
                    None,
                ),
                entry(
                    "keyboard.layout",
                    json!("us"),
                    Origin::Default,
                    None,
                    true,
                    None,
                ),
                entry(
                    "idle.timeout_ms",
                    json!(300_000),
                    Origin::User,
                    Some(json!(450_000)),
                    true,
                    None,
                ),
                entry(
                    "render.device",
                    json!("dev-1"),
                    Origin::Managed,
                    Some(json!("dev-1")),
                    false,
                    Some("org.tetsu.comp.render.v1.SetRenderDevice"),
                ),
                entry(
                    "output.order",
                    json!(["a", "b"]),
                    Origin::Default,
                    None,
                    false,
                    None,
                ),
            ],
            diagnostics,
        }
    }

    const TABLE: &str = "\
KEY                VALUE      ORIGIN                    WRITE
layout.gaps_outer  7          user                      yes
keyboard.layout    \"us\"       default                   yes
idle.timeout_ms    300000     user (overrides managed)  yes
render.device      \"dev-1\"    managed                   org.tetsu.comp.render.v1.SetRenderDevice
output.order       [\"a\",\"b\"]  default                   user file only
";

    #[test]
    fn the_table_is_aligned_and_complete() {
        assert_eq!(table(&reply(vec![])), TABLE);
    }

    #[test]
    fn the_diagnostics_follow_the_table_in_all_their_forms() {
        let text = table(&reply(vec![
            diagnostic(Some(3), Some("idle.timeout_ms"), "expected an integer"),
            diagnostic(Some(9), None, "not a key"),
            diagnostic(None, Some("layout.gaps_outer"), "out of range"),
            diagnostic(None, None, "the file is not valid TOML"),
        ]));
        let expected = format!(
            "{TABLE}\n\
             diagnostics of the user file:\n  \
             line 3: idle.timeout_ms: expected an integer\n  \
             line 9: not a key\n  \
             layout.gaps_outer: out of range\n  \
             the file is not valid TOML\n"
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn no_diagnostics_means_no_section() {
        assert!(!table(&reply(vec![])).contains("diagnostics"));
    }

    #[test]
    fn no_entries_leaves_the_header() {
        let empty = GetConfigReply::default();
        assert_eq!(table(&empty), "KEY  VALUE  ORIGIN  WRITE\n");
    }

    #[test]
    fn json_is_the_wire_reply_with_its_nulls() {
        let sent = reply(vec![diagnostic(None, None, "x")]);
        let text = json(&sent);
        let back: GetConfigReply = serde_json::from_str(&text).unwrap();
        assert_eq!(back, sent);
        let wire: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(wire["entries"][0]["managed_value"], Value::Null);
        assert!(
            wire["entries"][0]
                .as_object()
                .unwrap()
                .contains_key("method")
        );
        assert!(
            wire["diagnostics"][0]
                .as_object()
                .unwrap()
                .contains_key("line")
        );
    }
}
