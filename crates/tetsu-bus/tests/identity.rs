//! Peer identity fails closed (#28 item 1): a peer without a live pidfd, or
//! with one that does not match its pid, is refused.

use std::io::ErrorKind;
use std::os::fd::AsFd;
use std::process::{Child, Command, Stdio};

use rustix::process::{Pid, PidfdFlags, pidfd_open};
use tetsu_bus::peer::Peer;

fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap()
}

fn pidfd_of(pid: i32) -> rustix::fd::OwnedFd {
    let pid = Pid::from_raw(pid).expect("a positive pid");
    pidfd_open(pid, PidfdFlags::empty()).expect("pidfd_open")
}

fn sleeper() -> Child {
    Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn sleep")
}

#[test]
fn a_peer_without_a_pidfd_is_refused() {
    let err = Peer::from_credentials(1000, 1000, own_pid(), None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::PermissionDenied);
}

#[test]
fn the_pidfd_of_a_reaped_child_is_refused() {
    let mut child = Command::new("true").spawn().expect("spawn true");
    let pid = i32::try_from(child.id()).unwrap();
    let pidfd = pidfd_of(pid);
    child.wait().unwrap();
    let err = Peer::from_credentials(1000, 1000, pid, Some(pidfd.as_fd())).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::PermissionDenied);
}

#[test]
fn a_pidfd_of_another_process_than_the_claimed_pid_is_refused() {
    let mut child = sleeper();
    let pidfd = pidfd_of(i32::try_from(child.id()).unwrap());
    // The credentials say this process, the pidfd says the child.
    let result = Peer::from_credentials(1000, 1000, own_pid(), Some(pidfd.as_fd()));
    child.kill().unwrap();
    child.wait().unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::PermissionDenied);
}

#[test]
fn a_live_pidfd_of_the_claimed_pid_resolves_the_cgroup() {
    let pidfd = pidfd_of(own_pid());
    let peer = Peer::from_credentials(1000, 1001, own_pid(), Some(pidfd.as_fd())).unwrap();
    let proc = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    assert_eq!(
        Some(peer.cgroup.clone()),
        tetsu_bus::peer::parse_proc_cgroup(&proc)
    );
    assert_eq!((peer.uid, peer.gid, peer.pid), (1000, 1001, own_pid()));
}

// A connection whose peer is gone, end to end. The test re-executes its own
// binary as a helper that connects while the listener is bound but not yet
// serving, writes a call and exits; the socket lives on in a child.

mod gone {
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use serde_json::json;
    use tetsu_bus::peer::Peer;
    use tetsu_bus::server::{self, Handler, Request, Response};
    use tokio::task::LocalSet;

    const MODE: &str = "TETSU_IDENTITY_MODE";
    const SOCKET: &str = "TETSU_IDENTITY_SOCKET";
    const MARKER: &str = "TETSU_IDENTITY_MARKER";
    const CALL: &[u8] = b"{\"method\":\"org.example.Echo\",\"parameters\":{}}\0";
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// Counts the calls that reach it.
    struct Counting(Rc<Cell<u32>>);

    impl Handler for Counting {
        async fn call(&self, _peer: Rc<Peer>, _request: Request) -> Response {
            self.0.set(self.0.get() + 1);
            Response::Reply(json!({}))
        }
    }

    /// Not a test unless the parent re-executed us with a mode set.
    #[test]
    fn helper_connector() {
        let Ok(mode) = std::env::var(MODE) else {
            return;
        };
        let socket = std::env::var(SOCKET).unwrap();
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.write_all(CALL).unwrap();
        match mode.as_str() {
            "alive" => {
                // Stay until the server answered, then report it.
                stream.set_read_timeout(Some(TIMEOUT)).unwrap();
                let mut chunk = [0u8; 256];
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "the server closed without an answer");
            }
            "dead" => {
                // Hand the socket to a child that reports when the server
                // closes it, then exit: the connector is gone.
                let marker = std::env::var(MARKER).unwrap();
                Command::new("sh")
                    .args(["-c", "cat >/dev/null; : > \"$1\"", "sh", &marker])
                    .stdin(Stdio::from(OwnedFd::from(stream)))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
            }
            other => panic!("unknown mode {other}"),
        }
    }

    fn run_helper(mode: &str, socket: &Path, marker: &Path) -> std::process::ExitStatus {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "gone::helper_connector", "--test-threads=1"])
            .env(MODE, mode)
            .env(SOCKET, socket)
            .env(MARKER, marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
    }

    fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("tetsu").join("org.example.v1")
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    async fn wait_for(path: &Path) -> bool {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if path.exists() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[test]
    fn a_connection_whose_peer_is_gone_never_reaches_the_handler() {
        let dir = tempfile::tempdir().unwrap();
        let socket = socket_in(&dir);
        let marker = dir.path().join("closed");
        let rt = runtime();
        let _guard = rt.enter();
        let listener = server::bind(&socket).unwrap();
        // Bound but not serving: the connect is queued in the backlog.
        let status = run_helper("dead", &socket, &marker);
        assert!(status.success(), "the helper failed: {status}");
        let calls = Rc::new(Cell::new(0));
        let handler = Rc::new(Counting(calls.clone()));
        let local = LocalSet::new();
        let closed = local.block_on(&rt, async {
            tokio::task::spawn_local(server::serve(listener, handler));
            wait_for(&marker).await
        });
        assert!(closed, "the server did not close the connection");
        assert_eq!(calls.get(), 0, "a call from a gone peer was served");
    }

    #[test]
    fn a_connection_whose_peer_is_alive_is_served() {
        let dir = tempfile::tempdir().unwrap();
        let socket = socket_in(&dir);
        let marker = dir.path().join("unused");
        let rt = runtime();
        let _guard = rt.enter();
        let listener = server::bind(&socket).unwrap();
        let calls = Rc::new(Cell::new(0));
        let handler = Rc::new(Counting(calls.clone()));
        let local = LocalSet::new();
        let status = local.block_on(&rt, async {
            tokio::task::spawn_local(server::serve(listener, handler));
            let (socket, marker) = (socket.clone(), marker.clone());
            tokio::task::spawn_blocking(move || run_helper("alive", &socket, &marker))
                .await
                .unwrap()
        });
        assert!(status.success(), "the live helper was not served: {status}");
        assert_eq!(calls.get(), 1);
    }
}
