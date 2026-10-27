//! The vocabulary of the vault: ids, kinds, hosts and tool ids, each
//! checked on the way in. An error names the field and the rule, never the
//! rejected text.

use std::fmt;

use tetsu_bus::secrets::SecretKind;

/// A rejected input: the field and the rule it broke.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {field}: {reason}")]
pub struct ModelError {
    /// The wire name of the field.
    pub field: &'static str,
    /// The rule it broke.
    pub reason: &'static str,
}

fn invalid(field: &'static str, reason: &'static str) -> ModelError {
    ModelError { field, reason }
}

macro_rules! text_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// The checked text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

text_newtype!(
    /// The name of a secret: `[a-z0-9][a-z0-9_-]{0,63}`.
    SecretId
);
text_newtype!(
    /// An upstream host: a lowercase DNS name or IP, optionally `:port`.
    Host
);
text_newtype!(
    /// A tool id: `[a-z0-9][a-z0-9._-]*`.
    ToolId
);
text_newtype!(
    /// An HTTP header name, lowercase (an RFC 7230 token).
    HeaderName
);

impl SecretId {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        todo!("{text}")
    }
}

impl Host {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        todo!("{text}")
    }
}

impl ToolId {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        todo!("{text}")
    }
}

impl HeaderName {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        todo!("{text}")
    }
}

/// How a value is presented upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KindSpec {
    /// `Authorization: Bearer <value>`.
    Bearer,
    /// The value in this header.
    Header(HeaderName),
    /// HTTP basic authentication.
    Basic,
}

impl KindSpec {
    /// The CLI form: `bearer`, `header:<name>` or `basic`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        todo!("{text}")
    }

    /// The wire form: the kind and, for a header, its name.
    pub fn to_wire(&self) -> (SecretKind, Option<String>) {
        todo!()
    }

    /// From the wire form; a header needs its name and the others refuse one.
    pub fn from_wire(kind: SecretKind, header_name: Option<&str>) -> Result<Self, ModelError> {
        todo!("{kind:?} {header_name:?}")
    }
}

/// A value to store: not empty, no NUL.
pub fn check_value(value: &str) -> Result<(), ModelError> {
    todo!("{}", value.len())
}

/// A secret without its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    /// The name.
    pub id: SecretId,
    /// How the value is presented.
    pub kind: KindSpec,
    /// The only host the value may go to.
    pub host: Host,
    /// Tool ids that may receive the value.
    pub allowed_tools: Vec<ToolId>,
    /// Unix seconds when added.
    pub created_at: u64,
    /// Unix seconds when last rotated.
    pub rotated_at: Option<u64>,
}

#[cfg(test)]
mod tests;
