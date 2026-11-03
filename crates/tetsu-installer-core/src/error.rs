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
    /// A Secure Boot enrollment failed at a write and was rolled back as far
    /// as it could be; the message says what the firmware holds now.
    #[error("{}", enrollment_message(.var, .source, .outcome))]
    EnrollmentFailed {
        /// The variable whose write failed: `db`, `KEK` or `PK`.
        var: &'static str,
        #[source]
        source: Box<InstallError>,
        outcome: Rollback,
    },
    /// The QR code cannot be built for the payload.
    #[error("cannot build QR code: {0}")]
    Qr(String),
}

/// What a failed enrollment left in the firmware.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rollback {
    /// The first write failed: nothing changed.
    NothingWritten,
    /// Every variable written before the failure was put back.
    Restored(Vec<&'static str>),
    /// Some restores failed: Tetsu's entries are still in `left`.
    Left {
        restored: Vec<&'static str>,
        /// The variable and why its restore failed.
        left: Vec<(&'static str, String)>,
    },
}

/// `db`, `db and KEK`, `db, KEK and PK`.
fn names(vars: &[&str]) -> String {
    match vars {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn were(vars: &[&str]) -> &'static str {
    if vars.len() == 1 { "was" } else { "were" }
}

fn enrollment_message(var: &str, source: &InstallError, outcome: &Rollback) -> String {
    let failed = format!("the {var} write failed ({source})");
    match outcome {
        Rollback::NothingWritten => {
            format!("{failed}: nothing was written, db and KEK are unchanged")
        }
        Rollback::Restored(vars) => format!(
            "{failed}: {} {} restored, the firmware is still in Setup Mode",
            names(vars),
            were(vars)
        ),
        Rollback::Left { restored, left } => {
            let done = if restored.is_empty() {
                String::new()
            } else {
                format!("{} {} restored; ", names(restored), were(restored))
            };
            format!(
                "{failed}: {done}Tetsu's entries are left in {}: reset the Secure Boot keys in \
                 the firmware setup (or clear them and run `tetsu secureboot enroll` again)",
                names(&left.iter().map(|(v, _)| *v).collect::<Vec<_>>())
            )
        }
    }
}

impl InstallError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
