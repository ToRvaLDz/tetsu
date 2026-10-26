//! The sync on real directory trees. Chown goes to the caller's own ids, so
//! no root is needed.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::apply::{SyncReport, sync};
use super::plan::ConflictKind;

#[derive(Clone, Copy)]
enum Tree {
    Base,
    Old,
    New,
}
use Tree::*;

struct T {
    dir: tempfile::TempDir,
}

impl T {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for t in ["base", "old", "new"] {
            fs::create_dir(dir.path().join(t)).unwrap();
        }
        Self { dir }
    }

    fn root(&self, t: Tree) -> PathBuf {
        let name = match t {
            Base => "base",
            Old => "old",
            New => "new",
        };
        self.dir.path().join(name)
    }

    fn kept(&self) -> PathBuf {
        self.dir.path().join("kept")
    }

    fn put(&self, t: Tree, rel: &str, text: &str) {
        let p = self.root(t).join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn put_all(&self, rel: &str, base: &str, old: &str, new: &str) {
        self.put(Base, rel, base);
        self.put(Old, rel, old);
        self.put(New, rel, new);
    }

    fn mode(&self, t: Tree, rel: &str, mode: u32) {
        let p = self.root(t).join(rel);
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn read(&self, t: Tree, rel: &str) -> Option<String> {
        fs::read_to_string(self.root(t).join(rel)).ok()
    }

    fn exists(&self, t: Tree, rel: &str) -> bool {
        fs::symlink_metadata(self.root(t).join(rel)).is_ok()
    }

    fn kept_text(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.kept().join(rel)).ok()
    }

    fn sync(&self) -> SyncReport {
        sync(
            &self.root(Base),
            &self.root(Old),
            &self.root(New),
            &self.kept(),
        )
        .unwrap()
    }
}

fn names(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn a_file_only_the_user_changed_is_copied_with_its_mode_and_time() {
    let t = T::new();
    t.put_all("hosts", "a\n", "user\n", "a\n");
    t.mode(Old, "hosts", 0o600);
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    fs::File::options()
        .write(true)
        .open(t.root(Old).join("hosts"))
        .unwrap()
        .set_modified(when)
        .unwrap();
    let report = t.sync();
    assert_eq!(names(&report.taken), ["hosts"]);
    assert_eq!(t.read(New, "hosts").as_deref(), Some("user\n"));
    let meta = fs::metadata(t.root(New).join("hosts")).unwrap();
    assert_eq!(meta.mode() & 0o7777, 0o600);
    assert_eq!(meta.modified().unwrap(), when);
    assert!(report.conflicts.is_empty());
    assert!(!t.kept().exists());
}

#[test]
fn a_file_only_the_transaction_changed_is_kept() {
    let t = T::new();
    t.put_all("os-release", "a\n", "a\n", "new\n");
    t.put(New, "added", "x");
    let report = t.sync();
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(t.read(New, "os-release").as_deref(), Some("new\n"));
    assert_eq!(t.read(New, "added").as_deref(), Some("x"));
}

#[test]
fn a_file_added_by_the_user_is_copied() {
    let t = T::new();
    t.put(Old, "tetsu.conf", "SYNC\n");
    t.mode(Old, "tetsu.conf", 0o640);
    let report = t.sync();
    assert_eq!(names(&report.taken), ["tetsu.conf"]);
    let meta = fs::metadata(t.root(New).join("tetsu.conf")).unwrap();
    assert_eq!(meta.mode() & 0o7777, 0o640);
}

#[test]
fn both_changed_differently_the_new_one_wins_and_the_users_is_kept_aside() {
    let t = T::new();
    t.put_all("sub/pacman.conf", "a\n", "user\n", "tx\n");
    let report = t.sync();
    assert_eq!(t.read(New, "sub/pacman.conf").as_deref(), Some("tx\n"));
    assert_eq!(t.kept_text("sub/pacman.conf").as_deref(), Some("user\n"));
    assert_eq!(report.conflicts.len(), 1);
    let c = &report.conflicts[0];
    assert_eq!(c.path, "sub/pacman.conf");
    assert_eq!(c.kind, ConflictKind::BothChanged);
    assert!(c.kept);
    assert!(report.taken.is_empty());
    let mode = fs::metadata(t.kept()).unwrap().mode() & 0o7777;
    assert_eq!(mode, 0o700);
}

#[test]
fn both_changed_identically_is_a_no_op() {
    let t = T::new();
    t.put_all("same", "a\n", "x\n", "x\n");
    let report = t.sync();
    assert!(report.is_empty(), "{report:?}");
    assert!(!t.kept().exists());
}

#[test]
fn a_file_the_user_deleted_is_deleted_from_the_new_tree() {
    let t = T::new();
    t.put(Base, "motd", "hi\n");
    t.put(New, "motd", "hi\n");
    let report = t.sync();
    assert!(!t.exists(New, "motd"));
    assert_eq!(names(&report.deleted), ["motd"]);
}

#[test]
fn a_file_the_transaction_deleted_while_the_user_edited_it_stays_deleted() {
    let t = T::new();
    t.put(Base, "old.conf", "a\n");
    t.put(Old, "old.conf", "user\n");
    let report = t.sync();
    assert!(!t.exists(New, "old.conf"));
    assert_eq!(t.kept_text("old.conf").as_deref(), Some("user\n"));
    assert_eq!(report.conflicts[0].kind, ConflictKind::TxDeleted);
    assert!(report.conflicts[0].kept);
}

#[test]
fn a_file_the_transaction_deleted_and_the_user_did_not_touch_stays_deleted() {
    let t = T::new();
    t.put(Base, "old.conf", "a\n");
    t.put(Old, "old.conf", "a\n");
    assert!(t.sync().is_empty());
    assert!(!t.exists(New, "old.conf"));
}

#[test]
fn a_file_the_user_deleted_while_the_transaction_edited_it_is_kept() {
    let t = T::new();
    t.put(Base, "edited", "a\n");
    t.put(New, "edited", "tx\n");
    let report = t.sync();
    assert_eq!(t.read(New, "edited").as_deref(), Some("tx\n"));
    assert_eq!(report.conflicts[0].kind, ConflictKind::UserDeleted);
    assert!(!report.conflicts[0].kept);
    assert!(!t.kept().exists());
}

#[test]
fn a_directory_added_by_the_user_is_copied_whole() {
    let t = T::new();
    t.put(Old, "mine/deep/a.conf", "A\n");
    t.put(Old, "mine/b.conf", "B\n");
    t.mode(Old, "mine/b.conf", 0o600);
    let report = t.sync();
    assert_eq!(names(&report.taken), ["mine"]);
    assert_eq!(t.read(New, "mine/deep/a.conf").as_deref(), Some("A\n"));
    let meta = fs::metadata(t.root(New).join("mine/b.conf")).unwrap();
    assert_eq!(meta.mode() & 0o7777, 0o600);
}

#[test]
fn a_directory_the_user_removed_goes_but_keeps_what_the_transaction_added() {
    let t = T::new();
    for tree in [Base, New] {
        t.put(tree, "gone/a.conf", "a\n");
        t.put(tree, "gone-all/a.conf", "a\n");
    }
    t.put(New, "gone/added-by-tx", "n\n");
    fs::create_dir(t.root(Old).join("unrelated")).unwrap();
    let report = t.sync();
    assert!(!t.exists(New, "gone-all"));
    assert!(!t.exists(New, "gone/a.conf"));
    assert_eq!(t.read(New, "gone/added-by-tx").as_deref(), Some("n\n"));
    assert!(names(&report.deleted).contains(&"gone-all"));
}

#[test]
fn the_mode_the_user_gave_a_directory_is_carried_over() {
    let t = T::new();
    for tree in [Base, Old, New] {
        t.put(tree, "d/f", "x");
    }
    fs::set_permissions(t.root(Old).join("d"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(t.root(New).join("d"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(t.root(Base).join("d"), fs::Permissions::from_mode(0o755)).unwrap();
    t.sync();
    let mode = fs::metadata(t.root(New).join("d")).unwrap().mode() & 0o7777;
    assert_eq!(mode, 0o700);
}

#[test]
fn symlinks_are_compared_and_copied_by_target() {
    let t = T::new();
    for tree in [Base, New] {
        symlink("../usr/share/a", t.root(tree).join("localtime")).unwrap();
    }
    symlink("../usr/share/user", t.root(Old).join("localtime")).unwrap();
    symlink("target", t.root(Old).join("newlink")).unwrap();
    let report = t.sync();
    let target = |rel: &str| fs::read_link(t.root(New).join(rel)).unwrap();
    assert_eq!(target("localtime"), PathBuf::from("../usr/share/user"));
    assert_eq!(target("newlink"), PathBuf::from("target"));
    assert_eq!(names(&report.taken), ["localtime", "newlink"]);
}

#[test]
fn a_file_the_user_replaced_by_a_symlink_is_replaced() {
    let t = T::new();
    t.put(Base, "resolv.conf", "a\n");
    t.put(New, "resolv.conf", "a\n");
    symlink("/run/x", t.root(Old).join("resolv.conf")).unwrap();
    t.sync();
    assert_eq!(
        fs::read_link(t.root(New).join("resolv.conf")).unwrap(),
        PathBuf::from("/run/x")
    );
}

#[test]
fn accounts_changed_on_both_sides_are_merged_without_a_conflict() {
    let t = T::new();
    t.put(Base, "passwd", "root:x:0:0\n");
    t.put(Old, "passwd", "root:x:0:0\nvmafter:x:1000:1000\n");
    t.put(New, "passwd", "root:x:0:0\nsystemd-x:x:900:900\n");
    t.mode(New, "passwd", 0o644);
    let report = t.sync();
    assert_eq!(
        t.read(New, "passwd").as_deref(),
        Some("root:x:0:0\nsystemd-x:x:900:900\nvmafter:x:1000:1000\n")
    );
    assert_eq!(names(&report.merged), ["passwd"]);
    assert!(report.conflicts.is_empty());
    assert!(!t.kept().exists());
}

#[test]
fn an_account_file_only_the_user_changed_is_copied() {
    let t = T::new();
    t.put(Base, "group", "root:x:0:\n");
    t.put(New, "group", "root:x:0:\n");
    t.put(Old, "group", "root:x:0:\nme:x:1000:\n");
    let report = t.sync();
    assert_eq!(
        t.read(New, "group").as_deref(),
        Some("root:x:0:\nme:x:1000:\n")
    );
    assert_eq!(names(&report.taken), ["group"]);
}

#[test]
fn an_account_name_deeper_in_the_tree_is_a_plain_file() {
    let t = T::new();
    t.put_all("sub/passwd", "a\n", "user\n", "tx\n");
    let report = t.sync();
    assert_eq!(report.conflicts.len(), 1);
    assert!(report.merged.is_empty());
}

#[test]
fn a_fifo_is_skipped_and_noted() {
    let t = T::new();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &t.root(Old).join("pipe"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let report = t.sync();
    assert_eq!(names(&report.skipped), ["pipe"]);
    assert!(!t.exists(New, "pipe"));
}

#[test]
fn a_second_run_is_a_no_op() {
    let t = T::new();
    t.put_all("user-only", "a\n", "u\n", "a\n");
    t.put_all("both", "a\n", "u\n", "n\n");
    t.put(Base, "deleted", "a\n");
    t.put(New, "deleted", "a\n");
    t.put(Old, "added/x", "x\n");
    t.put(Base, "passwd", "r\n");
    t.put(Old, "passwd", "r\nu\n");
    t.put(New, "passwd", "r\nn\n");
    let first = t.sync();
    assert!(!first.is_empty());
    let after = fs::read_to_string(t.root(New).join("passwd")).unwrap();
    let second = t.sync();
    assert!(
        second.taken.is_empty() && second.deleted.is_empty(),
        "{second:?}"
    );
    // The passwd merge is stable and the conflict is reported again.
    assert_eq!(
        fs::read_to_string(t.root(New).join("passwd")).unwrap(),
        after
    );
    assert_eq!(second.conflicts.len(), 1);
    assert_eq!(t.kept_text("both").as_deref(), Some("u\n"));
}

#[test]
fn the_owner_is_carried_over() {
    let t = T::new();
    t.put(Old, "owned", "x");
    let report = t.sync();
    assert_eq!(names(&report.taken), ["owned"]);
    let (old, new) = (
        fs::metadata(t.root(Old).join("owned")).unwrap(),
        fs::metadata(t.root(New).join("owned")).unwrap(),
    );
    assert_eq!((old.uid(), old.gid()), (new.uid(), new.gid()));
}
