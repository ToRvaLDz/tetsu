//! Terminal echo control for secret prompts.

use std::io::stdin;
use std::os::fd::AsFd;

use rustix::termios::{LocalModes, OptionalActions, Termios, isatty, tcgetattr, tcsetattr};

/// Whether standard input is a terminal (false for serial logs, pipes and
/// tests).
pub fn stdin_is_tty() -> bool {
    isatty(stdin().as_fd())
}

/// Echo is off while the guard lives; dropping it restores the terminal.
pub struct EchoOff {
    saved: Termios,
}

impl EchoOff {
    /// Turn echo off on standard input; `None` when it is not a terminal.
    pub fn new() -> Option<Self> {
        let fd = stdin();
        let saved = tcgetattr(fd.as_fd()).ok()?;
        let mut quiet = saved.clone();
        quiet.local_modes.remove(LocalModes::ECHO);
        // The newline the user types is not echoed either; keep it so the
        // next line starts clean.
        quiet.local_modes.insert(LocalModes::ECHONL);
        tcsetattr(fd.as_fd(), OptionalActions::Flush, &quiet).ok()?;
        Some(Self { saved })
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        let _ = tcsetattr(stdin().as_fd(), OptionalActions::Flush, &self.saved);
    }
}

/// A pseudo-terminal pair for tests: the test drives the master side, the
/// code under test gets the slave.
#[cfg(test)]
pub(crate) mod testpty {
    use std::ffi::OsStr;
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Read};
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    use rustix::fs::{OFlags, fcntl_setfl};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

    /// How long a test waits for the terminal before it fails.
    const DEADLINE: Duration = Duration::from_secs(10);

    pub struct Pty {
        pub master: File,
        pub slave: File,
    }

    pub fn open() -> Pty {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("openpt");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let name = ptsname(&master, Vec::new()).expect("ptsname");
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .open(OsStr::from_bytes(name.to_bytes()))
            .expect("open the pty slave");
        // Reads on the master never block; they are polled against a deadline.
        fcntl_setfl(&master, OFlags::NONBLOCK).expect("nonblocking master");
        Pty {
            master: File::from(master),
            slave,
        }
    }

    /// Read from the master until `needle` shows up; fails the test at the
    /// deadline. Returns everything read so far.
    pub fn read_until(master: &File, needle: &str) -> String {
        let start = Instant::now();
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            match (&*master).read(&mut buf) {
                Ok(0) => {}
                Ok(n) => seen.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => panic!("master read: {e}"),
            }
            let text = String::from_utf8_lossy(&seen).into_owned();
            if text.contains(needle) {
                return text;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "timeout waiting for {needle:?}, saw {text:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Everything the master can read right now.
    pub fn drain(master: &File) -> String {
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(n @ 1..) = (&*master).read(&mut buf) {
            seen.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&seen).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_is_off_while_the_guard_lives_and_comes_back_after() {
        let pty = testpty::open();
        let before = tcgetattr(&pty.slave).unwrap().local_modes;
        assert!(before.contains(LocalModes::ECHO));
        let guard = EchoOff::on(pty.slave.as_fd()).expect("a terminal");
        let during = tcgetattr(&pty.slave).unwrap().local_modes;
        assert!(!during.contains(LocalModes::ECHO));
        assert!(during.contains(LocalModes::ECHONL));
        drop(guard);
        assert_eq!(tcgetattr(&pty.slave).unwrap().local_modes, before);
    }

    #[test]
    fn a_pipe_is_not_a_terminal() {
        let (read, _write) = std::io::pipe().unwrap();
        assert!(EchoOff::on(read.as_fd()).is_none());
    }
}
