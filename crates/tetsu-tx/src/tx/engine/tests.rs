//! Engine tests over a fake runner, a fake Btrfs that copies files, a
//! temporary ESP and a fake space probe.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;
use crate::boot::BootError;
use crate::boot::keys::KeyRef;
use crate::store::btrfs::tests::FakeBtrfs;
use crate::store::lock::TxLock;
use crate::store::record::{BootVerdict, Record};
use crate::tx::fake::{Reply, ScriptedRunner};

const QI: &str = "\
Name            : tetsu-base
Version         : 1-1
Depends On      : bash
Provides        : None

Name            : bash
Version         : 5.2-1
Depends On      : None
Provides        : None

Name            : vim
Version         : 9.0-1
Depends On      : None
Provides        : None
";
const QQE: &str = "tetsu-base\nvim\n";
const AFTER: &str = "bash 5.2-1\ntetsu-base 1-1\nvim 9.1-1\n";
const INITRD: &str = "usr/lib/tetsu/initrd.img";

struct Plenty;

impl SpaceProbe for Plenty {
    fn available_bytes(&self, _: &Path) -> Result<u64, BootError> {
        Ok(1 << 40)
    }
}

/// Everything on disk a failed transaction must leave alone.
#[derive(Debug, PartialEq, Eq)]
struct State {
    snapshots: Vec<u64>,
    esp: Vec<String>,
    records: Vec<String>,
}

struct Env {
    _tmp: tempfile::TempDir,
    layout: Layout,
    records: Records,
    esp: Esp,
    btrfs: FakeBtrfs,
    lock: PathBuf,
    run_dir: PathBuf,
    cache: PathBuf,
    runner: ScriptedRunner,
    booted: u64,
    clock: Box<dyn Fn() -> u64>,
    /// Runs when the fake ukify has written its output.
    after_ukify: Rc<RefCell<Box<dyn Fn()>>>,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The directory after `-D` in a container command line.
fn container_root(line: &[String]) -> PathBuf {
    let at = line.iter().position(|a| a == "-D").unwrap();
    PathBuf::from(&line[at + 1])
}

/// The scratch `/var` bound into a container command line.
fn container_scratch(line: &[String]) -> PathBuf {
    let bind = line
        .iter()
        .find_map(|a| a.strip_prefix("--bind=")?.strip_suffix(":/var"))
        .unwrap();
    PathBuf::from(bind)
}

impl Env {
    /// Snapshot 1 booted and blessed good; `qi` and `after` are what the
    /// host queries before and after the package phase.
    fn with(qi: &str, after: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().join("top");
        fs::create_dir(&top).unwrap();
        let layout = Layout::new(&top);
        let btrfs = FakeBtrfs::new(&top);
        btrfs.copy_files.set(true);
        let esp_root = tmp.path().join("esp");
        fs::create_dir_all(esp_root.join("EFI/Linux")).unwrap();
        let tick = Cell::new(1000);
        let runner = ScriptedRunner::new();
        runner.on_tail(&["-Qi"], Reply::Out(qi.into()));
        runner.on_tail(&["-Qqe"], Reply::Out(QQE.into()));
        runner.on_tail(&["-Q"], Reply::Out(after.into()));
        let after_ukify: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));
        let hook = Rc::clone(&after_ukify);
        runner.on_effect(&["ukify"], move |line| {
            let out = line
                .iter()
                .find_map(|a| a.strip_prefix("--output="))
                .unwrap();
            fs::write(out, b"uki").unwrap();
            (hook.borrow())();
        });
        let ver = ["mkinitcpio", "-k", "6.1", "-g", "/usr/lib/tetsu/initrd.img"];
        runner.on_tail_effect(&ver, |line| {
            write(&container_root(line).join(INITRD), "rebuilt");
        });
        let env = Self {
            records: Records::new(&tmp.path().join("records")),
            esp: Esp::open(&esp_root),
            lock: tmp.path().join("run/tx.lock"),
            run_dir: tmp.path().join("run"),
            cache: tmp.path().join("cache"),
            layout,
            btrfs,
            runner,
            booted: 1,
            clock: Box::new(move || {
                tick.set(tick.get() + 1);
                tick.get()
            }),
            after_ukify,
            _tmp: tmp,
        };
        env.add_snapshot(1, "root:x:0:0:root:/root:/bin/bash\n");
        env
    }

    fn new() -> Self {
        Self::with(QI, AFTER)
    }

    /// A committed, blessed snapshot with a kernel, an initrd and a marker
    /// file naming it.
    fn add_snapshot(&self, n: u64, passwd: &str) {
        let root = self.layout.root(n);
        self.btrfs.adopt(&root);
        write(&root.join("usr/lib/modules/6.1/vmlinuz"), "kernel");
        write(&root.join(INITRD), &format!("initrd-{n}"));
        write(&root.join("usr/lib/os-release"), "NAME=Test\n");
        write(&root.join("tag"), &format!("from-{n}"));
        self.btrfs.adopt(&self.layout.etc(n));
        write(&self.layout.etc(n).join("passwd"), passwd);
        write(&self.layout.etc(n).join("kernel/cmdline"), "quiet\n");
        self.btrfs.adopt(&self.layout.etc_base(n));
        self.btrfs.set_readonly(&root, true).unwrap();
        write(&self.layout.committed_marker(n), "");
        self.records
            .write(&Record {
                counter: n,
                boot: Some(BootVerdict::Good),
                ..Record::default()
            })
            .unwrap();
        write(&self.esp.dir().join(format!("tetsu-{n}.efi")), "uki");
        self.btrfs.take_ops();
    }

    /// Snapshot 1 and a booted snapshot 2 that has a user account.
    fn two_snapshots() -> Self {
        let mut env = Self::new();
        env.add_snapshot(
            2,
            "root:x:0:0:root:/root:/bin/zsh\nmarco:x:1000:1000::/home/marco:/bin/bash\n",
        );
        env.booted = 2;
        env
    }

    fn engine<'a>(&'a self, runner: &'a dyn Runner) -> Engine<'a> {
        Engine {
            layout: self.layout.clone(),
            records: self.records.clone(),
            esp: self.esp.clone(),
            keys: SigningKeys {
                db_key: KeyRef::File("/k".into()),
                db_cert: "/c".into(),
                pcr_key: None,
                pcr_public: None,
            },
            paths: Paths {
                lock: self.lock.clone(),
                run_dir: self.run_dir.clone(),
                cache: self.cache.clone(),
            },
            booted: self.booted,
            default_cmdline: "fallback".into(),
            runner,
            btrfs: &self.btrfs,
            probe: &Plenty,
            clock: &*self.clock,
        }
    }

    fn run(&self, request: &Request) -> Result<Report, TxError> {
        self.engine(&self.runner).run(request)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .map(|r| {
                r.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn state(&self) -> State {
        State {
            snapshots: self.layout.list_snapshots().unwrap(),
            esp: Self::names(self.esp.dir()),
            records: Self::names(self.records.dir()),
        }
    }

    fn scratch_dirs(&self) -> Vec<String> {
        Self::names(&self.run_dir)
            .into_iter()
            .filter(|n| n.starts_with("tx-var-"))
            .collect()
    }

    fn record(&self, n: u64) -> Record {
        self.records.load().unwrap().remove(&n).unwrap()
    }

    fn calls(&self, program: &str) -> Vec<String> {
        self.runner
            .lines()
            .into_iter()
            .filter(|l| l.split(' ').next() == Some(program))
            .collect()
    }

    /// The system is as it was and the next transaction can start.
    fn assert_unchanged(&self, before: &State, what: &str) {
        assert_eq!(&self.state(), before, "{what}");
        assert!(self.scratch_dirs().is_empty(), "{what}: scratch left");
        TxLock::acquire(&self.lock).unwrap_or_else(|e| panic!("{what}: lock held: {e}"));
    }
}

fn pkgs(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

fn change(name: &str, from: Option<&str>, to: Option<&str>) -> PackageChange {
    PackageChange {
        name: name.into(),
        from: from.map(str::to_owned),
        to: to.map(str::to_owned),
    }
}

fn failure(e: TxError) -> (String, Option<String>) {
    match e {
        TxError::Failed {
            reason, proposal, ..
        } => (reason, proposal),
        other => panic!("{other:?}"),
    }
}

/// Fails the `at`-th call (counting from 0) and forwards the others.
struct FailAt<'a> {
    inner: &'a ScriptedRunner,
    at: usize,
    seen: Cell<usize>,
}

impl FailAt<'_> {
    fn step(&self, program: &str) -> Result<(), BootError> {
        let n = self.seen.get();
        self.seen.set(n + 1);
        if n == self.at {
            return Err(BootError::Tool {
                tool: program.to_owned(),
                status: "exit status: 1".to_owned(),
                stderr: "injected failure".to_owned(),
            });
        }
        Ok(())
    }
}

impl Runner for FailAt<'_> {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.step(program)?;
        self.inner.run(program, args)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.step(program)?;
        self.inner.output(program, args)
    }

    fn check_tool(&self, program: &str) -> Result<(), BootError> {
        self.inner.check_tool(program)
    }
}

#[test]
fn update_commits_the_next_snapshot() {
    let env = Env::new();
    let report = env.run(&Request::Update).unwrap();
    assert_eq!(report.counter, 2);
    assert_eq!(report.kind, TxKind::Update);
    assert!(report.packages.is_empty() && report.warnings.is_empty());
    assert_eq!(
        report.changes,
        [change("vim", Some("9.0-1"), Some("9.1-1"))]
    );
    assert_eq!(env.layout.list_snapshots().unwrap(), [1, 2]);
    assert!(env.layout.is_committed(2));
    assert_eq!(env.btrfs.readonly(&env.layout.root(2)), Some(true));
    assert_eq!(Env::names(env.esp.dir()), ["tetsu-1.efi", "tetsu-2+3.efi"]);
    let record = env.record(2);
    assert_eq!(record.kind, Some(TxKind::Update));
    assert_eq!(record.source, Some(1));
    assert_eq!(record.changes, report.changes);
    assert_eq!(record.boot, None);
    assert!(record.started_at < record.finished_at && record.started_at.is_some());
    assert_eq!(
        fs::read_to_string(env.layout.root(2).join(INITRD)).unwrap(),
        "rebuilt"
    );
    let ukify = env.calls("ukify").join("\n");
    assert!(
        ukify.contains("--cmdline=quiet rootflags=subvol=@snapshots/2/root"),
        "{ukify}"
    );
    assert!(env.scratch_dirs().is_empty());
}

#[test]
fn update_starts_with_the_two_keyring_calls() {
    let env = Env::new();
    env.run(&Request::Update).unwrap();
    let pacman: Vec<String> = env
        .calls("systemd-nspawn")
        .into_iter()
        .filter_map(|l| l.split(" pacman ").nth(1).map(str::to_owned))
        .collect();
    assert_eq!(
        pacman[..4],
        [
            "-Syw --noconfirm archlinux-keyring",
            "-S --noconfirm --needed archlinux-keyring",
            "-Suw --noconfirm",
            "-Su --noconfirm",
        ]
    );
    assert_eq!(pacman[4], "-Qdtq");
}

#[test]
fn a_cmdline_missing_from_the_snapshot_falls_back_to_the_default() {
    let env = Env::new();
    fs::remove_file(env.layout.etc(1).join("kernel/cmdline")).unwrap();
    env.run(&Request::Update).unwrap();
    let ukify = env.calls("ukify").join("\n");
    assert!(
        ukify.contains("--cmdline=fallback rootflags=subvol=@snapshots/2/root"),
        "{ukify}"
    );
}

#[test]
fn install_records_the_packages_and_their_changes() {
    let after = format!("{AFTER}htop 3-1\n");
    let env = Env::with(QI, &after);
    let report = env.run(&Request::Install(pkgs(&["htop"]))).unwrap();
    assert_eq!(report.kind, TxKind::Install);
    assert_eq!(report.packages, ["htop"]);
    assert_eq!(
        report.changes,
        [
            change("htop", None, Some("3-1")),
            change("vim", Some("9.0-1"), Some("9.1-1"))
        ]
    );
    let record = env.record(2);
    assert_eq!(record.kind, Some(TxKind::Install));
    assert_eq!(record.packages, ["htop"]);
    assert!(Env::names(env.esp.dir()).contains(&"tetsu-2+3.efi".to_owned()));
    let lines = env.calls("systemd-nspawn").join("\n");
    assert!(lines.contains("pacman -Si htop"), "{lines}");
    assert!(
        lines.contains("pacman -S --noconfirm --needed htop"),
        "{lines}"
    );
}

#[test]
fn remove_drops_a_user_package() {
    let env = Env::with(QI, "bash 5.2-1\ntetsu-base 1-1\n");
    let report = env.run(&Request::Remove(pkgs(&["vim"]))).unwrap();
    assert_eq!(report.kind, TxKind::Remove);
    assert_eq!(report.changes, [change("vim", Some("9.0-1"), None)]);
    assert_eq!(env.record(2).kind, Some(TxKind::Remove));
    assert!(
        env.calls("systemd-nspawn")
            .join("\n")
            .contains("pacman -Rns --noconfirm vim")
    );
}

#[test]
fn removing_a_base_package_is_not_a_user_package() {
    let env = Env::new();
    let before = env.state();
    let e = env.run(&Request::Remove(pkgs(&["bash"]))).unwrap_err();
    assert!(
        matches!(e, TxError::NotUserPackage(ref p) if p == "bash"),
        "{e:?}"
    );
    env.assert_unchanged(&before, "remove base package");
    assert!(!env.runner.lines().join("\n").contains("-Rns"));
}

#[test]
fn installing_an_unknown_package_is_not_found() {
    let env = Env::new();
    let before = env.state();
    env.runner.on_tail(
        &["pacman", "-Si", "nope"],
        Reply::Fail("error: package 'nope' was not found".into()),
    );
    let e = env.run(&Request::Install(pkgs(&["nope"]))).unwrap_err();
    assert!(
        matches!(e, TxError::PackageNotFound(ref p) if p == "nope"),
        "{e:?}"
    );
    env.assert_unchanged(&before, "install unknown package");
}

#[test]
fn a_package_that_would_replace_the_base_names_it() {
    let env = Env::with(QI, "tetsu-base 1-1\nvim 9.0-1\nfoo 1-1\n");
    let before = env.state();
    let (reason, _) = failure(env.run(&Request::Install(pkgs(&["foo"]))).unwrap_err());
    assert!(reason.contains("bash"), "{reason}");
    env.assert_unchanged(&before, "base conflict");
}

#[test]
fn install_and_remove_need_tetsu_base() {
    let qi = QI.replace("tetsu-base", "something-else");
    for request in [
        Request::Install(pkgs(&["htop"])),
        Request::Remove(pkgs(&["vim"])),
    ] {
        let env = Env::with(&qi, AFTER);
        let before = env.state();
        let (reason, proposal) = failure(env.run(&request).unwrap_err());
        assert!(reason.contains("tetsu-base"), "{reason}");
        assert!(proposal.is_some_and(|p| p.contains("tetsu-base")));
        env.assert_unchanged(&before, "no tetsu-base");
    }
}

#[test]
fn an_update_without_tetsu_base_goes_on_unguarded() {
    let qi = QI.replace("tetsu-base", "something-else");
    let env = Env::with(&qi, "vim 9.1-1\n");
    assert_eq!(env.run(&Request::Update).unwrap().counter, 2);
}

#[test]
fn an_update_blocked_by_a_user_package_proposes_removing_it() {
    let env = Env::new();
    let before = env.state();
    env.runner.on_tail(
        &["pacman", "-Su", "--noconfirm"],
        Reply::Fail(
            "error: failed to prepare transaction (could not satisfy dependencies)\n\
             :: installing foo (2-1) breaks dependency 'foo<2' required by vim"
                .into(),
        ),
    );
    let (reason, proposal) = failure(env.run(&Request::Update).unwrap_err());
    assert!(reason.contains("update failed"), "{reason}");
    assert_eq!(proposal.as_deref(), Some("tx remove vim, then tx update"));
    env.assert_unchanged(&before, "blocked update");
}

#[test]
fn an_update_blocked_by_a_user_dkms_package_proposes_removing_it() {
    let env = Env::new();
    write(&env.layout.root(1).join("usr/bin/dkms"), "#!/bin/sh");
    let build = [
        "dkms",
        "autoinstall",
        "-k",
        "6.1",
        "--dkmstree",
        "/usr/lib/dkms",
    ];
    env.runner
        .on_tail(&build, Reply::Fail("Error! Bad return status".into()));
    env.runner.on_tail(
        &["dkms", "status", "-k", "6.1"],
        Reply::Out("foo/1.0, 6.1, x86_64: built\n".into()),
    );
    env.runner.on_tail(
        &["pacman", "-Qqo", "/usr/src/foo-1.0"],
        Reply::Out("vim\n".into()),
    );
    let before = env.state();
    let (reason, proposal) = failure(env.run(&Request::Update).unwrap_err());
    assert!(reason.contains("foo/1.0"), "{reason}");
    assert_eq!(proposal.as_deref(), Some("tx remove vim, then tx update"));
    env.assert_unchanged(&before, "dkms blocked update");
}

#[test]
fn dkms_runs_for_the_new_kernel_when_the_snapshot_has_it() {
    let env = Env::new();
    write(&env.layout.root(1).join("usr/bin/dkms"), "#!/bin/sh");
    let report = env.run(&Request::Update).unwrap();
    assert!(
        report.log.iter().any(|l| l.contains("DKMS")),
        "{:?}",
        report.log
    );
    assert!(env.record(2).log.iter().any(|l| l.contains("DKMS")));
}

/// Fail each runner call of a happy run in turn: nothing may change.
fn every_failure_leaves_the_system_unchanged(make: impl Fn() -> Env, request: &Request) {
    let probe = make();
    probe.run(request).unwrap();
    let total = probe.runner.lines().len();
    assert!(total > 0);
    for at in 0..total {
        let env = make();
        let before = env.state();
        let failing = FailAt {
            inner: &env.runner,
            at,
            seen: Cell::new(0),
        };
        let result = env.engine(&failing).run(request);
        let what = format!(
            "{request:?}, call {at} of {total} ({})",
            probe.runner.lines()[at]
        );
        assert!(result.is_err(), "{what}: succeeded");
        env.assert_unchanged(&before, &what);
    }
}

#[test]
fn a_failure_at_any_update_call_leaves_the_system_unchanged() {
    every_failure_leaves_the_system_unchanged(Env::new, &Request::Update);
}

#[test]
fn a_failure_at_any_install_call_leaves_the_system_unchanged() {
    let make = || Env::with(QI, &format!("{AFTER}htop 3-1\n"));
    every_failure_leaves_the_system_unchanged(make, &Request::Install(pkgs(&["htop"])));
}

#[test]
fn a_failure_at_any_remove_call_leaves_the_system_unchanged() {
    let make = || Env::with(QI, "bash 5.2-1\ntetsu-base 1-1\n");
    every_failure_leaves_the_system_unchanged(make, &Request::Remove(pkgs(&["vim"])));
}

#[test]
fn a_failure_at_any_rollback_call_leaves_the_system_unchanged() {
    every_failure_leaves_the_system_unchanged(Env::two_snapshots, &Request::Rollback(Some(1)));
}

#[test]
fn a_btrfs_failure_before_the_commit_leaves_the_system_unchanged() {
    for op in ["snapshot", "sync"] {
        let env = Env::new();
        let before = env.state();
        *env.btrfs.fail.borrow_mut() = Some(op);
        assert!(env.run(&Request::Update).is_err(), "{op}");
        env.assert_unchanged(&before, op);
    }
}

#[test]
fn a_running_transaction_makes_this_one_busy() {
    let env = Env::new();
    let before = env.state();
    let held = TxLock::acquire(&env.lock).unwrap();
    assert!(matches!(
        env.run(&Request::Update),
        Err(TxError::InProgress)
    ));
    drop(held);
    assert_eq!(env.state(), before);
}

#[test]
fn an_empty_package_list_fails_before_anything_starts() {
    let env = Env::new();
    for request in [Request::Install(vec![]), Request::Remove(vec![])] {
        assert!(matches!(env.run(&request), Err(TxError::Failed { .. })));
    }
    assert!(env.runner.lines().is_empty());
}

#[test]
fn the_var_report_goes_into_the_log_and_the_record() {
    let env = Env::new();
    env.runner
        .on_tail_effect(&["pacman", "-Su", "--noconfirm"], |line| {
            write(&container_scratch(line).join("lib/foo/state"), "x");
        });
    let report = env.run(&Request::Update).unwrap();
    assert!(
        report.log.iter().any(|l| l.contains("/var/lib/foo/state")),
        "{:?}",
        report.log
    );
    assert_eq!(env.record(2).log, report.log);
}

#[test]
fn stale_scratch_dirs_are_swept_under_the_lock() {
    let env = Env::new();
    write(&env.run_dir.join("tx-var-77/leftover"), "x");
    write(&env.run_dir.join("keep-me/file"), "x");
    env.run(&Request::Update).unwrap();
    assert_eq!(Env::names(&env.run_dir), ["keep-me", "tx.lock"]);
}

#[test]
fn failures_after_the_commit_are_warnings() {
    let env = Env::new();
    let blocker = env.records.path(2);
    // The record cannot be written: its name is taken by a directory.
    *env.after_ukify.borrow_mut() = Box::new(move || fs::create_dir_all(&blocker).unwrap());
    let report = env.run(&Request::Update).unwrap();
    assert_eq!(report.counter, 2);
    assert!(!report.warnings.is_empty());
    assert!(env.layout.is_committed(2));
    assert!(Env::names(env.esp.dir()).contains(&"tetsu-2+3.efi".to_owned()));
}

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

#[test]
fn the_scratch_dirs_follow_the_nspawn_prefix() {
    assert_eq!(
        scratch_dir(Path::new(DEFAULT_RUN_DIR), 5),
        crate::tx::nspawn::scratch_dir(5)
    );
}

#[test]
fn every_container_binds_the_configured_cache_including_the_initrd_build() {
    let env = Env::new();
    env.run(&Request::Update).unwrap();
    let bind = format!("--bind={}:/var/cache/pacman/pkg", env.cache.display());
    let lines = env.calls("systemd-nspawn");
    assert!(!lines.is_empty());
    for l in &lines {
        assert!(l.contains(&bind), "{l}");
    }
    assert!(lines.iter().any(|l| l.contains("mkinitcpio")));
}
