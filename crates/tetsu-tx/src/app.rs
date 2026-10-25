//! The wiring of `tetsu-tx`: mount the top level when needed, build the
//! engine from the options, run the command and turn the result into text
//! and an exit code. The host (runner, Btrfs, space probe, `/proc/cmdline`,
//! clock) is injected, so this runs against fakes.

use std::path::Path;
use std::process::ExitCode;

use crate::boot::space::SpaceProbe;
use crate::boot::tool::Runner;
use crate::cli::Invocation;
use crate::store::btrfs::Btrfs;
use crate::tx::TxError;

/// What the process prints and exits with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub code: u8,
}

/// The outside world of one run.
pub struct Host<'a> {
    pub runner: &'a dyn Runner,
    pub btrfs: &'a dyn Btrfs,
    pub probe: &'a dyn SpaceProbe,
    /// The text of `/proc/cmdline`.
    pub proc_cmdline: &'a str,
    /// Unix seconds.
    pub clock: &'a dyn Fn() -> u64,
}

/// Mount the top level (`subvolid=5`) of the filesystem `/` is on at `top`
/// unless something is mounted there already. Returns whether it mounted.
pub fn mount_top(runner: &dyn Runner, top: &Path) -> Result<bool, TxError> {
    let _ = (runner, top);
    todo!()
}

/// The booted snapshot: the override, else `rootflags=subvol=` of the
/// kernel command line.
pub fn resolve_booted(over: Option<u64>, proc_cmdline: &str) -> Option<u64> {
    let _ = (over, proc_cmdline);
    todo!()
}

/// Run one parsed command.
pub fn execute(invocation: &Invocation, host: &Host<'_>) -> Outcome {
    let _ = (invocation, host);
    todo!()
}

/// The program: parse, run, print, exit code.
pub fn main(args: &[String]) -> ExitCode {
    let _ = args;
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::BootError;
    use crate::cli::parse;
    use crate::store::btrfs::tests::FakeBtrfs;
    use crate::store::record::{BootVerdict, Record, Records, TxKind};
    use crate::tx::fake::{Reply, ScriptedRunner};
    use std::fs;

    struct Plenty;

    impl SpaceProbe for Plenty {
        fn available_bytes(&self, _: &Path) -> Result<u64, BootError> {
            Ok(1 << 40)
        }
    }

    const BOOTED: &str = "root=UUID=x rw rootflags=subvol=@snapshots/2/root quiet";

    struct Fixture {
        tmp: tempfile::TempDir,
        runner: ScriptedRunner,
        btrfs: FakeBtrfs,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            fs::create_dir(tmp.path().join("top")).unwrap();
            let btrfs = FakeBtrfs::new(&tmp.path().join("top"));
            Self {
                tmp,
                runner: ScriptedRunner::new(),
                btrfs,
            }
        }

        fn path(&self, name: &str) -> String {
            self.tmp.path().join(name).display().to_string()
        }

        /// The path options pointing into the fixture.
        fn flags(&self) -> Vec<String> {
            let mut flags = Vec::new();
            for (flag, dir) in [
                ("--top", "top"),
                ("--esp", "esp"),
                ("--records", "records"),
                ("--keys", "keys"),
                ("--lock", "run/tx.lock"),
                ("--cache", "cache"),
            ] {
                flags.push(flag.to_owned());
                flags.push(self.path(dir));
            }
            flags
        }

        fn run(&self, words: &[&str], proc_cmdline: &str) -> Outcome {
            let mut args = self.flags();
            args.extend(words.iter().map(|w| (*w).to_owned()));
            let invocation = parse(&args).unwrap();
            let host = Host {
                runner: &self.runner,
                btrfs: &self.btrfs,
                probe: &Plenty,
                proc_cmdline,
                clock: &|| 42,
            };
            execute(&invocation, &host)
        }
    }

    #[test]
    fn the_booted_snapshot_comes_from_the_override_or_the_kernel_command_line() {
        assert_eq!(resolve_booted(None, BOOTED), Some(2));
        assert_eq!(resolve_booted(Some(5), BOOTED), Some(5));
        assert_eq!(resolve_booted(None, "root=/dev/sda1"), None);
        assert_eq!(resolve_booted(Some(5), ""), Some(5));
    }

    #[test]
    fn a_mounted_top_level_is_left_alone() {
        let fx = Fixture::new();
        assert!(!mount_top(&fx.runner, Path::new("/mnt/top")).unwrap());
        let lines = fx.runner.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("findmnt "), "{lines:?}");
        assert!(lines[0].ends_with(" /mnt/top"), "{lines:?}");
    }

    #[test]
    fn an_unmounted_top_level_is_mounted_from_the_root_device() {
        let fx = Fixture::new();
        fx.runner.on(
            &["findmnt", "-n", "-o", "TARGET"],
            Reply::Fail(String::new()),
        );
        fx.runner.on(
            &["findmnt", "-n", "-o", "SOURCE", "/"],
            Reply::Out("/dev/nvme0n1p2[/@snapshots/3/root]\n".into()),
        );
        let top = fx.tmp.path().join("run/top");
        assert!(mount_top(&fx.runner, &top).unwrap());
        assert!(top.is_dir());
        let mount = fx.runner.lines().pop().unwrap();
        assert_eq!(
            mount,
            format!("mount -o subvolid=5 /dev/nvme0n1p2 {}", top.display())
        );
    }

    #[test]
    fn a_root_device_that_cannot_be_told_fails_without_mounting() {
        let fx = Fixture::new();
        fx.runner.on(
            &["findmnt", "-n", "-o", "TARGET"],
            Reply::Fail(String::new()),
        );
        fx.runner.on(
            &["findmnt", "-n", "-o", "SOURCE", "/"],
            Reply::Out("\n".into()),
        );
        let top = fx.tmp.path().join("run/top");
        assert!(matches!(
            mount_top(&fx.runner, &top),
            Err(TxError::Failed { .. })
        ));
        assert!(!fx.runner.lines().iter().any(|l| l.starts_with("mount ")));
    }

    #[test]
    fn list_prints_the_records_with_the_marks() {
        let fx = Fixture::new();
        let esp = fx.tmp.path().join("esp/EFI/Linux");
        fs::create_dir_all(&esp).unwrap();
        for name in ["tetsu-1.efi", "tetsu-2+3.efi"] {
            fs::write(esp.join(name), b"uki").unwrap();
        }
        let records = Records::new(&fx.tmp.path().join("records"));
        for (counter, kind, boot) in [
            (1, None, Some(BootVerdict::Good)),
            (2, Some(TxKind::Update), None),
        ] {
            let record = Record {
                counter,
                kind,
                boot,
                source: kind.map(|_| 1),
                ..Record::default()
            };
            records.write(&record).unwrap();
        }
        let out = fx.run(&["list"], "rootflags=subvol=@snapshots/1/root");
        assert_eq!(out.code, 0, "{out:?}");
        assert!(out.stdout.contains("booted"), "{}", out.stdout);
        assert!(out.stdout.contains("default"), "{}", out.stdout);
        assert!(out.stdout.contains("pending"), "{}", out.stdout);
        let json = fx.run(&["list", "--json"], "");
        let value: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 2);
        assert_eq!(value[1]["default"], true);
        assert_eq!(value[0]["booted"], false);
        assert!(fx.runner.lines().is_empty(), "list runs nothing");
    }

    #[test]
    fn help_prints_the_usage_and_succeeds() {
        let fx = Fixture::new();
        let out = fx.run(&["--help"], "");
        assert_eq!(out.code, 0);
        assert!(out.stdout.starts_with("usage: tetsu-tx"));
    }

    #[test]
    fn a_transaction_needs_to_know_the_booted_snapshot() {
        let fx = Fixture::new();
        let out = fx.run(&["update"], "root=/dev/sda1");
        assert_eq!(out.code, 1, "{out:?}");
        assert!(out.stderr.contains("--booted"), "{}", out.stderr);
        assert!(out.stdout.is_empty());
        assert!(fx.runner.lines().is_empty());
    }

    #[test]
    fn a_failed_transaction_prints_the_error_and_exits_1() {
        let fx = Fixture::new();
        // The top level holds no layout: the booted snapshot is not there.
        let out = fx.run(&["update"], BOOTED);
        assert_eq!(out.code, 1, "{out:?}");
        assert!(
            out.stderr.starts_with("error: snapshot 2 not found"),
            "{}",
            out.stderr
        );
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn a_running_transaction_exits_3() {
        let fx = Fixture::new();
        let _held =
            crate::store::lock::TxLock::acquire(&fx.tmp.path().join("run/tx.lock")).unwrap();
        let out = fx.run(&["update"], BOOTED);
        assert_eq!(out.code, 3, "{out:?}");
        assert!(out.stderr.contains("in progress"), "{}", out.stderr);
    }
}
