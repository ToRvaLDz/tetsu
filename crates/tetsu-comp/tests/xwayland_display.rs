//! The X11 side without a compositor: the satellite policy (who is
//! Xwayland, which argv) and the X11 display's lock and sockets.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::process::Command;

use tetsu_comp::trust::Trust;
use tetsu_comp::xwayland::satellite::{
    peer_trust, probe_command, satellite_command, window_origin,
};

fn args(command: &Command) -> Vec<&OsStr> {
    command.get_args().collect()
}

fn envs(command: &Command) -> HashMap<OsString, Option<OsString>> {
    command
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(OsStr::to_owned)))
        .collect()
}

#[test]
fn only_the_live_satellite_is_xwayland() {
    assert_eq!(peer_trust(Some(41), Some(41)), Trust::Xwayland);
    assert_eq!(peer_trust(Some(42), Some(41)), Trust::Untrusted);
    assert_eq!(peer_trust(None, Some(41)), Trust::Untrusted);
    assert_eq!(peer_trust(Some(41), None), Trust::Untrusted);
    assert_eq!(peer_trust(None, None), Trust::Untrusted);
}

#[test]
fn an_xwayland_window_hides_the_satellite_pid() {
    assert_eq!(window_origin(Trust::Xwayland, Some(42)), (None, true));
    for trust in [Trust::Untrusted, Trust::Locker, Trust::Input] {
        assert_eq!(window_origin(trust, Some(42)), (Some(42), false));
        assert_eq!(window_origin(trust, None), (None, false));
    }
}

#[test]
fn satellite_gets_the_display_and_both_listening_fds() {
    let command = satellite_command("/x/sat", ":3", &[7, 8], OsStr::new("wayland-9"));
    assert_eq!(command.get_program(), "/x/sat");
    assert_eq!(args(&command), [":3", "-listenfd", "7", "-listenfd", "8"]);
    let envs = envs(&command);
    assert_eq!(envs.get(OsStr::new("DISPLAY")), Some(&None));
    assert_eq!(envs.get(OsStr::new("WAYLAND_SOCKET")), Some(&None));
    assert_eq!(
        envs.get(OsStr::new("WAYLAND_DISPLAY")),
        Some(&Some(OsString::from("wayland-9")))
    );
}

#[test]
fn satellite_without_an_abstract_socket_gets_one_fd() {
    let command = satellite_command("/x/sat", ":0", &[5], OsStr::new("wayland-1"));
    assert_eq!(args(&command), [":0", "-listenfd", "5"]);
}

#[test]
fn the_probe_asks_for_listenfd_support() {
    let command = probe_command("/x/sat");
    assert_eq!(command.get_program(), "/x/sat");
    assert_eq!(args(&command), [":0", "--test-listenfd-support"]);
    assert_eq!(envs(&command).get(OsStr::new("DISPLAY")), Some(&None));
}

mod display {
    use std::fs;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    use tetsu_comp::xwayland::display::{X11Display, pid_alive};

    fn lock(root: &Path, n: u32) -> std::path::PathBuf {
        root.join(format!(".X{n}-lock"))
    }

    fn socket(root: &Path, n: u32) -> std::path::PathBuf {
        root.join(format!(".X11-unix/X{n}"))
    }

    fn open(root: &Path) -> std::io::Result<X11Display> {
        X11Display::open(root, 0, pid_alive)
    }

    fn listener(fd: std::os::fd::BorrowedFd<'_>) -> UnixListener {
        UnixListener::from(fd.try_clone_to_owned().unwrap())
    }

    /// A pid nobody has any more: a child that ran and was reaped.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn the_first_display_is_zero_with_a_lock_and_both_sockets() {
        let root = tempfile::tempdir().unwrap();
        let display = open(root.path()).unwrap();
        assert_eq!(display.display_name(), ":0");
        let dir = fs::metadata(root.path().join(".X11-unix")).unwrap();
        assert_eq!(dir.permissions().mode() & 0o7777, 0o1777);
        let text = fs::read_to_string(lock(root.path(), 0)).unwrap();
        assert_eq!(text, format!("{:>10}\n", std::process::id()));
        assert!(socket(root.path(), 0).exists());
        let abstract_fd = display.abstract_fd().unwrap();
        let address = listener(abstract_fd).local_addr().unwrap();
        let expected = format!("{}/.X11-unix/X0", root.path().display());
        assert_eq!(address.as_abstract_name(), Some(expected.as_bytes()));
    }

    #[test]
    fn a_second_display_takes_the_next_number() {
        let root = tempfile::tempdir().unwrap();
        let first = open(root.path()).unwrap();
        let second = open(root.path()).unwrap();
        assert_eq!((first.display_name(), second.display_name()), (":0", ":1"));
    }

    #[test]
    fn a_lock_of_a_dead_process_is_reclaimed() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".X11-unix")).unwrap();
        fs::set_permissions(
            root.path().join(".X11-unix"),
            fs::Permissions::from_mode(0o1777),
        )
        .unwrap();
        fs::write(lock(root.path(), 0), format!("{:>10}\n", dead_pid())).unwrap();
        let display = open(root.path()).unwrap();
        assert_eq!(display.display_name(), ":0");
        let text = fs::read_to_string(lock(root.path(), 0)).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());
    }

    #[test]
    fn a_garbage_or_live_lock_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        fs::write(lock(root.path(), 0), "not a pid\n").unwrap();
        fs::write(
            lock(root.path(), 1),
            format!("{:>10}\n", std::process::id()),
        )
        .unwrap();
        let display = open(root.path()).unwrap();
        assert_eq!(display.display_name(), ":2");
        assert_eq!(
            fs::read_to_string(lock(root.path(), 0)).unwrap(),
            "not a pid\n"
        );
    }

    #[test]
    fn a_leftover_socket_under_a_free_lock_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let first = open(root.path()).unwrap();
        drop(first);
        drop(UnixListener::bind(socket(root.path(), 0)).unwrap());
        assert!(socket(root.path(), 0).exists());
        let display = open(root.path()).unwrap();
        assert_eq!(display.display_name(), ":0");
        UnixStream::connect(socket(root.path(), 0)).unwrap();
    }

    #[test]
    fn an_unsafe_x11_directory_is_refused() {
        for (mode, what) in [(0o777, "sticky"), (0o1755, "writable")] {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join(".X11-unix");
            fs::create_dir(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
            let message = open(root.path()).err().unwrap().to_string();
            assert!(message.contains(what), "{mode:o}: {message}");
            assert!(message.contains(".X11-unix"), "{mode:o}: {message}");
        }
    }

    #[test]
    fn fifty_taken_locks_are_an_error() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..50 {
            fs::write(
                lock(root.path(), n),
                format!("{:>10}\n", std::process::id()),
            )
            .unwrap();
        }
        let message = open(root.path()).err().unwrap().to_string();
        assert!(message.contains("no free X11 display"), "{message}");
        assert!(message.contains("50"), "{message}");
    }

    #[test]
    fn dropping_the_display_removes_the_lock_and_the_socket() {
        let root = tempfile::tempdir().unwrap();
        let display = open(root.path()).unwrap();
        drop(display);
        assert!(!lock(root.path(), 0).exists());
        assert!(!socket(root.path(), 0).exists());
    }

    #[test]
    fn a_client_makes_the_unix_listener_readable() {
        let root = tempfile::tempdir().unwrap();
        let display = open(root.path()).unwrap();
        let unix = listener(display.unix_fd());
        unix.set_nonblocking(true).unwrap();
        assert!(unix.accept().is_err());
        let _client = UnixStream::connect(socket(root.path(), 0)).unwrap();
        assert!(unix.accept().is_ok());
    }
}
