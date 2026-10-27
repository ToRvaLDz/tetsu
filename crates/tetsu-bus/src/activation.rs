//! Socket activation: adopting the descriptor systemd passes in.
//!
//! systemd passes sockets from fd 3 up, announced by `LISTEN_PID`,
//! `LISTEN_FDS` and `LISTEN_FDNAMES`. A socket is adopted only when it is
//! addressed to this process, carries the name of the socket file and is
//! bound to the path the server was asked to serve.

use std::fmt;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixListener;
use std::path::Path;

/// First descriptor systemd passes (`SD_LISTEN_FDS_START`).
const FDS_START: RawFd = 3;

/// A `LISTEN_*` variable that does not follow the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationError(String);

impl fmt::Display for ActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ActivationError {}

/// The descriptor named `name` among those passed to `my_pid`: `None` when
/// nothing is passed to this process or the name is not among them. Pure:
/// the environment arrives as arguments.
pub(crate) fn find(
    pid_env: Option<&str>,
    fds_env: Option<&str>,
    names_env: Option<&str>,
    my_pid: u32,
    name: &str,
) -> Result<Option<RawFd>, ActivationError> {
    let (Some(pid), Some(fds)) = (pid_env, fds_env) else {
        return Ok(None);
    };
    if pid.trim().parse::<u32>().ok() != Some(my_pid) {
        return Ok(None);
    }
    let count: usize = fds
        .trim()
        .parse()
        .map_err(|_| ActivationError(format!("LISTEN_FDS `{fds}` is not a count")))?;
    let Some(names) = names_env else {
        return Ok(None);
    };
    let names: Vec<&str> = names.split(':').collect();
    if names.len() > count {
        return Err(ActivationError(format!(
            "LISTEN_FDNAMES names {} descriptors but LISTEN_FDS is {count}",
            names.len()
        )));
    }
    let Some(index) = names.iter().position(|n| *n == name) else {
        return Ok(None);
    };
    let index = RawFd::try_from(index)
        .map_err(|_| ActivationError("descriptor index out of range".to_owned()))?;
    Ok(Some(FDS_START + index))
}

/// Adopts the socket systemd passed for `path`, if any. The name looked up
/// is the file name of `path`. The adopted socket must be bound to `path`.
pub(crate) fn adopt(path: &Path) -> io::Result<Option<UnixListener>> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Ok(None);
    };
    let var = |k: &str| std::env::var(k).ok();
    let fd = find(
        var("LISTEN_PID").as_deref(),
        var("LISTEN_FDS").as_deref(),
        var("LISTEN_FDNAMES").as_deref(),
        std::process::id(),
        name,
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let Some(fd) = fd else {
        return Ok(None);
    };
    // SAFETY: `find` returned a descriptor systemd addressed to this process
    // by name; nothing else in the process owns it.
    #[expect(unsafe_code)]
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    rustix::io::fcntl_setfd(&owned, rustix::io::FdFlags::CLOEXEC)?;
    let listener = UnixListener::from(owned);
    let addr = listener.local_addr()?;
    if addr.as_pathname() != Some(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the socket systemd passed as {name} is not bound to {}",
                path.display()
            ),
        ));
    }
    listener.set_nonblocking(true)?;
    Ok(Some(listener))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 4242;
    const NAME: &str = "org.tetsu.tx.v1";

    fn find_in(
        pid: Option<&str>,
        fds: Option<&str>,
        names: Option<&str>,
    ) -> Result<Option<i32>, ActivationError> {
        find(pid, fds, names, ME, NAME)
    }

    #[test]
    fn a_matching_name_gives_fd_three_plus_its_index() {
        assert_eq!(find_in(Some("4242"), Some("1"), Some(NAME)), Ok(Some(3)));
        assert_eq!(
            find_in(Some("4242"), Some("3"), Some("a:org.tetsu.tx.v1:b")),
            Ok(Some(4))
        );
    }

    #[test]
    fn another_pid_gives_none() {
        assert_eq!(find_in(Some("1"), Some("1"), Some(NAME)), Ok(None));
    }

    #[test]
    fn no_listen_variables_give_none() {
        assert_eq!(find_in(None, None, None), Ok(None));
        assert_eq!(find_in(Some("4242"), None, None), Ok(None));
    }

    #[test]
    fn an_absent_name_gives_none() {
        assert_eq!(find_in(Some("4242"), Some("2"), Some("a:b")), Ok(None));
        assert_eq!(find_in(Some("4242"), Some("1"), None), Ok(None));
    }

    #[test]
    fn a_malformed_fd_count_is_an_error() {
        assert!(find_in(Some("4242"), Some("many"), Some(NAME)).is_err());
        assert!(find_in(Some("4242"), Some("-1"), Some(NAME)).is_err());
    }

    #[test]
    fn more_names_than_descriptors_is_an_error() {
        assert!(find_in(Some("4242"), Some("1"), Some("a:b")).is_err());
    }
}
