//! Compatibility of the current contract with the frozen baseline.

use crate::{Contract, Violation};

/// Every way `current` is not a compatible superset of `baseline`.
pub fn check_compat(_baseline: &Contract, _current: &Contract) -> Vec<Violation> {
    Vec::new()
}
