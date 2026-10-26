//! `org.tetsu.tx.v1`: the wire types, shaped exactly as the IDL.
//!
//! The AUR types (`AurReview`, `AurPin`) are not here yet.

use serde::{Deserialize, Serialize};

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.tx.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.tx.v1.varlink");
/// `TransactionInProgress (id: string)`.
pub const TRANSACTION_IN_PROGRESS: &str = "org.tetsu.tx.v1.TransactionInProgress";
/// `PackageNotFound (name: string)`.
pub const PACKAGE_NOT_FOUND: &str = "org.tetsu.tx.v1.PackageNotFound";
/// `NotUserPackage (name: string)`.
pub const NOT_USER_PACKAGE: &str = "org.tetsu.tx.v1.NotUserPackage";
/// `SnapshotNotFound (id: string)`.
pub const SNAPSHOT_NOT_FOUND: &str = "org.tetsu.tx.v1.SnapshotNotFound";
/// `TransactionFailed (id: string, reason: string)`.
pub const TRANSACTION_FAILED: &str = "org.tetsu.tx.v1.TransactionFailed";
/// `AurPackageNotFound (name: string)`.
pub const AUR_PACKAGE_NOT_FOUND: &str = "org.tetsu.tx.v1.AurPackageNotFound";
/// `AurCommitMismatch (name: string, reviewed: string, current: string)`.
pub const AUR_COMMIT_MISMATCH: &str = "org.tetsu.tx.v1.AurCommitMismatch";
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.tx.v1.PermissionDenied";

/// `PackageChange`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    /// The package.
    pub name: String,
    /// Null when the package was added.
    #[serde(default)]
    pub from: Option<String>,
    /// Null when the package was removed.
    #[serde(default)]
    pub to: Option<String>,
}

/// `Transaction.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    /// Update the base image and the repo packages.
    Update,
    /// Add repo packages.
    Install,
    /// Build and add AUR packages.
    InstallAur,
    /// Remove packages.
    Remove,
    /// Roll back to an earlier snapshot.
    Rollback,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `Transaction.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxState {
    /// Still running.
    Running,
    /// Finished and took effect at the next boot.
    Succeeded,
    /// Finished with the system unchanged.
    Failed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `Transaction`. Nullable fields are written as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// Opaque; compare for equality only.
    pub id: String,
    /// What it does.
    pub kind: TxKind,
    /// The packages asked for.
    pub packages: Vec<String>,
    /// Where it stands.
    pub state: TxState,
    /// RFC 3339 UTC.
    pub started_at: String,
    /// RFC 3339 UTC, once finished.
    #[serde(default)]
    pub finished_at: Option<String>,
    /// The snapshot produced.
    #[serde(default)]
    pub snapshot: Option<String>,
    /// What it changed, filled when it finishes.
    pub changes: Vec<PackageChange>,
    /// May quote pacman or build output; untrusted text.
    #[serde(default)]
    pub failure: Option<String>,
}

/// `Progress.phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The new snapshot is being made.
    Snapshot,
    /// An AUR package is being built.
    Build,
    /// Packages are being changed.
    Packages,
    /// The boot entry is being made.
    BootEntry,
    /// The transaction is being committed.
    Commit,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `Progress`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Where the transaction is.
    pub phase: Phase,
    /// Null when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<i64>,
    /// Untrusted text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Progress {
    /// A phase with no percent and no message.
    pub fn phase(phase: Phase) -> Self {
        Self {
            phase,
            percent: None,
            message: None,
        }
    }
}

/// `TxEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxEventKind {
    /// The first event: the running transaction.
    Initial,
    /// A transaction started.
    Started,
    /// A transaction moved to a phase.
    Progress,
    /// A transaction finished.
    Finished,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchTransactions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxEvent {
    /// Which event.
    pub kind: TxEventKind,
    /// `initial`: the running transaction, zero or one entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<Vec<Transaction>>,
    /// `started`, `finished`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<Transaction>,
    /// `progress`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `progress`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
}

impl TxEvent {
    fn of(kind: TxEventKind) -> Self {
        Self {
            kind,
            running: None,
            transaction: None,
            id: None,
            progress: None,
        }
    }

    /// `initial`.
    pub fn initial(running: Vec<Transaction>) -> Self {
        Self {
            running: Some(running),
            ..Self::of(TxEventKind::Initial)
        }
    }

    /// `started`.
    pub fn started(transaction: Transaction) -> Self {
        Self {
            transaction: Some(transaction),
            ..Self::of(TxEventKind::Started)
        }
    }

    /// `progress`.
    pub fn progress(id: &str, progress: Progress) -> Self {
        Self {
            id: Some(id.to_owned()),
            progress: Some(progress),
            ..Self::of(TxEventKind::Progress)
        }
    }

    /// `finished`.
    pub fn finished(transaction: Transaction) -> Self {
        Self {
            transaction: Some(transaction),
            ..Self::of(TxEventKind::Finished)
        }
    }
}

/// The reply of a write method: `progress` while it runs, `transaction`
/// at the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxReply {
    /// A progress reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// The final reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<Transaction>,
}

impl TxReply {
    /// A progress reply.
    pub fn progress(progress: Progress) -> Self {
        Self {
            progress: Some(progress),
            transaction: None,
        }
    }

    /// The final reply.
    pub fn finished(transaction: Transaction) -> Self {
        Self {
            progress: None,
            transaction: Some(transaction),
        }
    }
}

/// `UserPackage.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageSource {
    /// From a repository.
    Repo,
    /// From the AUR.
    Aur,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `UserPackage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPackage {
    /// The package.
    pub name: String,
    /// Where it comes from.
    pub source: PackageSource,
    /// The AUR commit the package is pinned to.
    #[serde(default)]
    pub aur_commit: Option<String>,
}

/// `GetStatus.default_boot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultBoot {
    /// Never tried.
    Untried,
    /// Booted fine.
    Good,
    /// Failed and fell back.
    Bad,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// The reply of `GetStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The booted snapshot.
    pub booted: String,
    /// The snapshot the next boot uses.
    pub default: String,
    /// State of the default entry.
    pub default_boot: DefaultBoot,
}

/// The parameters of `Install` and `Remove`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PackagesParams {
    /// Package names.
    pub packages: Vec<String>,
}

/// The parameters of `Rollback`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RollbackParams {
    /// Snapshot id; null for the newest good one older than the booted one.
    #[serde(default)]
    pub to: Option<String>,
}

/// The parameters of `ListTransactions`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListParams {
    /// Maximum number of transactions; null for all.
    #[serde(default)]
    pub limit: Option<i64>,
}
