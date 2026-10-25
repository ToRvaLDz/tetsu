//! Installer core: install state, steps and hardware detection behind a
//! `Frontend`. Nothing here writes to a disk or to firmware variables.

pub mod agent;
pub mod disk;
pub mod entropy;
pub mod env;
pub mod error;
pub mod frontend;
pub mod keyboard;
pub mod preflight;
pub mod secret;
#[cfg(test)]
pub(crate) mod testing;
pub mod unlock;
pub mod user;
