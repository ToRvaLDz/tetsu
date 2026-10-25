//! Snapshot store on a real Btrfs: a sparse image, `mkfs.btrfs`, a loop
//! mount of the top level (subvolid=5).
//!
//! Needs root and btrfs-progs, so every test is ignored by default and the
//! reason says how to run it. Inside the VM, or on a dev machine:
//!
//! ```sh
//! cargo test -p tetsu-tx --test store_btrfs --no-run   # prints the binary
//! sudo <binary> --ignored --test-threads=1
//! ```
//!
//! Run as root a test fails, never skips, when a tool is missing.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use tetsu_tx::boot::esp::{Esp, EspState};
use tetsu_tx::boot::tool::SystemRunner;
use tetsu_tx::store::btrfs::{Btrfs, CliBtrfs};
use tetsu_tx::store::layout::Layout;
use tetsu_tx::store::lock::TxLock;
use tetsu_tx::store::record::{BootVerdict, Records};
use tetsu_tx::store::snapshot::{
    capture_etc_base, delete_snapshot, init_layout, mark_committed, seal,
};
use tetsu_tx::store::{Store, StoreError};

const IMAGE_BYTES: u64 = 256 * 1024 * 1024;

/// A loop-mounted Btrfs top level, unmounted on drop.
struct LoopBtrfs {
    dir: TempDir,
    mnt: PathBuf,
}

impl LoopBtrfs {
    fn new() -> Self {
        assert!(
            is_root(),
            "the loopback Btrfs tests must run as root (sudo)"
        );
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("btrfs.img");
        fs::File::create(&image)
            .unwrap()
            .set_len(IMAGE_BYTES)
            .unwrap();
        run("mkfs.btrfs", &["-q".as_ref(), image.as_os_str()]);
        let mnt = dir.path().join("top");
        fs::create_dir(&mnt).unwrap();
        run(
            "mount",
            &[
                "-o".as_ref(),
                "loop,subvolid=5".as_ref(),
                image.as_os_str(),
                mnt.as_os_str(),
            ],
        );
        Self { dir, mnt }
    }

    fn work(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for LoopBtrfs {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.mnt).status();
    }
}

/// The effective uid owns `/proc/self`.
fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

fn run(program: &str, args: &[&std::ffi::OsStr]) {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e} (install btrfs-progs / util-linux)"));
    assert!(
        out.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An ESP holding committed entries named `names`.
fn esp_with(dir: &Path, names: &[&str]) -> EspState {
    let esp = Esp::open(dir);
    fs::create_dir_all(esp.dir()).unwrap();
    for n in names {
        fs::write(esp.dir().join(n), b"uki").unwrap();
    }
    esp.scan().unwrap()
}

fn is_read_only(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::ReadOnlyFilesystem
}

/// Transaction zero: layout, payload, `etc-base`, seal.
fn transaction_zero(layout: &Layout, btrfs: &dyn Btrfs) {
    assert_eq!(init_layout(layout, btrfs).unwrap(), 1);
    fs::create_dir_all(layout.root(1).join("usr/lib")).unwrap();
    fs::write(layout.root(1).join("usr/lib/os-release"), b"ID=tetsu\n").unwrap();
    fs::write(layout.etc(1).join("hostname"), b"gs66\n").unwrap();
    capture_etc_base(layout, btrfs, 1).unwrap();
    seal(layout, btrfs, 1).unwrap();
    mark_committed(layout, 1).unwrap();
}

#[test]
#[ignore = "needs root and btrfs-progs: run the binary with sudo and --ignored (see the file header)"]
fn layout_snapshot_seal_and_delete() {
    let fs_ = LoopBtrfs::new();
    let layout = Layout::new(&fs_.mnt);
    let btrfs = CliBtrfs::new(SystemRunner);
    transaction_zero(&layout, &btrfs);

    for s in ["@snapshots", "@var", "@home", "@root", "@swap"] {
        assert!(btrfs.is_subvolume(&layout.shared(s)).unwrap(), "{s}");
    }
    assert_eq!(
        fs::read_link(layout.shared("@var").join("lib/pacman")).unwrap(),
        Path::new("/usr/lib/pacman")
    );

    // Sealed: the root refuses writes, the nested etc still takes them.
    let err = fs::write(layout.root(1).join("usr/new"), b"x").unwrap_err();
    assert!(is_read_only(&err), "{err:?}");
    fs::write(layout.etc(1).join("motd"), b"hi\n").unwrap();
    let err = fs::write(layout.etc_base(1).join("x"), b"x").unwrap_err();
    assert!(is_read_only(&err), "{err:?}");

    // A transaction from the booted snapshot carries the nested etc over.
    let records = Records::new(&fs_.work("records"));
    let store = Store::new(layout.clone(), records, &btrfs);
    let esp_dir = fs_.work("esp");
    esp_with(&esp_dir, &["tetsu-1.efi"]);
    let begun = store
        .begin(
            &fs_.work("run").join("tx.lock"),
            || Esp::open(&esp_dir).scan(),
            1,
        )
        .unwrap();
    assert_eq!(begun.counter, 2);
    assert!(btrfs.is_subvolume(&layout.etc(2)).unwrap());
    assert_eq!(fs::read(layout.etc(2).join("motd")).unwrap(), b"hi\n");
    assert_eq!(fs::read(layout.etc_base(2).join("motd")).unwrap(), b"hi\n");
    assert_eq!(
        fs::read(layout.root(2).join("usr/lib/os-release")).unwrap(),
        b"ID=tetsu\n"
    );
    // Writable until sealed.
    fs::write(layout.root(2).join("usr/new"), b"x").unwrap();
    store.seal(&begun).unwrap();
    let err = fs::write(layout.root(2).join("usr/newer"), b"x").unwrap_err();
    assert!(is_read_only(&err), "{err:?}");

    // A sealed snapshot with its nested etc deletes cleanly.
    delete_snapshot(&layout, &btrfs, 1).unwrap();
    assert!(!layout.snapshot_dir(1).exists());
    assert_eq!(layout.list_snapshots().unwrap(), vec![2]);
}

#[test]
#[ignore = "needs root and btrfs-progs: run the binary with sudo and --ignored (see the file header)"]
fn orphans_records_and_retention() {
    let fs_ = LoopBtrfs::new();
    let layout = Layout::new(&fs_.mnt);
    let btrfs = CliBtrfs::new(SystemRunner);
    transaction_zero(&layout, &btrfs);
    let records = Records::new(&fs_.work("records"));
    let store = Store::new(layout.clone(), records.clone(), &btrfs);
    let lock_path = fs_.work("run").join("tx.lock");
    let esp_dir = fs_.work("esp");

    // Twelve committed transactions, each booted and blessed in turn.
    let scan = || Esp::open(&esp_dir).scan();
    esp_with(&esp_dir, &["tetsu-1.efi"]);
    for booted in 1..=12u64 {
        let begun = store.begin(&lock_path, scan, booted).unwrap();
        assert!(begun.gc_failures.is_empty());
        assert_eq!(begun.counter, booted + 1);
        store.seal(&begun).unwrap();
        let esp = esp_with(&esp_dir, &[&format!("tetsu-{}.efi", begun.counter)]);
        store.mark_committed(&begun).unwrap();
        // Record lost on purpose for every third transaction: rebuilt from
        // the ESP, verdict from the blessed name.
        let all = records.load_or_reconstruct(&esp).unwrap();
        if !begun.counter.is_multiple_of(3) {
            records.set_boot(begun.counter, BootVerdict::Good).unwrap();
        } else {
            assert!(all[&begun.counter].reconstructed);
            fs::remove_file(records.path(begun.counter)).unwrap();
            let all = records.load_or_reconstruct(&esp).unwrap();
            assert_eq!(all[&begun.counter].boot, Some(BootVerdict::Good));
        }
        drop(begun);
    }
    assert_eq!(
        layout.list_snapshots().unwrap(),
        (1..=13).collect::<Vec<_>>()
    );

    // An interrupted transaction: created, never committed.
    let begun = store.begin(&lock_path, scan, 13).unwrap();
    assert_eq!(begun.counter, 14);
    drop(begun);

    // Busy while held.
    let lock = TxLock::acquire(&lock_path).unwrap();
    assert!(matches!(
        store.begin(&lock_path, scan, 13),
        Err(StoreError::Busy { .. })
    ));
    let esp = scan().unwrap();
    let deleted = store.gc_orphans(&lock, &esp, Some(13)).unwrap();
    assert_eq!(deleted, vec![14]);

    // Retention: keep the newest ten. The ESP keeps only the last four
    // entries, as boot GC would leave it.
    for n in 1..=9 {
        fs::remove_file(esp_dir.join("EFI/Linux").join(format!("tetsu-{n}.efi"))).unwrap();
    }
    let esp = scan().unwrap();
    let deleted = store.apply_retention(&lock, &esp, 13).unwrap();
    assert_eq!(deleted, vec![1, 2, 3]);
    assert_eq!(
        layout.list_snapshots().unwrap(),
        (4..=13).collect::<Vec<_>>()
    );
}
