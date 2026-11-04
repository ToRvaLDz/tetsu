//! The X11 display the compositor owns: the lock `<root>/.X<n>-lock` and
//! the listening sockets `<root>/.X11-unix/X<n>` (a file and an abstract
//! name), as Xwayland itself and niri make them. A lock whose owner is dead
//! is taken over. Dropping the display removes the lock and the socket file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixListener};
use std::path::{Path, PathBuf};

/// How many display numbers are tried from `start`.
pub const TRIES: u32 = 50;

/// The mode of `.X11-unix`: everyone writes, the sticky bit protects.
const X11_DIR_MODE: u32 = 0o1777;

/// Whether `pid` may still be alive: true unless the kernel says no such
/// process (a process of another account counts as alive).
pub fn pid_alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

/// Removes a file when dropped.
#[derive(Debug)]
struct Unlink(PathBuf);

impl Drop for Unlink {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// An X11 display: its number, its listening sockets and the files that
/// hold it. The sockets close before the files go.
#[derive(Debug)]
pub struct X11Display {
    number: u32,
    name: String,
    unix: UnixListener,
    abstract_listener: UnixListener,
    _socket: Unlink,
    _lock: Unlink,
}

impl X11Display {
    /// Takes the first free display from `start` under `root` (`/tmp` for
    /// real). `alive` tells whether the pid of an existing lock still runs.
    pub fn open(root: &Path, start: u32, alive: impl Fn(u32) -> bool) -> io::Result<Self> {
        let dir = root.join(".X11-unix");
        ensure_x11_dir(root, &dir)?;
        let mut last_bind = None;
        for number in start..start.saturating_add(TRIES) {
            let Some(lock) = take_lock(&root.join(format!(".X{number}-lock")), &alive)? else {
                continue;
            };
            match bind(&dir, number) {
                Ok((abstract_listener, unix, socket)) => {
                    return Ok(Self {
                        number,
                        name: format!(":{number}"),
                        unix,
                        abstract_listener,
                        _socket: socket,
                        _lock: lock,
                    });
                }
                Err(e) => last_bind = Some(e),
            }
        }
        let tail = last_bind.map_or_else(String::new, |e| format!(" (last bind error: {e})"));
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "no free X11 display in {TRIES} tries from :{start} under {}{tail}",
                root.display()
            ),
        ))
    }

    /// The display number.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// `:<n>`, the value of `DISPLAY`.
    pub fn display_name(&self) -> &str {
        &self.name
    }

    /// The listening sockets: the one at `.X11-unix/X<n>`, then the one at
    /// the abstract name `<root>/.X11-unix/X<n>`.
    pub fn listeners(&self) -> [BorrowedFd<'_>; 2] {
        [self.unix.as_fd(), self.abstract_listener.as_fd()]
    }
}

/// Creates `.X11-unix` with mode 1777, or checks the one there: owned by
/// us or by the owner of `root`, writable by all, sticky (as Mutter does).
fn ensure_x11_dir(root: &Path, dir: &Path) -> io::Result<()> {
    let context = |e: io::Error, what: &str| {
        io::Error::new(e.kind(), format!("{}: {what}: {e}", dir.display()))
    };
    match fs::create_dir(dir) {
        Ok(()) => {
            // `mkdir` obeys the umask; the mode must not.
            return fs::set_permissions(dir, fs::Permissions::from_mode(X11_DIR_MODE))
                .map_err(|e| context(e, "cannot set its mode"));
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(context(e, "cannot create it")),
    }
    let meta = fs::symlink_metadata(dir).map_err(|e| context(e, "cannot stat it"))?;
    let root_meta = fs::symlink_metadata(root).map_err(|e| context(e, "cannot stat its parent"))?;
    let refused = |why: &str| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: {why}", dir.display()),
        ))
    };
    if !meta.is_dir() {
        return refused("not a directory");
    }
    let me = rustix::process::getuid().as_raw();
    if meta.uid() != root_meta.uid() && meta.uid() != me {
        return refused("owned by another account");
    }
    if meta.mode() & 0o022 != 0o022 {
        return refused("not writable by group and others");
    }
    if meta.mode() & 0o1000 == 0 {
        return refused("missing the sticky bit");
    }
    Ok(())
}

/// Takes the lock at `path`: created with `O_EXCL` and our pid in it. A
/// lock whose owner `alive` says is gone is removed and taken; one that
/// cannot be read or whose owner lives is left alone (`None`).
fn take_lock(path: &Path, alive: &impl Fn(u32) -> bool) -> io::Result<Option<Unlink>> {
    if let Some(lock) = create_lock(path)? {
        return Ok(Some(lock));
    }
    let owner = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    match owner {
        Some(pid) if !alive(pid) => {
            eprintln!(
                "tetsu-comp: reclaiming {} of dead pid {pid}",
                path.display()
            );
            match fs::remove_file(path) {
                Ok(()) => create_lock(path),
                Err(_) => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

/// Creates the lock file with our pid, or `None` when it exists.
fn create_lock(path: &Path) -> io::Result<Option<Unlink>> {
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
        Err(e) => return Err(e),
    };
    let lock = Unlink(path.to_owned());
    writeln!(file, "{:>10}", std::process::id())?;
    Ok(Some(lock))
}

type Bound = (UnixListener, UnixListener, Unlink);

/// Binds the abstract name first (a live server there is never disturbed),
/// then the file, replacing a leftover one.
fn bind(dir: &Path, number: u32) -> io::Result<Bound> {
    let path = dir.join(format!("X{number}"));
    let name = path.as_os_str().as_encoded_bytes();
    let abstract_listener = UnixListener::bind_addr(&SocketAddr::from_abstract_name(name)?)?;
    let _ = fs::remove_file(&path);
    let unix = UnixListener::bind(&path)?;
    Ok((abstract_listener, unix, Unlink(path)))
}
