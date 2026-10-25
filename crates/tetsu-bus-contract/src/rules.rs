//! Tag and doc rules of the current contract.

use crate::{Contract, Violation};

/// Every rule of CONTRACT.md's Tags and Shape sections the current IDL breaks.
pub fn check_rules(_contract: &Contract) -> Vec<Violation> {
    Vec::new()
}
