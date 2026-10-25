//! The base guard: the packages of the base closure must survive a
//! transaction. The closure is computed from the pre-transaction DB and the
//! installed set is compared after the install phase.

use std::collections::BTreeSet;

use super::TxError;
use super::pkg::{self, BASE_PACKAGE, Packages};
use crate::store::record::TxKind;

/// The base closure the guard protects, or `None` when `tetsu-base` is not
/// installed. Without it the base cannot be told apart from the user layer:
/// `Install` and `Remove` fail, `Update` and `Rollback` go on unguarded.
pub fn base_before(
    infos: &[pkg::PackageInfo],
    kind: TxKind,
) -> Result<Option<BTreeSet<String>>, TxError> {
    match pkg::base_closure(infos, BASE_PACKAGE) {
        Some(closure) => Ok(Some(closure)),
        None if matches!(kind, TxKind::Install | TxKind::InstallAur | TxKind::Remove) => {
            Err(TxError::Failed {
                reason: format!(
                    "{BASE_PACKAGE} is not installed: the base cannot be told apart from your packages"
                ),
                proposal: Some(format!("install {BASE_PACKAGE}, then retry")),
                log: Vec::new(),
            })
        }
        None => Ok(None),
    }
}

/// Fail naming every package of `closure_before` that `after` no longer
/// holds: a base package removed, or replaced by another.
pub fn check_base_kept(closure_before: &BTreeSet<String>, after: &Packages) -> Result<(), TxError> {
    let lost: Vec<&str> = closure_before
        .iter()
        .filter(|name| !after.contains_key(*name))
        .map(String::as_str)
        .collect();
    if lost.is_empty() {
        return Ok(());
    }
    Err(TxError::failed(format!(
        "the transaction would remove or replace base package(s): {}",
        lost.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::pkg::PackageInfo;

    fn info(name: &str, depends: &[&str]) -> PackageInfo {
        PackageInfo {
            name: name.into(),
            version: "1".into(),
            depends: depends.iter().map(|s| (*s).into()).collect(),
            provides: Vec::new(),
        }
    }

    fn closure(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    fn installed(names: &[&str]) -> Packages {
        names
            .iter()
            .map(|n| ((*n).to_owned(), "1".to_owned()))
            .collect()
    }

    #[test]
    fn a_kept_base_passes_even_with_new_and_upgraded_packages() {
        let mut after = installed(&["tetsu-base", "bash", "vim"]);
        after.insert("bash".into(), "2".into());
        assert!(check_base_kept(&closure(&["tetsu-base", "bash"]), &after).is_ok());
    }

    #[test]
    fn a_removed_or_replaced_base_package_is_named() {
        let before = closure(&["tetsu-base", "bash", "sudo"]);
        let err = check_base_kept(&before, &installed(&["tetsu-base", "doas"])).unwrap_err();
        match err {
            TxError::Failed { reason, .. } => {
                assert!(
                    reason.contains("bash") && reason.contains("sudo"),
                    "{reason}"
                );
                assert!(!reason.contains("doas"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_closure_comes_from_the_base_package() {
        let infos = [
            info("tetsu-base", &["bash"]),
            info("bash", &[]),
            info("vim", &[]),
        ];
        let c = base_before(&infos, TxKind::Install).unwrap().unwrap();
        assert_eq!(c, closure(&["tetsu-base", "bash"]));
    }

    #[test]
    fn without_tetsu_base_install_and_remove_fail_and_update_runs() {
        let infos = [info("bash", &[])];
        for kind in [TxKind::Install, TxKind::InstallAur, TxKind::Remove] {
            match base_before(&infos, kind).unwrap_err() {
                TxError::Failed { reason, .. } => {
                    assert!(reason.contains("tetsu-base"), "{reason}");
                }
                other => panic!("{other:?}"),
            }
        }
        for kind in [TxKind::Update, TxKind::Rollback] {
            assert_eq!(base_before(&infos, kind).unwrap(), None);
        }
    }
}
