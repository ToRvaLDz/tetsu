//! The contract loads: every IDL file parses and the streaming list is read.

use std::path::PathBuf;

use tetsu_bus_contract::load_dir;

fn idl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl")
}

#[test]
fn current_idl_loads() {
    let contract = load_dir(&idl_dir()).expect("idl loads");
    assert_eq!(contract.interfaces.len(), 10);
    assert!(contract.is_streaming("org.tetsu.notify.v1", "Show"));
}

#[test]
fn baseline_loads() {
    let contract = load_dir(&idl_dir().join("baseline")).expect("baseline loads");
    // The baseline lags behind the IDL until the next bless.
    assert!(contract.interfaces.len() >= 9);
}

#[cfg(unix)]
#[test]
fn unreadable_directory_entry_is_an_error_not_skipped() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = std::env::temp_dir().join(format!("tetsu-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A name that is not UTF-8 cannot be told apart from a *.varlink file.
    let name = OsString::from_vec(b"bad-\xff.varlink".to_vec());
    std::fs::write(dir.join(name), "interface org.x.y").unwrap();
    let result = tetsu_bus_contract::read_dir_sources(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err(), "{result:?}");
}
