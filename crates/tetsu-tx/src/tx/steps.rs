//! The steps of a transaction that run in the container over the new
//! snapshot: keyring, update, install, remove, orphans, kernel version and
//! initrd. Downloads run online and applying runs offline, so a failed
//! download touches nothing and an install never reaches the network.

use std::path::Path;

use super::TxError;
use super::nspawn::Nspawn;
use crate::boot::tool::Runner;

/// Download the keyring (online).
pub fn keyring_download(runner: &dyn Runner, ns: &Nspawn) -> Result<(), TxError> {
    let _ = (runner, ns);
    todo!()
}

/// Install the downloaded keyring (offline).
pub fn keyring_install(runner: &dyn Runner, ns: &Nspawn) -> Result<(), TxError> {
    let _ = (runner, ns);
    todo!()
}

/// Download a full upgrade (online).
pub fn update_download(runner: &dyn Runner, ns: &Nspawn) -> Result<(), TxError> {
    let _ = (runner, ns);
    todo!()
}

/// Apply the downloaded upgrade (offline).
pub fn update_install(runner: &dyn Runner, ns: &Nspawn) -> Result<(), TxError> {
    let _ = (runner, ns);
    todo!()
}

/// Packages installed as dependencies that nothing needs any more.
pub fn orphans(runner: &dyn Runner, ns: &Nspawn) -> Result<Vec<String>, TxError> {
    let _ = (runner, ns);
    todo!()
}

/// Remove `orphans` with their configuration and unneeded dependencies;
/// nothing to do for an empty list.
pub fn remove_orphans(runner: &dyn Runner, ns: &Nspawn, orphans: &[String]) -> Result<(), TxError> {
    let _ = (runner, ns, orphans);
    todo!()
}

/// Check every name against the sync databases: `PackageNotFound` for the
/// first that does not exist (or is not a package name).
pub fn check_available(runner: &dyn Runner, ns: &Nspawn, pkgs: &[String]) -> Result<(), TxError> {
    let _ = (runner, ns, pkgs);
    todo!()
}

/// Check, then download `pkgs` (online). No `-y`: no partial upgrade.
pub fn install_download(runner: &dyn Runner, ns: &Nspawn, pkgs: &[String]) -> Result<(), TxError> {
    let _ = (runner, ns, pkgs);
    todo!()
}

/// Install the downloaded `pkgs` (offline).
pub fn install_apply(runner: &dyn Runner, ns: &Nspawn, pkgs: &[String]) -> Result<(), TxError> {
    let _ = (runner, ns, pkgs);
    todo!()
}

/// Remove user packages (offline), without cascading to dependents. Every
/// name must be in `user_layer`; a user dependent makes pacman refuse and
/// the failure names it.
pub fn remove(
    runner: &dyn Runner,
    ns: &Nspawn,
    pkgs: &[String],
    user_layer: &[String],
) -> Result<(), TxError> {
    let _ = (runner, ns, pkgs, user_layer);
    todo!()
}

/// The kernel version of a snapshot: the single
/// `usr/lib/modules/<ver>/vmlinuz`.
pub fn kernel_version(root: &Path) -> Result<String, TxError> {
    let _ = root;
    todo!()
}

/// Build the initrd of `ver` at `/usr/lib/tetsu/initrd.img` inside `root`
/// (offline). Public so the installer reuses it for transaction zero.
pub fn build_initrd(
    runner: &dyn Runner,
    root: &Path,
    scratch: &Path,
    ver: &str,
) -> Result<(), TxError> {
    let _ = (runner, root, scratch, ver);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::fake::{Reply, ScriptedRunner};
    use crate::tx::nspawn::{DEFAULT_CACHE, Network};
    use std::fs;

    fn ns() -> Nspawn {
        Nspawn::new(Path::new("/r"), Path::new("/s"), Path::new(DEFAULT_CACHE))
    }

    /// The `systemd-nspawn` line of `cmd` in the test container.
    fn line(net: Network, cmd: &str) -> String {
        let mut parts = vec!["systemd-nspawn".to_owned()];
        let cmd: Vec<&str> = cmd.split(' ').collect();
        parts.extend(
            ns().args(net, &cmd)
                .iter()
                .map(|a| a.to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn reason(e: TxError) -> String {
        match e {
            TxError::Failed { reason, .. } => reason,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn keyring_downloads_online_then_installs_offline() {
        let r = ScriptedRunner::new();
        keyring_download(&r, &ns()).unwrap();
        keyring_install(&r, &ns()).unwrap();
        assert_eq!(
            r.lines(),
            [
                line(Network::Online, "pacman -Syw --noconfirm archlinux-keyring"),
                line(
                    Network::Offline,
                    "pacman -S --noconfirm --needed archlinux-keyring"
                ),
            ]
        );
    }

    #[test]
    fn update_downloads_online_then_applies_offline() {
        let r = ScriptedRunner::new();
        update_download(&r, &ns()).unwrap();
        update_install(&r, &ns()).unwrap();
        assert_eq!(
            r.lines(),
            [
                line(Network::Online, "pacman -Suw --noconfirm"),
                line(Network::Offline, "pacman -Su --noconfirm"),
            ]
        );
    }

    #[test]
    fn a_failed_step_quotes_pacman_stderr() {
        let r = ScriptedRunner::new();
        r.on_tail(
            &["pacman", "-Su", "--noconfirm"],
            Reply::Fail("error: unable to satisfy dependency 'x' required by y".into()),
        );
        match update_install(&r, &ns()).unwrap_err() {
            TxError::Failed { reason, log, .. } => {
                assert!(reason.contains("required by y"), "{reason}");
                assert_eq!(
                    log,
                    ["error: unable to satisfy dependency 'x' required by y"]
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn orphans_are_listed_and_removed_with_rns() {
        let r = ScriptedRunner::new();
        r.on_tail(&["pacman", "-Qdtq"], Reply::Out("liba\nlibb\n".into()));
        let found = orphans(&r, &ns()).unwrap();
        assert_eq!(found, ["liba", "libb"]);
        remove_orphans(&r, &ns(), &found).unwrap();
        assert_eq!(
            r.lines(),
            [
                line(Network::Offline, "pacman -Qdtq"),
                line(Network::Offline, "pacman -Rns --noconfirm liba libb"),
            ]
        );
    }

    #[test]
    fn no_orphans_is_exit_one_with_no_output_and_removes_nothing() {
        let r = ScriptedRunner::new();
        r.on_tail(&["pacman", "-Qdtq"], Reply::Fail(String::new()));
        let found = orphans(&r, &ns()).unwrap();
        assert!(found.is_empty());
        remove_orphans(&r, &ns(), &found).unwrap();
        assert_eq!(r.lines().len(), 1);
    }

    #[test]
    fn a_real_orphan_query_failure_is_reported() {
        let r = ScriptedRunner::new();
        r.on_tail(
            &["pacman", "-Qdtq"],
            Reply::Fail("error: database corrupt".into()),
        );
        assert!(orphans(&r, &ns()).is_err());
    }

    #[test]
    fn install_checks_every_name_then_downloads_online() {
        let r = ScriptedRunner::new();
        install_download(&r, &ns(), &names(&["vim", "htop"])).unwrap();
        install_apply(&r, &ns(), &names(&["vim", "htop"])).unwrap();
        assert_eq!(
            r.lines(),
            [
                line(Network::Offline, "pacman -Si vim"),
                line(Network::Offline, "pacman -Si htop"),
                line(Network::Online, "pacman -Sw --noconfirm --needed vim htop"),
                line(Network::Offline, "pacman -S --noconfirm --needed vim htop"),
            ]
        );
    }

    #[test]
    fn an_unknown_package_stops_before_any_download() {
        let r = ScriptedRunner::new();
        r.on_tail(
            &["pacman", "-Si", "nope"],
            Reply::Fail("error: package 'nope' was not found".into()),
        );
        let err = install_download(&r, &ns(), &names(&["vim", "nope"])).unwrap_err();
        assert!(matches!(err, TxError::PackageNotFound(n) if n == "nope"));
        assert!(r.lines().iter().all(|l| !l.contains("-Sw")));
    }

    #[test]
    fn another_si_failure_is_not_a_missing_package() {
        let r = ScriptedRunner::new();
        r.on_tail(
            &["pacman", "-Si", "vim"],
            Reply::Fail("error: database corrupt".into()),
        );
        let err = check_available(&r, &ns(), &names(&["vim"])).unwrap_err();
        assert!(reason(err).contains("database corrupt"));
    }

    #[test]
    fn names_that_could_be_options_never_reach_pacman() {
        let r = ScriptedRunner::new();
        for bad in ["--root=/x", "", "a b", "-S"] {
            let err = check_available(&r, &ns(), &names(&[bad])).unwrap_err();
            assert!(matches!(err, TxError::PackageNotFound(_)), "{bad}");
        }
        assert!(r.lines().is_empty());
        assert!(check_available(&r, &ns(), &[]).is_err());
    }

    #[test]
    fn remove_runs_rns_without_cascade() {
        let r = ScriptedRunner::new();
        let user = names(&["vim", "htop"]);
        remove(&r, &ns(), &names(&["vim"]), &user).unwrap();
        assert_eq!(
            r.lines(),
            [line(Network::Offline, "pacman -Rns --noconfirm vim")]
        );
    }

    #[test]
    fn remove_refuses_what_is_not_in_the_user_layer() {
        let r = ScriptedRunner::new();
        let err = remove(&r, &ns(), &names(&["vim", "bash"]), &names(&["vim"])).unwrap_err();
        assert!(matches!(err, TxError::NotUserPackage(n) if n == "bash"));
        assert!(r.lines().is_empty());
    }

    #[test]
    fn remove_names_the_user_dependent_that_blocks_it() {
        let r = ScriptedRunner::new();
        r.on_tail(
            &["pacman", "-Rns", "--noconfirm", "libfoo"],
            Reply::Fail(
                "error: failed to prepare transaction (could not satisfy dependencies)\n\
                 :: removing libfoo breaks dependency 'libfoo' required by mytool"
                    .into(),
            ),
        );
        let user = names(&["libfoo", "mytool"]);
        match remove(&r, &ns(), &names(&["libfoo"]), &user).unwrap_err() {
            TxError::Failed {
                reason, proposal, ..
            } => {
                assert!(reason.contains("mytool"), "{reason}");
                assert!(proposal.unwrap().contains("mytool"));
            }
            other => panic!("{other:?}"),
        }
    }

    fn snapshot_with_kernels(vers: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for v in vers {
            let dir = root.path().join("usr/lib/modules").join(v);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("vmlinuz"), b"k").unwrap();
        }
        root
    }

    #[test]
    fn the_kernel_version_is_the_single_vmlinuz_directory() {
        let root = snapshot_with_kernels(&["6.12.1-arch1-1"]);
        // A modules directory without a kernel (leftover of a removed one).
        fs::create_dir_all(root.path().join("usr/lib/modules/6.11.0")).unwrap();
        assert_eq!(kernel_version(root.path()).unwrap(), "6.12.1-arch1-1");
    }

    #[test]
    fn no_kernel_or_several_is_a_failure() {
        let none = snapshot_with_kernels(&[]);
        assert!(kernel_version(none.path()).is_err());
        let two = snapshot_with_kernels(&["6.12.1", "6.6.60-lts"]);
        let why = reason(kernel_version(two.path()).unwrap_err());
        assert!(
            why.contains("6.12.1") && why.contains("6.6.60-lts"),
            "{why}"
        );
    }

    #[test]
    fn initrd_is_built_offline_at_the_tetsu_path() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("usr/lib/tetsu");
        fs::create_dir_all(&out).unwrap();
        let img = out.join("initrd.img");
        let r = ScriptedRunner::new();
        r.on_tail_effect(
            &[
                "mkinitcpio",
                "-k",
                "6.12.1",
                "-g",
                "/usr/lib/tetsu/initrd.img",
            ],
            {
                let img = img.clone();
                move |_| fs::write(&img, b"initrd").unwrap()
            },
        );
        build_initrd(&r, root.path(), Path::new("/s"), "6.12.1").unwrap();
        assert!(img.is_file());
        let expected = {
            let ns = Nspawn::new(root.path(), Path::new("/s"), Path::new(DEFAULT_CACHE));
            let mut parts = vec!["systemd-nspawn".to_owned()];
            parts.extend(
                ns.args(
                    Network::Offline,
                    &[
                        "mkinitcpio",
                        "-k",
                        "6.12.1",
                        "-g",
                        "/usr/lib/tetsu/initrd.img",
                    ],
                )
                .iter()
                .map(|a| a.to_string_lossy().into_owned()),
            );
            parts.join(" ")
        };
        assert_eq!(r.lines(), [expected]);
    }

    #[test]
    fn initrd_that_mkinitcpio_did_not_write_is_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let r = ScriptedRunner::new();
        assert!(build_initrd(&r, root.path(), Path::new("/s"), "6.12.1").is_err());
    }

    #[test]
    fn initrd_rejects_a_kernel_version_that_is_not_one_component() {
        let root = tempfile::tempdir().unwrap();
        let r = ScriptedRunner::new();
        for bad in ["", "..", "a/b"] {
            assert!(build_initrd(&r, root.path(), Path::new("/s"), bad).is_err());
        }
        assert!(r.lines().is_empty());
    }
}
