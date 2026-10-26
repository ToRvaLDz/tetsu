//! What the service asks of the host: run a transaction, read the records,
//! the user layer and the status. A trait, so the service is tested with
//! the engine behind a fake.

use crate::app::{Host, mount_top, now_secs, resolve_booted, transact_with};
use crate::boot::entry::BootState;
use crate::boot::esp::{Esp, EspState};
use crate::boot::space::StatvfsProbe;
use crate::boot::tool::{Runner, SystemRunner};
use crate::cli::Options;
use crate::store::btrfs::CliBtrfs;
use crate::store::layout::Layout;
use crate::store::record::{Record, Records};
use crate::store::select;
use crate::tx::engine::{EngineEvent, Report, Request, RunCtx};
use crate::tx::{TxError, pkg};

/// Which snapshot is booted, which one the next boot uses and how that one
/// stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusView {
    pub booted: u64,
    pub default: u64,
    pub default_boot: BootState,
}

/// The host as the service sees it. Calls block: the service runs them on
/// a blocking thread.
pub trait Backend: Send + Sync + 'static {
    /// Runs one transaction under `id`, telling `observer` where it is.
    fn run(
        &self,
        id: &str,
        request: &Request,
        observer: &(dyn Fn(EngineEvent) + Sync),
    ) -> Result<Report, TxError>;

    /// The committed transactions, newest first.
    fn transactions(&self) -> Result<Vec<Record>, String>;

    /// The names of the user layer packages.
    fn user_packages(&self) -> Result<Vec<String>, String>;

    /// The booted and the default snapshot.
    fn status(&self) -> Result<StatusView, String>;
}

/// The real host: Btrfs, pacman and systemd-nspawn through their commands.
/// It keeps no handle: everything is built inside each call.
#[derive(Clone, Debug)]
pub struct HostBackend {
    pub options: Options,
    /// The text of `/proc/cmdline`.
    pub proc_cmdline: String,
}

impl Backend for HostBackend {
    fn run(
        &self,
        id: &str,
        request: &Request,
        observer: &(dyn Fn(EngineEvent) + Sync),
    ) -> Result<Report, TxError> {
        let btrfs = CliBtrfs::new(SystemRunner::new());
        let host = Host {
            runner: &SystemRunner::new(),
            btrfs: &btrfs,
            probe: &StatvfsProbe,
            proc_cmdline: &self.proc_cmdline,
            clock: &now_secs,
        };
        let ctx = RunCtx {
            id: Some(id.to_owned()),
            observer,
        };
        transact_with(&self.options, &host, request, ctx)
    }

    fn transactions(&self) -> Result<Vec<Record>, String> {
        read_transactions(&self.options)
    }

    fn user_packages(&self) -> Result<Vec<String>, String> {
        read_user_packages(&self.options, &SystemRunner::new())
    }

    fn status(&self) -> Result<StatusView, String> {
        read_status(&self.options, &self.proc_cmdline)
    }
}

fn scan(options: &Options) -> Result<EspState, String> {
    Esp::open(&options.esp).scan().map_err(|e| e.to_string())
}

/// The committed transactions, newest first. Reads the ESP and the records;
/// writes nothing.
pub fn read_transactions(options: &Options) -> Result<Vec<Record>, String> {
    let esp = scan(options)?;
    let records = Records::new(&options.records)
        .load_derived(&esp)
        .map_err(|e| e.to_string())?;
    Ok(records.into_values().rev().collect())
}

/// The booted and the default snapshot. The booted one is `--booted`, else
/// the kernel command line's; it must have an entry on the ESP.
pub fn read_status(options: &Options, proc_cmdline: &str) -> Result<StatusView, String> {
    let esp = scan(options)?;
    let booted = resolve_booted(options.booted, proc_cmdline).ok_or(
        "cannot tell which snapshot is booted: the kernel command line has no \
         rootflags=subvol=@snapshots/<n>/root; pass --booted <n>",
    )?;
    if !esp.entries.iter().any(|e| e.name.counter == booted) {
        return Err(format!("the booted snapshot {booted} has no boot entry"));
    }
    let default = esp.default_entry().ok_or("there is no boot entry")?;
    Ok(StatusView {
        booted,
        default: default.name.counter,
        default_boot: default.state(),
    })
}

/// The user layer of the default snapshot: its explicitly installed
/// packages outside the base closure.
pub fn read_user_packages(options: &Options, runner: &dyn Runner) -> Result<Vec<String>, String> {
    let default = select::default_counter(&scan(options)?).ok_or("there is no boot entry")?;
    mount_top(runner, &options.top).map_err(|e| e.to_string())?;
    let root = Layout::new(&options.top).root(default);
    let infos = pkg::infos(runner, &root).map_err(|e| e.to_string())?;
    let explicit = pkg::explicit(runner, &root).map_err(|e| e.to_string())?;
    let closure = pkg::base_closure(&infos, pkg::BASE_PACKAGE).ok_or_else(|| {
        format!(
            "{} is not installed in snapshot {default}",
            pkg::BASE_PACKAGE
        )
    })?;
    Ok(pkg::user_layer(&explicit, &closure))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::store::record::{BootVerdict, TxKind};
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
    const BOOTED_1: &str = "root=UUID=x rw rootflags=subvol=@snapshots/1/root quiet";

    struct Host {
        tmp: tempfile::TempDir,
        options: Options,
    }

    /// An ESP with the given entry file names and a records directory.
    fn host(entries: &[&str]) -> Host {
        let tmp = tempfile::tempdir().unwrap();
        let esp = tmp.path().join("esp");
        fs::create_dir_all(esp.join("EFI/Linux")).unwrap();
        for name in entries {
            fs::write(esp.join("EFI/Linux").join(name), b"uki").unwrap();
        }
        let options = Options {
            top: tmp.path().join("top"),
            esp,
            records: tmp.path().join("records"),
            ..Options::default()
        };
        Host { tmp, options }
    }

    fn write_record(options: &Options, counter: u64, id: Option<&str>, kind: Option<TxKind>) {
        crate::store::record::Records::new(&options.records)
            .write(&Record {
                counter,
                id: id.map(str::to_owned),
                kind,
                boot: Some(BootVerdict::Good),
                ..Record::default()
            })
            .unwrap();
    }

    fn user_runner() -> ScriptedRunner {
        let runner = ScriptedRunner::new();
        runner.on_tail(&["-Qi"], Reply::Out(QI.into()));
        runner.on_tail(&["-Qqe"], Reply::Out("tetsu-base\nvim\n".into()));
        runner
    }

    #[test]
    fn transactions_are_listed_newest_first_with_their_ids() {
        let h = host(&["tetsu-1.efi", "tetsu-2.efi", "tetsu-3+3.efi"]);
        write_record(&h.options, 1, None, Some(TxKind::Update));
        write_record(&h.options, 2, Some("tx-aa"), Some(TxKind::Install));
        write_record(&h.options, 3, Some("tx-bb"), Some(TxKind::Remove));
        let listed = read_transactions(&h.options).unwrap();
        let seen: Vec<(u64, Option<&str>)> = listed
            .iter()
            .map(|r| (r.counter, r.id.as_deref()))
            .collect();
        assert_eq!(seen, [(3, Some("tx-bb")), (2, Some("tx-aa")), (1, None)]);
    }

    #[test]
    fn an_entry_without_a_record_is_listed_and_nothing_is_written() {
        let h = host(&["tetsu-1.efi"]);
        let listed = read_transactions(&h.options).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].reconstructed);
        assert!(!h.tmp.path().join("records").exists());
    }

    #[test]
    fn status_reads_the_booted_snapshot_from_the_command_line() {
        let h = host(&["tetsu-1.efi", "tetsu-2+3.efi"]);
        let status = read_status(&h.options, BOOTED_1).unwrap();
        assert_eq!(
            status,
            StatusView {
                booted: 1,
                default: 2,
                default_boot: BootState::Untried,
            }
        );
    }

    #[test]
    fn status_prefers_the_booted_override() {
        let mut h = host(&["tetsu-1.efi", "tetsu-2.efi"]);
        h.options.booted = Some(2);
        let status = read_status(&h.options, BOOTED_1).unwrap();
        assert_eq!((status.booted, status.default), (2, 2));
        assert_eq!(status.default_boot, BootState::Good);
        let none = read_status(&h.options, "").unwrap();
        assert_eq!(none.booted, 2);
    }

    #[test]
    fn status_reports_a_bad_default_entry() {
        // The newest entry that is not bad is the default: here entry 1.
        let h = host(&["tetsu-1.efi", "tetsu-2+0-3.efi"]);
        let status = read_status(&h.options, BOOTED_1).unwrap();
        assert_eq!(status.default, 1);
        assert_eq!(status.default_boot, BootState::Good);
    }

    #[test]
    fn status_with_an_unknown_booted_snapshot_is_an_error() {
        let h = host(&["tetsu-1.efi", "tetsu-2+3.efi"]);
        let nine = "rootflags=subvol=@snapshots/9/root";
        let error = read_status(&h.options, nine).unwrap_err();
        assert!(error.contains('9'), "{error}");
        let error = read_status(&h.options, "root=/dev/sda1").unwrap_err();
        assert!(error.contains("booted"), "{error}");
    }

    #[test]
    fn status_without_an_entry_is_an_error() {
        let h = host(&[]);
        assert!(read_status(&h.options, BOOTED_1).is_err());
    }

    #[test]
    fn user_packages_are_the_user_layer_of_the_default_snapshot() {
        let h = host(&["tetsu-1.efi", "tetsu-2+3.efi"]);
        let runner = user_runner();
        let packages = read_user_packages(&h.options, &runner).unwrap();
        assert_eq!(packages, ["vim"]);
        let root = crate::store::layout::Layout::new(&h.options.top).root(2);
        let lines = runner.lines().join("\n");
        assert!(lines.contains(&root.display().to_string()), "{lines}");
    }

    #[test]
    fn user_packages_need_the_base_package() {
        let h = host(&["tetsu-1.efi"]);
        let runner = ScriptedRunner::new();
        runner.on_tail(&["-Qi"], Reply::Out(String::new()));
        let error = read_user_packages(&h.options, &runner).unwrap_err();
        assert!(error.contains("tetsu-base"), "{error}");
    }
}
