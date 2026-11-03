//! tetsu-secrets: the secrets vault.
//!
//! - [`unlock`] reads the vault key from the credential systemd decrypted,
//!   [`init`] seals a fresh one at first login;
//! - [`store`] is the oo7 keyring file, [`model`] the checked vocabulary;
//! - [`broker`] puts the rules around the store (binding, audit before
//!   acting, no agent writes), [`audit`] writes the journal entries;
//! - [`service`] serves `org.tetsu.secrets.v1`, [`daemon`] runs it;
//! - [`cli`] is `tetsu secrets`, [`staging`] the import of the key the
//!   installer staged.
//!
//! See the README for paths, units and the audit fields.

pub mod audit;
pub mod broker;
pub mod caller;
pub mod cli;
pub mod daemon;
pub mod init;
pub mod model;
pub mod paths;
pub mod service;
pub mod staging;
pub mod store;
pub mod unlock;
pub mod vault;
