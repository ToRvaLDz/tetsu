//! Discovery: which interfaces are served, and where.
//!
//! Two kinds of route, in this order of trust:
//!
//! - **pinned** interfaces (capd and notifyd, #28 item 6) are reached only
//!   at a fixed path, after checking who serves it; a socket in a scanned
//!   directory that claims one of them is ignored and never called;
//! - **scanned** interfaces: the sockets in the scan directories answer
//!   `GetInfo`, the first claim of an interface wins.
//!
//! The tools listed are the contract tools of the interfaces found.
//! Nothing a server says becomes tool text: `GetInfo` only decides which
//! shipped interface is present. [`assemble`] is the pure step; [`scan`],
//! [`probe_pinned`] and [`discover`] do the IO.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tetsu_bus::access::{ExpectedServer, capd_path};
use tetsu_bus::dynamic::Client;
use tetsu_bus::endpoint::Endpoint;

use crate::tools::Tool;

/// How long one `GetInfo` may take, connect included.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The registry directory under a scope directory.
const REGISTRY: &str = "registry";
/// The directory of the per-user sockets under the runtime directory.
const USER_DIR: &str = "tetsu";
/// The system socket directory.
const SYSTEM_DIR: &str = "/run/tetsu";
/// Interface of the capability service.
const CAP_INTERFACE: &str = "org.tetsu.cap.v1";
/// Interface of the notification service.
const NOTIFY_INTERFACE: &str = "org.tetsu.notify.v1";
/// The unit that serves capd.
const CAPD_UNIT: &str = "tetsu-capd.service";
/// The unit that serves notifyd.
const NOTIFYD_UNIT: &str = "tetsu-notifyd.service";
/// The interface every Varlink service answers.
const VARLINK_SERVICE: &str = "org.varlink.service";
/// The longest text of a server or a path kept in a report.
const REPORT_LIMIT: usize = 120;

/// The directories scanned for sockets, in order.
#[derive(Debug, Clone)]
pub struct Dirs {
    dirs: Vec<PathBuf>,
    timeout: Duration,
}

impl Dirs {
    /// Scans exactly these directories.
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            timeout: PROBE_TIMEOUT,
        }
    }

    /// The same directories with another `GetInfo` timeout.
    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// The scan order: the registry and the directory of the user, then
    /// the same two of the system.
    pub fn layout(runtime_dir: Option<&Path>, system: &Path) -> Self {
        let mut dirs = Vec::new();
        if let Some(runtime) = runtime_dir {
            let user = runtime.join(USER_DIR);
            dirs.push(user.join(REGISTRY));
            dirs.push(user);
        }
        dirs.push(system.join(REGISTRY));
        dirs.push(system.to_path_buf());
        Self::new(dirs)
    }

    /// The directories of this machine: `$XDG_RUNTIME_DIR` and `/run/tetsu`.
    pub fn production() -> Self {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        Self::layout(runtime.as_deref(), Path::new(SYSTEM_DIR))
    }

    /// The directories, in scan order.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }
}

/// The interfaces reached only at a fixed, verified endpoint.
#[derive(Debug, Clone)]
pub struct Pinned {
    entries: Vec<(String, Endpoint)>,
}

impl Pinned {
    /// These `(interface, endpoint)` pairs.
    pub fn new(entries: Vec<(String, Endpoint)>) -> Self {
        Self { entries }
    }

    /// Capd and notifyd of the user `euid`, each under that user's manager.
    pub fn production(euid: u32) -> Self {
        let unit = |unit: &str| ExpectedServer::UserUnit {
            uid: euid,
            unit: unit.to_owned(),
        };
        let notify = PathBuf::from(format!("/run/user/{euid}/{USER_DIR}/{NOTIFY_INTERFACE}"));
        Self::new(vec![
            (
                CAP_INTERFACE.to_owned(),
                Endpoint::new(capd_path(euid), unit(CAPD_UNIT)),
            ),
            (
                NOTIFY_INTERFACE.to_owned(),
                Endpoint::new(notify, unit(NOTIFYD_UNIT)),
            ),
        ])
    }

    /// The `(interface, endpoint)` pairs.
    pub fn entries(&self) -> &[(String, Endpoint)] {
        &self.entries
    }

    /// Whether `path` is the own path of one of the endpoints.
    fn owns(&self, path: &Path) -> bool {
        self.entries.iter().any(|(_, e)| e.path() == path)
    }

    fn names(&self) -> BTreeSet<&str> {
        self.entries.iter().map(|(i, _)| i.as_str()).collect()
    }
}

/// The interfaces a scanned socket answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The socket, as found in the scan directory.
    pub path: PathBuf,
    /// Its `GetInfo` interfaces.
    pub interfaces: Vec<String>,
}

/// The check of a pinned endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedProbe {
    /// The interface that must be served there.
    pub interface: String,
    /// Where, and by whom.
    pub endpoint: Endpoint,
    /// The `GetInfo` interfaces, or why the check failed.
    pub outcome: Result<Vec<String>, String>,
}

/// Where the calls of an interface go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A socket found by the scan; connected as it is.
    Scanned(PathBuf),
    /// A fixed endpoint; the server is checked on every connection.
    Pinned(Endpoint),
}

/// Something left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// What was left out: a path or an interface and its path.
    pub subject: String,
    /// Why.
    pub reason: String,
}

/// What discovery found.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    /// The tools of the interfaces found, sorted by id.
    pub tools: Vec<Tool>,
    /// Where each interface found is served.
    pub routes: BTreeMap<String, Route>,
    /// What was left out.
    pub skipped: Vec<Skipped>,
}

/// Text from a server or the file system for a report: control characters
/// dropped, cut to a limit.
fn clean(text: &str) -> String {
    let mut chars = text.chars().filter(|c| !c.is_control());
    let kept: String = chars.by_ref().take(REPORT_LIMIT).collect();
    if chars.next().is_some() {
        format!("{kept}...")
    } else {
        kept
    }
}

fn skipped(subject: impl AsRef<str>, reason: impl AsRef<str>) -> Skipped {
    Skipped {
        subject: clean(subject.as_ref()),
        reason: clean(reason.as_ref()),
    }
}

/// Combines the probes into the catalog. Pure.
///
/// Pinned interfaces first: one that checked out is routed to its
/// endpoint, one that did not is reported and absent. Then the scanned
/// sockets in order: an interface is claimed by the first socket that
/// announces it; a pinned one is never claimed, an unknown one is reported.
pub fn assemble(tools: &[Tool], scanned: &[Probe], pinned: &[PinnedProbe]) -> Catalog {
    let known: BTreeSet<&str> = tools.iter().map(|t| t.interface.as_str()).collect();
    let pinned_names: BTreeSet<&str> = pinned.iter().map(|p| p.interface.as_str()).collect();
    let mut routes = BTreeMap::new();
    let mut left_out = Vec::new();

    for p in pinned {
        match &p.outcome {
            Ok(interfaces) if interfaces.contains(&p.interface) => {
                routes.insert(p.interface.clone(), Route::Pinned(p.endpoint.clone()));
            }
            Ok(_) => left_out.push(skipped(
                format!("{} at {}", p.interface, p.endpoint.path().display()),
                "the server does not speak the interface",
            )),
            Err(reason) => left_out.push(skipped(
                format!("{} at {}", p.interface, p.endpoint.path().display()),
                reason,
            )),
        }
    }

    for probe in scanned {
        for interface in &probe.interfaces {
            let at = format!("{interface} at {}", probe.path.display());
            if interface == VARLINK_SERVICE {
                continue;
            }
            if pinned_names.contains(interface.as_str()) {
                left_out.push(skipped(at, "served only at its fixed, verified endpoint"));
            } else if !known.contains(interface.as_str()) {
                left_out.push(skipped(at, "not in the contract of this build"));
            } else if routes.contains_key(interface) {
                left_out.push(skipped(at, "already served by an earlier socket"));
            } else {
                routes.insert(interface.clone(), Route::Scanned(probe.path.clone()));
            }
        }
    }

    let mut found: Vec<Tool> = tools
        .iter()
        .filter(|t| routes.contains_key(&t.interface))
        .cloned()
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Catalog {
        tools: found,
        routes,
        skipped: left_out,
    }
}

/// `GetInfo` on a socket, within `timeout`, connect included.
async fn probe_path(path: &Path, timeout: Duration) -> Result<Vec<String>, String> {
    let work = async {
        let mut client = Client::connect(path).await.map_err(|e| e.to_string())?;
        client.get_info().await.map_err(|e| e.to_string())
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(info) => info.map(|i| i.interfaces),
        Err(_) => Err("no answer in time".to_owned()),
    }
}

/// Checks each pinned endpoint: who serves it, and what it speaks.
pub async fn probe_pinned(pinned: &Pinned, timeout: Duration) -> Vec<PinnedProbe> {
    let mut out = Vec::new();
    for (interface, endpoint) in pinned.entries() {
        let work = async {
            let mut client = Client::connect_to(endpoint)
                .await
                .map_err(|e| e.to_string())?;
            client.get_info().await.map_err(|e| e.to_string())
        };
        let outcome = match tokio::time::timeout(timeout, work).await {
            Ok(info) => info.map(|i| i.interfaces),
            Err(_) => Err("no answer in time".to_owned()),
        };
        out.push(PinnedProbe {
            interface: interface.clone(),
            endpoint: endpoint.clone(),
            outcome,
        });
    }
    out
}

/// Probes the sockets of the scan directories, in order. A socket named
/// like a pinned interface is never touched. The same socket seen twice
/// (a registry link and its directory) is probed once.
pub async fn scan(dirs: &Dirs, pinned: &Pinned) -> (Vec<Probe>, Vec<Skipped>) {
    let mut probes = Vec::new();
    let mut left_out = Vec::new();
    let mut seen = BTreeSet::new();
    let pinned_names = pinned.names();
    for dir in dirs.dirs() {
        // A missing directory is fine; so is one we cannot read.
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let shown = path.display().to_string();
            if pinned_names.contains(name) {
                // The endpoint itself is not news; a twin elsewhere is.
                if !pinned.owns(&path) {
                    left_out.push(skipped(
                        shown,
                        "named like an interface with a fixed endpoint",
                    ));
                }
                continue;
            }
            // `metadata` follows a link: a dangling one is an error.
            let Ok(meta) = fs::metadata(&path) else {
                left_out.push(skipped(shown, "dangling link"));
                continue;
            };
            if meta.is_dir() {
                continue;
            }
            if !meta.file_type().is_socket() {
                left_out.push(skipped(shown, "not a socket"));
                continue;
            }
            let Ok(real) = fs::canonicalize(&path) else {
                left_out.push(skipped(shown, "cannot be resolved"));
                continue;
            };
            if !seen.insert(real) {
                continue;
            }
            match probe_path(&path, dirs.timeout).await {
                Ok(interfaces) => probes.push(Probe { path, interfaces }),
                Err(reason) => left_out.push(skipped(shown, reason)),
            }
        }
    }
    (probes, left_out)
}

/// Scans, checks the pinned endpoints and assembles the catalog.
pub async fn discover(tools: &[Tool], dirs: &Dirs, pinned: &Pinned) -> Catalog {
    let (probes, mut left_out) = scan(dirs, pinned).await;
    let checked = probe_pinned(pinned, dirs.timeout).await;
    let mut catalog = assemble(tools, &probes, &checked);
    left_out.append(&mut catalog.skipped);
    catalog.skipped = left_out;
    catalog
}
