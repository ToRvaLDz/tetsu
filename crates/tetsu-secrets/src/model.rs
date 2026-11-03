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

fn lower_alnum(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

impl SecretId {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        let mut chars = text.chars();
        let ok = text.len() <= 64
            && chars.next().is_some_and(lower_alnum)
            && chars.all(|c| lower_alnum(c) || c == '_' || c == '-');
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(invalid(
                "id",
                "must be 1 to 64 of a-z, 0-9, `-` and `_`, starting with a letter or digit",
            ))
        }
    }
}

const HOST_RULE: &str = "must be a lowercase DNS name or IP with an optional port";

fn dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| lower_alnum(c) || c == '-')
}

fn port(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 5
        && text.chars().all(|c| c.is_ascii_digit())
        && text.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p))
}

/// `[v6]` or `[v6]:port`.
fn bracketed(text: &str) -> bool {
    let Some((inner, rest)) = text.strip_prefix('[').and_then(|t| t.split_once(']')) else {
        return false;
    };
    let port_ok = rest.is_empty() || rest.strip_prefix(':').is_some_and(port);
    port_ok && inner.parse::<std::net::Ipv6Addr>().is_ok()
}

fn dns_or_v4(text: &str) -> bool {
    let (name, port_ok) = match text.rsplit_once(':') {
        Some((name, p)) => (name, port(p)),
        None => (text, true),
    };
    port_ok && name.len() <= 253 && name.split('.').all(dns_label)
}

impl Host {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        if bracketed(text) || (!text.starts_with('[') && dns_or_v4(text)) {
            Ok(Self(text.to_owned()))
        } else {
            Err(invalid("host", HOST_RULE))
        }
    }
}

impl ToolId {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        let mut chars = text.chars();
        let ok = chars.next().is_some_and(lower_alnum)
            && chars.all(|c| lower_alnum(c) || matches!(c, '.' | '_' | '-'));
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(invalid(
                "allowed_tools",
                "a tool id is a-z, 0-9, `.`, `_` and `-`, starting with a letter or digit",
            ))
        }
    }
}

/// RFC 7230 `tchar`.
fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

impl HeaderName {
    /// Checks `text`.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        if !text.is_empty() && text.chars().all(token_char) {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(invalid("header_name", "must be an HTTP header name"))
        }
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
        let bad = || invalid("kind", "must be bearer, header:<name> or basic");
        match text {
            "bearer" => Ok(Self::Bearer),
            "basic" => Ok(Self::Basic),
            _ => {
                let name = text.strip_prefix("header:").ok_or_else(bad)?;
                HeaderName::parse(name).map(Self::Header).map_err(|_| bad())
            }
        }
    }

    /// The wire form: the kind and, for a header, its name.
    pub fn to_wire(&self) -> (SecretKind, Option<String>) {
        match self {
            Self::Bearer => (SecretKind::Bearer, None),
            Self::Basic => (SecretKind::Basic, None),
            Self::Header(name) => (SecretKind::Header, Some(name.as_str().to_owned())),
        }
    }

    /// From the wire form; a header needs its name and the others refuse one.
    pub fn from_wire(kind: SecretKind, header_name: Option<&str>) -> Result<Self, ModelError> {
        match (kind, header_name) {
            (SecretKind::Header, Some(name)) => HeaderName::parse(name).map(Self::Header),
            (SecretKind::Header, None) => {
                Err(invalid("header_name", "is required for kind header"))
            }
            (SecretKind::Bearer | SecretKind::Basic, Some(_)) => {
                Err(invalid("header_name", "is only for kind header"))
            }
            (SecretKind::Bearer, None) => Ok(Self::Bearer),
            (SecretKind::Basic, None) => Ok(Self::Basic),
            (SecretKind::Unknown, _) => Err(invalid("kind", "is not a kind this build knows")),
        }
    }
}

/// A value to store: not empty, no NUL.
pub fn check_value(value: &str) -> Result<(), ModelError> {
    if value.is_empty() || value.contains('\0') {
        Err(invalid("value", "must not be empty or contain NUL"))
    } else {
        Ok(())
    }
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
