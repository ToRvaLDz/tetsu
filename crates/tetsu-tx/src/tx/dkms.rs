//! Out-of-tree kernel modules: build them for the new kernel inside the
//! snapshot and, when that fails, say which package owns the module.

use super::TxError;
use super::nspawn::Nspawn;
use crate::boot::tool::Runner;

/// What the DKMS step did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dkms {
    /// The snapshot has no `usr/bin/dkms`.
    Skipped,
    /// `dkms autoinstall` succeeded.
    Built,
}

/// A module that `dkms status` does not report as installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub name: String,
    pub version: String,
}

/// The modules `dkms status` lists in any state but `installed`.
pub fn not_installed(status: &str) -> Vec<Module> {
    let _ = status;
    todo!()
}

/// Build every DKMS module for kernel `ver` (offline). On failure the
/// report names the modules left uninstalled and their owning packages; the
/// proposal names the owners that are in `user_layer`.
pub fn build_modules(
    runner: &dyn Runner,
    ns: &Nspawn,
    ver: &str,
    user_layer: &[String],
) -> Result<Dkms, TxError> {
    let _ = (runner, ns, ver, user_layer);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::fake::{Reply, ScriptedRunner};
    use crate::tx::nspawn::{DEFAULT_CACHE, Network};
    use std::fs;
    use std::path::Path;

    fn snapshot(with_dkms: bool) -> (tempfile::TempDir, Nspawn) {
        let root = tempfile::tempdir().unwrap();
        if with_dkms {
            fs::create_dir_all(root.path().join("usr/bin")).unwrap();
            fs::write(root.path().join("usr/bin/dkms"), b"#!/bin/sh").unwrap();
        }
        let ns = Nspawn::new(root.path(), Path::new("/s"), Path::new(DEFAULT_CACHE));
        (root, ns)
    }

    fn line(ns: &Nspawn, cmd: &[&str]) -> String {
        let mut parts = vec!["systemd-nspawn".to_owned()];
        parts.extend(
            ns.args(Network::Offline, cmd)
                .iter()
                .map(|a| a.to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }

    const STATUS: &str = "\
nvidia/550.54, 6.12.1-arch1-1, x86_64: built
virtualbox-host/7.0.14, 6.12.1-arch1-1, x86_64: installed
zfs/2.2.3, 6.12.1-arch1-1, x86_64: installed (WARNING! Diff between built and installed module!)
broken/1.0, 6.12.1-arch1-1, x86_64: added
Error! something unrelated
";

    #[test]
    fn status_lists_the_modules_not_installed() {
        let m = not_installed(STATUS);
        let got: Vec<_> = m
            .iter()
            .map(|m| (m.name.as_str(), m.version.as_str()))
            .collect();
        assert_eq!(got, [("nvidia", "550.54"), ("broken", "1.0")]);
    }

    #[test]
    fn a_snapshot_without_dkms_skips_the_step() {
        let (_root, ns) = snapshot(false);
        let r = ScriptedRunner::new();
        assert_eq!(
            build_modules(&r, &ns, "6.12.1", &[]).unwrap(),
            Dkms::Skipped
        );
        assert!(r.lines().is_empty());
    }

    #[test]
    fn autoinstall_runs_offline_with_the_tree_under_usr_lib() {
        let (_root, ns) = snapshot(true);
        let r = ScriptedRunner::new();
        assert_eq!(build_modules(&r, &ns, "6.12.1", &[]).unwrap(), Dkms::Built);
        assert_eq!(
            r.lines(),
            [line(
                &ns,
                &[
                    "dkms",
                    "autoinstall",
                    "-k",
                    "6.12.1",
                    "--dkmstree",
                    "/usr/lib/dkms"
                ]
            )]
        );
    }

    fn failing(ns_status: &str) -> ScriptedRunner {
        let r = ScriptedRunner::new();
        r.on_tail(
            &[
                "dkms",
                "autoinstall",
                "-k",
                "6.12.1",
                "--dkmstree",
                "/usr/lib/dkms",
            ],
            Reply::Fail("Error! Bad return status for module build".into()),
        );
        r.on_tail(
            &["dkms", "status", "-k", "6.12.1"],
            Reply::Out(ns_status.into()),
        );
        r.on_tail(
            &["pacman", "-Qqo", "/usr/src/nvidia-550.54"],
            Reply::Out("nvidia-dkms\n".into()),
        );
        r
    }

    #[test]
    fn a_failed_build_names_the_module_and_its_user_owner() {
        let (_root, ns) = snapshot(true);
        let r = failing(STATUS);
        let user = vec!["nvidia-dkms".to_owned(), "vim".to_owned()];
        match build_modules(&r, &ns, "6.12.1", &user).unwrap_err() {
            TxError::Failed {
                reason,
                proposal,
                log,
            } => {
                assert!(reason.contains("nvidia/550.54"), "{reason}");
                assert!(reason.contains("nvidia-dkms"), "{reason}");
                assert!(proposal.unwrap().contains("nvidia-dkms"));
                assert!(log.iter().any(|l| l.contains("Bad return status")));
            }
            other => panic!("{other:?}"),
        }
        let lines = r.lines();
        assert!(lines.iter().any(|l| l.ends_with("dkms status -k 6.12.1")));
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("pacman -Qqo /usr/src/nvidia-550.54"))
        );
    }

    #[test]
    fn a_module_owned_by_the_base_makes_no_proposal() {
        let (_root, ns) = snapshot(true);
        let r = failing(STATUS);
        match build_modules(&r, &ns, "6.12.1", &["vim".to_owned()]).unwrap_err() {
            TxError::Failed {
                proposal, reason, ..
            } => {
                assert!(proposal.is_none());
                assert!(reason.contains("nvidia/550.54"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failing_status_still_reports_the_build_failure() {
        let (_root, ns) = snapshot(true);
        let r = ScriptedRunner::new();
        r.on_tail(
            &[
                "dkms",
                "autoinstall",
                "-k",
                "6.12.1",
                "--dkmstree",
                "/usr/lib/dkms",
            ],
            Reply::Fail("Error! build failed".into()),
        );
        r.on_tail(
            &["dkms", "status", "-k", "6.12.1"],
            Reply::Fail("boom".into()),
        );
        match build_modules(&r, &ns, "6.12.1", &[]).unwrap_err() {
            TxError::Failed { reason, .. } => assert!(reason.contains("build failed"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
}
