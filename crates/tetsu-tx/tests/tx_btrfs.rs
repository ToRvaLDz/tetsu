//! The transaction engine on a real Btrfs, with the real `btrfs`, `ukify`
//! and `sbsign`. `pacman`, `systemd-nspawn` and `mkinitcpio` are stubbed by
//! a runner that answers the package queries and writes the initrd; every
//! other program runs for real.
//!
//! Needs root and btrfs-progs, so every test is ignored by default and the
//! reason says how to run it. Inside the VM, or on a dev machine:
//!
//! ```sh
//! cargo test -p tetsu-tx --test tx_btrfs --no-run   # prints the binary
//! sudo <binary> --ignored --test-threads=1
//! ```
//!
//! Run as root a test fails, never skips, when a tool is missing. The kernel
//! is a stand-in: ukify insists on a PE image for `--linux`, so the systemd
//! EFI stub plays the part of `vmlinuz`.

mod common;

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use common::{LoopBtrfs, STUB_DIR, ToolRunner, make_keys};
use tetsu_tx::app::{Host, execute};
use tetsu_tx::boot::BootError;
use tetsu_tx::boot::esp::Esp;
use tetsu_tx::boot::keys::SigningKeys;
use tetsu_tx::boot::space::StatvfsProbe;
use tetsu_tx::boot::tool::{Runner, SystemRunner};
use tetsu_tx::boot::uki::verify_signature;
use tetsu_tx::cli::parse;
use tetsu_tx::store::StoreError;
use tetsu_tx::store::btrfs::{Btrfs, CliBtrfs};
use tetsu_tx::store::layout::Layout;
use tetsu_tx::store::lock::TxLock;
use tetsu_tx::store::record::{BootVerdict, Record, Records, TxKind};
use tetsu_tx::store::snapshot::{capture_etc_base, init_layout, mark_committed, seal};
use tetsu_tx::tx::TxError;
use tetsu_tx::tx::engine::{Engine, Paths, Request};

const KERNEL: &str = "6.1.0";
const INITRD: &str = "usr/lib/tetsu/initrd.img";

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

/// Answers `pacman`, `systemd-nspawn` and `mkinitcpio`; runs the rest.
struct StubRunner {
    tools: ToolRunner,
    seen: Cell<usize>,
    /// The call (counting from 0) that fails.
    fail_at: Cell<Option<usize>>,
    /// A `pacman -Su` ran: the installed set shows the new vim.
    upgraded: Cell<bool>,
    calls: RefCell<Vec<String>>,
}

impl StubRunner {
    fn new() -> Self {
        Self {
            tools: ToolRunner,
            seen: Cell::new(0),
            fail_at: Cell::new(None),
            upgraded: Cell::new(false),
            calls: RefCell::default(),
        }
    }

    fn query(&self, flag: &str) -> String {
        match flag {
            "-Qi" => QI.to_owned(),
            "-Qqe" => "tetsu-base\nvim\n".to_owned(),
            _ => {
                let vim = if self.upgraded.get() {
                    "9.1-1"
                } else {
                    "9.0-1"
                };
                format!("bash 5.2-1\ntetsu-base 1-1\nvim {vim}\n")
            }
        }
    }

    /// What runs inside the container: the command after the nspawn options.
    fn container(&self, words: &[String]) -> Result<String, BootError> {
        let at = words
            .iter()
            .position(|w| ["pacman", "mkinitcpio", "dkms"].contains(&w.as_str()));
        let command = at.map_or(&[][..], |i| &words[i..]);
        match command.first().map(String::as_str) {
            Some("mkinitcpio") => {
                let dir = words.iter().position(|w| w == "-D").unwrap();
                let initrd = Path::new(&words[dir + 1]).join(INITRD);
                fs::write(&initrd, b"rebuilt initrd").map_err(|e| BootError::Io {
                    path: initrd,
                    source: e,
                })?;
            }
            Some("pacman") if command.get(1).map(String::as_str) == Some("-Su") => {
                self.upgraded.set(true);
            }
            _ => {}
        }
        Ok(String::new())
    }

    fn route(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        let words: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        self.calls
            .borrow_mut()
            .push(format!("{program} {}", words.join(" ")));
        let n = self.seen.get();
        self.seen.set(n + 1);
        if self.fail_at.get() == Some(n) {
            return Err(BootError::Tool {
                tool: program.to_owned(),
                status: "exit status: 1".to_owned(),
                stderr: "injected failure".to_owned(),
            });
        }
        match program {
            "env" => Ok(self.query(words.last().map_or("", String::as_str))),
            "systemd-nspawn" => self.container(&words),
            _ => self.tools.output(program, args),
        }
    }
}

impl Runner for StubRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.route(program, args).map(drop)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.route(program, args)
    }
}

/// Btrfs that fails `sync`, which seals end with: the failure comes after
/// the root is read-only, so the cleanup has to unseal it.
struct FlakyBtrfs<'a> {
    inner: &'a dyn Btrfs,
    fail_sync: Cell<bool>,
}

impl Btrfs for FlakyBtrfs<'_> {
    fn create(&self, path: &Path) -> Result<(), StoreError> {
        self.inner.create(path)
    }

    fn snapshot(&self, src: &Path, dst: &Path, readonly: bool) -> Result<(), StoreError> {
        self.inner.snapshot(src, dst, readonly)
    }

    fn delete(&self, path: &Path) -> Result<(), StoreError> {
        self.inner.delete(path)
    }

    fn set_readonly(&self, path: &Path, readonly: bool) -> Result<(), StoreError> {
        self.inner.set_readonly(path, readonly)
    }

    fn sync(&self, path: &Path) -> Result<(), StoreError> {
        if self.fail_sync.get() {
            return Err(StoreError::Tool(BootError::Tool {
                tool: "btrfs".to_owned(),
                status: "exit status: 1".to_owned(),
                stderr: "injected failure".to_owned(),
            }));
        }
        self.inner.sync(path)
    }

    fn is_subvolume(&self, path: &Path) -> Result<bool, StoreError> {
        self.inner.is_subvolume(path)
    }
}

/// Everything on disk a failed transaction must leave alone.
#[derive(Debug, PartialEq, Eq)]
struct State {
    snapshots: Vec<u64>,
    esp: Vec<String>,
    records: Vec<String>,
    scratch: Vec<String>,
}

fn clock() -> u64 {
    1_700_000_000
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

/// A Btrfs holding transaction zero, an ESP and keys.
struct World {
    fs_: LoopBtrfs,
    layout: Layout,
    btrfs: CliBtrfs<SystemRunner>,
    records: Records,
    esp: Esp,
    keys: SigningKeys,
    lock: PathBuf,
    run_dir: PathBuf,
    cache: PathBuf,
}

impl World {
    /// Transaction zero booted and blessed good.
    fn new() -> Self {
        let fs_ = LoopBtrfs::new();
        let layout = Layout::new(&fs_.mnt);
        let btrfs = CliBtrfs::new(SystemRunner::new());
        let keys = make_keys(&fs_.work("keys"));
        let world = Self {
            layout,
            btrfs,
            records: Records::new(&fs_.work("records")),
            esp: Esp::open(&fs_.work("esp")),
            keys,
            lock: fs_.work("run").join("tx.lock"),
            run_dir: fs_.work("run"),
            cache: fs_.work("cache"),
            fs_,
        };
        world.transaction_zero();
        world
    }

    fn transaction_zero(&self) {
        let (layout, btrfs) = (&self.layout, &self.btrfs);
        assert_eq!(init_layout(layout, btrfs).unwrap(), 1);
        let root = layout.root(1);
        fs::create_dir_all(root.join("usr/lib/modules").join(KERNEL)).unwrap();
        fs::create_dir_all(root.join("usr/lib/tetsu")).unwrap();
        fs::copy(
            Path::new(STUB_DIR).join("linuxx64.efi.stub"),
            root.join("usr/lib/modules").join(KERNEL).join("vmlinuz"),
        )
        .unwrap();
        fs::write(root.join(INITRD), b"initrd").unwrap();
        fs::write(root.join("usr/lib/os-release"), "NAME=Tetsu\nID=tetsu\n").unwrap();
        fs::write(root.join("tag"), b"from-1").unwrap();
        fs::write(
            layout.etc(1).join("passwd"),
            "root:x:0:0:root:/root:/bin/bash\n",
        )
        .unwrap();
        fs::create_dir_all(layout.etc(1).join("kernel")).unwrap();
        fs::write(layout.etc(1).join("kernel/cmdline"), "quiet rw\n").unwrap();
        capture_etc_base(layout, btrfs, 1).unwrap();
        seal(layout, btrfs, 1).unwrap();
        mark_committed(layout, 1).unwrap();
        self.records
            .write(&Record {
                counter: 1,
                boot: Some(BootVerdict::Good),
                ..Record::default()
            })
            .unwrap();
        fs::create_dir_all(self.esp.dir()).unwrap();
        fs::write(self.esp.dir().join("tetsu-1.efi"), b"uki").unwrap();
    }

    fn engine<'a>(
        &'a self,
        runner: &'a dyn Runner,
        btrfs: &'a dyn Btrfs,
        booted: u64,
    ) -> Engine<'a> {
        Engine {
            layout: self.layout.clone(),
            records: self.records.clone(),
            esp: self.esp.clone(),
            keys: self.keys.clone(),
            paths: Paths {
                lock: self.lock.clone(),
                run_dir: self.run_dir.clone(),
                cache: self.cache.clone(),
            },
            booted,
            default_cmdline: "quiet rw".to_owned(),
            runner,
            btrfs,
            probe: &StatvfsProbe,
            clock: &clock,
        }
    }

    fn state(&self) -> State {
        State {
            snapshots: self.layout.list_snapshots().unwrap(),
            esp: names(self.esp.dir()),
            records: names(self.records.dir()),
            scratch: names(&self.run_dir)
                .into_iter()
                .filter(|n| n.starts_with("tx-var-"))
                .collect(),
        }
    }

    /// Simulate booting `counter` and `systemd-bless-boot`: the entry loses
    /// its counting suffix and the record gets its verdict.
    fn bless(&self, counter: u64) {
        let state = self.esp.scan().unwrap();
        let entry = state
            .entries
            .iter()
            .find(|e| e.name.counter == counter)
            .expect("entry to bless");
        fs::rename(
            &entry.path,
            self.esp.dir().join(format!("tetsu-{counter}.efi")),
        )
        .unwrap();
        self.records.set_boot(counter, BootVerdict::Good).unwrap();
    }

    fn assert_signed(&self, entry: &str) {
        let path = self.esp.dir().join(entry);
        let cert = self.fs_.work("keys").join("db.crt");
        verify_signature(&ToolRunner, &path, &cert)
            .unwrap_or_else(|e| panic!("{entry} is not signed by the db key: {e}"));
    }

    fn assert_sealed(&self, counter: u64) {
        let err = fs::write(self.layout.root(counter).join("usr/new"), b"x").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ReadOnlyFilesystem, "{err:?}");
        // The nested etc stays writable.
        fs::write(self.layout.etc(counter).join("motd"), b"hi\n").unwrap();
    }

    fn list_json(&self, booted: u64) -> serde_json::Value {
        let args: Vec<String> = [
            "--top",
            self.fs_.mnt.to_str().unwrap(),
            "--esp",
            self.fs_.work("esp").to_str().unwrap(),
            "--records",
            self.records.dir().to_str().unwrap(),
            "--booted",
            &booted.to_string(),
            "list",
            "--json",
        ]
        .map(str::to_owned)
        .to_vec();
        let invocation = parse(&args).unwrap();
        let stub = StubRunner::new();
        let host = Host {
            runner: &stub,
            btrfs: &self.btrfs,
            probe: &StatvfsProbe,
            proc_cmdline: "",
            clock: &clock,
        };
        let out = execute(&invocation, &host);
        assert_eq!(out.code, 0, "{out:?}");
        serde_json::from_str(&out.stdout).unwrap()
    }
}

#[test]
#[ignore = "needs root, btrfs-progs, ukify and sbsigntools: run the binary with sudo and --ignored (see the file header)"]
fn update_and_rollback_commit_signed_snapshots() {
    let world = World::new();
    let stub = StubRunner::new();

    // Update: snapshot 2, sealed, with a signed UKI and its record.
    let report = world
        .engine(&stub, &world.btrfs, 1)
        .run(&Request::Update)
        .unwrap();
    assert_eq!(report.counter, 2, "{report:?}");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.changes.len(), 1);
    assert_eq!(world.state().snapshots, [1, 2]);
    assert_eq!(names(world.esp.dir()), ["tetsu-1.efi", "tetsu-2+3.efi"]);
    world.assert_signed("tetsu-2+3.efi");
    world.assert_sealed(2);
    assert!(world.layout.is_committed(2));
    assert_eq!(
        fs::read(world.layout.root(2).join(INITRD)).unwrap(),
        b"rebuilt initrd"
    );
    let record = world.records.load().unwrap().remove(&2).unwrap();
    assert_eq!(record.kind, Some(TxKind::Update));
    assert_eq!(record.source, Some(1));
    assert!(world.layout.root(2).join("tag").is_file());

    // Boot 2, add an account there, then roll back to 1.
    world.bless(2);
    fs::write(
        world.layout.etc(2).join("passwd"),
        "root:x:0:0:root:/root:/bin/zsh\nmarco:x:1000:1000::/home/marco:/bin/bash\n",
    )
    .unwrap();
    let report = world
        .engine(&stub, &world.btrfs, 2)
        .run(&Request::Rollback(Some(1)))
        .unwrap();
    assert_eq!(report.counter, 3, "{report:?}");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        fs::read_to_string(world.layout.root(3).join("tag")).unwrap(),
        "from-1"
    );
    assert_eq!(
        fs::read(world.layout.root(3).join(INITRD)).unwrap(),
        b"initrd"
    );
    assert_eq!(
        fs::read_to_string(world.layout.etc(3).join("passwd")).unwrap(),
        "root:x:0:0:root:/root:/bin/zsh\nmarco:x:1000:1000::/home/marco:/bin/bash\n"
    );
    world.assert_signed("tetsu-3+3.efi");
    world.assert_sealed(3);
    let record = world.records.load().unwrap().remove(&3).unwrap();
    assert_eq!(record.kind, Some(TxKind::Rollback));
    assert_eq!(record.source, Some(1));
    assert_eq!(record.rolled_back_from, Some(2));

    // `list` shows the three, with the booted and default marks.
    let listed = world.list_json(2);
    let rows = listed.as_array().unwrap();
    let counters: Vec<u64> = rows
        .iter()
        .map(|r| r["counter"].as_u64().unwrap())
        .collect();
    assert_eq!(counters, [1, 2, 3]);
    assert_eq!(rows[1]["booted"], true);
    assert_eq!(rows[2]["default"], true);
    assert_eq!(rows[2]["kind"], "rollback");
    assert_eq!(rows[1]["boot"], "good");
}

#[test]
#[ignore = "needs root, btrfs-progs, ukify and sbsigntools: run the binary with sudo and --ignored (see the file header)"]
fn a_failure_in_any_pre_commit_step_leaves_the_system_as_it_was() {
    let world = World::new();
    let before = world.state();

    // The seal fails after the root went read-only: the cleanup unseals it.
    let flaky = FlakyBtrfs {
        inner: &world.btrfs,
        fail_sync: Cell::new(true),
    };
    let stub = StubRunner::new();
    let err = world
        .engine(&stub, &flaky, 1)
        .run(&Request::Update)
        .unwrap_err();
    assert!(matches!(err, TxError::Failed { .. }), "{err:?}");
    assert_eq!(world.state(), before, "seal failure");
    TxLock::acquire(&world.lock).unwrap();

    // Fail each runner call in turn; the first call that is not reached
    // makes the run succeed, which ends the loop.
    let mut at = 0;
    loop {
        let stub = StubRunner::new();
        stub.fail_at.set(Some(at));
        let result = world.engine(&stub, &world.btrfs, 1).run(&Request::Update);
        if result.is_ok() {
            assert!(at >= 5, "only {at} calls before the commit");
            break;
        }
        let call = stub.calls.borrow().last().cloned().unwrap_or_default();
        assert_eq!(world.state(), before, "call {at}: {call}");
        TxLock::acquire(&world.lock).unwrap();
        at += 1;
    }
    assert_eq!(world.state().snapshots, [1, 2]);

    // The same for a rollback: its only call is ukify.
    world.bless(2);
    let before = world.state();
    let stub = StubRunner::new();
    stub.fail_at.set(Some(0));
    let err = world
        .engine(&stub, &world.btrfs, 2)
        .run(&Request::Rollback(Some(1)))
        .unwrap_err();
    assert!(matches!(err, TxError::Failed { .. }), "{err:?}");
    assert_eq!(world.state(), before, "rollback");
    TxLock::acquire(&world.lock).unwrap();
}
