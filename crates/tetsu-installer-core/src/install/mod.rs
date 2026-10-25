//! The installation proper: the plan built from the confirmed answers and
//! the executor that runs it.

use std::ffi::{OsStr, OsString};

mod boot;
pub mod config;
pub mod exec;
mod firmware;
pub mod log;
pub mod plan;
pub mod rundir;
mod storage;
mod system;
pub mod target;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_boot;
#[cfg(test)]
mod tests_firmware;

pub use exec::{Deps, Executor};
pub use plan::{Action, InstallPlan, KeyKind, Stage};
pub use target::InstallTarget;

/// Command arguments from anything path- or string-like.
pub(crate) fn argv(parts: &[&dyn AsRef<OsStr>]) -> Vec<OsString> {
    parts.iter().map(|p| (**p).as_ref().to_owned()).collect()
}
