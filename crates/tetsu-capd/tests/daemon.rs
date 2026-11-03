//! The built daemon, run as a process with its paths in a tempdir.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use tempfile::TempDir;
use tetsu_bus::cap::{self, Grant, Layer, SetParams, State};

const WAIT: Duration = Duration::from_secs(5);

struct Daemon {
    dir: TempDir,
    child: Child,
    journal: UnixDatagram,
}

impl Daemon {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let image = root.join("image.toml");
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("dist/capabilities.toml");
        std::fs::copy(shipped, &image).unwrap();
        let journal = UnixDatagram::bind(root.join("journal")).unwrap();
        journal.set_read_timeout(Some(WAIT)).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_tetsu-capd"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("TETSU_CAPD_IMAGE_FILE", &image)
            .env("TETSU_CAPD_JOURNAL_SOCKET", root.join("journal"))
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self {
            dir,
            child,
            journal,
        };
        daemon.wait_for_socket();
        daemon
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("run/tetsu/org.tetsu.cap.v1")
    }

    fn wait_for_socket(&self) {
        let start = Instant::now();
        while !self.socket().exists() {
            assert!(start.elapsed() < WAIT, "the socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn the_socket_is_private_and_lists_the_image_defaults() {
    let daemon = Daemon::start();
    let mode = std::fs::metadata(daemon.socket())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let list = block_on(async {
        let mut client = cap::Client::connect(&daemon.socket()).await.unwrap();
        client.list().await.unwrap()
    });
    for capability in &list {
        if capability.class == "notify.send" {
            assert_eq!(capability.state, State::Automatic);
            assert_eq!(capability.layer, Some(Layer::Image));
        } else {
            assert_eq!(capability.state, State::Unset, "{}", capability.class);
            assert_eq!(capability.layer, None);
        }
    }
    assert!(list.iter().any(|c| c.class == "notify.send"));
}

#[test]
fn set_writes_the_managed_file_and_a_journal_entry() {
    let daemon = Daemon::start();
    block_on(async {
        let mut client = cap::Client::connect(&daemon.socket()).await.unwrap();
        client
            .set(&SetParams {
                class: "aur.review".into(),
                scope: None,
                state: Some(Grant::Off),
            })
            .await
            .unwrap();
        let list = client.list().await.unwrap();
        let review = list.iter().find(|c| c.class == "aur.review").unwrap();
        assert_eq!(
            (review.state, review.layer),
            (State::Off, Some(Layer::Managed))
        );
    });
    let managed = daemon.dir.path().join("state/tetsu/capabilities.toml");
    let text = std::fs::read_to_string(&managed).unwrap();
    assert!(text.contains("\"aur.review\" = \"off\""), "{text}");

    let mut buf = vec![0u8; 65536];
    let n = daemon.journal.recv(&mut buf).expect("a journal datagram");
    let datagram = String::from_utf8_lossy(&buf[..n]);
    assert!(
        datagram.contains("MESSAGE_ID=b396d11fd3634da6be8cd49c2f5304c4"),
        "{datagram}"
    );
    assert!(datagram.contains("TETSU_CLASS=aur.review"));
}

#[test]
fn sigterm_exits_cleanly_and_removes_the_socket() {
    let mut daemon = Daemon::start();
    let pid = Pid::from_raw(i32::try_from(daemon.child.id()).unwrap()).unwrap();
    kill_process(pid, Signal::TERM).unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = daemon.child.try_wait().unwrap() {
            break status;
        }
        assert!(start.elapsed() < WAIT, "the daemon did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status:?}");
    assert!(!daemon.socket().exists());
}
