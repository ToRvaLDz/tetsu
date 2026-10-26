//! Rollback transactions.

use super::tests::{Env, INITRD, failure};
use super::*;
use std::fs;

#[test]
fn rollback_builds_the_new_snapshot_from_the_target() {
    let env = Env::two_snapshots();
    let report = env.run(&Request::Rollback(Some(1))).unwrap();
    assert_eq!(report.counter, 3);
    assert_eq!(report.kind, TxKind::Rollback);
    assert!(report.changes.is_empty());
    assert_eq!(
        fs::read_to_string(env.layout.root(3).join("tag")).unwrap(),
        "from-1"
    );
    assert_eq!(
        fs::read_to_string(env.layout.root(3).join(INITRD)).unwrap(),
        "initrd-1"
    );
    assert!(env.layout.is_committed(3));
    assert!(Env::names(env.esp.dir()).contains(&"tetsu-3+3.efi".to_owned()));
    let record = env.record(3);
    assert_eq!(record.kind, Some(TxKind::Rollback));
    assert_eq!(record.source, Some(1));
    assert_eq!(record.rolled_back_from, Some(2));
}

#[test]
fn rollback_runs_no_pacman_and_no_initrd_build() {
    let env = Env::two_snapshots();
    env.run(&Request::Rollback(Some(1))).unwrap();
    let lines = env.runner.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("ukify "), "{lines:?}");
}

#[test]
fn rollback_without_a_target_goes_to_the_last_good_snapshot() {
    let env = Env::two_snapshots();
    let report = env.run(&Request::Rollback(None)).unwrap();
    assert_eq!(report.counter, 3);
    assert_eq!(env.record(3).source, Some(1));
}

#[test]
fn rollback_keeps_the_accounts_of_the_running_system() {
    let env = Env::two_snapshots();
    env.run(&Request::Rollback(Some(1))).unwrap();
    let passwd = fs::read_to_string(env.layout.etc(3).join("passwd")).unwrap();
    assert_eq!(
        passwd,
        "root:x:0:0:root:/root:/bin/zsh\nmarco:x:1000:1000::/home/marco:/bin/bash\n"
    );
    // The rest of /etc is the target's, with no general sync.
    assert_eq!(
        fs::read_to_string(env.layout.etc(3).join("kernel/cmdline")).unwrap(),
        "quiet\n"
    );
}

#[test]
fn rollback_to_an_unknown_or_uncommitted_snapshot_is_not_found() {
    let env = Env::two_snapshots();
    // Snapshot 7 has a record (so it is no orphan) but was never committed.
    env.btrfs.adopt(&env.layout.root(7));
    env.records
        .write(&Record {
            counter: 7,
            ..Record::default()
        })
        .unwrap();
    let before = env.state();
    for target in [9, 7] {
        let e = env.run(&Request::Rollback(Some(target))).unwrap_err();
        assert!(
            matches!(e, TxError::SnapshotNotFound(n) if n == target),
            "{e:?}"
        );
    }
    env.assert_unchanged(&before, "unknown target");
    assert!(env.runner.lines().is_empty());
}

#[test]
fn rollback_with_no_good_older_snapshot_is_not_found() {
    let env = Env::new();
    let before = env.state();
    let e = env.run(&Request::Rollback(None)).unwrap_err();
    assert!(matches!(e, TxError::SnapshotNotFound(_)), "{e:?}");
    env.assert_unchanged(&before, "no target");
}

#[test]
fn rollback_to_a_snapshot_without_an_initrd_fails_cleanly() {
    let env = Env::two_snapshots();
    fs::remove_file(env.layout.root(1).join(INITRD)).unwrap();
    let before = env.state();
    let (reason, _) = failure(env.run(&Request::Rollback(Some(1))).unwrap_err());
    assert!(reason.contains("initrd"), "{reason}");
    env.assert_unchanged(&before, "missing initrd");
    assert!(env.runner.lines().is_empty());
}
