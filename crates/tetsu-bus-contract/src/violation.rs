//! What a check reports.

use std::fmt;

/// The kind of a contract violation; stable, so tests can assert on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    // Compatibility with the baseline.
    InterfaceRemoved,
    MethodRemoved,
    TypeRemoved,
    FieldRemoved,
    ErrorRemoved,
    EnumValueRemoved,
    Retyped,
    OptionalityChanged,
    NewRequired,
    TagRemoved,
    ErrorClassChanged,
    StreamingChanged,
    // Tag and doc rules.
    UnknownTag,
    MisplacedTag,
    DuplicateTag,
    ErrorClass,
    MissingDoc,
    MissingPermissionDenied,
    StreamingUnknown,
    WatchNotStreaming,
    // Event fixtures.
    MissingFixture,
    OrphanFixture,
    InvalidFixture,
    FixtureKindMismatch,
    FixtureUnknownField,
    FixtureMissingField,
    FixtureWrongType,
}

/// One broken rule, with a readable path to the member it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: Kind,
    /// For example `org.tetsu.tx.v1.Transaction.failure`.
    pub path: String,
    pub detail: String,
}

impl Violation {
    pub fn new(kind: Kind, path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.detail)
    }
}
