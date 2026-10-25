//! What the importer needs from a vault.

use std::fmt;

/// Secret text kept in memory only; `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// A vault refused or failed to store a secret.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct VaultError(pub String);

/// Where API keys are kept.
pub trait Vault {
    fn store_api_key(&mut self, provider: &str, key: &Secret) -> Result<(), VaultError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_value() {
        let s = Secret::new("sk-123");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(s.expose(), "sk-123");
    }
}
