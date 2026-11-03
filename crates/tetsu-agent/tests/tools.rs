//! The catalog: every contract method is exactly one tool.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_agent::contract;
use tetsu_agent::tools::{self, REASON, StreamMode, Tool};
use tetsu_bus::access::{MethodKind, method_kind};
use tetsu_bus_contract::{Contract, read_dir_sources};

const EXEMPT: [&str; 3] = [
    "org.tetsu.cap.v1.Intent",
    "org.tetsu.cap.v1.Set",
    "org.tetsu.cap.v1.Check",
];

/// The secrets write methods: managed by the user only, never agent tools.
const NOT_TOOLS: [&str; 3] = [
    "org.tetsu.secrets.v1.Add",
    "org.tetsu.secrets.v1.Rotate",
    "org.tetsu.secrets.v1.Remove",
];

fn idl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl")
}

fn shipped() -> (Contract, Vec<Tool>) {
    let contract = contract::load().expect("the embedded contract parses");
    let tools = tools::build(&contract).expect("the catalog builds");
    (contract, tools)
}

fn synthetic(idl: &str) -> Contract {
    Contract::from_sources(&[("test.varlink", idl)], "").expect("the synthetic IDL parses")
}

fn tool<'a>(tools: &'a [Tool], id: &str) -> &'a Tool {
    tools
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no tool {id}"))
}

// 1. The embedded contract is the shipped one.

#[test]
fn the_embedded_sources_equal_the_idl_directory() {
    let on_disk = read_dir_sources(&idl_dir()).unwrap();
    let on_disk: Vec<(&str, &str)> = on_disk
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    assert_eq!(contract::SOURCES.to_vec(), on_disk);
    let list = fs::read_to_string(idl_dir().join("streaming.list")).unwrap();
    assert_eq!(contract::STREAMING_LIST, list);
}

// 2. One tool per method.

#[test]
fn every_contract_method_is_exactly_one_tool() {
    let (contract, tools) = shipped();
    let mut expected = BTreeSet::new();
    for (iface, def) in &contract.interfaces {
        for method in def.methods.keys() {
            expected.insert(format!("{iface}.{method}"));
        }
    }
    for id in NOT_TOOLS {
        assert!(expected.remove(id), "{id} is a contract method");
    }
    let ids: Vec<&str> = tools.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids.len(), expected.len(), "one tool per method");
    let unique: BTreeSet<String> = ids.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(unique, expected);
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "sorted by id");
    for t in &tools {
        assert_eq!(t.id, format!("{}.{}", t.interface, t.method));
        assert!(!t.interface.starts_with("org.varlink.service"));
    }
}

#[test]
fn the_secrets_writes_are_not_tools_and_the_reads_are() {
    let (_, tools) = shipped();
    for id in NOT_TOOLS {
        assert!(tools.iter().all(|t| t.id != id), "{id} must not be a tool");
    }
    for id in [
        "org.tetsu.secrets.v1.ListSecrets",
        "org.tetsu.secrets.v1.WatchSecrets",
    ] {
        assert_eq!(tool(&tools, id).kind, MethodKind::Free, "{id}");
    }
    // The rule is the capability class, so a future write method of the
    // class is left out too.
    let contract = synthetic(
        "interface org.example.v1\n\
         # @capability secrets.manage\n\
         method Burn() -> ()\n\
         method GetThing() -> ()\n",
    );
    let ids: Vec<String> = tools::build(&contract)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, ["org.example.v1.GetThing"]);
}

// 3. Wire names.

#[test]
fn every_wire_name_round_trips_and_is_provider_safe() {
    let (_, tools) = shipped();
    for t in &tools {
        assert!(t.wire_name.len() <= 64, "{}", t.wire_name);
        assert!(
            t.wire_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{}",
            t.wire_name
        );
        assert_eq!(
            tools::from_wire(&t.wire_name).as_deref(),
            Some(t.id.as_str())
        );
        assert_eq!(tools::wire_name(&t.id).unwrap(), t.wire_name);
    }
    assert_eq!(
        tool(&tools, "org.tetsu.tx.v1.Install").wire_name,
        "org_tetsu_tx_v1_Install"
    );
}

#[test]
fn an_id_that_cannot_be_a_wire_name_is_refused() {
    let long = format!("org.example.{}.v1", "a".repeat(60));
    let contract = synthetic(&format!("interface {long}\nmethod GetThing() -> ()\n"));
    assert!(tools::build(&contract).is_err());
    assert!(tools::wire_name(&format!("{long}.GetThing")).is_err());
    assert!(tools::wire_name("org.example.v1.Get_Thing").is_err());
    assert!(tools::wire_name("org.example.v1.Get Thing").is_err());
    assert_eq!(tools::from_wire("not a name"), None);
    assert_eq!(tools::from_wire(""), None);
}

// 4. Kind, capability and class.

#[test]
fn kind_and_capability_follow_the_gate() {
    let (contract, tools) = shipped();
    for t in &tools {
        assert_eq!(t.kind, method_kind(&t.id), "{}", t.id);
        let method = &contract.interfaces[&t.interface].methods[&t.method];
        let tag = method.doc.capability().map(|c| c.expect("a valid tag"));
        assert_eq!(t.class, tag.as_ref().map(|c| c.class.clone()), "{}", t.id);
        assert_eq!(
            t.scope,
            tag.as_ref().and_then(|c| c.scope.clone()),
            "{}",
            t.id
        );
        let gated = t.kind == MethodKind::Gated;
        let exempt = EXEMPT.contains(&t.id.as_str());
        assert_eq!(t.class.is_some(), gated && !exempt, "{}", t.id);
    }
}

// 5. Destructive comes from the tag.

#[test]
fn destructive_is_read_from_the_tag() {
    let (_, tools) = shipped();
    let destructive: BTreeSet<&str> = tools
        .iter()
        .filter(|t| t.destructive)
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(
        destructive,
        BTreeSet::from(["org.tetsu.tx.v1.Update", "org.tetsu.tx.v1.InstallFromAur"])
    );
    let contract = synthetic(
        "interface org.example.v1\n\
         # Wipes.\n# @destructive\n# @capability thing.wipe\nmethod Wipe() -> ()\n\
         # Mends.\n# @capability thing.mend\nmethod Mend() -> ()\n",
    );
    let tools = tools::build(&contract).unwrap();
    assert!(tool(&tools, "org.example.v1.Wipe").destructive);
    assert!(!tool(&tools, "org.example.v1.Mend").destructive);
}

// 6. Stream modes.

#[test]
fn stream_modes_follow_the_streaming_list() {
    let (contract, tools) = shipped();
    for t in &tools {
        let streaming = contract.is_streaming(&t.interface, &t.method);
        let expected = if !streaming {
            StreamMode::Single
        } else if t.id == "org.tetsu.notify.v1.Show" {
            StreamMode::Hold
        } else if t.method.starts_with("Watch") {
            StreamMode::First
        } else {
            StreamMode::Collect
        };
        assert_eq!(t.stream, expected, "{}", t.id);
    }
    assert_eq!(
        tool(&tools, "org.tetsu.tx.v1.Install").stream,
        StreamMode::Collect
    );
}

#[test]
fn every_hold_entry_exists_and_is_streaming() {
    let (contract, tools) = shipped();
    assert!(!tools::HOLD.is_empty());
    for id in tools::HOLD {
        let t = tool(&tools, id);
        assert!(contract.is_streaming(&t.interface, &t.method), "{id}");
    }
}

// 7. Schemas.

const TYPES: &str = "\
interface org.example.v1

type Point (x: int, y: ?float)

type Color (red, green)

type Loop (next: ?Loop)

type Mutual (other: ?Other)

type Other (back: ?Mutual)

";

#[test]
fn each_varlink_type_has_its_schema() {
    let idl = format!(
        "{TYPES}method GetThing(
  # Whether it is on.
  on: bool,
  count: int,
  ratio: float,
  name: string,
  raw: object,
  anything: any,
  names: []string,
  counts: [string]int,
  mode: (one, two),
  maybe: ?string,
  point: Point,
  color: Color,
  inline: (a: int, b: ?string)
) -> ()
"
    );
    let tools = tools::build(&synthetic(&idl)).unwrap();
    let t = tool(&tools, "org.example.v1.GetThing");
    let expected = json!({
        "type": "object",
        "properties": {
            "on": { "type": "boolean", "description": "Whether it is on." },
            "count": { "type": "integer" },
            "ratio": { "type": "number" },
            "name": { "type": "string" },
            "raw": { "type": "object" },
            "anything": {},
            "names": { "type": "array", "items": { "type": "string" } },
            "counts": { "type": "object", "additionalProperties": { "type": "integer" } },
            "mode": { "type": "string", "enum": ["one", "two"] },
            "maybe": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "point": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "anyOf": [{ "type": "number" }, { "type": "null" }] },
                },
                "required": ["x"],
                "additionalProperties": false,
            },
            "color": { "type": "string", "enum": ["red", "green"] },
            "inline": {
                "type": "object",
                "properties": {
                    "a": { "type": "integer" },
                    "b": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                },
                "required": ["a"],
                "additionalProperties": false,
            },
        },
        "required": [
            "on", "count", "ratio", "name", "raw", "anything", "names", "counts",
            "mode", "point", "color", "inline",
        ],
        "additionalProperties": false,
    });
    assert_eq!(t.schema, expected);
}

#[test]
fn a_method_without_inputs_has_an_empty_object_schema() {
    let tools = tools::build(&synthetic(&format!("{TYPES}method GetNone() -> ()\n"))).unwrap();
    assert_eq!(
        tool(&tools, "org.example.v1.GetNone").schema,
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    );
}

#[test]
fn a_recursive_type_is_refused() {
    for input in ["l: Loop", "m: Mutual"] {
        let idl = format!("{TYPES}method GetIt({input}) -> ()\n");
        let err = tools::build(&synthetic(&idl)).unwrap_err();
        assert!(err.to_string().contains("recursive"), "{err}");
    }
}

#[test]
fn an_unknown_type_is_refused() {
    let idl = format!("{TYPES}method GetIt(m: Nowhere) -> ()\n");
    let err = tools::build(&synthetic(&idl)).unwrap_err();
    assert!(err.to_string().contains("Nowhere"), "{err}");
}

// 8. The reserved reason.

#[test]
fn gated_tools_have_an_optional_reason_and_reads_do_not() {
    let (contract, tools) = shipped();
    for t in &tools {
        let props = t.schema["properties"].as_object().expect("properties");
        let required = t.schema.get("required").and_then(Value::as_array);
        match t.kind {
            MethodKind::Gated => {
                assert_eq!(props[REASON]["type"], "string", "{}", t.id);
                assert!(
                    required.is_none_or(|r| !r.contains(&json!(REASON))),
                    "{}",
                    t.id
                );
            }
            MethodKind::Free => assert!(!props.contains_key(REASON), "{}", t.id),
        }
        let method = &contract.interfaces[&t.interface].methods[&t.method];
        assert!(method.inputs.iter().all(|f| f.name != REASON));
    }
    assert!(REASON.starts_with('_'));
}

// 9. Descriptions.

#[test]
fn the_description_is_the_prose_plus_the_approval_line() {
    let contract = synthetic(
        "interface org.example.v1\n\
         # Does a thing.\n# Over two lines.\n#\n# Second paragraph.\n# @capability thing.do\nmethod Do() -> ()\n\
         # Wipes.\n# @destructive\n# @capability thing.wipe\nmethod Wipe() -> ()\n\
         # Reads it.\nmethod GetIt() -> ()\n",
    );
    let tools = tools::build(&contract).unwrap();
    assert_eq!(
        tool(&tools, "org.example.v1.Do").description,
        "Does a thing. Over two lines.\n\nSecond paragraph.\n\nMay ask the user for approval."
    );
    assert_eq!(
        tool(&tools, "org.example.v1.Wipe").description,
        "Wipes.\n\nAlways asks the user for approval (destructive)."
    );
    assert_eq!(
        tool(&tools, "org.example.v1.GetIt").description,
        "Reads it."
    );
    for t in &shipped().1 {
        assert!(!t.description.contains("@capability"), "{}", t.id);
        assert!(!t.description.contains("@destructive"), "{}", t.id);
    }
}

// 10. Untrusted outputs.

#[test]
fn untrusted_outputs_are_recorded_by_path() {
    let (_, tools) = shipped();
    let windows = tool(&tools, "org.tetsu.comp.window.v1.ListWindows");
    assert!(
        windows
            .untrusted_outputs
            .contains(&"windows[].title".to_owned())
    );
    assert!(
        windows
            .untrusted_outputs
            .contains(&"windows[].app_id".to_owned())
    );
    let review = tool(&tools, "org.tetsu.tx.v1.ReviewAurPackages");
    assert!(
        review
            .untrusted_outputs
            .contains(&"reviews[].pkgbuild".to_owned())
    );
    let status = tool(&tools, "org.tetsu.tx.v1.GetStatus");
    assert!(status.untrusted_outputs.is_empty());
}
