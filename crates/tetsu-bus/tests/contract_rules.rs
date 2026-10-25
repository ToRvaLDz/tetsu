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
# @capability test.do
method Do(
  # The name.
  # @untrusted
  name: string
) -> (result: string)

# Wipes everything.
# @destructive
# @deprecated
# @capability test.wipe
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
fn destructive_on_input_parameter() {
    expect(
        &mutated("# @untrusted\n  name", "# @destructive\n  name"),
        Kind::MisplacedTag,
        "Do(in).name",
    );
}

#[test]
fn deprecated_on_input_parameter_is_allowed() {
    let v = mutated("# @untrusted\n  name", "# @deprecated\n  name");
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn deprecated_on_output_field_is_allowed() {
    let v = mutated(
        "-> (result: string)",
        "-> (\n  # @deprecated\n  result: string\n)",
    );
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn untrusted_on_output_field_is_allowed() {
    let v = mutated(
        "-> (result: string)",
        "-> (\n  # @untrusted\n  result: string\n)",
    );
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn deprecated_on_error_parameter_is_allowed() {
    let v = mutated("# @untrusted\n  why", "# @deprecated\n  why");
    assert!(v.is_empty(), "{v:#?}");
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

// The capability tag.

/// The `org.tetsu.cap.v1` methods the required-capability rule exempts.
const CAP_IDL: &str = r#"
interface org.tetsu.cap.v1

# Declares an intent.
method Intent(
  # The method.
  method: string
) -> ()

# Sets a grant.
method Set(
  # The class.
  class: string
) -> ()

# Asks for a decision.
method Check(
  # The method.
  method: string
) -> ()

# The caller may not use this interface.
# @permanent
error PermissionDenied ()
"#;

/// Violations of GOOD with `extra` appended.
fn extended(extra: &str) -> Vec<Violation> {
    rules_of(&format!("{GOOD}\n{extra}\n"), STREAMING)
}

#[test]
fn state_changing_method_without_capability() {
    expect(
        &mutated("# @capability test.do\n", ""),
        Kind::MissingCapability,
        "Do",
    );
}

#[test]
fn untagged_reads_pass() {
    let v = extended(
        "# Lists items.\nmethod ListItems() -> (items: []Item)\n\n\
         # Gets the item.\nmethod GetItem() -> (item: Item)",
    );
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn read_prefix_needs_a_word_boundary() {
    let v = extended("# Starts listening.\nmethod Listen() -> ()");
    expect(&v, Kind::MissingCapability, "Listen");
}

#[test]
fn tagged_read_passes() {
    let v =
        extended("# Lists items.\n# @capability test.read\nmethod ListItems() -> (items: []Item)");
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn capability_with_scope_naming_an_input_passes() {
    let v = mutated(
        "# @capability test.do\n",
        "# @capability test.do scope=name\n",
    );
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn malformed_capability_tags() {
    for bad in [
        "@capability",
        "@capability packages",
        "@capability Test.do",
        "@capability test.Do",
        "@capability test..do",
        "@capability 1test.do",
        "@capability test.do extra",
        "@capability test.do scope=nope",
        "@capability test.do scope=",
        "@capability test.do scope=name extra",
        "@capability scope=name",
    ] {
        let v = mutated("@capability test.do", bad);
        expect(&v, Kind::InvalidCapability, "Do");
    }
}

#[test]
fn capability_on_field() {
    expect(
        &mutated("# @untrusted\n  title", "# @capability test.x\n  title"),
        Kind::MisplacedTag,
        "Item.title",
    );
}

#[test]
fn capability_on_error() {
    expect(
        &mutated(
            "# Try again later.",
            "# Try again later.\n# @capability test.x",
        ),
        Kind::MisplacedTag,
        "Busy",
    );
}

#[test]
fn duplicate_capability() {
    expect(
        &mutated(
            "# @capability test.do\n",
            "# @capability test.do\n# @capability test.do\n",
        ),
        Kind::DuplicateTag,
        "Do",
    );
}

#[test]
fn capability_interface_methods_are_exempt() {
    let v = check_rules(&Contract::from_sources(&[("cap.varlink", CAP_IDL)], "").unwrap());
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn exemption_is_limited_to_the_capability_interface() {
    let idl = CAP_IDL.replacen("org.tetsu.cap.v1", "org.other.v1", 1);
    let v = check_rules(&Contract::from_sources(&[("other.varlink", &idl)], "").unwrap());
    for m in ["Intent", "Set", "Check"] {
        assert!(
            v.iter()
                .any(|x| x.kind == Kind::MissingCapability && x.path == format!("org.other.v1.{m}")),
            "{m}: {v:#?}"
        );
    }
}
