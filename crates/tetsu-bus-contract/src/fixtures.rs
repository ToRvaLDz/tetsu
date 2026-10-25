//! Event fixtures: one JSON example per event `kind`, checked against the IDL.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::{
    Contract, CustomType, Field, Interface, Kind, LoadError, Ty, TypeDef, Variant, Violation,
};

/// One fixture file: its path relative to the fixtures root
/// (`<interface>/<Type>.<kind>.json`, `/`-separated) and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    pub path: String,
    pub content: String,
}

/// An event type: the only output of a streaming method, an object type with
/// a `kind` field of inline enum type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventType {
    pub interface: String,
    pub name: String,
    pub kinds: Vec<String>,
}

impl EventType {
    /// Where the fixture of `kind` lives, relative to the fixtures root.
    pub fn fixture_path(&self, kind: &str) -> String {
        format!("{}/{}.{kind}.json", self.interface, self.name)
    }
}

/// The event types of a contract, sorted by interface and name.
pub fn event_types(contract: &Contract) -> Vec<EventType> {
    let mut found: BTreeMap<(String, String), EventType> = BTreeMap::new();
    for (iname, iface) in &contract.interfaces {
        for (mname, method) in &iface.methods {
            if !contract.is_streaming(iname, mname) || method.outputs.len() != 1 {
                continue;
            }
            let Ty::Custom(tname) = method.outputs[0].ty.unwrap_optional() else {
                continue;
            };
            let Some(CustomType {
                def: TypeDef::Object(fields),
                ..
            }) = iface.types.get(tname)
            else {
                continue;
            };
            let kinds = fields
                .iter()
                .find(|f| f.name == "kind")
                .and_then(|f| match &f.ty {
                    Ty::Enum(variants) => Some(variants.iter().map(|v| v.name.clone()).collect()),
                    _ => None,
                });
            if let Some(kinds) = kinds {
                found.insert(
                    (iname.clone(), tname.clone()),
                    EventType {
                        interface: iname.clone(),
                        name: tname.clone(),
                        kinds,
                    },
                );
            }
        }
    }
    found.into_values().collect()
}

/// Checks the fixtures against the event types of the contract: one fixture
/// per kind, none for anything else, each matching the declared types.
pub fn check_fixtures(contract: &Contract, fixtures: &[Fixture]) -> Vec<Violation> {
    let mut out = Vec::new();
    let types = event_types(contract);
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for t in &types {
        for kind in &t.kinds {
            let path = t.fixture_path(kind);
            if !fixtures.iter().any(|f| f.path == path) {
                out.push(Violation::new(
                    Kind::MissingFixture,
                    &path,
                    format!("no fixture for kind `{kind}` of {}", t.name),
                ));
            }
            expected.insert(path);
        }
    }
    for f in fixtures {
        if !expected.contains(&f.path) {
            out.push(Violation::new(
                Kind::OrphanFixture,
                &f.path,
                "no event type and kind of the contract matches this fixture",
            ));
            continue;
        }
        let Some((t, kind)) = parse_name(&f.path) else {
            continue;
        };
        let Some(ty) = types
            .iter()
            .find(|e| e.name == t && e.fixture_path(&kind) == f.path)
        else {
            continue;
        };
        let iface = &contract.interfaces[&ty.interface];
        check_fixture(iface, &t, &kind, f, &mut out);
    }
    out
}

/// `(Type, kind)` of `<interface>/<Type>.<kind>.json`.
fn parse_name(path: &str) -> Option<(String, String)> {
    let (_, file) = path.split_once('/')?;
    let stem = file.strip_suffix(".json")?;
    let (t, kind) = stem.split_once('.')?;
    Some((t.to_string(), kind.to_string()))
}

struct Checker<'a> {
    iface: &'a Interface,
    fixture: &'a str,
    out: &'a mut Vec<Violation>,
}

impl Checker<'_> {
    fn push(&mut self, kind: Kind, detail: String) {
        self.out.push(Violation::new(kind, self.fixture, detail));
    }

    fn wrong(&mut self, path: &str, expected: &str, got: &Value) {
        let got = match got {
            Value::Null => "null",
            Value::Bool(_) => "a bool",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        };
        self.push(
            Kind::FixtureWrongType,
            format!("{path}: expected {expected}, got {got}"),
        );
    }

    fn object(&mut self, fields: &[Field], v: &Value, path: &str) {
        let Value::Object(map) = v else {
            return self.wrong(path, "an object", v);
        };
        for key in map.keys() {
            if !fields.iter().any(|f| &f.name == key) {
                self.push(
                    Kind::FixtureUnknownField,
                    format!("{}: field not in the IDL", join(path, key)),
                );
            }
        }
        for f in fields {
            let fpath = join(path, &f.name);
            match map.get(&f.name) {
                Some(value) => self.value(&f.ty, value, &fpath),
                None if f.ty.is_optional() => {}
                None => self.push(
                    Kind::FixtureMissingField,
                    format!("{fpath}: required field is missing"),
                ),
            }
        }
    }

    fn value(&mut self, ty: &Ty, v: &Value, path: &str) {
        match ty {
            Ty::Bool if !v.is_boolean() => self.wrong(path, "a bool", v),
            Ty::Int if !v.is_i64() => self.wrong(path, "an int", v),
            Ty::Float if !v.is_number() => self.wrong(path, "a float", v),
            Ty::String if !v.is_string() => self.wrong(path, "a string", v),
            // The IDL's `object` carries any JSON value but null: the contract
            // uses it for config values, which are scalars as well.
            Ty::ForeignObject if v.is_null() => self.wrong(path, "a JSON value", v),
            Ty::Optional(inner) => {
                if !v.is_null() {
                    self.value(inner, v, path);
                }
            }
            Ty::Array(inner) => match v {
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        self.value(inner, item, &format!("{path}[{i}]"));
                    }
                }
                _ => self.wrong(path, "an array", v),
            },
            Ty::Map(inner) => match v {
                Value::Object(map) => {
                    for (k, item) in map {
                        self.value(inner, item, &join(path, k));
                    }
                }
                _ => self.wrong(path, "an object", v),
            },
            Ty::Enum(variants) => self.enum_value(variants, v, path),
            Ty::Object(fields) => self.object(fields, v, path),
            Ty::Custom(name) => match self.iface.types.get(name).map(|t| &t.def) {
                Some(TypeDef::Object(fields)) => self.object(fields, v, path),
                Some(TypeDef::Enum(variants)) => self.enum_value(variants, v, path),
                None => self.push(
                    Kind::FixtureWrongType,
                    format!("{path}: type {name} is not declared"),
                ),
            },
            _ => {}
        }
    }

    fn enum_value(&mut self, variants: &[Variant], v: &Value, path: &str) {
        match v.as_str() {
            Some(s) if variants.iter().any(|x| x.name == s) => {}
            Some(s) => self.push(
                Kind::FixtureWrongType,
                format!("{path}: `{s}` is not a declared enum value"),
            ),
            None => self.wrong(path, "an enum value (string)", v),
        }
    }
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

fn check_fixture(
    iface: &Interface,
    type_name: &str,
    kind: &str,
    f: &Fixture,
    out: &mut Vec<Violation>,
) {
    let value: Value = match serde_json::from_str(&f.content) {
        Ok(v) => v,
        Err(e) => {
            out.push(Violation::new(
                Kind::InvalidFixture,
                &f.path,
                format!("not JSON: {e}"),
            ));
            return;
        }
    };
    if !value.is_object() {
        out.push(Violation::new(
            Kind::InvalidFixture,
            &f.path,
            "a fixture must be a JSON object",
        ));
        return;
    }
    if let Some(Value::String(declared)) = value.get("kind")
        && declared != kind
    {
        out.push(Violation::new(
            Kind::FixtureKindMismatch,
            &f.path,
            format!("file name says kind `{kind}`, the fixture says `{declared}`"),
        ));
    }
    let Some(TypeDef::Object(fields)) = iface.types.get(type_name).map(|t| &t.def) else {
        return;
    };
    Checker {
        iface,
        fixture: &f.path,
        out,
    }
    .object(fields, &value, "");
}

/// Reads every file below `root` (two levels: `<interface>/<file>`) as a
/// fixture, sorted by path.
pub fn read_fixtures(root: &Path) -> Result<Vec<Fixture>, LoadError> {
    let io = |e: std::io::Error, p: &Path| LoadError(format!("{}: {e}", p.display()));
    let mut out = Vec::new();
    for dir in fs::read_dir(root).map_err(|e| io(e, root))? {
        let dir = dir.map_err(|e| io(e, root))?.path();
        if !dir.is_dir() {
            continue;
        }
        for file in fs::read_dir(&dir).map_err(|e| io(e, &dir))? {
            let file = file.map_err(|e| io(e, &dir))?.path();
            if !file.is_file() {
                continue;
            }
            let content = fs::read_to_string(&file).map_err(|e| io(e, &file))?;
            let rel = file
                .strip_prefix(root)
                .map_err(|e| LoadError(e.to_string()))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Fixture { path: rel, content });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
