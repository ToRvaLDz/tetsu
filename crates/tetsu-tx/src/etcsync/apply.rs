//! Apply the three-way sync to real directory trees.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, lchown, symlink};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::SyncError;
use super::node::{Kind, Node};
use super::plan::{Action, ConflictKind, decide};
use crate::tx::accounts::{ACCOUNT_FILES, merge};

/// Prefix of the temporary name a file is written under before the rename.
const TMP_PREFIX: &str = ".tetsu-sync.";

/// A path where both sides changed it and the transaction's version won.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Relative to `/etc`.
    pub path: String,
    pub kind: ConflictKind,
    /// The user's copy was kept aside.
    pub kept: bool,
}

/// What a sync did, with every path relative to `/etc`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// The user's version was carried into the new tree.
    pub taken: Vec<String>,
    /// Deleted from the new tree because the user deleted it.
    pub deleted: Vec<String>,
    /// Account files merged from both sides.
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
    /// Sockets, FIFOs and devices, not synced.
    pub skipped: Vec<String>,
}

impl SyncReport {
    /// Nothing was changed, reported or skipped.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Sync the user's changes (`old` against `base`) into `new`. The user's
/// copy of a conflicting path goes under `kept`, created `0700` on first
/// use. Running it again on its own result changes nothing.
pub fn sync(base: &Path, old: &Path, new: &Path, kept: &Path) -> Result<SyncReport, SyncError> {
    let mut run = Run {
        base,
        old,
        new,
        kept,
        report: SyncReport::default(),
    };
    run.walk(Path::new(""))?;
    Ok(run.report)
}

struct Run<'a> {
    base: &'a Path,
    old: &'a Path,
    new: &'a Path,
    kept: &'a Path,
    report: SyncReport,
}

impl Run<'_> {
    /// Decide and apply every child of the directory `rel`.
    fn walk(&mut self, rel: &Path) -> Result<(), SyncError> {
        let mut names: BTreeSet<OsString> = BTreeSet::new();
        for root in [self.base, self.old, self.new] {
            names.extend(child_names(&root.join(rel))?);
        }
        for name in names {
            let path = rel.join(&name);
            if name.to_string_lossy().starts_with(TMP_PREFIX) {
                // Left by a cut sync: never part of the trees.
                remove_any(&self.new.join(&path))?;
                continue;
            }
            let account = rel.as_os_str().is_empty()
                && name.to_str().is_some_and(|n| ACCOUNT_FILES.contains(&n));
            self.entry(&path, account)?;
        }
        Ok(())
    }

    fn entry(&mut self, rel: &Path, account: bool) -> Result<(), SyncError> {
        let (base, old, new) = (
            read(&self.base.join(rel))?,
            read(&self.old.join(rel))?,
            read(&self.new.join(rel))?,
        );
        let action = decide(base.as_ref(), old.as_ref(), new.as_ref(), account);
        let name = rel.to_string_lossy().into_owned();
        let (old_path, new_path) = (self.old.join(rel), self.new.join(rel));
        match action {
            Action::Keep => {}
            Action::Skip => self.report.skipped.push(name),
            Action::TakeOld => {
                copy_tree(&old_path, &new_path)?;
                self.report.taken.push(name);
            }
            Action::DeleteNew => {
                if new.as_ref().is_some_and(Node::is_dir) {
                    // The transaction may have put things in it: those stay.
                    self.walk(rel)?;
                    match fs::remove_dir(&new_path) {
                        Ok(()) => self.report.deleted.push(name),
                        Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => {}
                        Err(e) => return Err(SyncError::io(&new_path, e)),
                    }
                } else {
                    fs::remove_file(&new_path).map_err(|e| SyncError::io(&new_path, e))?;
                    self.report.deleted.push(name);
                }
            }
            Action::MergeAccounts => {
                merge_accounts(&old_path, &new_path, new.as_ref())?;
                self.report.merged.push(name);
            }
            Action::Conflict { kind, keep_old } => {
                if keep_old {
                    self.keep_aside(rel)?;
                }
                self.report.conflicts.push(Conflict {
                    path: name,
                    kind,
                    kept: keep_old,
                });
            }
            Action::Recurse { take_old_attrs } => {
                if take_old_attrs && let Some(o) = &old {
                    set_attrs(&new_path, o)?;
                    self.report.taken.push(name);
                }
                self.walk(rel)?;
            }
        }
        Ok(())
    }

    /// Copy the user's version of `rel` under the kept directory.
    fn keep_aside(&self, rel: &Path) -> Result<(), SyncError> {
        let dest = self.kept.join(rel);
        private_dir(self.kept)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| SyncError::io(parent, e))?;
        }
        remove_any(&dest)?;
        copy_tree(&self.old.join(rel), &dest)
    }
}

/// The names in the directory `dir`; nothing when it is not a directory.
fn child_names(dir: &Path) -> Result<Vec<OsString>, SyncError> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_dir() => {}
        Ok(_) => return Ok(Vec::new()),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(SyncError::io(dir, e)),
    }
    let mut names = Vec::new();
    for item in fs::read_dir(dir).map_err(|e| SyncError::io(dir, e))? {
        names.push(item.map_err(|e| SyncError::io(dir, e))?.file_name());
    }
    Ok(names)
}

fn read(path: &Path) -> Result<Option<Node>, SyncError> {
    Node::read(path).map_err(|e| SyncError::io(path, e))
}

/// Create `dir` and its parents, then make `dir` itself `0700`.
pub(super) fn private_dir(dir: &Path) -> Result<(), SyncError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| SyncError::io(dir, e))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| SyncError::io(dir, e))
}

/// Remove whatever is at `path`, a directory with its tree; a missing path
/// is fine.
fn remove_any(path: &Path) -> Result<(), SyncError> {
    let result = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(SyncError::io(path, e)),
    };
    result.map_err(|e| SyncError::io(path, e))
}

/// The owner and mode of `node` onto `path`, without following a symlink.
fn set_attrs(path: &Path, node: &Node) -> Result<(), SyncError> {
    lchown(path, Some(node.uid), Some(node.gid)).map_err(|e| SyncError::io(path, e))?;
    if !matches!(node.kind, Kind::Symlink(_)) {
        // After the chown: it clears setuid and setgid.
        fs::set_permissions(path, fs::Permissions::from_mode(node.mode))
            .map_err(|e| SyncError::io(path, e))?;
    }
    Ok(())
}

fn tmp_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().unwrap_or(OsStr::new("x"));
    let mut tmp = OsString::from(TMP_PREFIX);
    tmp.push(name);
    dest.with_file_name(tmp)
}

/// Copy `src` (a file, symlink or directory tree) to `dest`, replacing what
/// is there. Owner, mode and, for files, the modification time are kept.
/// Files and links are written under a temporary name and renamed.
fn copy_tree(src: &Path, dest: &Path) -> Result<(), SyncError> {
    let node = read(src)?
        .ok_or_else(|| SyncError::io(src, std::io::Error::new(ErrorKind::NotFound, "vanished")))?;
    // A rename cannot replace a directory, or be replaced by one.
    match fs::symlink_metadata(dest) {
        Ok(m) if m.file_type().is_dir() || node.is_dir() => remove_any(dest)?,
        _ => {}
    }
    match &node.kind {
        Kind::Special => Ok(()),
        Kind::Dir => {
            fs::create_dir(dest).map_err(|e| SyncError::io(dest, e))?;
            lchown(dest, Some(node.uid), Some(node.gid)).map_err(|e| SyncError::io(dest, e))?;
            for name in child_names(src)? {
                copy_tree(&src.join(&name), &dest.join(&name))?;
            }
            // Last, so a read-only mode does not stop the copy.
            set_attrs(dest, &node)
        }
        Kind::Symlink(target) => {
            let tmp = tmp_path(dest);
            remove_any(&tmp)?;
            symlink(target, &tmp).map_err(|e| SyncError::io(&tmp, e))?;
            set_attrs(&tmp, &node)?;
            fs::rename(&tmp, dest).map_err(|e| SyncError::io(dest, e))
        }
        Kind::File(data) => {
            let modified = fs::symlink_metadata(src)
                .and_then(|m| m.modified())
                .map_err(|e| SyncError::io(src, e))?;
            write_file(dest, data, &node, Some(modified))
        }
    }
}

/// Write `data` to `dest` through a temporary name and a rename, with the
/// owner and mode of `attrs`.
fn write_file(
    dest: &Path,
    data: &[u8],
    attrs: &Node,
    modified: Option<std::time::SystemTime>,
) -> Result<(), SyncError> {
    let tmp = tmp_path(dest);
    remove_any(&tmp)?;
    let io = |e| SyncError::io(&tmp, e);
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(io)?;
    file.write_all(data).map_err(io)?;
    if let Some(time) = modified {
        file.set_times(FileTimes::new().set_modified(time))
            .map_err(io)?;
    }
    file.sync_all().map_err(io)?;
    drop(file);
    set_attrs(&tmp, attrs)?;
    fs::rename(&tmp, dest).map_err(|e| SyncError::io(dest, e))
}

/// Write the union of the two account files over `new`, keeping its owner
/// and mode.
fn merge_accounts(old: &Path, new: &Path, new_node: Option<&Node>) -> Result<(), SyncError> {
    let Some(attrs) = new_node else {
        return Ok(());
    };
    let read_text = |p: &Path| fs::read_to_string(p).map_err(|e| SyncError::io(p, e));
    let merged = merge(&read_text(new)?, &read_text(old)?);
    let modified = fs::metadata(new).and_then(|m| m.modified()).ok();
    write_file(new, merged.as_bytes(), attrs, modified)
}
