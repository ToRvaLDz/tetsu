//! The `/var` report: what the install phase left in the throwaway `/var`
//! that no `tmpfiles.d` entry of the snapshot accounts for. A package that
//! writes state under `/var` at install time loses it with the scratch, so
//! the transaction log says so.

use std::path::Path;

use super::TxError;

/// Paths named by the `tmpfiles.d` entries of the snapshot `root`
/// (`usr/lib/tmpfiles.d`, then `etc/tmpfiles.d`, which overrides a file of
/// the same name; a symlink there masks it). Entries with specifiers are
/// skipped; a glob is cut to its leading directory.
pub fn tmpfiles_paths(root: &Path) -> Result<Vec<String>, TxError> {
    let _ = root;
    todo!()
}

/// Whether `path` is an entry or lies under one. Entries too generic to say
/// anything (`/var`, `/var/lib`, `/var/log`...: fewer than three components)
/// cover nothing.
pub fn covered(path: &str, entries: &[String]) -> bool {
    let _ = (path, entries);
    todo!()
}

/// The files, symlinks and empty directories under `scratch` (a `/var`) that
/// no `tmpfiles.d` entry of `root` covers, as `/var/...` paths, sorted. The
/// bound package cache is excluded.
pub fn var_report(scratch: &Path, root: &Path) -> Result<Vec<String>, TxError> {
    let _ = (scratch, root);
    todo!()
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
