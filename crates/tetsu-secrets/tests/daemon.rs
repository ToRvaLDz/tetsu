//! The built `tetsu-secrets serve`, started the way the user unit starts
//! it: a plain key file in `$CREDENTIALS_DIRECTORY`, a scratch `$HOME` and
//! runtime directory, and a test socket standing in for the journal.

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use tetsu_bus::secrets::{AddParams, Client, RotateParams, SecretKind, Sensitive};
use tetsu_installer_core::install::config::staged_api_key;
use tetsu_secrets::audit::Action;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
/// The staged key, and every value the test adds: none may leak.
const SENTINEL: &str = "S3NT1NEL-SENTINEL";
const WAIT: Duration = Duration::from_secs(20);

/// A scratch environment for one daemon.
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["home", "run", "creds"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        fs::set_permissions(dir.path().join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn runtime(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn creds(&self) -> PathBuf {
        self.dir.path().join("creds")
    }

    fn socket(&self) -> PathBuf {
        self.runtime().join("tetsu/org.tetsu.secrets.v1")
    }

    fn journal(&self) -> PathBuf {
        self.dir.path().join("journal.sock")
    }

    fn set_key(&self, key: &str) {
        fs::write(self.creds().join("tetsu-secrets.vault-key"), key).unwrap();
    }

    fn stage(&self, provider: &str) -> PathBuf {
        let path = self.home().join(".local/share/tetsu/staged-api-key");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, staged_api_key(provider, SENTINEL)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn start(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_tetsu-secrets"))
            .arg("serve")
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", self.runtime())
            .env("CREDENTIALS_DIRECTORY", self.creds())
            .env("TETSU_SECRETS_JOURNAL_SOCKET", self.journal())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

impl Scratch {
    /// Starts the daemon the way systemd starts a socket-activated service:
    /// `listener` as fd 3, `LISTEN_FDS=1` and `LISTEN_PID` the daemon's pid.
    /// The pid is known only in the child, so a shell sets it and `exec`s
    /// (the pid stays); the listener reaches the shell as stdin.
    fn start_activated(&self, listener: UnixListener) -> Child {
        let script = concat!("LISTEN_PID=$$ LISTEN_FDS=1 exec \"$0\" serve 3<&0 </dev/null");
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg(env!("CARGO_BIN_EXE_tetsu-secrets"))
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", self.runtime())
            .env("CREDENTIALS_DIRECTORY", self.creds())
            .env("TETSU_SECRETS_JOURNAL_SOCKET", self.journal())
            .stdin(OwnedFd::from(listener))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

/// Collects the datagrams sent to the stand-in journal.
struct Journal {
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Journal {
    fn start(path: &Path) -> Self {
        let socket = UnixDatagram::bind(path).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        let thread = thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            while !stop2.load(Ordering::Relaxed) {
                if let Ok(n) = socket.recv(&mut buf) {
                    seen2.lock().unwrap().push(buf[..n].to_vec());
                }
            }
        });
        Self {
            seen,
            stop,
            thread: Some(thread),
        }
    }

    /// The actions recorded so far, by `MESSAGE_ID`, in order.
    fn actions(&self) -> Vec<Action> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|d| {
                let text = String::from_utf8_lossy(d);
                Action::ALL
                    .into_iter()
                    .find(|a| text.contains(&format!("MESSAGE_ID={}\n", a.message_id())))
                    .expect("a datagram with a known MESSAGE_ID")
            })
            .collect()
    }

    fn texts(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }

    fn finish(mut self) -> Vec<Vec<u8>> {
        thread::sleep(Duration::from_millis(150));
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
        self.texts()
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn terminate(mut child: Child) -> Output {
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    kill_process(pid, Signal::TERM).unwrap();
    wait_for("the daemon to exit", || child.try_wait().unwrap().is_some());
    child.wait_with_output().unwrap()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn add_params(id: &str) -> AddParams {
    AddParams {
        id: id.into(),
        kind: SecretKind::Bearer,
        header_name: None,
        host: "api.example.com".into(),
        allowed_tools: None,
        value: Sensitive::new(SENTINEL),
    }
}

#[test]
fn the_daemon_unlocks_imports_serves_and_audits() {
    let scratch = Scratch::new();
    scratch.set_key(KEY);
    let staged = scratch.stage("openai");
    let journal = Journal::start(&scratch.journal());
    let child = scratch.start();
    let socket = scratch.socket();
    wait_for("the socket", || socket.exists());

    runtime().block_on(async {
        let mut client = Client::connect(&socket).await.unwrap();
        let listed = client.list().await.unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0].id, "openai");
        assert_eq!(listed[0].host, "api.openai.com");
        assert_eq!(listed[0].kind, SecretKind::Bearer);
        client.add(&add_params("demo")).await.unwrap();
        let rotate = RotateParams {
            id: "demo".into(),
            value: Sensitive::new(SENTINEL),
        };
        client.rotate(&rotate).await.unwrap();
        let dup = client.add(&add_params("demo")).await.unwrap_err();
        assert_eq!(dup.remote_name(), Some("org.tetsu.secrets.v1.SecretExists"));
        client.remove("demo").await.unwrap();
        let listed = client.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!format!("{listed:?}").contains(SENTINEL));
    });
    assert!(!staged.exists(), "the staged key file is gone");
    let vault = scratch
        .home()
        .join(".local/share/tetsu/secrets/vault.keyring");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&vault), 0o600);
    assert_eq!(mode(vault.parent().unwrap()), 0o700);

    let out = terminate(child);
    assert!(out.status.success(), "{:?}", out.status);
    assert!(!socket.exists(), "the socket is removed at exit");
    let datagrams = journal.finish();
    use Action::*;
    let actions: Vec<_> = datagrams
        .iter()
        .map(|d| {
            Action::ALL
                .into_iter()
                .find(|a| contains(d, &format!("MESSAGE_ID={}", a.message_id())))
                .unwrap()
        })
        .collect();
    assert_eq!(
        actions,
        [Unlock, Add, List, Add, Rotate, Add, Remove, List],
        "{:?}",
        datagrams
            .iter()
            .map(|d| String::from_utf8_lossy(d).into_owned())
            .collect::<Vec<_>>()
    );
    for datagram in &datagrams {
        assert!(contains(datagram, "SYSLOG_IDENTIFIER=tetsu-secrets\n"));
        assert!(!contains(datagram, SENTINEL));
        assert!(!contains(datagram, KEY));
    }
    for stream in [&out.stdout, &out.stderr] {
        assert!(!contains(stream, SENTINEL));
        assert!(!contains(stream, KEY));
    }
}

#[test]
fn the_failed_add_is_in_the_journal_too() {
    let scratch = Scratch::new();
    scratch.set_key(KEY);
    let journal = Journal::start(&scratch.journal());
    let child = scratch.start();
    let socket = scratch.socket();
    wait_for("the socket", || socket.exists());
    runtime().block_on(async {
        let mut client = Client::connect(&socket).await.unwrap();
        client.add(&add_params("demo")).await.unwrap();
        client.add(&add_params("demo")).await.unwrap_err();
    });
    terminate(child);
    let datagrams = journal.finish();
    let failed = datagrams
        .iter()
        .filter(|d| contains(d, "TETSU_OUTCOME=failed\n"))
        .count();
    assert_eq!(failed, 1);
}

#[test]
fn a_wrong_key_on_an_existing_vault_exits_with_an_unlock_failure() {
    let scratch = Scratch::new();
    scratch.set_key(KEY);
    let journal = Journal::start(&scratch.journal());
    let first = scratch.start();
    let socket = scratch.socket();
    wait_for("the socket", || socket.exists());
    runtime().block_on(async {
        let mut client = Client::connect(&socket).await.unwrap();
        client.add(&add_params("demo")).await.unwrap();
    });
    terminate(first);
    let before = journal.texts().len();

    scratch.set_key(OTHER_KEY);
    let mut second = scratch.start();
    wait_for("the daemon to give up", || {
        second.try_wait().unwrap().is_some()
    });
    let out = second.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!scratch.socket().exists());
    let datagrams = journal.finish();
    let after = &datagrams[before..];
    assert_eq!(after.len(), 1);
    let unlock = format!("MESSAGE_ID={}\n", Action::Unlock.message_id());
    assert!(contains(&after[0], &unlock));
    assert!(contains(&after[0], "TETSU_OUTCOME=failed\n"));
    for stream in [&out.stdout, &out.stderr] {
        assert!(!contains(stream, SENTINEL));
        assert!(!contains(stream, OTHER_KEY));
        assert!(!contains(stream, KEY));
    }
}

#[test]
fn a_missing_credential_exits_with_an_unlock_failure() {
    let scratch = Scratch::new();
    let journal = Journal::start(&scratch.journal());
    let mut child = scratch.start();
    wait_for("the daemon to give up", || {
        child.try_wait().unwrap().is_some()
    });
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!scratch.socket().exists());
    assert_eq!(journal.actions(), [Action::Unlock]);
    let _ = journal.finish();
    assert!(String::from_utf8_lossy(&out.stderr).contains("tetsu-secrets.vault-key"));
}

#[test]
fn a_socket_from_systemd_is_served_and_left_in_place() {
    let scratch = Scratch::new();
    scratch.set_key(KEY);
    let journal = Journal::start(&scratch.journal());
    // What the socket unit does: bind the path before the service starts.
    let socket = scratch.socket();
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    let child = scratch.start_activated(listener);

    runtime().block_on(async {
        let mut client = Client::connect(&socket).await.unwrap();
        assert!(client.list().await.unwrap().is_empty());
        client.add(&add_params("demo")).await.unwrap();
        assert_eq!(client.list().await.unwrap().len(), 1);
    });
    let out = terminate(child);
    assert!(out.status.success(), "{:?}", out.status);
    // The path belongs to the socket unit: the daemon does not remove it.
    assert!(socket.exists(), "the activated socket stays");
    use Action::*;
    let datagrams = journal.finish();
    let actions: Vec<_> = datagrams
        .iter()
        .map(|d| {
            Action::ALL
                .into_iter()
                .find(|a| contains(d, &format!("MESSAGE_ID={}", a.message_id())))
                .unwrap()
        })
        .collect();
    assert_eq!(actions, [Unlock, List, Add, List]);
    assert!(contains(&datagrams[0], "TETSU_OUTCOME=ok\n"));
}
