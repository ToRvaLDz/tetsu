//! The `/var` report: what the install phase left in the throwaway `/var`
//! that no `tmpfiles.d` entry of the snapshot accounts for. A package that
//! writes state under `/var` at install time loses it with the scratch, so
//! the transaction log says so.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::TxError;

/// The bound package cache inside the scratch `/var`, relative to it.
const CACHE_REL: &str = "cache/pacman/pkg";

fn io_failure(path: &Path, e: &std::io::Error) -> TxError {
    TxError::failed(format!("{}: {e}", path.display()))
}

/// The first two fields of a `tmpfiles.d` line: the type and the path, the
/// path possibly double-quoted.
fn path_column(line: &str) -> Option<&str> {
    let mut fields = line.trim().splitn(2, char::is_whitespace);
    fields.next()?;
    let rest = fields.next()?.trim_start();
    match rest.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').map(|(path, _)| path),
        None => rest.split_whitespace().next(),
    }
}

/// The `/var` directory an entry stands for: no specifiers, a glob cut to
/// the directory before it.
fn entry_path(raw: &str) -> Option<String> {
    if raw.contains('%') {
        return None;
    }
    let cut = match raw.find(['*', '?', '[']) {
        Some(i) => raw[..i].rfind('/').map_or("", |slash| &raw[..slash]),
        None => raw,
    };
    let path = cut.trim_end_matches('/');
    (path == "/var" || path.starts_with("/var/")).then(|| path.to_owned())
}

/// The effective `*.conf` files: `etc` overrides `usr/lib` by name, and a
/// symlink (usually to `/dev/null`) masks the name.
fn effective_files(root: &Path) -> Result<Vec<PathBuf>, TxError> {
    let mut by_name: BTreeMap<OsString, Option<PathBuf>> = BTreeMap::new();
    for dir in ["usr/lib/tmpfiles.d", "etc/tmpfiles.d"] {
        let dir = root.join(dir);
        let read = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(io_failure(&dir, &e)),
        };
        for item in read {
            let item = item.map_err(|e| io_failure(&dir, &e))?;
            let name = item.file_name();
            if Path::new(&name).extension() != Some(OsStr::new("conf")) {
                continue;
            }
            let kind = item.file_type().map_err(|e| io_failure(&item.path(), &e))?;
            by_name.insert(name, kind.is_file().then(|| item.path()));
        }
    }
    Ok(by_name.into_values().flatten().collect())
}

/// Paths under `/var` named by the `tmpfiles.d` entries of the snapshot
/// `root` (`usr/lib/tmpfiles.d`, then `etc/tmpfiles.d`, which overrides a
/// file of the same name; a symlink there masks it). Entries with
/// specifiers are skipped; a glob is cut to its leading directory.
pub fn tmpfiles_paths(root: &Path) -> Result<Vec<String>, TxError> {
    let mut paths = Vec::new();
    for file in effective_files(root)? {
        let text = fs::read_to_string(&file).map_err(|e| io_failure(&file, &e))?;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(path) = path_column(line).and_then(entry_path) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

/// Whether `path` is an entry or lies under one. Entries too generic to say
/// anything (`/var`, `/var/lib`, `/var/log`...: fewer than three components)
/// cover nothing.
pub fn covered(path: &str, entries: &[String]) -> bool {
    entries.iter().any(|entry| {
        let specific = entry.split('/').filter(|c| !c.is_empty()).count() >= 3;
        specific
            && path
                .strip_prefix(entry.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// Collect the leaves under `dir` (`rel` is its path below the scratch).
fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), TxError> {
    let mut any = false;
    for item in fs::read_dir(dir).map_err(|e| io_failure(dir, &e))? {
        let item = item.map_err(|e| io_failure(dir, &e))?;
        any = true;
        let child = if rel.is_empty() {
            item.file_name().to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", item.file_name().to_string_lossy())
        };
        let kind = item.file_type().map_err(|e| io_failure(&item.path(), &e))?;
        if kind.is_dir() {
            if child != CACHE_REL {
                walk(&item.path(), &child, out)?;
            }
        } else {
            out.push(format!("/var/{child}"));
        }
    }
    if !any && !rel.is_empty() {
        out.push(format!("/var/{rel}"));
    }
    Ok(())
}

/// The files, symlinks and empty directories under `scratch` (a `/var`) that
/// no `tmpfiles.d` entry of `root` covers, as `/var/...` paths, sorted. The
/// bound package cache is excluded.
pub fn var_report(scratch: &Path, root: &Path) -> Result<Vec<String>, TxError> {
    let entries = tmpfiles_paths(root)?;
    let mut found = Vec::new();
    walk(scratch, "", &mut found)?;
    found.retain(|p| !covered(p, &entries));
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_path_column_is_read_with_modifiers_comments_and_globs() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "usr/lib/tmpfiles.d/foo.conf",
            "# comment\n\nd /var/lib/foo 0755 root root -\nd! /var/cache/foo/data - - -\n\
             L+ \"/var/lib/quoted dir\" - - - - x\nr /var/tmp/x*\nd /run/%u/x\nZ\n",
        );
        let mut got = tmpfiles_paths(root.path()).unwrap();
        got.sort();
        assert_eq!(
            got,
            [
                "/var/cache/foo/data",
                "/var/lib/foo",
                "/var/lib/quoted dir",
                "/var/tmp"
            ]
        );
    }

    #[test]
    fn etc_overrides_and_a_symlink_masks() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "usr/lib/tmpfiles.d/a.conf",
            "d /var/lib/a/one - - -\n",
        );
        write(
            root.path(),
            "usr/lib/tmpfiles.d/b.conf",
            "d /var/lib/b/one - - -\n",
        );
        write(
            root.path(),
            "usr/lib/tmpfiles.d/c.conf",
            "d /var/lib/c/one - - -\n",
        );
        write(
            root.path(),
            "etc/tmpfiles.d/a.conf",
            "d /var/lib/a/two - - -\n",
        );
        symlink("/dev/null", root.path().join("etc/tmpfiles.d/b.conf")).unwrap();
        let mut got = tmpfiles_paths(root.path()).unwrap();
        got.sort();
        assert_eq!(got, ["/var/lib/a/two", "/var/lib/c/one"]);
    }

    #[test]
    fn no_tmpfiles_dirs_is_no_entries() {
        let root = tempfile::tempdir().unwrap();
        assert!(tmpfiles_paths(root.path()).unwrap().is_empty());
    }

    #[test]
    fn covered_is_a_path_prefix_match_ignoring_generic_entries() {
        let entries = strings(&["/var/lib/foo", "/var", "/var/lib"]);
        assert!(covered("/var/lib/foo", &entries));
        assert!(covered("/var/lib/foo/state/x", &entries));
        assert!(!covered("/var/lib/foobar", &entries));
        assert!(!covered("/var/lib/other", &entries));
    }

    #[test]
    fn the_report_lists_leaves_not_covered_and_skips_the_cache() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "usr/lib/tmpfiles.d/foo.conf",
            "d /var/lib/foo - - -\n",
        );
        let scratch = tempfile::tempdir().unwrap();
        let s = scratch.path();
        write(s, "lib/foo/state.db", "x");
        write(s, "lib/bar/data", "x");
        write(s, "lib/bar/more/deep", "x");
        fs::create_dir_all(s.join("lib/emptydir")).unwrap();
        // The bind-mount point of the package cache, with a download in it.
        write(s, "cache/pacman/pkg/vim.pkg.tar.zst", "x");
        symlink("/etc/hostname", s.join("link")).unwrap();
        assert_eq!(
            var_report(s, root.path()).unwrap(),
            [
                "/var/lib/bar/data",
                "/var/lib/bar/more/deep",
                "/var/lib/emptydir",
                "/var/link",
            ]
        );
    }

    #[test]
    fn an_empty_scratch_reports_nothing() {
        let root = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        assert!(var_report(scratch.path(), root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_missing_scratch_is_a_failure() {
        let root = tempfile::tempdir().unwrap();
        assert!(var_report(&root.path().join("gone"), root.path()).is_err());
    }
}
