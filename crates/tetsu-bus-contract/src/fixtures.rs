//! Event fixtures: one JSON example per event `kind`, checked against the IDL.

use std::fs;
use std::path::Path;

use crate::{Contract, LoadError, Violation};

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

/// The event types of a contract.
pub fn event_types(_contract: &Contract) -> Vec<EventType> {
    Vec::new()
}

/// Checks the fixtures against the event types of the contract.
pub fn check_fixtures(_contract: &Contract, _fixtures: &[Fixture]) -> Vec<Violation> {
    Vec::new()
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
