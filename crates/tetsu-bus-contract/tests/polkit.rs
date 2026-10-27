//! Derivazione delle azioni polkit dalle classi `@capability`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use tetsu_bus_contract::{Contract, capability_classes, load_dir, polkit_action_id, polkit_policy};

fn contract(sources: &[(&str, &str)]) -> Contract {
    Contract::from_sources(sources, "").expect("sources parse")
}

const ALPHA: &str = "interface org.tetsu.alpha.v1

# @capability packages.install
Install() -> ()

# @capability packages.install
Reinstall() -> ()

# @capability aur.re_view
Review() -> ()

Status() -> ()
";

const BETA: &str = "interface org.tetsu.beta.v1

# @capability notify.send
Send() -> ()

# @capability packages.install
Other() -> ()
";

#[test]
fn one_action_per_distinct_class_sorted_with_methods() {
    let c = contract(&[("a.varlink", ALPHA), ("b.varlink", BETA)]);
    let classes = capability_classes(&c).expect("classes");
    let keys: Vec<&str> = classes.keys().map(String::as_str).collect();
    assert_eq!(keys, ["aur.re_view", "notify.send", "packages.install"]);
    assert_eq!(
        classes["packages.install"],
        [
            "org.tetsu.alpha.v1.Install",
            "org.tetsu.alpha.v1.Reinstall",
            "org.tetsu.beta.v1.Other"
        ]
    );
}

#[test]
fn underscore_maps_to_dash() {
    assert_eq!(polkit_action_id("aur.re_view"), "org.tetsu.aur.re-view");
    assert_eq!(
        polkit_action_id("packages.install"),
        "org.tetsu.packages.install"
    );
    let c = contract(&[("a.varlink", ALPHA)]);
    assert!(
        polkit_policy(&c)
            .unwrap()
            .contains("<action id=\"org.tetsu.aur.re-view\">")
    );
}

#[test]
fn methods_without_capability_contribute_nothing() {
    let c = contract(&[("a.varlink", ALPHA)]);
    let all: Vec<String> = capability_classes(&c)
        .unwrap()
        .into_values()
        .flatten()
        .collect();
    assert!(!all.iter().any(|m| m.ends_with(".Status")));
}

#[test]
fn malformed_capability_is_an_error() {
    let bad = "interface org.tetsu.bad.v1

# @capability Packages
Do() -> ()
";
    let c = contract(&[("bad.varlink", bad)]);
    assert!(capability_classes(&c).is_err());
    assert!(polkit_policy(&c).is_err());
}

#[test]
fn no_class_at_all_is_an_error() {
    let none = "interface org.tetsu.none.v1

Get() -> ()
";
    let c = contract(&[("n.varlink", none)]);
    assert!(polkit_policy(&c).is_err());
}

#[test]
fn every_action_defaults_to_auth_admin() {
    let c = contract(&[("a.varlink", ALPHA), ("b.varlink", BETA)]);
    let xml = polkit_policy(&c).unwrap();
    assert!(
        xml.starts_with(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE policyconfig PUBLIC"
        )
    );
    let actions = xml.matches("<action id=").count();
    assert_eq!(actions, 3);
    for tag in ["allow_any", "allow_inactive", "allow_active"] {
        assert_eq!(
            xml.matches(&format!("<{tag}>auth_admin</{tag}>")).count(),
            actions,
            "{tag}"
        );
    }
}

/// Le classi dell'IDL reale, lette in modo indipendente dal testo.
fn scanned_ids(dir: &std::path::Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "varlink") {
            continue;
        }
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            if let Some(rest) = line.strip_prefix("# @capability ") {
                let class = rest.split_whitespace().next().unwrap();
                ids.insert(format!("org.tetsu.{}", class.replace('_', "-")));
            }
        }
    }
    ids
}

#[test]
fn policy_matches_the_idl_on_disk() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    let c = load_dir(&dir).expect("idl loads");
    let from_contract: BTreeSet<String> = capability_classes(&c)
        .unwrap()
        .keys()
        .map(|k| polkit_action_id(k))
        .collect();
    assert_eq!(from_contract, scanned_ids(&dir));
    assert!(!from_contract.is_empty());
}
