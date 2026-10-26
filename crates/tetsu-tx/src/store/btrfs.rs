//! Btrfs subvolume operations behind a trait, so the snapshot lifecycle is
//! unit-tested with a fake and the argv with a recording runner.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use super::StoreError;
use crate::boot::tool::Runner;

/// Inode number of every subvolume root (`BTRFS_FIRST_FREE_OBJECTID`).
const SUBVOLUME_ROOT_INO: u64 = 256;

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
    /// Whether `path` is a subvolume root (not a plain directory). `false`
    /// when it does not exist.
    fn is_subvolume(&self, path: &Path) -> Result<bool, StoreError>;
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

    fn btrfs(&self, args: &[&OsStr]) -> Result<(), StoreError> {
        let args: Vec<OsString> = args.iter().map(|a| a.to_os_string()).collect();
        self.runner.run("btrfs", &args)?;
        Ok(())
    }
}

impl<R: Runner> Btrfs for CliBtrfs<R> {
    fn create(&self, path: &Path) -> Result<(), StoreError> {
        self.btrfs(&["subvolume".as_ref(), "create".as_ref(), path.as_os_str()])
    }

    fn snapshot(&self, src: &Path, dst: &Path, readonly: bool) -> Result<(), StoreError> {
        let mut args: Vec<&OsStr> = vec!["subvolume".as_ref(), "snapshot".as_ref()];
        if readonly {
            args.push("-r".as_ref());
        }
        args.extend([src.as_os_str(), dst.as_os_str()]);
        self.btrfs(&args)
    }

    fn delete(&self, path: &Path) -> Result<(), StoreError> {
        self.btrfs(&["subvolume".as_ref(), "delete".as_ref(), path.as_os_str()])
    }

    fn set_readonly(&self, path: &Path, readonly: bool) -> Result<(), StoreError> {
        let value = if readonly { "true" } else { "false" };
        self.btrfs(&[
            "property".as_ref(),
            "set".as_ref(),
            "-ts".as_ref(),
            path.as_os_str(),
            "ro".as_ref(),
            value.as_ref(),
        ])
    }

    fn sync(&self, path: &Path) -> Result<(), StoreError> {
        self.btrfs(&["filesystem".as_ref(), "sync".as_ref(), path.as_os_str()])
    }

    fn is_subvolume(&self, path: &Path) -> Result<bool, StoreError> {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            Ok(m) => Ok(m.is_dir() && m.ino() == SUBVOLUME_ROOT_INO),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(StoreError::io(path, e)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::boot::BootError;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    /// Records every command line it is asked to run.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub calls: RefCell<Vec<Vec<String>>>,
        pub fail: bool,
    }

    impl Runner for &Recorder {
        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            self.run(program, args).map(|()| String::new())
        }

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

    /// In-memory Btrfs over plain directories: tracks which paths are
    /// subvolumes and their `ro` flag, and records every operation with
    /// paths relative to `top`.
    pub(crate) struct FakeBtrfs {
        pub top: PathBuf,
        pub ops: RefCell<Vec<String>>,
        pub subvols: RefCell<BTreeMap<PathBuf, bool>>,
        /// Operation (first word of the record) that fails.
        pub fail: RefCell<Option<&'static str>>,
        /// A snapshot also copies the files of its source (nested
        /// subvolumes stay empty directories).
        pub copy_files: Cell<bool>,
    }

    impl FakeBtrfs {
        pub fn new(top: &Path) -> Self {
            Self {
                top: top.to_path_buf(),
                ops: RefCell::default(),
                subvols: RefCell::default(),
                fail: RefCell::default(),
                copy_files: Cell::new(false),
            }
        }

        fn copy_tree(&self, src: &Path, dst: &Path) -> Result<(), StoreError> {
            let read = fs::read_dir(src).map_err(|e| StoreError::io(src, e))?;
            for item in read {
                let item = item.map_err(|e| StoreError::io(src, e))?;
                let from = item.path();
                let to = dst.join(item.file_name());
                if self.subvols.borrow().contains_key(&from) {
                    continue;
                }
                let is_dir = item
                    .file_type()
                    .map_err(|e| StoreError::io(&from, e))?
                    .is_dir();
                if is_dir {
                    fs::create_dir(&to).map_err(|e| StoreError::io(&to, e))?;
                    self.copy_tree(&from, &to)?;
                } else {
                    fs::copy(&from, &to).map_err(|e| StoreError::io(&from, e))?;
                }
            }
            Ok(())
        }

        fn rel(&self, p: &Path) -> String {
            p.strip_prefix(&self.top).unwrap_or(p).display().to_string()
        }

        fn record(&self, op: &'static str, line: String) -> Result<(), StoreError> {
            self.ops.borrow_mut().push(line);
            if *self.fail.borrow() == Some(op) {
                return Err(StoreError::Tool(BootError::Tool {
                    tool: "btrfs".to_owned(),
                    status: "exit status: 1".to_owned(),
                    stderr: op.to_owned(),
                }));
            }
            Ok(())
        }

        fn require(&self, p: &Path) -> Result<(), StoreError> {
            if self.subvols.borrow().contains_key(p) {
                Ok(())
            } else {
                Err(StoreError::io(p, io::Error::from(io::ErrorKind::NotFound)))
            }
        }

        /// Mark an existing directory as a subvolume.
        pub fn adopt(&self, p: &Path) {
            fs::create_dir_all(p).unwrap();
            self.subvols.borrow_mut().insert(p.to_path_buf(), false);
        }

        pub fn readonly(&self, p: &Path) -> Option<bool> {
            self.subvols.borrow().get(p).copied()
        }

        pub fn take_ops(&self) -> Vec<String> {
            std::mem::take(&mut self.ops.borrow_mut())
        }
    }

    impl Btrfs for FakeBtrfs {
        fn create(&self, path: &Path) -> Result<(), StoreError> {
            self.record("create", format!("create {}", self.rel(path)))?;
            fs::create_dir(path).map_err(|e| StoreError::io(path, e))?;
            self.subvols.borrow_mut().insert(path.to_path_buf(), false);
            Ok(())
        }

        fn snapshot(&self, src: &Path, dst: &Path, readonly: bool) -> Result<(), StoreError> {
            let flag = if readonly { " -r" } else { "" };
            let line = format!("snapshot{flag} {} {}", self.rel(src), self.rel(dst));
            self.record("snapshot", line)?;
            self.require(src)?;
            fs::create_dir(dst).map_err(|e| StoreError::io(dst, e))?;
            // Not recursive: a nested subvolume leaves an empty directory.
            let nested: Vec<PathBuf> = self
                .subvols
                .borrow()
                .keys()
                .filter(|p| p.parent() == Some(src))
                .cloned()
                .collect();
            for n in nested {
                fs::create_dir(dst.join(n.file_name().unwrap())).unwrap();
            }
            if self.copy_files.get() {
                self.copy_tree(src, dst)?;
            }
            self.subvols
                .borrow_mut()
                .insert(dst.to_path_buf(), readonly);
            Ok(())
        }

        fn delete(&self, path: &Path) -> Result<(), StoreError> {
            self.record("delete", format!("delete {}", self.rel(path)))?;
            self.require(path)?;
            let subvols = self.subvols.borrow();
            if subvols.keys().any(|p| p != path && p.starts_with(path)) {
                return Err(StoreError::io(path, io::Error::other("nested subvolume")));
            }
            if path.parent().and_then(|p| subvols.get(p)) == Some(&true) {
                return Err(StoreError::io(
                    path,
                    io::Error::from(io::ErrorKind::ReadOnlyFilesystem),
                ));
            }
            drop(subvols);
            fs::remove_dir_all(path).map_err(|e| StoreError::io(path, e))?;
            self.subvols.borrow_mut().remove(path);
            Ok(())
        }

        fn set_readonly(&self, path: &Path, readonly: bool) -> Result<(), StoreError> {
            self.record("ro", format!("ro {} {readonly}", self.rel(path)))?;
            self.require(path)?;
            self.subvols
                .borrow_mut()
                .insert(path.to_path_buf(), readonly);
            Ok(())
        }

        fn sync(&self, path: &Path) -> Result<(), StoreError> {
            self.record("sync", format!("sync {}", self.rel(path)))
        }

        fn is_subvolume(&self, path: &Path) -> Result<bool, StoreError> {
            Ok(self.subvols.borrow().contains_key(path))
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
        assert!(
            matches!(err, StoreError::Tool(BootError::Tool { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn plain_directories_are_not_subvolumes() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        fs::create_dir(&plain).unwrap();
        let b = CliBtrfs::new(crate::boot::tool::SystemRunner::new());
        assert!(!b.is_subvolume(&plain).unwrap());
        assert!(!b.is_subvolume(&dir.path().join("missing")).unwrap());
    }
}
