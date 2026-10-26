//! One path of an `/etc` tree, read with lstat: what the three-way sync
//! compares.

use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// What a path is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A regular file and its content.
    File(Vec<u8>),
    Dir,
    /// A symlink and its target; never followed.
    Symlink(PathBuf),
    /// A socket, FIFO or device node: never synced.
    Special,
}

/// A path's type, permission bits, owner and content or target. Extended
/// attributes, ACLs and times are not part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    /// Permission bits, setuid/setgid/sticky included.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

impl Node {
    /// Read `path` without following it; `None` when it does not exist.
    pub fn read(path: &Path) -> io::Result<Option<Node>> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let kind = match meta.file_type() {
            t if t.is_symlink() => Kind::Symlink(fs::read_link(path)?),
            t if t.is_dir() => Kind::Dir,
            t if t.is_file() => Kind::File(fs::read(path)?),
            _ => Kind::Special,
        };
        Ok(Some(Node {
            kind,
            mode: meta.mode() & 0o7777,
            uid: meta.uid(),
            gid: meta.gid(),
        }))
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    pub fn is_special(&self) -> bool {
        self.kind == Kind::Special
    }
}

/// Two reads of a path are the same node; two absent paths are too.
pub fn same(a: Option<&Node>, b: Option<&Node>) -> bool {
    a == b
}

#[cfg(test)]
pub(super) mod fixtures {
    use super::*;

    pub fn file(content: &str, mode: u32) -> Node {
        Node {
            kind: Kind::File(content.as_bytes().to_vec()),
            mode,
            uid: 0,
            gid: 0,
        }
    }

    pub fn dir(mode: u32) -> Node {
        Node {
            kind: Kind::Dir,
            mode,
            uid: 0,
            gid: 0,
        }
    }

    pub fn link(target: &str) -> Node {
        Node {
            kind: Kind::Symlink(target.into()),
            mode: 0o777,
            uid: 0,
            gid: 0,
        }
    }

    pub fn special() -> Node {
        Node {
            kind: Kind::Special,
            mode: 0o644,
            uid: 0,
            gid: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn a_missing_path_or_one_below_a_file_is_none() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("f"), "x").unwrap();
        assert_eq!(Node::read(&t.path().join("nope")).unwrap(), None);
        assert_eq!(Node::read(&t.path().join("f/below")).unwrap(), None);
    }

    #[test]
    fn files_dirs_and_links_are_read_without_following() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        fs::write(&f, "hello").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o640)).unwrap();
        let got = Node::read(&f).unwrap().unwrap();
        assert_eq!(got.kind, Kind::File(b"hello".to_vec()));
        assert_eq!(got.mode, 0o640);
        symlink("f", t.path().join("l")).unwrap();
        symlink("dangling", t.path().join("d")).unwrap();
        let l = Node::read(&t.path().join("l")).unwrap().unwrap();
        assert_eq!(l.kind, Kind::Symlink("f".into()));
        let d = Node::read(&t.path().join("d")).unwrap().unwrap();
        assert_eq!(d.kind, Kind::Symlink("dangling".into()));
        assert!(Node::read(t.path()).unwrap().unwrap().is_dir());
    }

    #[test]
    fn a_fifo_is_special() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &p,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(Node::read(&p).unwrap().unwrap().is_special());
    }

    #[test]
    fn same_compares_type_mode_owner_and_content() {
        let a = file("x", 0o644);
        assert!(same(Some(&a), Some(&file("x", 0o644))));
        assert!(!same(Some(&a), Some(&file("y", 0o644))));
        assert!(!same(Some(&a), Some(&file("x", 0o600))));
        let mut other_owner = a.clone();
        other_owner.uid = 1000;
        assert!(!same(Some(&a), Some(&other_owner)));
        let mut other_group = a.clone();
        other_group.gid = 1000;
        assert!(!same(Some(&a), Some(&other_group)));
        assert!(!same(Some(&dir(0o755)), Some(&file("", 0o755))));
        assert!(!same(Some(&link("a")), Some(&link("b"))));
        assert!(same(None, None));
        assert!(!same(Some(&a), None));
    }
}
