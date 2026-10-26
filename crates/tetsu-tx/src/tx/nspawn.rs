//! The container a transaction runs its package commands in: a pure argv
//! builder for `systemd-nspawn` over the new snapshot.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// The program.
pub const PROGRAM: &str = "systemd-nspawn";
/// The host package cache, bound in so downloads survive the transaction.
pub const DEFAULT_CACHE: &str = "/var/cache/pacman/pkg";

/// Whether the container can reach the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    /// Downloads: the host resolver is bind-mounted over the snapshot's
    /// `/etc/resolv.conf` (copying fails on its symlink).
    Online,
    /// Installs: no network at all.
    Offline,
}

/// One container over a snapshot root.
#[derive(Clone, Debug)]
pub struct Nspawn {
    pub root: PathBuf,
    /// Bound over `/var`; kept across the phases of one transaction.
    pub scratch: PathBuf,
    /// Bound over `/var/cache/pacman/pkg`.
    pub cache: PathBuf,
}

impl Nspawn {
    pub fn new(root: &Path, scratch: &Path, cache: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            scratch: scratch.to_path_buf(),
            cache: cache.to_path_buf(),
        }
    }

    /// Arguments of `systemd-nspawn` that run `command` in the container.
    pub fn args(&self, network: Network, command: &[&str]) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["-D".into(), self.root.clone().into()];
        args.extend(
            [
                "--quiet",
                "--register=no",
                "--link-journal=no",
                "--timezone=off",
                "--setenv=LC_ALL=C",
            ]
            .map(OsString::from),
        );
        args.push(bind(&self.scratch, "/var"));
        args.push(bind(&self.cache, "/var/cache/pacman/pkg"));
        args.push(
            match network {
                Network::Online => "--resolv-conf=bind-host",
                Network::Offline => "--private-network",
            }
            .into(),
        );
        args.extend(command.iter().map(OsString::from));
        args
    }
}

/// `path` as systemd-nspawn reads one side of a `--bind=` spec: `\` and `:`
/// escaped with a backslash.
fn escape(path: &OsStr) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in path.as_bytes() {
        if b == b'\\' || b == b':' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

fn bind(host: &Path, inside: &str) -> OsString {
    let mut spec = b"--bind=".to_vec();
    spec.extend(escape(host.as_os_str()));
    spec.push(b':');
    spec.extend(escape(OsStr::new(inside)));
    OsString::from_vec(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(args: &[OsString]) -> Vec<&str> {
        args.iter().map(|a| a.to_str().unwrap()).collect()
    }

    fn container() -> Nspawn {
        Nspawn::new(
            Path::new("/top/@snapshots/5/root"),
            Path::new("/run/tetsu/tx-var-5"),
            Path::new(DEFAULT_CACHE),
        )
    }

    #[test]
    fn online_binds_the_host_resolver() {
        let args = container().args(Network::Online, &["pacman", "-Syw"]);
        assert_eq!(
            strs(&args),
            [
                "-D",
                "/top/@snapshots/5/root",
                "--quiet",
                "--register=no",
                "--link-journal=no",
                "--timezone=off",
                "--setenv=LC_ALL=C",
                "--bind=/run/tetsu/tx-var-5:/var",
                "--bind=/var/cache/pacman/pkg:/var/cache/pacman/pkg",
                "--resolv-conf=bind-host",
                "pacman",
                "-Syw",
            ]
        );
    }

    #[test]
    fn offline_has_a_private_network_and_no_resolver() {
        let args = container().args(Network::Offline, &["mkinitcpio"]);
        let args = strs(&args);
        assert!(args.contains(&"--private-network"));
        assert!(!args.iter().any(|a| a.starts_with("--resolv-conf")));
        assert_eq!(args.last(), Some(&"mkinitcpio"));
    }

    #[test]
    fn paths_are_parameters() {
        let c = Nspawn::new(Path::new("/r"), Path::new("/s"), Path::new("/c"));
        let args = c.args(Network::Offline, &[]);
        let args = strs(&args);
        assert!(args.contains(&"--bind=/s:/var"));
        assert!(args.contains(&"--bind=/c:/var/cache/pacman/pkg"));
        assert_eq!(args[1], "/r");
    }

    #[test]
    fn colons_and_backslashes_in_paths_are_escaped() {
        let c = Nspawn::new(Path::new("/r"), Path::new("/s:v\\x"), Path::new("/c:d"));
        let args = c.args(Network::Offline, &[]);
        let args = strs(&args);
        assert!(args.contains(&"--bind=/s\\:v\\\\x:/var"), "{args:?}");
        assert!(
            args.contains(&"--bind=/c\\:d:/var/cache/pacman/pkg"),
            "{args:?}"
        );
    }
}
