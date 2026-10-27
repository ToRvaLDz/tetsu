//! Discovery: which interfaces are served, and where.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_agent::contract;
use tetsu_agent::discovery::{self, Catalog, Dirs, Pinned, PinnedProbe, Probe, Route};
use tetsu_agent::tools::{self, Tool};
use tetsu_bus::access::ExpectedServer;
use tetsu_bus::endpoint::Endpoint;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response};
use tokio::sync::oneshot;
use tokio::task::LocalSet;

const CAP: &str = "org.tetsu.cap.v1";
const NOTIFY: &str = "org.tetsu.notify.v1";
const TX: &str = "org.tetsu.tx.v1";
const WIN: &str = "org.tetsu.comp.window.v1";

fn all_tools() -> Vec<Tool> {
    tools::build(&contract::load().unwrap()).unwrap()
}

fn ids(catalog: &Catalog) -> Vec<&str> {
    catalog.tools.iter().map(|t| t.id.as_str()).collect()
}

fn interfaces_of(catalog: &Catalog) -> BTreeSet<&str> {
    catalog.tools.iter().map(|t| t.interface.as_str()).collect()
}

fn probe(path: &str, interfaces: &[&str]) -> Probe {
    Probe {
        path: PathBuf::from(path),
        interfaces: interfaces.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn pinned_ok(interface: &str, path: &str, interfaces: &[&str]) -> PinnedProbe {
    PinnedProbe {
        interface: interface.to_owned(),
        endpoint: Endpoint::new(path, ExpectedServer::Cgroup("/x".to_owned())),
        outcome: Ok(interfaces.iter().map(|s| (*s).to_owned()).collect()),
    }
}

fn route_path(catalog: &Catalog, interface: &str) -> PathBuf {
    match &catalog.routes[interface] {
        Route::Scanned(p) => p.clone(),
        Route::Pinned(e) => e.path().to_path_buf(),
    }
}

// assemble (pure)

#[test]
fn the_interfaces_found_give_exactly_their_tools_sorted() {
    let all = all_tools();
    let scanned = [probe("/a/one", &["org.varlink.service", TX, WIN])];
    let catalog = discovery::assemble(&all, &scanned, &[]);
    assert_eq!(interfaces_of(&catalog), BTreeSet::from([TX, WIN]));
    let expected: Vec<&str> = all
        .iter()
        .filter(|t| t.interface == TX || t.interface == WIN)
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids(&catalog), expected);
    assert_eq!(route_path(&catalog, TX), PathBuf::from("/a/one"));
    assert!(catalog.skipped.is_empty());
}

#[test]
fn an_unknown_interface_is_reported_and_has_no_tools() {
    let scanned = [probe("/a/one", &["org.example.v1", TX])];
    let catalog = discovery::assemble(&all_tools(), &scanned, &[]);
    assert_eq!(interfaces_of(&catalog), BTreeSet::from([TX]));
    assert!(!catalog.routes.contains_key("org.example.v1"));
    assert!(
        catalog
            .skipped
            .iter()
            .any(|s| s.subject.contains("org.example.v1"))
    );
}

#[test]
fn the_first_claim_wins_and_the_second_is_reported() {
    let scanned = [probe("/a/first", &[TX]), probe("/b/second", &[TX, WIN])];
    let catalog = discovery::assemble(&all_tools(), &scanned, &[]);
    assert_eq!(route_path(&catalog, TX), PathBuf::from("/a/first"));
    assert_eq!(route_path(&catalog, WIN), PathBuf::from("/b/second"));
    assert!(
        catalog
            .skipped
            .iter()
            .any(|s| s.subject.contains(TX) && s.subject.contains("/b/second"))
    );
}

#[test]
fn a_pinned_interface_announced_by_a_scanned_socket_is_ignored() {
    let pinned = [pinned_ok(CAP, "/pinned/cap", &["org.varlink.service", CAP])];
    let scanned = [probe("/evil/sock", &[CAP, NOTIFY])];
    let catalog = discovery::assemble(&all_tools(), &scanned, &pinned);
    assert_eq!(route_path(&catalog, CAP), PathBuf::from("/pinned/cap"));
    assert!(matches!(catalog.routes[CAP], Route::Pinned(_)));
    // The scanned socket still gets its other interface.
    assert_eq!(route_path(&catalog, NOTIFY), PathBuf::from("/evil/sock"));
    assert!(
        catalog
            .skipped
            .iter()
            .any(|s| s.subject.contains("/evil/sock"))
    );
}

#[test]
fn a_pinned_interface_claimed_only_by_a_scanned_socket_is_absent() {
    let pinned = [PinnedProbe {
        interface: CAP.to_owned(),
        endpoint: Endpoint::new("/pinned/cap", ExpectedServer::Cgroup("/x".to_owned())),
        outcome: Err("not the expected server".to_owned()),
    }];
    let scanned = [probe("/evil/sock", &[CAP])];
    let catalog = discovery::assemble(&all_tools(), &scanned, &pinned);
    assert!(catalog.tools.is_empty());
    assert!(catalog.routes.is_empty());
    assert!(
        catalog
            .skipped
            .iter()
            .any(|s| s.subject.contains(CAP) && s.reason.contains("not the expected server"))
    );
}

#[test]
fn a_pinned_server_that_does_not_speak_its_interface_is_absent() {
    let pinned = [pinned_ok(CAP, "/pinned/cap", &["org.varlink.service"])];
    let catalog = discovery::assemble(&all_tools(), &[], &pinned);
    assert!(catalog.tools.is_empty());
    assert_eq!(catalog.skipped.len(), 1);
}

#[test]
fn the_scan_order_is_the_registry_then_the_scope_directory() {
    let dirs = Dirs::layout(Some(Path::new("/run/user/1000")), Path::new("/run/tetsu"));
    assert_eq!(
        dirs.dirs(),
        [
            PathBuf::from("/run/user/1000/tetsu/registry"),
            PathBuf::from("/run/user/1000/tetsu"),
            PathBuf::from("/run/tetsu/registry"),
            PathBuf::from("/run/tetsu"),
        ]
    );
    let no_runtime = Dirs::layout(None, Path::new("/run/tetsu"));
    assert_eq!(
        no_runtime.dirs(),
        [
            PathBuf::from("/run/tetsu/registry"),
            PathBuf::from("/run/tetsu")
        ]
    );
}

#[test]
fn the_production_pins_are_capd_and_notifyd_of_this_user() {
    let pinned = Pinned::production(1000);
    let by_interface: Vec<(&str, &Endpoint)> = pinned
        .entries()
        .iter()
        .map(|(i, e)| (i.as_str(), e))
        .collect();
    assert_eq!(by_interface.len(), 2);
    let cap = by_interface.iter().find(|(i, _)| *i == CAP).unwrap().1;
    assert_eq!(
        cap.path(),
        Path::new("/run/user/1000/tetsu/org.tetsu.cap.v1")
    );
    assert_eq!(
        cap.expected(),
        &ExpectedServer::UserUnit {
            uid: 1000,
            unit: "tetsu-capd.service".to_owned()
        }
    );
    let notify = by_interface.iter().find(|(i, _)| *i == NOTIFY).unwrap().1;
    assert_eq!(
        notify.path(),
        Path::new("/run/user/1000/tetsu/org.tetsu.notify.v1")
    );
    assert_eq!(
        notify.expected(),
        &ExpectedServer::UserUnit {
            uid: 1000,
            unit: "tetsu-notifyd.service".to_owned()
        }
    );
}

// The scan and the probe (IO)

type Calls = Rc<RefCell<Vec<String>>>;

/// A fake service: answers `GetInfo` with its interfaces, records every call.
struct Fake {
    interfaces: Vec<&'static str>,
    calls: Calls,
    silent: bool,
}

impl Handler for Fake {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        self.calls.borrow_mut().push(request.method.clone());
        if self.silent {
            // Holds the answer open forever.
            let (tx, rx) = oneshot::channel::<Result<Value, server::ErrorReply>>();
            tokio::task::spawn_local(async move {
                std::future::pending::<()>().await;
                drop(tx);
            });
            return Response::Pending(rx);
        }
        match request.method.as_str() {
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "v", "product": "p", "version": "1", "url": "u",
                "interfaces": self.interfaces,
            })),
            other => Response::method_not_found(other),
        }
    }
}

fn own_cgroup() -> String {
    let proc = fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap();
}

struct World {
    root: tempfile::TempDir,
    dirs: Dirs,
}

impl World {
    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    /// Starts a fake service at `rel` and returns its call log.
    fn serve(&self, rel: &str, interfaces: &[&'static str], silent: bool) -> Calls {
        let calls: Calls = Rc::default();
        let path = self.path(rel);
        let listener = server::bind(&path).unwrap();
        let handler = Rc::new(Fake {
            interfaces: interfaces.to_vec(),
            calls: calls.clone(),
            silent,
        });
        tokio::task::spawn_local(server::serve(listener, handler));
        calls
    }
}

fn run<F, Fut>(test: F)
where
    F: FnOnce(World) -> Fut,
    Fut: Future<Output = ()>,
{
    let root = tempfile::tempdir().unwrap();
    // The four scan directories, in order.
    let layout = [
        "runtime/tetsu/registry",
        "runtime/tetsu",
        "system/registry",
        "system",
    ];
    for dir in layout {
        private_dir(&root.path().join(dir));
    }
    let dirs = Dirs::new(layout.iter().map(|d| root.path().join(d)).collect())
        .with_timeout(Duration::from_millis(200));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        test(World { root, dirs }).await;
    });
}

async fn discover(world: &World, pinned: &Pinned) -> Catalog {
    discovery::discover(&all_tools(), &world.dirs, pinned).await
}

#[test]
fn a_registry_symlink_and_a_plain_socket_are_both_found() {
    run(|world| async move {
        world.serve("runtime/tetsu/org.tetsu.tx.v1", &[TX], false);
        world.serve("system/org.tetsu.comp.window.v1", &[WIN], false);
        symlink(
            world.path("runtime/tetsu/org.tetsu.tx.v1"),
            world.path("runtime/tetsu/registry/org.tetsu.tx.v1"),
        )
        .unwrap();
        let catalog = discover(&world, &Pinned::new(Vec::new())).await;
        assert_eq!(interfaces_of(&catalog), BTreeSet::from([TX, WIN]));
        // The same socket seen through the registry and its directory is
        // one claim, not a duplicate.
        assert!(catalog.skipped.is_empty(), "{:?}", catalog.skipped);
    });
}

#[test]
fn a_regular_file_and_a_dangling_link_are_skipped() {
    run(|world| async move {
        fs::write(world.path("runtime/tetsu/notes.txt"), "hello").unwrap();
        symlink(
            world.path("nowhere"),
            world.path("runtime/tetsu/registry/dangling"),
        )
        .unwrap();
        world.serve("system/org.tetsu.tx.v1", &[TX], false);
        let catalog = discover(&world, &Pinned::new(Vec::new())).await;
        assert_eq!(interfaces_of(&catalog), BTreeSet::from([TX]));
    });
}

#[test]
fn a_server_that_never_answers_is_skipped_within_the_timeout() {
    run(|world| async move {
        world.serve("runtime/tetsu/registry/stuck", &[], true);
        world.serve("system/org.tetsu.tx.v1", &[TX], false);
        let started = std::time::Instant::now();
        let catalog = discover(&world, &Pinned::new(Vec::new())).await;
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(interfaces_of(&catalog), BTreeSet::from([TX]));
        assert!(catalog.skipped.iter().any(|s| s.subject.contains("stuck")));
    });
}

#[test]
fn a_missing_directory_is_fine() {
    run(|world| async move {
        let dirs = Dirs::new(vec![world.path("does/not/exist"), world.path("system")])
            .with_timeout(Duration::from_millis(200));
        world.serve("system/org.tetsu.tx.v1", &[TX], false);
        let catalog = discovery::discover(&all_tools(), &dirs, &Pinned::new(Vec::new())).await;
        assert_eq!(interfaces_of(&catalog), BTreeSet::from([TX]));
        assert!(catalog.skipped.is_empty());
    });
}

#[test]
fn a_planted_socket_named_like_a_pinned_interface_never_receives_a_call() {
    run(|world| async move {
        let planted = world.serve("runtime/tetsu/registry/org.tetsu.cap.v1", &[CAP], false);
        // With the interface pinned, the planted one is never called.
        let real = Endpoint::new(
            world.path("runtime/tetsu/org.tetsu.cap.v1"),
            ExpectedServer::Cgroup(own_cgroup()),
        );
        let real_calls = world.serve("runtime/tetsu/org.tetsu.cap.v1", &[CAP], false);
        let pinned = Pinned::new(vec![(CAP.to_owned(), real)]);
        let catalog = discover(&world, &pinned).await;
        assert!(planted.borrow().is_empty(), "{:?}", planted.borrow());
        assert_eq!(
            real_calls.borrow().as_slice(),
            ["org.varlink.service.GetInfo"]
        );
        assert!(matches!(catalog.routes[CAP], Route::Pinned(_)));
    });
}

#[test]
fn the_pinned_server_of_the_right_cgroup_is_found_and_a_wrong_one_is_absent() {
    run(|world| async move {
        let calls = world.serve("runtime/tetsu/org.tetsu.notify.v1", &[NOTIFY], false);
        let path = world.path("runtime/tetsu/org.tetsu.notify.v1");
        let right = Pinned::new(vec![(
            NOTIFY.to_owned(),
            Endpoint::new(&path, ExpectedServer::Cgroup(own_cgroup())),
        )]);
        let catalog = discover(&world, &right).await;
        assert_eq!(interfaces_of(&catalog), BTreeSet::from([NOTIFY]));
        assert!(matches!(catalog.routes[NOTIFY], Route::Pinned(_)));

        calls.borrow_mut().clear();
        let wrong = Pinned::new(vec![(
            NOTIFY.to_owned(),
            Endpoint::new(&path, ExpectedServer::Cgroup("/not/this".to_owned())),
        )]);
        let catalog = discover(&world, &wrong).await;
        assert!(catalog.tools.is_empty());
        assert!(catalog.skipped.iter().any(|s| s.subject.contains(NOTIFY)));
        assert!(calls.borrow().is_empty(), "a wrong server got a call");
    });
}
