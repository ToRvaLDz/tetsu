//! Btrfs subvolume operations behind a trait, so the snapshot lifecycle is
//! unit-tested with a fake and the argv with a recording runner.

use std::ffi::OsString;
use std::path::Path;

use super::StoreError;
use crate::boot::tool::Runner;

/// The subvolume operations the store needs.
pub trait Btrfs {
    fn create(&self, path: &Path) -> Result<(), StoreError>;
    /// Snapshot `src` (not recursive: nested subvolumes appear as empty
    /// directories) to `dst`.
    fn snapshot(&self, src: &Path, dst: &Path, readonly: bool) -> Result<(), StoreError>;
    fn delete(&self, path: &Path) -> Result<(), StoreError>;
    fn set_readonly(&self, path: &Path, readonly: bool) -> Result<(), StoreError>;
    /// `btrfs filesystem sync`: commit the filesystem transaction.
    fn sync(&self, path: &Path) -> Result<(), StoreError>;
}

/// [`Btrfs`] through the `btrfs` command of btrfs-progs.
#[derive(Clone, Debug, Default)]
pub struct CliBtrfs<R> {
    runner: R,
}

impl<R: Runner> CliBtrfs<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    fn btrfs(&self, args: &[&std::ffi::OsStr]) -> Result<(), StoreError> {
        let _ = args;
        todo!()
    }
}

impl<R: Runner> Btrfs for CliBtrfs<R> {
    fn create(&self, path: &Path) -> Result<(), StoreError> {
        let _ = path;
        todo!()
    }

    fn snapshot(&self, src: &Path, dst: &Path, readonly: bool) -> Result<(), StoreError> {
        let _ = (src, dst, readonly);
        todo!()
    }

    fn delete(&self, path: &Path) -> Result<(), StoreError> {
        let _ = path;
        todo!()
    }

    fn set_readonly(&self, path: &Path, readonly: bool) -> Result<(), StoreError> {
        let _ = (path, readonly);
        todo!()
    }

    fn sync(&self, path: &Path) -> Result<(), StoreError> {
        let _ = path;
        todo!()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::boot::BootError;
    use std::cell::RefCell;

    /// Records every command line it is asked to run.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub calls: RefCell<Vec<Vec<String>>>,
        pub fail: bool,
    }

    impl Runner for &Recorder {
        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            let mut line = vec![program.to_owned()];
            line.extend(args.iter().map(|a| a.to_string_lossy().into_owned()));
            self.calls.borrow_mut().push(line);
            if self.fail {
                return Err(BootError::Tool {
                    tool: program.to_owned(),
                    status: "exit status: 1".to_owned(),
                    stderr: "boom".to_owned(),
                });
            }
            Ok(())
        }
    }

    fn argv(f: impl FnOnce(&CliBtrfs<&Recorder>)) -> Vec<String> {
        let rec = Recorder::default();
        f(&CliBtrfs::new(&rec));
        let calls = rec.calls.into_inner();
        assert_eq!(calls.len(), 1, "{calls:?}");
        calls.into_iter().next().unwrap()
    }

    #[test]
    fn create_argv() {
        let a = argv(|b| b.create(Path::new("/t/@var")).unwrap());
        assert_eq!(a, ["btrfs", "subvolume", "create", "/t/@var"]);
    }

    #[test]
    fn snapshot_argv() {
        let a = argv(|b| b.snapshot(Path::new("/s"), Path::new("/d"), false).unwrap());
        assert_eq!(a, ["btrfs", "subvolume", "snapshot", "/s", "/d"]);
        let a = argv(|b| b.snapshot(Path::new("/s"), Path::new("/d"), true).unwrap());
        assert_eq!(a, ["btrfs", "subvolume", "snapshot", "-r", "/s", "/d"]);
    }

    #[test]
    fn delete_argv() {
        let a = argv(|b| b.delete(Path::new("/t/x")).unwrap());
        assert_eq!(a, ["btrfs", "subvolume", "delete", "/t/x"]);
    }

    #[test]
    fn set_readonly_argv() {
        let a = argv(|b| b.set_readonly(Path::new("/r"), true).unwrap());
        assert_eq!(a, ["btrfs", "property", "set", "-ts", "/r", "ro", "true"]);
        let a = argv(|b| b.set_readonly(Path::new("/r"), false).unwrap());
        assert_eq!(a, ["btrfs", "property", "set", "-ts", "/r", "ro", "false"]);
    }

    #[test]
    fn sync_argv() {
        let a = argv(|b| b.sync(Path::new("/t")).unwrap());
        assert_eq!(a, ["btrfs", "filesystem", "sync", "/t"]);
    }

    #[test]
    fn tool_failure_is_a_store_error() {
        let rec = Recorder {
            fail: true,
            ..Recorder::default()
        };
        let err = CliBtrfs::new(&rec).create(Path::new("/x")).unwrap_err();
        assert!(matches!(err, StoreError::Tool(BootError::Tool { .. })), "{err:?}");
    }
}
