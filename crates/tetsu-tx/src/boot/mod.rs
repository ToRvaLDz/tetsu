//! Boot side of a transaction: UKI assembly and signing, ESP commit,
//! retention and free-space checks. No root, no Btrfs, no orchestration.

use std::path::PathBuf;

/// Errors raised by the boot module.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// The ESP cannot hold the incoming entry even after garbage collection.
    #[error("insufficient ESP space: need {needed} bytes, {available} available")]
    InsufficientSpace { needed: u64, available: u64 },
    /// An external tool exited with a failure status.
    #[error("{tool} failed ({status}): {stderr}")]
    Tool {
        tool: String,
        status: String,
        stderr: String,
    },
    /// An external tool is not installed.
    #[error("required tool not found: {tool}")]
    ToolMissing { tool: String },
    /// Filesystem error, with the path involved.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The signing key URI scheme is recognised but not implemented yet.
    #[error("unsupported key URI: {0}")]
    UnsupportedKeyUri(String),
    /// A new entry would not be newer than the existing ones.
    #[error("stale counter {counter}: newest existing entry is {newest}")]
    StaleCounter { counter: u64, newest: u64 },
}

impl BootError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub mod entry;
pub mod esp;
pub mod retention;
pub mod space;
pub mod keys;
pub mod tool;
pub mod uki;
