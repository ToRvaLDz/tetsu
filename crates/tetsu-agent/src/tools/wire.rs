//! Provider-safe tool names that map 1:1 back to the tool id.
//!
//! The wire name is the id with `.` replaced by `_`. It is reversible
//! because the segments of a Varlink interface name and a method name
//! cannot contain `_`.

use std::fmt;

/// The longest wire name, the strictest rule known among providers.
pub const MAX_WIRE_LEN: usize = 64;

/// Why an id cannot be a wire name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub String);

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WireError {}

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// The wire name of a tool id. Refused when it would not map back, is too
/// long, or has a character a provider would reject.
pub fn wire_name(id: &str) -> Result<String, WireError> {
    if id.is_empty() {
        return Err(WireError("empty tool id".to_owned()));
    }
    if id.contains('_') {
        return Err(WireError(format!(
            "`{id}` holds `_`, so it is not reversible"
        )));
    }
    let name = id.replace('.', "_");
    if !name.chars().all(allowed) {
        return Err(WireError(format!(
            "`{id}` holds a character a provider rejects"
        )));
    }
    if name.len() > MAX_WIRE_LEN {
        return Err(WireError(format!(
            "`{id}` is {} characters, over the limit of {MAX_WIRE_LEN}",
            name.len()
        )));
    }
    Ok(name)
}

/// The tool id a wire name stands for, or `None` for a string that is not
/// a wire name. It does not check that the tool exists.
pub fn from_wire(name: &str) -> Option<String> {
    let valid = !name.is_empty() && name.len() <= MAX_WIRE_LEN && name.chars().all(allowed);
    valid.then(|| name.replace('_', "."))
}
