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
