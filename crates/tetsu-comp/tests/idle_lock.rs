//! Idle starts the locker, on the whole compositor, headless: after
//! `[lock] idle_timeout_s` with no activity the compositor starts
//! `[lock] command` with locker trust, and the bus goes `locked`, then
//! `unlocked`. Activity holds it off, `0` turns it off, the key reloads
//! live, and an empty command locks nothing. These tests run on the wall
//! clock, with wide margins; the timer itself is in `tests/idle.rs`.

mod common;
#[path = "common/locker.rs"]
mod locker;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::Client;
use common::comp::Comp;
use locker::Locker;

const GET_SESSION: &str = "org.tetsu.comp.session.v1.GetSession";
const WATCH_SESSION: &str = "org.tetsu.comp.session.v1.WatchSession";
const ROLE: &str = "TETSU_COMP_TEST_ROLE";
const OUT: &str = "TETSU_COMP_TEST_OUT";
/// The longest wait for the idle lock.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// How long a session that must not lock is watched.
const QUIET: Duration = Duration::from_millis(2_500);

/// The locker the compositor starts on idle: it locks, waits for
/// `locked`, reports it and unlocks. It is a test of this binary, run on
/// purpose; alone it returns at once.
#[test]
#[ignore = "run by the compositor under test"]
fn helper_idle_locker() {
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

/// A scratch directory and the out-file the helper writes in it.
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("idle-locked")
    }

    fn config_file(&self) -> PathBuf {
        self.dir.path().join("comp.toml")
    }

    /// A `[lock]` table whose command re-runs this binary as
    /// `helper_idle_locker`; `env` hands it its role, as the hotplug
    /// start takes no environment.
    fn lock_table(&self, idle_timeout_s: u32) -> String {
        let exe = std::env::current_exe().unwrap();
        let argv = [
            "env".to_owned(),
            format!("{ROLE}=locker"),
            format!("{OUT}={}", self.out().display()),
            exe.to_string_lossy().into_owned(),
            "--exact".to_owned(),
            "helper_idle_locker".to_owned(),
            "--ignored".to_owned(),
            "--nocapture".to_owned(),
        ];
        let quoted: Vec<String> = argv.iter().map(|a| format!("{a:?}")).collect();
        format!(
            "[lock]\ncommand = [{}]\nidle_timeout_s = {idle_timeout_s}\n",
            quoted.join(", ")
        )
    }

    /// Writes the user file whole, through a rename, so the reload poll
    /// sees a new file.
    fn write_config(&self, text: &str) {
        let tmp = self.dir.path().join("comp.toml.new");
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(tmp, self.config_file()).unwrap();
    }
}

fn bus_locked(comp: &Comp) -> bool {
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    client.call(&contract, GET_SESSION)["locked"]
        .as_bool()
        .unwrap()
}

/// Waits until the helper reported `locked`.
fn wait_idle_locked(out: &Path) {
    let start = Instant::now();
    while !out.exists() {
        assert!(
            start.elapsed() < LOCK_WAIT,
            "the session did not lock on idle"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read_to_string(out).unwrap(), "locked");
}

/// For `QUIET`, the helper never ran and the bus never said locked.
fn stays_unlocked(comp: &mut Comp, out: &Path, mut each: impl FnMut(&mut Comp)) {
    let start = Instant::now();
    while start.elapsed() < QUIET {
        each(comp);
        assert!(!out.exists(), "the locker ran");
        assert!(!bus_locked(comp), "the session locked");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn an_idle_session_starts_the_locker_and_the_bus_follows() {
    let scratch = Scratch::new();
    let comp = Comp::start_with(&[], Some(&scratch.lock_table(1)));
    let contract = common::contract();
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_SESSION, true);
    let initial = watch.event(&contract, WATCH_SESSION);
    assert_eq!(initial["kind"], "initial", "{initial}");
    assert_eq!(initial["locked"], false, "{initial}");

    wait_idle_locked(&scratch.out());
    assert_eq!(watch.event(&contract, WATCH_SESSION)["kind"], "locked");
    assert_eq!(watch.event(&contract, WATCH_SESSION)["kind"], "unlocked");
}

#[test]
fn activity_holds_the_idle_lock_off() {
    let scratch = Scratch::new();
    let mut comp = Comp::start_hotplug(&[], Some(&scratch.lock_table(1)));
    let out = scratch.out();
    stays_unlocked(&mut comp, &out, |comp| comp.control("activity"));
    wait_idle_locked(&out);
}

#[test]
fn a_zero_timeout_never_locks() {
    let scratch = Scratch::new();
    let mut comp = Comp::start_with(&[], Some(&scratch.lock_table(0)));
    stays_unlocked(&mut comp, &scratch.out(), |_| {});
}

#[test]
fn a_new_timeout_in_the_user_file_applies_live() {
    let scratch = Scratch::new();
    scratch.write_config(&scratch.lock_table(0));
    let config = scratch.config_file();
    let mut comp = Comp::start_with(&["--config", config.to_str().unwrap()], None);
    let out = scratch.out();
    stays_unlocked(&mut comp, &out, |_| {});
    scratch.write_config(&scratch.lock_table(1));
    wait_idle_locked(&out);
}

#[test]
fn an_empty_lock_command_locks_nothing_and_the_compositor_stays_up() {
    let comp = Comp::start_with(&[], Some("[lock]\ncommand = []\nidle_timeout_s = 1\n"));
    let start = Instant::now();
    while start.elapsed() < QUIET {
        assert!(!bus_locked(&comp), "the session locked");
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(!bus_locked(&comp));
}
