//! `ext-session-lock-v1` on the whole compositor, started headless, with a
//! real locker client: it covers every output, takes the keyboard from the
//! windows, unlocks only for its owner, and a dead locker leaves the session
//! locked. The bus reports it all.

mod common;
#[path = "common/locker.rs"]
mod locker;
#[path = "common/wayland.rs"]
mod wayland;

use std::path::Path;
use std::time::{Duration, Instant};

use common::Client;
use common::comp::Comp;
use locker::Locker;
use wayland::WlClient;

const GET_SESSION: &str = "org.tetsu.comp.session.v1.GetSession";
const WATCH_SESSION: &str = "org.tetsu.comp.session.v1.WatchSession";
const ROLE: &str = "TETSU_COMP_TEST_ROLE";
const OUT: &str = "TETSU_COMP_TEST_OUT";

fn bus_locked(comp: &Comp) -> bool {
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    client.call(&contract, GET_SESSION)["locked"]
        .as_bool()
        .unwrap()
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The bus keeps saying locked for `span`.
fn stays_locked(comp: &Comp, span: Duration) {
    let start = Instant::now();
    while start.elapsed() < span {
        assert!(bus_locked(comp), "the session unlocked");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn config_with_lock_command(command: &str) -> String {
    format!("[lock]\ncommand = {command}\n")
}

/// A `[lock] command` that re-runs this test binary as `helper_locker`.
fn helper_command() -> String {
    let exe = std::env::current_exe().unwrap();
    let argv = [
        exe.to_string_lossy().into_owned(),
        "--exact".to_owned(),
        "helper_locker".to_owned(),
        "--ignored".to_owned(),
        "--nocapture".to_owned(),
    ];
    let quoted: Vec<String> = argv.iter().map(|a| format!("{a:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

/// The locker the compositor starts after the first one died: it takes the
/// session over, waits for `locked`, reports it and unlocks. It is a test of
/// this binary, run on purpose; alone it returns at once.
#[test]
#[ignore = "run by the compositor under test"]
fn helper_locker() {
    if std::env::var(ROLE).as_deref() != Ok("locker") {
        return;
    }
    let out = std::env::var(OUT).expect("TETSU_COMP_TEST_OUT");
    let mut locker = Locker::connect_env();
    locker.lock();
    locker.wait_locked();
    std::fs::write(Path::new(&out), "locked").unwrap();
    locker.unlock();
}

#[test]
fn the_compositor_advertises_the_session_lock() {
    let comp = Comp::start();
    let globals = wayland::globals(&comp.wayland_socket());
    assert!(
        globals.iter().any(|g| g == "ext_session_lock_manager_v1"),
        "{globals:?}"
    );
}

#[test]
fn lock_surfaces_cover_every_output_and_the_bus_says_locked() {
    let comp = Comp::start_with(&["--headless-outputs", "2"], None);
    assert!(!bus_locked(&comp));
    let mut locker = Locker::connect(&comp.wayland_socket());
    locker.lock();
    locker.wait_for_configures(2);
    locker.wait_locked();
    let mut configures = locker.configures();
    configures.sort();
    assert_eq!(
        configures,
        [
            (Some("HEADLESS-1".to_owned()), (1920, 1080)),
            (Some("HEADLESS-2".to_owned()), (1920, 1080)),
        ]
    );
    eventually("the bus to say locked", || bus_locked(&comp));
}

#[test]
fn an_output_hotplugged_while_locked_gets_a_lock_surface() {
    let mut comp = Comp::start_hotplug(&["--headless-outputs", "1"], None);
    let mut locker = Locker::connect(&comp.wayland_socket());
    locker.lock();
    locker.wait_locked();
    locker.wait_for_configures(1);
    comp.control("add 1280x720");
    locker.wait_for_configures(2);
    assert_eq!(
        locker.configures()[1],
        (Some("HEADLESS-2".to_owned()), (1280, 720))
    );
    assert!(bus_locked(&comp));
    stays_locked(&comp, Duration::from_millis(300));
}

#[test]
fn the_lock_takes_the_keyboard_and_unlock_gives_it_back() {
    let comp = Comp::start();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let first = wl.map("a", "one");
    wl.wait_for_keyboard_events(&first, 1);
    assert_eq!(wl.keyboard_events(&first), [true]);

    let mut locker = Locker::connect(&comp.wayland_socket());
    locker.lock();
    locker.wait_locked();
    wl.wait_for_keyboard_events(&first, 2);
    assert_eq!(wl.keyboard_events(&first), [true, false]);
    locker.wait_for_keyboard_events(1);
    assert_eq!(locker.keyboard_events(), [true]);

    // A window that maps while locked is the model's focus, and still gets
    // no keyboard.
    let second = wl.map("b", "two");
    wl.sync();
    locker.settle(Duration::from_millis(200));
    wl.sync();
    assert!(wl.keyboard_events(&second).is_empty());
    assert_eq!(wl.keyboard_events(&first), [true, false]);
    assert_eq!(locker.keyboard_events(), [true]);

    locker.unlock();
    wl.wait_for_keyboard_events(&second, 1);
    assert_eq!(wl.keyboard_events(&second), [true]);
}

#[test]
fn an_exclusive_overlay_does_not_take_the_keyboard_while_locked() {
    let comp = Comp::start();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let mut locker = Locker::connect(&comp.wayland_socket());
    locker.lock();
    locker.wait_locked();
    locker.wait_for_keyboard_events(1);
    let _overlay = wl.overlay_exclusive();
    wl.sync();
    locker.settle(Duration::from_millis(200));
    assert_eq!(locker.keyboard_events(), [true], "the lock lost the focus");
    assert!(bus_locked(&comp));
}

#[test]
fn another_client_cannot_lock_or_unlock() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_SESSION, true);
    assert_eq!(watch.event(&contract, WATCH_SESSION)["locked"], false);

    let mut owner = Locker::connect(&comp.wayland_socket());
    owner.lock();
    owner.wait_locked();
    assert_eq!(watch.event(&contract, WATCH_SESSION)["kind"], "locked");

    let mut other = Locker::connect(&comp.wayland_socket());
    other.lock();
    other.wait_finished();
    assert!(!other.is_locked());
    stays_locked(&comp, Duration::from_millis(300));

    owner.unlock();
    assert_eq!(watch.event(&contract, WATCH_SESSION)["kind"], "unlocked");
    assert!(!bus_locked(&comp));
}

#[test]
fn a_locker_that_dies_leaves_the_session_locked_and_a_new_one_takes_over() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("relaunched");
    let config = config_with_lock_command(&helper_command());
    let comp = Comp::start_with_env(
        &[],
        Some(&config),
        &[(ROLE, "locker"), (OUT, out.to_str().unwrap())],
    );
    let contract = common::contract();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("a", "one");
    wl.wait_for_keyboard_events(&top, 1);

    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_SESSION, true);
    watch.event(&contract, WATCH_SESSION);

    let mut owner = Locker::connect(&comp.wayland_socket());
    owner.lock();
    owner.wait_locked();
    assert_eq!(watch.event(&contract, WATCH_SESSION)["kind"], "locked");
    wl.wait_for_keyboard_events(&top, 2);
    owner.crash();

    // The compositor starts [lock] command, which takes the lock over,
    // reports `locked`, and unlocks.
    let start = Instant::now();
    while !out.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the locker was not relaunched"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(watch.event(&contract, WATCH_SESSION)["kind"], "unlocked");
    // Locked the whole time in between; the window got no keyboard until
    // the new owner unlocked.
    wl.wait_for_keyboard_events(&top, 3);
    assert_eq!(wl.keyboard_events(&top), [true, false, true]);
}

#[test]
fn with_no_lock_command_a_dead_locker_leaves_the_session_locked_for_good() {
    let comp = Comp::start_with(&[], Some(&config_with_lock_command("[]")));
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("a", "one");
    wl.wait_for_keyboard_events(&top, 1);

    let mut owner = Locker::connect(&comp.wayland_socket());
    owner.lock();
    owner.wait_locked();
    wl.wait_for_keyboard_events(&top, 2);
    owner.crash();

    // Nothing unlocks, nothing takes over, no window gets the keyboard.
    stays_locked(&comp, Duration::from_millis(1_200));
    let mut intruder = Locker::connect(&comp.wayland_socket());
    intruder.lock();
    intruder.wait_finished();
    assert!(!intruder.is_locked());
    wl.sync();
    assert_eq!(wl.keyboard_events(&top), [true, false]);
    stays_locked(&comp, Duration::from_millis(300));
}
