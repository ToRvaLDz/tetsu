//! Event fixtures: one test per way a fixture can disagree with the IDL.

use tetsu_bus_contract::{Contract, Fixture, Kind, Violation, check_fixtures, event_types};

const IDL: &str = r#"
interface org.test.v1

type Item (id: string)

type Event (
  kind: (initial, changed),
  seq: int,
  items: ?[]Item,
  item: ?Item,
  count: ?int,
  ratio: ?float,
  on: ?bool,
  text: ?string,
  meta: ?object,
  blobs: ?[]object,
  labels: ?[string]string,
  mode: ?(fast, slow),
  size: ?(w: int, h: ?int),
  level: ?Level,
  anything: ?any,
  anys: ?[]any
)

type Level (low, high)

# Streams events.
method WatchEvents() -> (event: Event)
"#;

const STREAMING: &str = "org.test.v1.WatchEvents\n";

const INITIAL: &str = r#"{"kind":"initial","seq":1,"items":[{"id":"a"}]}"#;
const CHANGED: &str = r#"{"kind":"changed","seq":2,"item":{"id":"a"}}"#;
const INITIAL_PATH: &str = "org.test.v1/Event.initial.json";
const CHANGED_PATH: &str = "org.test.v1/Event.changed.json";

fn contract() -> Contract {
    Contract::from_sources(&[("test.varlink", IDL)], STREAMING).expect("test idl parses")
}

fn fixture(path: &str, content: &str) -> Fixture {
    Fixture {
        path: path.to_string(),
        content: content.to_string(),
    }
}

fn good() -> Vec<Fixture> {
    vec![
        fixture(INITIAL_PATH, INITIAL),
        fixture(CHANGED_PATH, CHANGED),
    ]
}

/// The good set with the `changed` fixture replaced by `content`.
fn with_changed(content: &str) -> Vec<Violation> {
    check_fixtures(
        &contract(),
        &[
            fixture(INITIAL_PATH, INITIAL),
            fixture(CHANGED_PATH, content),
        ],
    )
}

/// The `changed` fixture with one extra field given as `"name":value`.
fn with_field(member: &str) -> Vec<Violation> {
    with_changed(&format!(r#"{{"kind":"changed","seq":2,{member}}}"#))
}

fn expect(violations: &[Violation], kind: Kind, path: &str, detail_part: &str) {
    assert!(
        violations
            .iter()
            .any(|v| v.kind == kind && v.path == path && v.detail.contains(detail_part)),
        "expected {kind:?} at {path} mentioning {detail_part:?}, got {violations:#?}"
    );
}

fn expect_clean(violations: &[Violation]) {
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn event_types_are_found() {
    let types = event_types(&contract());
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].interface, "org.test.v1");
    assert_eq!(types[0].name, "Event");
    assert_eq!(types[0].kinds, ["initial", "changed"]);
}

#[test]
fn good_fixtures_pass() {
    expect_clean(&check_fixtures(&contract(), &good()));
}

#[test]
fn missing_fixture_for_kind() {
    let v = check_fixtures(&contract(), &[fixture(INITIAL_PATH, INITIAL)]);
    expect(&v, Kind::MissingFixture, CHANGED_PATH, "changed");
}

#[test]
fn fixture_for_unknown_kind() {
    let mut f = good();
    f.push(fixture(
        "org.test.v1/Event.vanished.json",
        r#"{"kind":"vanished","seq":3}"#,
    ));
    let v = check_fixtures(&contract(), &f);
    expect(
        &v,
        Kind::OrphanFixture,
        "org.test.v1/Event.vanished.json",
        "",
    );
}

#[test]
fn fixture_for_unknown_type() {
    let mut f = good();
    f.push(fixture("org.test.v1/Ghost.initial.json", "{}"));
    let v = check_fixtures(&contract(), &f);
    expect(
        &v,
        Kind::OrphanFixture,
        "org.test.v1/Ghost.initial.json",
        "",
    );
}

#[test]
fn fixture_for_unknown_interface() {
    let mut f = good();
    f.push(fixture("org.other.v1/Event.initial.json", "{}"));
    let v = check_fixtures(&contract(), &f);
    expect(
        &v,
        Kind::OrphanFixture,
        "org.other.v1/Event.initial.json",
        "",
    );
}

#[test]
fn fixture_with_unexpected_file_name() {
    let mut f = good();
    f.push(fixture("org.test.v1/notes.txt", "hello"));
    let v = check_fixtures(&contract(), &f);
    expect(&v, Kind::OrphanFixture, "org.test.v1/notes.txt", "");
}

#[test]
fn fixture_for_non_event_type() {
    let mut f = good();
    f.push(fixture("org.test.v1/Item.initial.json", "{}"));
    let v = check_fixtures(&contract(), &f);
    expect(&v, Kind::OrphanFixture, "org.test.v1/Item.initial.json", "");
}

#[test]
fn fixture_kind_mismatch() {
    let v = with_changed(r#"{"kind":"initial","seq":2}"#);
    expect(&v, Kind::FixtureKindMismatch, CHANGED_PATH, "initial");
}

#[test]
fn fixture_without_kind() {
    let v = with_changed(r#"{"seq":2}"#);
    expect(&v, Kind::FixtureMissingField, CHANGED_PATH, "kind");
}

#[test]
fn fixture_unknown_field() {
    let v = with_field(r#""surprise":1"#);
    expect(&v, Kind::FixtureUnknownField, CHANGED_PATH, "surprise");
}

#[test]
fn fixture_unknown_nested_field() {
    let v = with_field(r#""item":{"id":"a","extra":1}"#);
    expect(&v, Kind::FixtureUnknownField, CHANGED_PATH, "item.extra");
}

#[test]
fn fixture_missing_required_field() {
    let v = with_changed(r#"{"kind":"changed"}"#);
    expect(&v, Kind::FixtureMissingField, CHANGED_PATH, "seq");
}

#[test]
fn fixture_missing_required_nested_field() {
    let v = with_field(r#""item":{}"#);
    expect(&v, Kind::FixtureMissingField, CHANGED_PATH, "item.id");
}

#[test]
fn fixture_null_for_required_field() {
    let v = with_changed(r#"{"kind":"changed","seq":null}"#);
    expect(&v, Kind::FixtureWrongType, CHANGED_PATH, "seq");
}

#[test]
fn fixture_null_for_optional_field() {
    expect_clean(&with_field(r#""count":null"#));
}

#[test]
fn fixture_not_json() {
    let v = with_changed("{not json");
    expect(&v, Kind::InvalidFixture, CHANGED_PATH, "");
}

#[test]
fn fixture_not_an_object() {
    let v = with_changed("[1,2]");
    expect(&v, Kind::InvalidFixture, CHANGED_PATH, "object");
}

#[test]
fn int_field_wrong_type() {
    expect(
        &with_field(r#""count":"3""#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "count",
    );
}

#[test]
fn int_field_rejects_fraction() {
    expect(
        &with_field(r#""count":3.5"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "count",
    );
}

#[test]
fn int_field_accepts_integer() {
    expect_clean(&with_field(r#""count":3"#));
}

#[test]
fn float_field_wrong_type() {
    expect(
        &with_field(r#""ratio":"0.5""#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "ratio",
    );
}

#[test]
fn float_field_accepts_number() {
    expect_clean(&with_field(r#""ratio":0.5"#));
    expect_clean(&with_field(r#""ratio":1"#));
}

#[test]
fn bool_field_wrong_type() {
    expect(
        &with_field(r#""on":1"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "on",
    );
}

#[test]
fn string_field_wrong_type() {
    expect(
        &with_field(r#""text":5"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "text",
    );
}

#[test]
fn object_field_rejects_null_element() {
    expect(
        &with_field(r#""blobs":[{"a":1},null]"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "blobs[1]",
    );
}

#[test]
fn object_field_accepts_any_json_object() {
    expect_clean(&with_field(r#""meta":{"anything":[1,{"x":null}]}"#));
    expect_clean(&with_field(r#""blobs":[{},{"a":1}]"#));
}

#[test]
fn object_field_rejects_scalar() {
    expect(
        &with_field(r#""meta":300000"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "meta",
    );
}

#[test]
fn object_field_rejects_non_object_element() {
    expect(
        &with_field(r#""blobs":[{"a":1},[1]]"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "blobs[1]",
    );
}

#[test]
fn any_field_accepts_anything() {
    expect_clean(&with_field(r#""anything":[1,"x"]"#));
    expect_clean(&with_field(r#""anything":300000"#));
    expect_clean(&with_field(r#""anything":"text""#));
    expect_clean(&with_field(r#""anys":[true,[1],{"a":1},"x"]"#));
}

#[test]
fn any_field_rejects_null_element() {
    expect(
        &with_field(r#""anys":[1,null]"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "anys[1]",
    );
}

#[test]
fn array_field_wrong_type() {
    expect(
        &with_field(r#""items":{"id":"a"}"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "items",
    );
}

#[test]
fn array_element_wrong_type() {
    expect(
        &with_field(r#""items":[{"id":"a"},"b"]"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "items[1]",
    );
}

#[test]
fn map_field_wrong_type() {
    expect(
        &with_field(r#""labels":["a"]"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "labels",
    );
}

#[test]
fn map_value_wrong_type() {
    expect(
        &with_field(r#""labels":{"a":"x","b":2}"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "labels.b",
    );
}

#[test]
fn inline_enum_unknown_value() {
    expect(
        &with_field(r#""mode":"turbo""#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "mode",
    );
}

#[test]
fn inline_enum_known_value() {
    expect_clean(&with_field(r#""mode":"fast""#));
}

#[test]
fn custom_enum_unknown_value() {
    expect(
        &with_field(r#""level":"extreme""#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "level",
    );
}

#[test]
fn custom_enum_known_value() {
    expect_clean(&with_field(r#""level":"high""#));
}

#[test]
fn custom_object_wrong_type() {
    expect(
        &with_field(r#""item":"a""#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "item",
    );
}

#[test]
fn custom_object_nested_value_wrong_type() {
    expect(
        &with_field(r#""item":{"id":7}"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "item.id",
    );
}

#[test]
fn inline_object_wrong_value() {
    expect(
        &with_field(r#""size":{"w":"wide"}"#),
        Kind::FixtureWrongType,
        CHANGED_PATH,
        "size.w",
    );
}

#[test]
fn inline_object_optional_member_may_be_null_or_absent() {
    expect_clean(&with_field(r#""size":{"w":1,"h":null}"#));
    expect_clean(&with_field(r#""size":{"w":1}"#));
}

#[test]
fn only_streaming_single_output_objects_with_a_kind_are_events() {
    let idl = r#"
interface org.test.v1
type WithKind (kind: (a, b))
type NoKind (id: string)
type PlainKind (kind: string)
method Plain() -> (event: WithKind)
method WatchTwo() -> (event: WithKind, extra: ?int)
method WatchNoKind() -> (event: NoKind)
method WatchPlainKind() -> (event: PlainKind)
"#;
    let streaming = "org.test.v1.WatchTwo\norg.test.v1.WatchNoKind\norg.test.v1.WatchPlainKind\n";
    let c = Contract::from_sources(&[("t.varlink", idl)], streaming).unwrap();
    assert!(event_types(&c).is_empty());
    expect_clean(&check_fixtures(&c, &[]));
}

#[test]
fn a_streaming_method_with_one_output_makes_its_type_an_event() {
    let idl = r#"
interface org.test.v1
type Reply (kind: (shown, invoked))
method Show() -> (reply: Reply)
"#;
    let c = Contract::from_sources(&[("t.varlink", idl)], "org.test.v1.Show\n").unwrap();
    let types = event_types(&c);
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].name, "Reply");
    let v = check_fixtures(&c, &[]);
    expect(
        &v,
        Kind::MissingFixture,
        "org.test.v1/Reply.shown.json",
        "shown",
    );
    expect(
        &v,
        Kind::MissingFixture,
        "org.test.v1/Reply.invoked.json",
        "invoked",
    );
}
