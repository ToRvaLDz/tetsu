//! The real contract: the shipped IDL against its frozen baseline, the tag
//! and doc rules, and the event fixtures.

use std::path::PathBuf;

use tetsu_bus_contract::{
    Violation, check_compat, check_fixtures, check_rules, load_dir, read_fixtures,
};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn report(what: &str, violations: &[Violation]) {
    let lines: Vec<String> = violations.iter().map(|v| format!("  {v}")).collect();
    assert!(
        violations.is_empty(),
        "{what}: {} violation(s)\n{}",
        violations.len(),
        lines.join("\n")
    );
}

#[test]
fn current_idl_is_a_compatible_superset_of_the_baseline() {
    let baseline = load_dir(&crate_dir().join("idl/baseline")).expect("baseline loads");
    let current = load_dir(&crate_dir().join("idl")).expect("idl loads");
    report("compat", &check_compat(&baseline, &current));
}

#[test]
fn baseline_passes_the_rules_it_was_blessed_under() {
    let baseline = load_dir(&crate_dir().join("idl/baseline")).expect("baseline loads");
    report("baseline rules", &check_rules(&baseline));
}

#[test]
fn current_idl_passes_the_tag_and_doc_rules() {
    let current = load_dir(&crate_dir().join("idl")).expect("idl loads");
    report("rules", &check_rules(&current));
}

#[test]
fn event_fixtures_match_the_idl() {
    let current = load_dir(&crate_dir().join("idl")).expect("idl loads");
    let fixtures =
        read_fixtures(&crate_dir().join("tests/fixtures/events")).expect("fixtures read");
    report("fixtures", &check_fixtures(&current, &fixtures));
}
