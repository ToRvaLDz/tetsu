//! The hand-written catalog must equal what the IDL tags say.

use std::collections::BTreeMap;
use std::path::PathBuf;

use tetsu_bus_contract::load_dir;
use tetsu_capd::catalog::{self, ENTRIES};

type Row = (Option<String>, Option<String>, bool);

fn idl_rows() -> BTreeMap<String, Row> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    let contract = load_dir(&dir).expect("the bus contract loads");
    let mut rows = BTreeMap::new();
    for (iname, iface) in &contract.interfaces {
        for (mname, method) in &iface.methods {
            let capability = method.doc.capability().map(|c| c.expect("a valid tag"));
            let row = (
                capability.as_ref().map(|c| c.class.clone()),
                capability.and_then(|c| c.scope),
                method.doc.has_tag("@destructive"),
            );
            rows.insert(format!("{iname}.{mname}"), row);
        }
    }
    rows
}

fn table_rows() -> BTreeMap<String, Row> {
    ENTRIES
        .iter()
        .map(|e| {
            let row = (
                e.class.map(str::to_owned),
                e.scope_param.map(str::to_owned),
                e.destructive,
            );
            (e.method.to_owned(), row)
        })
        .collect()
}

#[test]
fn the_table_has_every_method_of_the_idl_and_nothing_more() {
    let idl = idl_rows();
    let table = table_rows();
    assert_eq!(table.len(), ENTRIES.len(), "no method is listed twice");
    for (method, row) in &idl {
        assert_eq!(
            table.get(method),
            Some(row),
            "{method} differs or is missing"
        );
    }
    for method in table.keys() {
        assert!(idl.contains_key(method), "{method} is not in the IDL");
    }
}

#[test]
fn lookup_finds_a_method_and_refuses_an_unknown_name() {
    let update = catalog::lookup("org.tetsu.tx.v1.Update").unwrap();
    assert_eq!(update.class, Some("system.update"));
    assert!(update.destructive);
    let install = catalog::lookup("org.tetsu.tx.v1.Install").unwrap();
    assert!(!install.destructive);
    let read = catalog::lookup("org.tetsu.tx.v1.ListTransactions").unwrap();
    assert_eq!(read.class, None);
    assert!(catalog::lookup("org.tetsu.tx.v1.Nope").is_none());
    assert!(catalog::lookup("").is_none());
}

#[test]
fn classes_group_methods_and_carry_the_destructive_flag() {
    let classes = catalog::classes();
    let update = classes.iter().find(|c| c.name == "system.update").unwrap();
    assert!(update.destructive);
    assert_eq!(update.methods, vec!["org.tetsu.tx.v1.Update"]);
    let notify = classes.iter().find(|c| c.name == "notify.send").unwrap();
    assert_eq!(notify.methods.len(), 3);
    assert!(!notify.destructive);
    let mut names: Vec<_> = classes.iter().map(|c| c.name).collect();
    names.dedup();
    assert_eq!(names.len(), classes.len(), "one entry per class");
    assert!(catalog::is_class("packages.install"));
    assert!(!catalog::is_class("packages.nope"));
}
