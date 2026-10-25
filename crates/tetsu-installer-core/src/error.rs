//! Errors raised by the installer core.

use std::path::PathBuf;

use tetsu_tx::boot::BootError;
use tetsu_tx::store::StoreError;
use tetsu_tx::tx::TxError;

/// Errors raised while collecting the installation answers.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The frontend has no more input (closed stream, user abort).
    #[error("installation aborted")]
    Aborted,
    /// The machine cannot be installed to; the message says why.
    #[error("preflight failed: {0}")]
    Preflight(String),
    /// No disk can receive the installation.
    #[error("no candidate disk found")]
    NoDisk,
    /// `lsblk` produced output that cannot be parsed.
    #[error("cannot parse lsblk output: {0}")]
    Lsblk(#[from] serde_json::Error),
    /// An external tool failed.
    #[error(transparent)]
    Tool(#[from] BootError),
    /// Filesystem error, with the path involved.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The snapshot store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A transaction step (initrd build) failed.
    #[error(transparent)]
    Tx(#[from] TxError),
    /// The installation was cancelled between two actions.
    #[error("installation cancelled")]
    Cancelled,
    /// An action of the install plan failed; the install stopped there.
    #[error("{action}: {source}")]
    ActionFailed {
        action: String,
        #[source]
        source: Box<InstallError>,
    },
    /// An installation step cannot proceed; the message says why.
    #[error("{0}")]
    Failed(String),
    /// An enrollment without the Microsoft certificates was not asked for.
    #[error(
        "the Microsoft Secure Boot certificates are missing: enrolling only Tetsu's keys \
         would stop Windows and firmware option ROMs from booting"
    )]
    NoMicrosoftCerts,
    /// The QR code cannot be built for the payload.
    #[error("cannot build QR code: {0}")]
    Qr(String),
}

impl InstallError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
