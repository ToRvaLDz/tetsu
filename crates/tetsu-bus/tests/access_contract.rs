//! The gate's name rule agrees with the IDL: no capability tag is ever
//! free, no read is gated by mistake, and every refusal is a declared error.

use std::path::PathBuf;

use serde_json::json;
use tetsu_bus::access::{MethodKind, method_kind, permission_denied};
use tetsu_bus_contract::{Contract, check_error, load_dir};

/// Untagged methods that are not reads: the capability service's own.
const UNTAGGED_GATED: [&str; 3] = [
    "org.tetsu.cap.v1.Check",
    "org.tetsu.cap.v1.Intent",
    "org.tetsu.cap.v1.Set",
];

fn contract() -> Contract {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl");
    load_dir(&dir).expect("the bus contract loads")
}

fn methods(contract: &Contract) -> Vec<(String, bool)> {
    contract
        .interfaces
        .values()
        .flat_map(|iface| {
            iface.methods.iter().map(|(name, m)| {
                (
                    format!("{}.{name}", iface.name),
                    m.doc.capability().is_some(),
                )
            })
        })
        .collect()
}

#[test]
fn the_idl_has_methods_to_check() {
    assert!(methods(&contract()).len() > 20);
}

#[test]
fn every_capability_method_is_gated() {
    for (fqn, tagged) in methods(&contract()) {
        if tagged {
            assert_eq!(method_kind(&fqn), MethodKind::Gated, "{fqn}");
        }
    }
}

#[test]
fn every_untagged_read_is_free_and_nothing_else_is() {
    for (fqn, tagged) in methods(&contract()) {
        if !tagged {
            let expected = if UNTAGGED_GATED.contains(&fqn.as_str()) {
                MethodKind::Gated
            } else {
                MethodKind::Free
            };
            assert_eq!(method_kind(&fqn), expected, "{fqn}");
        }
    }
}

#[test]
fn no_free_method_carries_a_capability() {
    for (fqn, tagged) in methods(&contract()) {
        assert!(
            !(tagged && method_kind(&fqn) == MethodKind::Free),
            "{fqn} is free but tagged"
        );
    }
}

#[test]
fn the_refusal_of_every_gated_method_is_a_declared_error() {
    let contract = contract();
    for (fqn, _) in methods(&contract) {
        if method_kind(&fqn) == MethodKind::Gated {
            let error = permission_denied(&fqn);
            let found = check_error(&contract, &error.error, &error.parameters);
            assert_eq!(found, vec![], "{fqn}");
            assert_eq!(error.parameters, json!({}));
        }
    }
}
