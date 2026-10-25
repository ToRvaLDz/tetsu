//! Package model: what a snapshot holds, which of it is the base, which is
//! the user layer, and what a transaction changed. Pure parsing and set
//! logic over `pacman` output, plus the read-only host queries.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;

use super::{PackageChange, TxError};
use crate::boot::tool::Runner;

/// The meta-package that pulls in the base system.
pub const BASE_PACKAGE: &str = "tetsu-base";

/// Installed packages: name to version.
pub type Packages = BTreeMap<String, String>;

/// The fields of one `pacman -Qi` record that the base closure needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub depends: Vec<String>,
    pub provides: Vec<String>,
}

/// Arguments of a read-only host query against the pacman DB of `root`:
/// `env LC_ALL=C pacman --root <root> --dbpath <root>/usr/lib/pacman <flags>`
/// (the program is `env`).
pub fn query_args(root: &Path, flags: &[&str]) -> Vec<OsString> {
    todo!("{root:?} {flags:?}")
}

/// Parse `pacman -Q` output (`name version` per line).
pub fn parse_installed(out: &str) -> Result<Packages, TxError> {
    todo!("{out}")
}

/// Parse `pacman -Qi` output (records separated by a blank line).
pub fn parse_info(out: &str) -> Result<Vec<PackageInfo>, TxError> {
    todo!("{out}")
}

/// Parse a `-q` listing: one package name per line.
pub fn parse_names(out: &str) -> Vec<String> {
    todo!("{out}")
}

/// The dependency closure of `root` over `infos`: names and provides are
/// resolved, version constraints are stripped, optional dependencies are
/// ignored. `None` when `root` is not installed.
pub fn base_closure(infos: &[PackageInfo], root: &str) -> Option<BTreeSet<String>> {
    todo!("{infos:?} {root}")
}

/// The explicitly installed packages outside the base closure, sorted.
pub fn user_layer(explicit: &[String], closure: &BTreeSet<String>) -> Vec<String> {
    todo!("{explicit:?} {closure:?}")
}

/// Packages whose version differs between `before` and `after`, by name.
pub fn diff(before: &Packages, after: &Packages) -> Vec<PackageChange> {
    todo!("{before:?} {after:?}")
}

/// The user packages named in a conflict or broken-dependency message of
/// pacman, in order of first appearance.
pub fn blocking_packages(stderr: &str, user_layer: &[String]) -> Vec<String> {
    todo!("{stderr} {user_layer:?}")
}

/// Installed packages of `root`.
pub fn installed(runner: &dyn Runner, root: &Path) -> Result<Packages, TxError> {
    todo!("{root:?} {}", runner.check_tool("x").is_ok())
}

/// `pacman -Qi` records of `root`.
pub fn infos(runner: &dyn Runner, root: &Path) -> Result<Vec<PackageInfo>, TxError> {
    todo!("{root:?} {}", runner.check_tool("x").is_ok())
}

/// Explicitly installed package names of `root`.
pub fn explicit(runner: &dyn Runner, root: &Path) -> Result<Vec<String>, TxError> {
    todo!("{root:?} {}", runner.check_tool("x").is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::fake::{Reply, ScriptedRunner};

    fn info(name: &str, depends: &[&str], provides: &[&str]) -> PackageInfo {
        PackageInfo {
            name: name.into(),
            version: "1-1".into(),
            depends: depends.iter().map(|s| (*s).into()).collect(),
            provides: provides.iter().map(|s| (*s).into()).collect(),
        }
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn query_args_are_a_read_only_host_query() {
        let args = query_args(Path::new("/snap/root"), &["-Qi"]);
        let args: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "LC_ALL=C",
                "pacman",
                "--root",
                "/snap/root",
                "--dbpath",
                "/snap/root/usr/lib/pacman",
                "-Qi"
            ]
        );
    }

    #[test]
    fn parses_the_installed_list() {
        let p = parse_installed("bash 5.2.037-2\nvim 9.1.0-1\n\n").unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p["vim"], "9.1.0-1");
        assert!(parse_installed("").unwrap().is_empty());
    }

    #[test]
    fn a_malformed_installed_line_is_a_failure() {
        assert!(matches!(
            parse_installed("bash\n"),
            Err(TxError::Failed { .. })
        ));
        assert!(parse_installed("a 1 extra\n").is_err());
    }

    const QI: &str = "\
Name            : bash
Version         : 5.2.037-2
Description     : The GNU Bourne Again shell
Architecture    : x86_64
Depends On      : readline  libreadline.so=8-64  glibc  ncurses
Optional Deps   : bash-completion: for tab completion
Required By     : None
Provides        : sh

Name            : tetsu-base
Version         : 1-1
Depends On      : bash>=5
                  linux
Optional Deps   : vim: editor
                  nano: another editor
Provides        : None
";

    #[test]
    fn parses_info_records_with_continuations_and_none() {
        let infos = parse_info(QI).unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "bash");
        assert_eq!(infos[0].version, "5.2.037-2");
        assert_eq!(
            infos[0].depends,
            ["readline", "libreadline.so=8-64", "glibc", "ncurses"]
        );
        assert_eq!(infos[0].provides, ["sh"]);
        assert_eq!(infos[1].depends, ["bash>=5", "linux"]);
        assert!(infos[1].provides.is_empty());
    }

    #[test]
    fn info_without_a_name_is_a_failure() {
        assert!(parse_info("Version : 1\n").is_err());
    }

    #[test]
    fn names_are_one_per_line() {
        assert_eq!(parse_names("a\n\n b \nc\n"), ["a", "b", "c"]);
    }

    #[test]
    fn closure_follows_names_provides_and_strips_constraints() {
        let infos = [
            info(
                "tetsu-base",
                &["bash>=5", "sh-provider", "libx.so=1-64"],
                &[],
            ),
            info("bash", &["glibc"], &["sh"]),
            info("glibc", &[], &[]),
            info("libx", &["glibc"], &["libx.so=1-64"]),
            info("zsh", &["glibc"], &["sh"]),
            info("vim", &["glibc"], &[]),
            info("sh-provider", &["sh"], &[]),
        ];
        let c = base_closure(&infos, BASE_PACKAGE).unwrap();
        // Both providers of `sh` count; vim is not reachable.
        assert_eq!(
            c,
            set(&["tetsu-base", "bash", "glibc", "libx", "zsh", "sh-provider"])
        );
    }

    #[test]
    fn closure_survives_cycles_and_unresolved_names() {
        let infos = [
            info("tetsu-base", &["a", "missing>=1"], &[]),
            info("a", &["b"], &[]),
            info("b", &["a"], &[]),
        ];
        let c = base_closure(&infos, "tetsu-base").unwrap();
        assert_eq!(c, set(&["tetsu-base", "a", "b"]));
    }

    #[test]
    fn closure_is_none_without_the_root_package() {
        assert!(base_closure(&[info("bash", &[], &[])], BASE_PACKAGE).is_none());
    }

    #[test]
    fn user_layer_is_explicit_minus_base_sorted() {
        let closure = set(&["tetsu-base", "bash"]);
        let explicit = strings(&["vim", "bash", "htop", "tetsu-base"]);
        assert_eq!(user_layer(&explicit, &closure), ["htop", "vim"]);
    }

    fn pkgs(list: &[(&str, &str)]) -> Packages {
        list.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn diff_reports_added_removed_and_changed_only() {
        let before = pkgs(&[("a", "1"), ("b", "1"), ("c", "1")]);
        let after = pkgs(&[("a", "1"), ("b", "2"), ("d", "1")]);
        let d = diff(&before, &after);
        let line = |n: &str, f: Option<&str>, t: Option<&str>| PackageChange {
            name: n.into(),
            from: f.map(Into::into),
            to: t.map(Into::into),
        };
        assert_eq!(
            d,
            [
                line("b", Some("1"), Some("2")),
                line("c", Some("1"), None),
                line("d", None, Some("1")),
            ]
        );
        assert!(diff(&before, &before).is_empty());
    }

    #[test]
    fn blocking_names_the_user_packages_in_a_broken_dependency() {
        let err = "\
error: failed to prepare transaction (could not satisfy dependencies)
:: installing glibc (2.41) breaks dependency 'glibc<2.41' required by mytool
:: removing libfoo breaks dependency 'libfoo' required by vim
";
        let user = strings(&["mytool", "vim", "htop"]);
        assert_eq!(blocking_packages(err, &user), ["mytool", "vim"]);
    }

    #[test]
    fn blocking_names_a_user_package_in_a_conflict() {
        let err = "\
error: failed to prepare transaction (conflicting dependencies)
:: jack2 and jack are in conflict
";
        assert_eq!(blocking_packages(err, &strings(&["jack"])), ["jack"]);
        assert!(blocking_packages(err, &strings(&["vim"])).is_empty());
    }

    #[test]
    fn blocking_ignores_unrelated_lines_and_dedups() {
        let err = "\
error: target not found: vim
:: vim and vi are in conflict
:: vim and neovim are in conflict (editor)
";
        assert_eq!(blocking_packages(err, &strings(&["vim"])), ["vim"]);
    }

    #[test]
    fn queries_run_through_env_on_the_host() {
        let r = ScriptedRunner::new();
        r.on(
            &[
                "env",
                "LC_ALL=C",
                "pacman",
                "--root",
                "/s",
                "--dbpath",
                "/s/usr/lib/pacman",
                "-Q",
            ],
            Reply::Out("bash 5\n".into()),
        );
        let got = installed(&r, Path::new("/s")).unwrap();
        assert_eq!(got["bash"], "5");
        assert_eq!(
            r.lines(),
            ["env LC_ALL=C pacman --root /s --dbpath /s/usr/lib/pacman -Q"]
        );
    }

    #[test]
    fn an_empty_explicit_list_is_not_a_failure() {
        // pacman -Qqe exits 1 with no output when nothing matches.
        let r = ScriptedRunner::new();
        r.on(&["env"], Reply::Fail(String::new()));
        assert!(explicit(&r, Path::new("/s")).unwrap().is_empty());
    }

    #[test]
    fn a_failing_query_with_a_message_is_a_failure() {
        let r = ScriptedRunner::new();
        r.on(
            &["env"],
            Reply::Fail("error: could not open database".into()),
        );
        assert!(infos(&r, Path::new("/s")).is_err());
    }

    #[test]
    fn infos_are_parsed_from_qi() {
        let r = ScriptedRunner::new();
        r.on(&["env"], Reply::Out(QI.into()));
        assert_eq!(infos(&r, Path::new("/s")).unwrap().len(), 2);
    }
}
