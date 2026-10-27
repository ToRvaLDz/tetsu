//! The three layer files, read and watched, and the managed file written.
//!
//! [`Store::poll`] compares a fingerprint (inode, size, modification time)
//! of each file, so a creation, an edit, a deletion and an editor that
//! saves by rename are all seen, with no extra dependency.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use tetsu_bus::config::{ConfigEvent, Diagnostic};
use toml::Value;

use super::keys;
use super::layer::{self, LayerParse};
use super::merge::{self, Values, View};
use super::paths::Paths;

/// What identifies a version of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    inode: u64,
    size: u64,
    mtime: (i64, i64),
}

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let meta = fs::metadata(path).ok()?;
    Some(Fingerprint {
        inode: meta.ino(),
        size: meta.size(),
        mtime: (meta.mtime(), meta.mtime_nsec()),
    })
}

/// One layer's file and what was last read from it.
#[derive(Debug)]
struct Source {
    path: Option<PathBuf>,
    seen: Option<Fingerprint>,
    values: Values,
    diagnostics: Vec<Diagnostic>,
}

impl Source {
    fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            seen: None,
            values: Values::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Reads the file again if it is not the version last read. Returns
    /// whether it was read.
    fn refresh(&mut self) -> bool {
        let Some(path) = self.path.clone() else {
            return false;
        };
        let now = fingerprint(&path);
        if now == self.seen {
            return false;
        }
        self.seen = now;
        match fs::read_to_string(&path) {
            Ok(text) => self.adopt(layer::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.values.clear();
                self.diagnostics.clear();
            }
            Err(e) => {
                // The last good values stay.
                self.diagnostics = vec![Diagnostic {
                    line: None,
                    key: None,
                    message: format!("the file is not readable: {e}"),
                }];
            }
        }
        true
    }

    fn adopt(&mut self, parsed: LayerParse) {
        if !parsed.broken {
            self.values = parsed.values;
        }
        self.diagnostics = parsed.diagnostics;
    }

    /// Logs what is wrong with a layer that is not the user file.
    fn log(&self, what: &str) {
        let Some(path) = &self.path else { return };
        for d in &self.diagnostics {
            let line = d.line.map(|n| format!(":{n}")).unwrap_or_default();
            eprintln!("tetsu-comp: {what} {}{line}: {}", path.display(), d.message);
        }
        for key in self.values.keys() {
            if what == "managed file" && keys::lookup(key).is_some_and(|k| k.runs_commands) {
                eprintln!(
                    "tetsu-comp: {what} {}: {key} runs commands; ignored",
                    path.display()
                );
            }
        }
    }
}

/// The managed state file could not be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateWriteFailed(pub String);

impl fmt::Display for StateWriteFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StateWriteFailed {}

/// The three layers and their merge.
#[derive(Debug)]
pub struct Store {
    defaults: Source,
    managed: Source,
    user: Source,
    view: View,
}

impl Store {
    /// Reads the three files. A missing file is an empty layer.
    pub fn new(paths: Paths) -> Self {
        let mut store = Self {
            defaults: Source::new(Some(paths.defaults)),
            managed: Source::new(paths.managed),
            user: Source::new(paths.user),
            view: merge::merge(&Values::new(), &Values::new(), &Values::new()),
        };
        store.defaults.refresh();
        store.managed.refresh();
        store.user.refresh();
        store.defaults.log("defaults file");
        store.managed.log("managed file");
        store.view = store.merged();
        store
    }

    fn merged(&self) -> View {
        merge::merge(
            &self.defaults.values,
            &self.managed.values,
            &self.user.values,
        )
    }

    /// The merged config.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// The problems of the user file as last loaded.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.user.diagnostics
    }

    /// What the managed layer sets.
    pub fn managed_values(&self) -> &Values {
        &self.managed.values
    }

    /// Reads again the files that changed. The events: a `changed` for each
    /// key that differs, then `diagnostics` if the user file was read.
    pub fn poll(&mut self) -> Vec<ConfigEvent> {
        let defaults = self.defaults.refresh();
        let managed = self.managed.refresh();
        let user = self.user.refresh();
        if defaults {
            self.defaults.log("defaults file");
        }
        if managed {
            self.managed.log("managed file");
        }
        if !(defaults || managed || user) {
            return Vec::new();
        }
        let new = self.merged();
        let mut events = merge::diff(&self.view, &new);
        self.view = new;
        if user {
            events.push(ConfigEvent::diagnostics(self.user.diagnostics.clone()));
        }
        events
    }

    /// Writes the whole managed layer, atomically, then follows it. The
    /// file is left alone when it exists and is not TOML.
    pub fn write_managed(&mut self, values: &Values) -> Result<Vec<ConfigEvent>, StateWriteFailed> {
        let path = self.managed.path.clone().ok_or_else(|| {
            StateWriteFailed("no state directory: neither XDG_STATE_HOME nor HOME is set".into())
        })?;
        match fs::read_to_string(&path) {
            Ok(text) if layer::parse(&text).broken => {
                return Err(StateWriteFailed(format!(
                    "{} is not valid TOML; fix or remove it",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(StateWriteFailed(format!(
                    "cannot read {}: {e}",
                    path.display()
                )));
            }
        }
        let text = render(values).map_err(|e| StateWriteFailed(format!("cannot encode: {e}")))?;
        write_atomic(&path, &text)
            .map_err(|e| StateWriteFailed(format!("cannot write {}: {e}", path.display())))?;
        Ok(self.poll())
    }
}

const HEADER: &str = "# Written by tetsu-comp (SetManaged). Your own settings go in\n\
                      # ~/.config/tetsu/comp.toml, which wins over this file.\n";

/// The TOML text of a layer.
fn render(values: &Values) -> Result<String, toml::ser::Error> {
    let mut root = toml::Table::new();
    for (key, value) in values {
        match key.split_once('.') {
            Some((table, name)) => {
                if let Value::Table(sub) = root
                    .entry(table.to_owned())
                    .or_insert_with(|| Value::Table(toml::Table::new()))
                {
                    sub.insert(name.to_owned(), value.clone());
                }
            }
            None => {
                root.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(format!("{HEADER}{}", toml::to_string(&root)?))
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = fs::remove_file(&tmp);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}
