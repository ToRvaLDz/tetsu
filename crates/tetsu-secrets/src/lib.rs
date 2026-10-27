//! tetsu-secrets: the secrets vault contract and the first-run import of
//! the API key the installer staged. The real vault is built up in steps;
//! see each module.

pub mod audit;
pub mod caller;
pub mod init;
pub mod model;
pub mod paths;
pub mod staging;
pub mod store;
pub mod unlock;
pub mod vault;
