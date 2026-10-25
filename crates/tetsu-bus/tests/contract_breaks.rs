//! Compatibility with the baseline: one test per breaking kind, and one per
//! additive change CONTRACT.md allows. Each negative test mutates a small
//! IDL and expects the matching violation.

use tetsu_bus_contract::{Contract, Kind, Violation, check_compat};

const IFACE: &str = "org.test.v1";

const BASE: &str = r#"
interface org.test.v1

type Item (
  id: string,
  # @untrusted
  title: ?string,
  state: (open, closed),
  nested: (x: int, y: ?int),
  tags: []string
)

type Color (red, green)

method Do(
  # The name.
  name: string,
  # @untrusted
  note: ?string,
  count: ?int
) -> (result: string, extra: ?string)

# Wipes.
# @destructive
method Wipe() -> ()

method Watch() -> (event: Item)

# @transient
error Busy (id: string)

# @permanent
error Other (
  # @untrusted
  why: string
)
"#;

const STREAMING: &str = "org.test.v1.Watch\n";

fn contract(idl: &str, streaming: &str) -> Contract {
    Contract::from_sources(&[("test.varlink", idl)], streaming).expect("test idl parses")
}

fn base() -> Contract {
    contract(BASE, STREAMING)
}

/// Violations of BASE with the first `from` replaced by `to`.
fn mutated(from: &str, to: &str) -> Vec<Violation> {
    assert!(BASE.contains(from), "mutation target {from:?} not in BASE");
    let idl = BASE.replacen(from, to, 1);
    check_compat(&base(), &contract(&idl, STREAMING))
}

/// Violations of BASE when `extra` is appended to the interface.
fn extended(extra: &str) -> Vec<Violation> {
    let idl = format!("{BASE}\n{extra}\n");
    check_compat(&base(), &contract(&idl, STREAMING))
}

fn expect(violations: &[Violation], kind: Kind, path_suffix: &str) {
    assert!(
        violations
            .iter()
            .any(|v| v.kind == kind && v.path == format!("{IFACE}.{path_suffix}")),
        "expected {kind:?} at {IFACE}.{path_suffix}, got {violations:#?}"
    );
}

fn expect_clean(violations: &[Violation]) {
    assert!(
        violations.is_empty(),
        "expected no violation: {violations:#?}"
    );
}

// Breaking changes.

#[test]
fn identical_contract_is_compatible() {
    expect_clean(&check_compat(&base(), &base()));
}

#[test]
fn interface_removed() {
    let empty = Contract::from_sources(&[], "").unwrap();
    let v = check_compat(&base(), &empty);
    assert!(
        v.iter()
            .any(|x| x.kind == Kind::InterfaceRemoved && x.path == IFACE),
        "{v:#?}"
    );
}

#[test]
fn interface_renamed() {
    let idl = BASE.replacen("interface org.test.v1", "interface org.test.w1", 1);
    let v = check_compat(&base(), &contract(&idl, "org.test.w1.Watch\n"));
    assert!(v.iter().any(|x| x.kind == Kind::InterfaceRemoved), "{v:#?}");
}

#[test]
fn method_removed() {
    expect(
        &mutated("method Wipe() -> ()", ""),
        Kind::MethodRemoved,
        "Wipe",
    );
}

#[test]
fn method_renamed() {
    expect(
        &mutated("method Wipe() -> ()", "method Erase() -> ()"),
        Kind::MethodRemoved,
        "Wipe",
    );
}

#[test]
fn type_removed() {
    expect(
        &mutated("type Color (red, green)", ""),
        Kind::TypeRemoved,
        "Color",
    );
}

#[test]
fn field_removed() {
    expect(
        &mutated(",\n  tags: []string", ""),
        Kind::FieldRemoved,
        "Item.tags",
    );
}

#[test]
fn field_renamed() {
    expect(
        &mutated("tags: []string", "labels: []string"),
        Kind::FieldRemoved,
        "Item.tags",
    );
}

#[test]
fn nested_field_removed() {
    expect(
        &mutated("nested: (x: int, y: ?int)", "nested: (x: int)"),
        Kind::FieldRemoved,
        "Item.nested.y",
    );
}

#[test]
fn input_parameter_removed() {
    expect(
        &mutated(",\n  count: ?int", ""),
        Kind::FieldRemoved,
        "Do(in).count",
    );
}

#[test]
fn output_removed() {
    expect(
        &mutated("(result: string, extra: ?string)", "(result: string)"),
        Kind::FieldRemoved,
        "Do(out).extra",
    );
}

#[test]
fn error_removed() {
    expect(
        &mutated("# @transient\nerror Busy (id: string)", ""),
        Kind::ErrorRemoved,
        "Busy",
    );
}

#[test]
fn error_renamed() {
    expect(
        &mutated("error Busy (id: string)", "error Occupied (id: string)"),
        Kind::ErrorRemoved,
        "Busy",
    );
}

#[test]
fn error_parameter_removed() {
    expect(
        &mutated("error Busy (id: string)", "error Busy ()"),
        Kind::FieldRemoved,
        "Busy.id",
    );
}

#[test]
fn enum_value_removed_inline() {
    expect(
        &mutated("state: (open, closed)", "state: (open)"),
        Kind::EnumValueRemoved,
        "Item.state.closed",
    );
}

#[test]
fn enum_value_removed_custom() {
    expect(
        &mutated("type Color (red, green)", "type Color (red)"),
        Kind::EnumValueRemoved,
        "Color.green",
    );
}

#[test]
fn enum_value_renamed() {
    expect(
        &mutated("state: (open, closed)", "state: (open, shut)"),
        Kind::EnumValueRemoved,
        "Item.state.closed",
    );
}

#[test]
fn field_retyped() {
    expect(&mutated("id: string", "id: int"), Kind::Retyped, "Item.id");
}

#[test]
fn field_retyped_inside_optional() {
    expect(
        &mutated("title: ?string", "title: ?int"),
        Kind::Retyped,
        "Item.title",
    );
}

#[test]
fn array_element_retyped() {
    expect(
        &mutated("tags: []string", "tags: []int"),
        Kind::Retyped,
        "Item.tags[]",
    );
}

#[test]
fn input_retyped() {
    expect(
        &mutated("name: string,", "name: int,"),
        Kind::Retyped,
        "Do(in).name",
    );
}

#[test]
fn output_retyped() {
    expect(
        &mutated("result: string", "result: []string"),
        Kind::Retyped,
        "Do(out).result",
    );
}

#[test]
fn error_parameter_retyped() {
    expect(
        &mutated("error Busy (id: string)", "error Busy (id: int)"),
        Kind::Retyped,
        "Busy.id",
    );
}

#[test]
fn custom_type_reference_retyped() {
    expect(
        &mutated("event: Item", "event: Color"),
        Kind::Retyped,
        "Watch(out).event",
    );
}

#[test]
fn custom_type_changed_from_enum_to_object() {
    expect(
        &mutated("type Color (red, green)", "type Color (\n  red: int\n)"),
        Kind::Retyped,
        "Color",
    );
}

#[test]
fn field_made_required() {
    expect(
        &mutated("title: ?string", "title: string"),
        Kind::OptionalityChanged,
        "Item.title",
    );
}

#[test]
fn field_made_optional() {
    expect(
        &mutated("id: string", "id: ?string"),
        Kind::OptionalityChanged,
        "Item.id",
    );
}

#[test]
fn output_made_optional() {
    expect(
        &mutated("result: string", "result: ?string"),
        Kind::OptionalityChanged,
        "Do(out).result",
    );
}

#[test]
fn output_made_required() {
    expect(
        &mutated("extra: ?string", "extra: string"),
        Kind::OptionalityChanged,
        "Do(out).extra",
    );
}

#[test]
fn input_made_required() {
    expect(
        &mutated("count: ?int", "count: int"),
        Kind::OptionalityChanged,
        "Do(in).count",
    );
}

#[test]
fn array_element_made_optional() {
    expect(
        &mutated("tags: []string", "tags: []?string"),
        Kind::OptionalityChanged,
        "Item.tags[]",
    );
}

#[test]
fn new_required_field() {
    expect(
        &mutated("tags: []string", "tags: []string,\n  rank: int"),
        Kind::NewRequired,
        "Item.rank",
    );
}

#[test]
fn new_required_nested_field() {
    expect(
        &mutated(
            "nested: (x: int, y: ?int)",
            "nested: (x: int, y: ?int, z: int)",
        ),
        Kind::NewRequired,
        "Item.nested.z",
    );
}

#[test]
fn new_required_input() {
    expect(
        &mutated("count: ?int", "count: ?int,\n  more: int"),
        Kind::NewRequired,
        "Do(in).more",
    );
}

#[test]
fn new_required_output() {
    expect(
        &mutated("extra: ?string", "extra: ?string, added: bool"),
        Kind::NewRequired,
        "Do(out).added",
    );
}

#[test]
fn new_required_error_parameter() {
    expect(
        &mutated(
            "error Busy (id: string)",
            "error Busy (id: string, since: int)",
        ),
        Kind::NewRequired,
        "Busy.since",
    );
}

#[test]
fn destructive_removed() {
    expect(&mutated("# @destructive\n", ""), Kind::TagRemoved, "Wipe");
}

#[test]
fn untrusted_removed_from_field() {
    expect(
        &mutated("# @untrusted\n  title: ?string", "title: ?string"),
        Kind::TagRemoved,
        "Item.title",
    );
}

#[test]
fn untrusted_removed_from_input_parameter() {
    expect(
        &mutated("# @untrusted\n  note: ?string", "note: ?string"),
        Kind::TagRemoved,
        "Do(in).note",
    );
}

#[test]
fn untrusted_removed_from_error_parameter() {
    expect(
        &mutated("# @untrusted\n  why: string", "why: string"),
        Kind::TagRemoved,
        "Other.why",
    );
}

#[test]
fn error_class_swapped() {
    expect(
        &mutated("# @transient\nerror Busy", "# @permanent\nerror Busy"),
        Kind::ErrorClassChanged,
        "Busy",
    );
}

#[test]
fn error_class_removed() {
    expect(
        &mutated("# @transient\nerror Busy", "error Busy"),
        Kind::ErrorClassChanged,
        "Busy",
    );
}

#[test]
fn method_moved_into_streaming() {
    let current = contract(BASE, "org.test.v1.Watch\norg.test.v1.Do\n");
    expect(
        &check_compat(&base(), &current),
        Kind::StreamingChanged,
        "Do",
    );
}

#[test]
fn method_moved_out_of_streaming() {
    let current = contract(BASE, "");
    expect(
        &check_compat(&base(), &current),
        Kind::StreamingChanged,
        "Watch",
    );
}

// Additive changes CONTRACT.md allows within v1.

#[test]
fn new_interface_is_allowed() {
    let both = Contract::from_sources(
        &[
            ("test.varlink", BASE),
            (
                "other.varlink",
                "interface org.test.other.v1\nmethod Ping() -> ()",
            ),
        ],
        STREAMING,
    )
    .unwrap();
    expect_clean(&check_compat(&base(), &both));
}

#[test]
fn new_method_is_allowed() {
    expect_clean(&extended("method Fresh() -> (ok: bool)"));
}

#[test]
fn new_streaming_method_is_allowed() {
    let idl = format!("{BASE}\nmethod WatchMore() -> (event: Item)\n");
    let current = contract(&idl, "org.test.v1.Watch\norg.test.v1.WatchMore\n");
    expect_clean(&check_compat(&base(), &current));
}

#[test]
fn new_error_is_allowed() {
    expect_clean(&extended("# @permanent\nerror Fresh (reason: string)"));
}

#[test]
fn new_type_is_allowed() {
    expect_clean(&extended("type Fresh (a: int)"));
}

#[test]
fn new_optional_field_is_allowed() {
    expect_clean(&mutated("tags: []string", "tags: []string,\n  rank: ?int"));
}

#[test]
fn new_optional_nested_field_is_allowed() {
    expect_clean(&mutated(
        "nested: (x: int, y: ?int)",
        "nested: (x: int, y: ?int, z: ?int)",
    ));
}

#[test]
fn new_optional_input_is_allowed() {
    expect_clean(&mutated("count: ?int", "count: ?int,\n  more: ?int"));
}

#[test]
fn new_optional_output_is_allowed() {
    expect_clean(&mutated("extra: ?string", "extra: ?string, added: ?bool"));
}

#[test]
fn new_optional_error_parameter_is_allowed() {
    expect_clean(&mutated(
        "error Busy (id: string)",
        "error Busy (id: string, since: ?int)",
    ));
}

#[test]
fn new_enum_value_is_allowed() {
    expect_clean(&mutated(
        "state: (open, closed)",
        "state: (open, closed, archived)",
    ));
}

#[test]
fn new_custom_enum_value_is_allowed() {
    expect_clean(&mutated(
        "type Color (red, green)",
        "type Color (red, green, blue)",
    ));
}

#[test]
fn input_required_to_optional_is_allowed() {
    expect_clean(&mutated("name: string,", "name: ?string,"));
}

#[test]
fn destructive_added_is_allowed() {
    expect_clean(&mutated("method Watch()", "# @destructive\nmethod Watch()"));
}

#[test]
fn untrusted_added_is_allowed() {
    expect_clean(&mutated("id: string,", "# @untrusted\n  id: string,"));
}

#[test]
fn deprecated_added_is_allowed() {
    expect_clean(&mutated("method Watch()", "# @deprecated\nmethod Watch()"));
}

#[test]
fn comments_may_change() {
    expect_clean(&mutated("# The name.", "# The new, better name."));
}
