//! Event fixtures: one JSON example per event `kind`, checked against the IDL.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::value::{Checker, FIXTURE_KINDS};
use crate::{Contract, CustomType, Interface, Kind, LoadError, Ty, TypeDef, Violation};

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
        origin: &f.path,
        kinds: FIXTURE_KINDS,
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
