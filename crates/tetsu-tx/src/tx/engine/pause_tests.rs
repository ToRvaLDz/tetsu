//! The pause points the engine reaches, in protocol order.

use super::tests::{Env, pkgs};
use super::*;
use crate::fault::{self, Point};

#[test]
fn an_update_reaches_every_pause_point_in_protocol_order() {
    let env = Env::new();
    fault::take_seen();
    env.run(&Request::Update).unwrap();
    assert_eq!(fault::take_seen(), Point::ALL);
}

#[test]
fn an_install_reaches_every_pause_point_in_protocol_order() {
    let env = Env::new();
    fault::take_seen();
    env.run(&Request::Install(pkgs(&["vim"]))).unwrap();
    assert_eq!(fault::take_seen(), Point::ALL);
}

#[test]
fn a_rollback_has_no_package_phase_to_pause_in() {
    let env = Env::two_snapshots();
    fault::take_seen();
    env.run(&Request::Rollback(Some(1))).unwrap();
    let expected: Vec<Point> = Point::ALL
        .into_iter()
        .filter(|p| !matches!(p, Point::Downloaded | Point::Installed))
        .collect();
    assert_eq!(fault::take_seen(), expected);
}
