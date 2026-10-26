//! The wiring of `tetsu-tx`: mount the top level when needed, build the
//! engine from the options, run the command and turn the result into text
//! and an exit code. The host (runner, Btrfs, space probe, `/proc/cmdline`,
//! clock) is injected, so this runs against fakes.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tetsu_bus::server::bind_system;
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::LocalSet;

use crate::boot::BootError;
use crate::boot::esp::Esp;
use crate::boot::keys::SigningKeys;
use crate::boot::space::{SpaceProbe, StatvfsProbe};
use crate::boot::tool::{Runner, SystemRunner};
use crate::cli::{
    Command, EXIT_FAILED, EXIT_USAGE, Invocation, Options, USAGE, exit_code, list_rows, parse,
    render_error, render_list, render_report,
};
use crate::fault;
use crate::serve::backend::HostBackend;
use crate::serve::policy::LocalRoot;
use crate::store::btrfs::{Btrfs, CliBtrfs};
use crate::store::layout::{Layout, booted_counter};
use crate::store::record::Records;
use crate::tx::TxError;
use crate::tx::engine::{Engine, Paths, Report, Request, RunCtx};

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
    // `findmnt --mountpoint` exits 1 when `top` is not a mount point.
    let probe = [
        OsString::from("-n"),
        "-o".into(),
        "TARGET".into(),
        "--mountpoint".into(),
        top.into(),
    ];
    match runner.output("findmnt", &probe) {
        Ok(_) => return Ok(false),
        Err(BootError::Tool { .. }) => {}
        Err(e) => return Err(e.into()),
    }
    let source = runner.output("findmnt", &["-n", "-o", "SOURCE", "/"].map(OsString::from))?;
    // A subvolume mount reads `/dev/x[/@snapshots/3/root]`.
    let device = source.trim().split('[').next().unwrap_or_default();
    if device.is_empty() {
        return Err(TxError::failed("cannot tell which device holds /"));
    }
    fs::create_dir_all(top).map_err(|e| BootError::io(top, e))?;
    let mount = [
        OsString::from("-o"),
        "subvolid=5".into(),
        device.into(),
        top.into(),
    ];
    runner.run("mount", &mount)?;
    Ok(true)
}

/// The booted snapshot: the override, else `rootflags=subvol=` of the
/// kernel command line.
pub fn resolve_booted(over: Option<u64>, proc_cmdline: &str) -> Option<u64> {
    over.or_else(|| booted_counter(proc_cmdline))
}

impl Outcome {
    fn done(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            code: 0,
        }
    }

    fn failed(stderr: String, code: u8) -> Self {
        Self {
            stdout: String::new(),
            stderr,
            code,
        }
    }
}

/// Run one parsed command.
pub fn execute(invocation: &Invocation, host: &Host<'_>) -> Outcome {
    let request = match &invocation.command {
        Command::Help => return Outcome::done(USAGE.to_owned()),
        Command::Serve { .. } => {
            return Outcome::failed(
                "error: serve is run by the program itself\n".to_owned(),
                EXIT_FAILED,
            );
        }
        Command::List { json } => return list(&invocation.options, host, *json),
        Command::Update => Request::Update,
        Command::Install(pkgs) => Request::Install(pkgs.clone()),
        Command::Remove(pkgs) => Request::Remove(pkgs.clone()),
        Command::Rollback(target) => Request::Rollback(*target),
    };
    match transact(&invocation.options, host, &request) {
        Ok(report) => Outcome::done(render_report(&report)),
        Err(e) => Outcome::failed(render_error(&e), exit_code(&e)),
    }
}

fn list(options: &Options, host: &Host<'_>, json: bool) -> Outcome {
    let read = || -> Result<_, String> {
        let esp = Esp::open(&options.esp).scan().map_err(|e| e.to_string())?;
        let records = Records::new(&options.records)
            .load_derived(&esp)
            .map_err(|e| e.to_string())?;
        Ok((esp, records))
    };
    match read() {
        Ok((esp, records)) => {
            let booted = resolve_booted(options.booted, host.proc_cmdline);
            Outcome::done(render_list(&list_rows(&records, &esp, booted), json))
        }
        Err(msg) => Outcome::failed(format!("error: {msg}\n"), EXIT_FAILED),
    }
}

fn transact(options: &Options, host: &Host<'_>, request: &Request) -> Result<Report, TxError> {
    transact_with(options, host, request, RunCtx::silent())
}

/// Run one transaction, telling `ctx.observer` where it is and writing
/// `ctx.id` into its record.
pub fn transact_with(
    options: &Options,
    host: &Host<'_>,
    request: &Request,
    ctx: RunCtx<'_>,
) -> Result<Report, TxError> {
    let Some(booted) = resolve_booted(options.booted, host.proc_cmdline) else {
        return Err(TxError::failed(
            "cannot tell which snapshot is booted: the kernel command line has no \
             rootflags=subvol=@snapshots/<n>/root; pass --booted <n>",
        ));
    };
    mount_top(host.runner, &options.top)?;
    let with_pcr = options.keys.join("pcr.key").is_file();
    let engine = Engine {
        layout: Layout::new(&options.top),
        records: Records::new(&options.records),
        esp: Esp::open(&options.esp),
        keys: SigningKeys::in_dir(&options.keys, with_pcr),
        paths: Paths {
            lock: options.lock.clone(),
            cache: options.cache.clone(),
            ..Paths::default()
        },
        booted,
        default_cmdline: options.cmdline.clone(),
        runner: host.runner,
        btrfs: host.btrfs,
        probe: host.probe,
        clock: host.clock,
    };
    engine.run_with(request, ctx)
}

/// Unix seconds now; 0 before the epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The program: parse, run, print, exit code.
pub fn main(args: &[String]) -> ExitCode {
    let invocation = match parse(args) {
        Ok(invocation) => invocation,
        Err(e) => {
            eprintln!("tetsu-tx: {e}\n\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    // Unreadable: the booted snapshot then has to come from `--booted`.
    let proc_cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if let Command::Serve { socket } = &invocation.command {
        return serve_main(&invocation.options, proc_cmdline, socket);
    }
    match fault::armed_from(std::env::var(fault::PAUSE_ENV).ok().as_deref()) {
        Ok(Some(point)) => fault::arm(point),
        Ok(None) => {}
        Err(e) => {
            eprintln!("tetsu-tx: {e}\n\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    }
    let clock = now_secs;
    let host = Host {
        runner: &SystemRunner::new(),
        btrfs: &CliBtrfs::new(SystemRunner::new()),
        probe: &StatvfsProbe,
        proc_cmdline: &proc_cmdline,
        clock: &clock,
    };
    let outcome = execute(&invocation, &host);
    // A closed pipe leaves nobody to tell: the exit code still says it all.
    let _ = std::io::stdout().write_all(outcome.stdout.as_bytes());
    let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    ExitCode::from(outcome.code)
}

/// `serve`: answer the bus until SIGTERM or SIGINT. Exit code 1 with a
/// message when it cannot bind or run.
fn serve_main(options: &Options, proc_cmdline: String, socket: &Path) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("tetsu-tx: cannot start the runtime: {e}");
            return ExitCode::from(EXIT_FAILED);
        }
    };
    let backend = Arc::new(HostBackend {
        options: options.clone(),
        proc_cmdline,
    });
    let result = LocalSet::new().block_on(&runtime, run_server(backend, socket));
    // A transaction still running on its thread dies with the process: the
    // collection of the next transaction recovers whatever it left.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tetsu-tx: {message}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

async fn run_server(backend: Arc<HostBackend>, socket: &Path) -> Result<(), String> {
    let listener =
        bind_system(socket).map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
    let stop = |e: std::io::Error| format!("cannot watch for signals: {e}");
    let mut term = signal(SignalKind::terminate()).map_err(stop)?;
    let mut int = signal(SignalKind::interrupt()).map_err(stop)?;
    tokio::select! {
        served = crate::serve::serve(listener, backend, Rc::new(LocalRoot)) => {
            served.map_err(|e| format!("serving failed: {e}"))
        }
        _ = term.recv() => Ok(()),
        _ = int.recv() => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::BootError;
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
    fn list_over_an_entry_without_a_record_shows_it_and_writes_nothing() {
        let fx = Fixture::new();
        let esp = fx.tmp.path().join("esp/EFI/Linux");
        fs::create_dir_all(&esp).unwrap();
        fs::write(esp.join("tetsu-1.efi"), b"uki").unwrap();
        let out = fx.run(&["list", "--json"], "rootflags=subvol=@snapshots/1/root");
        assert_eq!(out.code, 0, "{out:?}");
        let value: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert!(!fx.tmp.path().join("records").exists());
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
