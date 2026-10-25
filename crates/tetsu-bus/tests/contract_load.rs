//! The contract loads: every IDL file parses and the streaming list is read.

use std::path::PathBuf;

use tetsu_bus_contract::load_dir;

fn idl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl")
}

#[test]
fn current_idl_loads() {
    let contract = load_dir(&idl_dir()).expect("idl loads");
    assert_eq!(contract.interfaces.len(), 9);
    assert!(contract.is_streaming("org.tetsu.notify.v1", "Show"));
}

#[test]
fn baseline_loads() {
    let contract = load_dir(&idl_dir().join("baseline")).expect("baseline loads");
    assert_eq!(contract.interfaces.len(), 9);
}
