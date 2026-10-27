//! The grant store: three TOML layers, image under managed under user.
//!
//! ```toml
//! [class]
//! "packages.install" = "notify"
//!
//! [scope."mail.send"]
//! work = "ask"
//! ```
//!
//! Values are `automatic`, `notify`, `ask` or `off`. The nearest layer
//! wins key by key, and a scoped key, when any layer sets it, wins over
//! the class key. The layers are re-read on `refresh`, so a hand edit
//! takes effect at the next call. A layer that does not parse keeps the
//! last good snapshot and is reported; the caller decides what to do.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tetsu_bus::cap::{Grant, Layer, State};

use crate::catalog;

/// Where the three layers live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layers {
    /// The image defaults.
    pub image: PathBuf,
    /// Written by `Set`.
    pub managed: PathBuf,
    /// The user's own file.
    pub user: PathBuf,
}

/// Why the store could not read or write.
#[derive(Debug)]
pub enum StoreError {
    /// A layer does not parse.
    Malformed {
        /// The file.
        path: PathBuf,
        /// The 1-based line of the problem.
        line: usize,
        /// What is wrong.
        message: String,
    },
    /// A file or directory could not be read or written.
    Io {
        /// The path.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// The user file sets this key, so a managed value would not apply.
    OverriddenByUser,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed {
                path,
                line,
                message,
            } => write!(f, "{}:{line}: {message}", path.display()),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::OverriddenByUser => write!(f, "the user file sets this key"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The state of a key and the layer that sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    /// `Unset` when no layer sets it.
    pub state: State,
    /// The layer that sets the state.
    pub layer: Option<Layer>,
}

/// The shape of one layer file.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    class: BTreeMap<String, Grant>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    scope: BTreeMap<String, BTreeMap<String, Grant>>,
}

/// A key: a class and, optionally, a scope.
type Key = (String, Option<String>);

/// The keys one layer sets.
type Grants = BTreeMap<Key, Grant>;

const ORDER: [Layer; 3] = [Layer::Image, Layer::Managed, Layer::User];

fn slot(layer: Layer) -> usize {
    ORDER.iter().position(|l| *l == layer).unwrap_or(0)
}

/// The store: the last good parse of each layer.
#[derive(Debug)]
pub struct Store {
    layers: Layers,
    grants: [Grants; 3],
    warned: HashSet<String>,
}

impl Store {
    /// A store over these files. Nothing is read until `refresh`.
    pub fn new(layers: Layers) -> Self {
        Self {
            layers,
            grants: Default::default(),
            warned: HashSet::new(),
        }
    }

    /// The layer files.
    pub fn layers(&self) -> &Layers {
        &self.layers
    }

    fn path(&self, layer: Layer) -> &Path {
        match layer {
            Layer::Image => &self.layers.image,
            Layer::Managed => &self.layers.managed,
            _ => &self.layers.user,
        }
    }

    /// Re-reads the three layers. A missing file is an empty layer. When
    /// any layer fails, no snapshot changes and the first failure is
    /// returned.
    pub fn refresh(&mut self) -> Result<(), StoreError> {
        let mut next: [Grants; 3] = Default::default();
        for layer in ORDER {
            let path = self.path(layer).to_owned();
            next[slot(layer)] = self.read_layer(&path)?;
        }
        self.grants = next;
        Ok(())
    }

    fn read_layer(&mut self, path: &Path) -> Result<Grants, StoreError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Grants::new()),
            Err(source) => {
                return Err(StoreError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let file: File = toml::from_str(&text).map_err(|e| StoreError::Malformed {
            path: path.to_owned(),
            line: e.span().map_or(1, |s| line_of(&text, s.start)),
            message: e.message().to_owned(),
        })?;
        let mut grants = Grants::new();
        for (class, grant) in file.class {
            if self.known(&class, path) {
                grants.insert((class, None), grant);
            }
        }
        for (class, scopes) in file.scope {
            if self.known(&class, path) {
                for (scope, grant) in scopes {
                    grants.insert((class.clone(), Some(scope)), grant);
                }
            }
        }
        Ok(grants)
    }

    /// Whether the catalog has the class; names an unknown one once.
    fn known(&mut self, class: &str, path: &Path) -> bool {
        if catalog::is_class(class) {
            return true;
        }
        if self.warned.insert(class.to_owned()) {
            eprintln!(
                "tetsu-capd: ignoring unknown class \"{class}\" in {}",
                path.display()
            );
        }
        false
    }

    /// The state of a key from the last good snapshot. A scoped key wins
    /// over the class key when any layer sets it.
    pub fn effective(&self, class: &str, scope: Option<&str>) -> Effective {
        let scoped = scope.map(|s| (class.to_owned(), Some(s.to_owned())));
        let whole = (class.to_owned(), None);
        for key in scoped.iter().chain(std::iter::once(&whole)) {
            for layer in ORDER.iter().rev() {
                if let Some(grant) = self.grants[slot(*layer)].get(key) {
                    return Effective {
                        state: (*grant).into(),
                        layer: Some(*layer),
                    };
                }
            }
        }
        Effective {
            state: State::Unset,
            layer: None,
        }
    }

    /// Every `(class, scope)` that some layer sets with a scope.
    pub fn scoped_keys(&self) -> Vec<(String, String)> {
        let mut keys: Vec<(String, String)> = self
            .grants
            .iter()
            .flat_map(|g| g.keys())
            .filter_map(|(class, scope)| Some((class.clone(), scope.clone()?)))
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Sets or, with `None`, removes a managed value. Refreshes first and
    /// refuses while any layer is malformed, or when the user file sets the
    /// key. The file is written atomically, 0600 in a 0700 directory.
    pub fn set_managed(
        &mut self,
        class: &str,
        scope: Option<&str>,
        state: Option<Grant>,
    ) -> Result<(), StoreError> {
        self.refresh()?;
        let key: Key = (class.to_owned(), scope.map(str::to_owned));
        if self.grants[slot(Layer::User)].contains_key(&key) {
            return Err(StoreError::OverriddenByUser);
        }
        let mut managed = self.grants[slot(Layer::Managed)].clone();
        match state {
            Some(grant) => managed.insert(key, grant),
            None => managed.remove(&key),
        };
        let path = self.layers.managed.clone();
        write_atomic(&path, &render(&managed)).map_err(|source| StoreError::Io { path, source })?;
        self.grants[slot(Layer::Managed)] = managed;
        Ok(())
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

fn render(grants: &Grants) -> String {
    let mut file = File::default();
    for ((class, scope), grant) in grants {
        match scope {
            None => {
                file.class.insert(class.clone(), *grant);
            }
            Some(scope) => {
                file.scope
                    .entry(class.clone())
                    .or_default()
                    .insert(scope.clone(), *grant);
            }
        }
    }
    toml::to_string(&file).unwrap_or_default()
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
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}
