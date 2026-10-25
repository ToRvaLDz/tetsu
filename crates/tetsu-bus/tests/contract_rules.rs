//! Tag and doc rules: one test per rule, each on a small IDL that breaks
//! only that rule.

use tetsu_bus_contract::{Contract, Kind, Violation, check_rules};

const IFACE: &str = "org.test.v1";

const GOOD: &str = r#"
interface org.test.v1

type Item (
  id: string,
  state: (open, closed),
  # @untrusted
  title: ?string,
  # @deprecated
  old: ?int,
  nested: (
    # @untrusted
    text: ?string
  )
)

# Does a thing.
method Do(
  # The name.
  # @untrusted
  name: string
) -> (result: string)

# Wipes everything.
# @destructive
# @deprecated
method Wipe() -> ()

# Streams items.
method WatchItems() -> (event: Item)

# Try again later.
# @transient
error Busy (id: string)

# Do not retry.
# @permanent
error Other (
  # @untrusted
  why: string
)

# The caller may not use this interface.
# @permanent
error PermissionDenied ()
"#;

const STREAMING: &str = "org.test.v1.WatchItems\n";

fn contract(idl: &str, streaming: &str) -> Contract {
    Contract::from_sources(&[("test.varlink", idl)], streaming).expect("test idl parses")
}

fn rules_of(idl: &str, streaming: &str) -> Vec<Violation> {
    check_rules(&contract(idl, streaming))
}

/// Violations of GOOD with the first `from` replaced by `to`.
fn mutated(from: &str, to: &str) -> Vec<Violation> {
    assert!(GOOD.contains(from), "mutation target {from:?} not in GOOD");
    rules_of(&GOOD.replacen(from, to, 1), STREAMING)
}

fn expect(violations: &[Violation], kind: Kind, path_suffix: &str) {
    assert!(
        violations
            .iter()
            .any(|v| v.kind == kind && v.path == format!("{IFACE}.{path_suffix}")),
        "expected {kind:?} at {IFACE}.{path_suffix}, got {violations:#?}"
    );
}

#[test]
fn good_contract_passes() {
    let v = rules_of(GOOD, STREAMING);
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn unknown_tag() {
    expect(
        &mutated("# Does a thing.", "# Does a thing.\n# @frobnicate"),
        Kind::UnknownTag,
        "Do",
    );
}

#[test]
fn tag_with_trailing_text() {
    expect(
        &mutated("# @destructive\n", "# @destructive because data\n"),
        Kind::UnknownTag,
        "Wipe",
    );
}

#[test]
fn two_tags_on_one_line() {
    expect(
        &mutated("# @destructive\n", "# @destructive @deprecated\n"),
        Kind::UnknownTag,
        "Wipe",
    );
}

#[test]
fn duplicate_tag() {
    expect(
        &mutated("# @destructive\n", "# @destructive\n# @destructive\n"),
        Kind::DuplicateTag,
        "Wipe",
    );
}

#[test]
fn destructive_on_field() {
    expect(
        &mutated("# @untrusted\n  title", "# @destructive\n  title"),
        Kind::MisplacedTag,
        "Item.title",
    );
}

#[test]
fn destructive_on_error() {
    expect(
        &mutated("# Try again later.", "# Try again later.\n# @destructive"),
        Kind::MisplacedTag,
        "Busy",
    );
}

#[test]
fn transient_on_method() {
    expect(
        &mutated("# Does a thing.", "# Does a thing.\n# @transient"),
        Kind::MisplacedTag,
        "Do",
    );
}

#[test]
fn permanent_on_field() {
    expect(
        &mutated("# @deprecated\n  old", "# @permanent\n  old"),
        Kind::MisplacedTag,
        "Item.old",
    );
}

#[test]
fn untrusted_on_method() {
    expect(
        &mutated("# Does a thing.", "# Does a thing.\n# @untrusted"),
        Kind::MisplacedTag,
        "Do",
    );
}

#[test]
fn untrusted_on_error() {
    expect(
        &mutated("# Do not retry.", "# Do not retry.\n# @untrusted"),
        Kind::MisplacedTag,
        "Other",
    );
}

#[test]
fn deprecated_on_error() {
    expect(
        &mutated("# Do not retry.", "# Do not retry.\n# @deprecated"),
        Kind::MisplacedTag,
        "Other",
    );
}

#[test]
fn deprecated_on_input_parameter() {
    expect(
        &mutated("# @untrusted\n  name", "# @deprecated\n  name"),
        Kind::MisplacedTag,
        "Do(in).name",
    );
}

#[test]
fn tag_on_enum_value() {
    expect(
        &mutated(
            "state: (open, closed)",
            "state: (\n    # @untrusted\n    open, closed)",
        ),
        Kind::MisplacedTag,
        "Item.state.open",
    );
}

#[test]
fn tag_on_interface() {
    let idl = GOOD.replacen(
        "interface org.test.v1",
        "# @destructive\ninterface org.test.v1",
        1,
    );
    let v = rules_of(&idl, STREAMING);
    assert!(
        v.iter()
            .any(|x| x.kind == Kind::MisplacedTag && x.path == IFACE),
        "{v:#?}"
    );
}

#[test]
fn error_without_class() {
    expect(
        &mutated("# Try again later.\n# @transient\n", "# Try again later.\n"),
        Kind::ErrorClass,
        "Busy",
    );
}

#[test]
fn error_with_both_classes() {
    expect(
        &mutated("# @transient\n", "# @transient\n# @permanent\n"),
        Kind::ErrorClass,
        "Busy",
    );
}

#[test]
fn method_without_doc() {
    expect(&mutated("# Does a thing.\n", ""), Kind::MissingDoc, "Do");
}

#[test]
fn method_with_only_tags() {
    expect(
        &mutated("# Wipes everything.\n", ""),
        Kind::MissingDoc,
        "Wipe",
    );
}

#[test]
fn input_parameter_without_doc() {
    expect(
        &mutated("# The name.\n  # @untrusted\n  name", "name"),
        Kind::MissingDoc,
        "Do(in).name",
    );
}

#[test]
fn input_parameter_with_only_tags() {
    expect(
        &mutated("# The name.\n  # @untrusted", "# @untrusted"),
        Kind::MissingDoc,
        "Do(in).name",
    );
}

#[test]
fn permission_denied_missing() {
    let v = mutated("error PermissionDenied ()", "error Denied ()");
    assert!(
        v.iter()
            .any(|x| x.kind == Kind::MissingPermissionDenied && x.path == IFACE),
        "{v:#?}"
    );
}

#[test]
fn streaming_list_names_unknown_method() {
    let v = rules_of(GOOD, "org.test.v1.WatchItems\norg.test.v1.Nope\n");
    expect(&v, Kind::StreamingUnknown, "Nope");
}

#[test]
fn streaming_list_names_unknown_interface() {
    let v = rules_of(GOOD, "org.test.v1.WatchItems\norg.other.v1.Show\n");
    assert!(
        v.iter()
            .any(|x| x.kind == Kind::StreamingUnknown && x.path == "org.other.v1.Show"),
        "{v:#?}"
    );
}

#[test]
fn watch_method_missing_from_streaming_list() {
    let v = rules_of(GOOD, "");
    expect(&v, Kind::WatchNotStreaming, "WatchItems");
}

#[test]
fn streaming_list_comments_and_blanks_are_ignored() {
    let v = rules_of(GOOD, "# the list\n\norg.test.v1.WatchItems # stream\n");
    assert!(v.is_empty(), "{v:#?}");
}
