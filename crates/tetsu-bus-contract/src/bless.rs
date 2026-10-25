//! Freezing the current IDL as the new baseline.

use std::fmt;
use std::fs;
use std::path::Path;

use crate::load::{STREAMING_LIST, read_dir_sources};
use crate::{LoadError, Violation, check_compat, check_rules, load_dir};

/// Why the baseline was not updated.
#[derive(Debug)]
pub enum BlessError {
    Load(LoadError),
    /// The current IDL breaks the baseline or a tag rule.
    Violations(Vec<Violation>),
    Io(String),
}

impl fmt::Display for BlessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlessError::Load(e) => write!(f, "{e}"),
            BlessError::Violations(v) => write!(f, "{} violation(s)", v.len()),
            BlessError::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for BlessError {}

impl From<LoadError> for BlessError {
    fn from(e: LoadError) -> Self {
        BlessError::Load(e)
    }
}

/// Copies the `*.varlink` files and `streaming.list` of `idl_dir` into its
/// `baseline/` directory, but only when the current IDL is a compatible
/// superset of the existing baseline and passes the tag rules. Returns the
/// names of the files that changed. On a refusal nothing is written.
pub fn bless(idl_dir: &Path) -> Result<Vec<String>, BlessError> {
    let baseline_dir = idl_dir.join("baseline");
    let baseline = load_dir(&baseline_dir)?;
    let current = load_dir(idl_dir)?;

    let mut violations = check_compat(&baseline, &current);
    violations.extend(check_rules(&current));
    if !violations.is_empty() {
        return Err(BlessError::Violations(violations));
    }

    let mut files = read_dir_sources(idl_dir)?;
    let list_path = idl_dir.join(STREAMING_LIST);
    let list = fs::read_to_string(&list_path)
        .map_err(|e| BlessError::Io(format!("{}: {e}", list_path.display())))?;
    files.push((STREAMING_LIST.to_string(), list));

    let mut changed = Vec::new();
    for (name, text) in files {
        let target = baseline_dir.join(&name);
        if fs::read_to_string(&target).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        fs::write(&target, text)
            .map_err(|e| BlessError::Io(format!("{}: {e}", target.display())))?;
        changed.push(name);
    }
    Ok(changed)
}
