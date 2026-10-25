//! Freezing the current IDL as the new baseline.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

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
/// names of the files that changed. On a refusal nothing is written, and
/// the copy itself is all or nothing (see `replace_all`).
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

    let changes: Vec<(String, String)> = files
        .into_iter()
        .filter(|(name, text)| {
            fs::read_to_string(baseline_dir.join(name)).ok().as_deref() != Some(text.as_str())
        })
        .collect();
    replace_all(&baseline_dir, &changes)?;
    Ok(changes.into_iter().map(|(name, _)| name).collect())
}

/// Replaces the files in two phases: every new text goes to a temporary
/// name first, then the temporaries are renamed over the targets. A failure
/// removes the temporaries and puts back the files already replaced.
fn replace_all(dir: &Path, changes: &[(String, String)]) -> Result<(), BlessError> {
    let tmp_of = |name: &str| dir.join(format!(".{name}.tmp"));
    let mut written: Vec<PathBuf> = Vec::new();
    for (name, text) in changes {
        let tmp = tmp_of(name);
        written.push(tmp.clone());
        if let Err(e) = fs::write(&tmp, text) {
            remove_all(&written);
            return Err(BlessError::Io(format!(
                "{}: {e}; the baseline was not changed",
                tmp.display()
            )));
        }
    }

    // Previous contents, to put back on failure (None: the file is new).
    let mut done: Vec<(PathBuf, Option<String>)> = Vec::new();
    for (name, _) in changes {
        let target = dir.join(name);
        let previous = fs::read_to_string(&target).ok();
        if let Err(e) = fs::rename(tmp_of(name), &target) {
            remove_all(&written);
            let failed = restore(&done);
            let state = if failed.is_empty() {
                "the baseline was restored".to_string()
            } else {
                format!("the baseline may be torn, restore failed for: {failed:?}")
            };
            return Err(BlessError::Io(format!(
                "{}: {e}; {state}",
                target.display()
            )));
        }
        done.push((target, previous));
    }
    Ok(())
}

fn remove_all(paths: &[PathBuf]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}

/// Puts back the previous contents; returns the files that could not be.
fn restore(done: &[(PathBuf, Option<String>)]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for (path, previous) in done.iter().rev() {
        let result = match previous {
            Some(text) => fs::write(path, text),
            None => fs::remove_file(path),
        };
        if result.is_err() {
            failed.push(path.clone());
        }
    }
    failed
}
