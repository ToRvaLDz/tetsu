//! The package phase of a transaction: what runs in the container for each
//! request, with the base guard around it and the blocking user packages
//! named when an update cannot go through.

use std::path::Path;

use super::Request;
use crate::boot::tool::Runner;
use crate::fault::{self, Point};
use crate::tx::nspawn::Nspawn;
use crate::tx::pkg::{self, Packages};
use crate::tx::{TxError, guard, steps};

/// What the package phase left behind.
pub(super) struct Phase {
    pub before: Packages,
    pub after: Packages,
    /// The user layer once the phase is done, for DKMS attribution.
    pub user_layer: Vec<String>,
}

/// The proposal for an update a user package stands in the way of.
fn remove_then_update(blockers: &[String]) -> Option<String> {
    (!blockers.is_empty()).then(|| format!("tx remove {}, then tx update", blockers.join(" ")))
}

/// A failed update step with the user packages its message names.
fn blocked_by_user(e: TxError, user_layer: &[String]) -> TxError {
    match e {
        TxError::Failed {
            reason,
            proposal: None,
            log,
        } => {
            let blockers = pkg::blocking_packages(&log.join("\n"), user_layer);
            TxError::Failed {
                reason,
                proposal: remove_then_update(&blockers),
                log,
            }
        }
        other => other,
    }
}

/// A failed DKMS build of an update: the user packages owning the modules
/// that did not build are the ones to remove.
pub(super) fn dkms_blocked(e: TxError, request: &Request, user_layer: &[String]) -> TxError {
    let TxError::Failed {
        reason,
        proposal,
        log,
    } = e
    else {
        return e;
    };
    let mut owners: Vec<String> = Vec::new();
    for line in &log {
        let Some((_, list)) = line.split_once("(owned by ") else {
            continue;
        };
        for owner in list.trim_end_matches(')').split(", ") {
            if user_layer.iter().any(|u| u == owner) && !owners.iter().any(|o| o == owner) {
                owners.push(owner.to_owned());
            }
        }
    }
    let proposal = match request {
        Request::Update => remove_then_update(&owners).or(proposal),
        _ => proposal,
    };
    TxError::Failed {
        reason,
        proposal,
        log,
    }
}

fn update(runner: &dyn Runner, ns: &Nspawn, user_layer: &[String]) -> Result<(), TxError> {
    steps::keyring_download(runner, ns)?;
    steps::keyring_install(runner, ns)?;
    steps::update_download(runner, ns).map_err(|e| blocked_by_user(e, user_layer))?;
    fault::reach(Point::Downloaded);
    steps::update_install(runner, ns).map_err(|e| blocked_by_user(e, user_layer))?;
    let orphans = steps::orphans(runner, ns)?;
    steps::remove_orphans(runner, ns, &orphans)
}

/// The user layer after `request` ran.
fn user_layer_after(request: &Request, before: &[String]) -> Vec<String> {
    let mut layer = before.to_vec();
    match request {
        Request::Install(pkgs) => {
            for p in pkgs {
                if !layer.contains(p) {
                    layer.push(p.clone());
                }
            }
        }
        Request::Remove(pkgs) => layer.retain(|p| !pkgs.contains(p)),
        Request::Update | Request::Rollback(_) => {}
    }
    layer.sort();
    layer
}

/// Run the package phase of `request` over `root`. The base closure is
/// taken from the DB before and must survive an `Install`; `Install` and
/// `Remove` need `tetsu-base` installed.
pub(super) fn run(
    runner: &dyn Runner,
    ns: &Nspawn,
    root: &Path,
    request: &Request,
) -> Result<Phase, TxError> {
    let infos = pkg::infos(runner, root)?;
    let closure = guard::base_before(&infos, request.kind())?;
    let before: Packages = infos
        .iter()
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();
    let explicit = pkg::explicit(runner, root)?;
    let user_before = closure
        .as_ref()
        .map(|c| pkg::user_layer(&explicit, c))
        .unwrap_or_default();
    match request {
        Request::Update => update(runner, ns, &user_before)?,
        Request::Install(pkgs) => {
            steps::install_download(runner, ns, pkgs)?;
            fault::reach(Point::Downloaded);
            steps::install_apply(runner, ns, pkgs)?;
        }
        Request::Remove(pkgs) => steps::remove(runner, ns, pkgs, &user_before)?,
        Request::Rollback(_) => {
            return Err(TxError::failed("a rollback has no package phase"));
        }
    }
    let after = pkg::installed(runner, root)?;
    // The user layer must never remove or replace a base package. An update
    // is upstream's to change the base (a dropped dependency, a rename), and
    // a remove was checked against the user layer before it ran.
    if let (Request::Install(_), Some(closure)) = (request, &closure) {
        guard::check_base_kept(closure, &after)?;
    }
    Ok(Phase {
        before,
        after,
        user_layer: user_layer_after(request, &user_before),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn an_installed_package_joins_the_user_layer_even_if_it_was_a_dependency() {
        // `vim` was installed as a dependency, so it is not in `before`.
        let after = user_layer_after(&Request::Install(names(&["vim"])), &names(&["htop"]));
        assert_eq!(after, ["htop", "vim"]);
    }
}
