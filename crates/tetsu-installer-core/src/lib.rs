//! Installer core: install state, steps and hardware detection behind a
//! `Frontend`. Nothing here writes to a disk or to firmware variables.

pub mod agent;
pub mod answers;
pub mod disk;
pub mod driver;
pub mod efi;
pub mod entropy;
pub mod env;
pub mod error;
pub mod frontend;
pub mod install;
pub mod keyboard;
pub mod preflight;
pub mod secret;
pub mod secureboot;
pub mod summary;
#[cfg(test)]
pub(crate) mod testing;
pub mod unlock;
pub mod unlockfile;
pub mod user;
