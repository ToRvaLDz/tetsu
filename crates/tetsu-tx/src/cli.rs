//! The command line of `tetsu-tx`: argv parsing and the rendering of reports,
//! errors and the transaction list. Pure; `app` does the wiring.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::boot::esp::EspState;
use crate::boot::keys::DEFAULT_KEYS_DIR;
use crate::store::record::{BootVerdict, RECORDS_DIR, Record, TxKind};
use crate::tx::TxError;
use crate::tx::engine::{DEFAULT_LOCK, Report};
use crate::tx::nspawn::DEFAULT_CACHE;

/// Exit code of a failed transaction.
pub const EXIT_FAILED: u8 = 1;
/// Exit code of a usage error.
pub const EXIT_USAGE: u8 = 2;
/// Exit code when another transaction holds the lock.
pub const EXIT_BUSY: u8 = 3;

/// Where the top level of the Btrfs is mounted by default.
pub const DEFAULT_TOP: &str = "/run/tetsu/top";
/// Where the ESP is mounted by default.
pub const DEFAULT_ESP: &str = "/efi";

/// The usage text.
pub const USAGE: &str = "\
usage: tetsu-tx [options] <command>

commands:
  update                 upgrade the whole system
  install <pkg>...       install packages
  remove <pkg>...        remove packages of the user layer
  rollback [<n>]         boot snapshot <n> (default: the last good one) again
  list [--json]          show the transactions

options:
  --top <dir>       top-level Btrfs mount   (default /run/tetsu/top)
  --esp <dir>       ESP mount               (default /efi)
  --records <dir>   transaction records     (default /var/lib/tetsu/transactions)
  --keys <dir>      signing keys            (default /var/lib/tetsu/keys)
  --lock <file>     transaction lock        (default /run/tetsu/tx.lock)
  --cache <dir>     package cache           (default /var/cache/pacman/pkg)
  --cmdline <text>  kernel command line of a snapshot without etc/kernel/cmdline
  --booted <n>      booted snapshot, instead of reading /proc/cmdline
  -h, --help        this text

exit status: 0 done, 1 the transaction failed, 2 usage, 3 another transaction runs
";

/// What was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Update,
    Install(Vec<String>),
    Remove(Vec<String>),
    Rollback(Option<u64>),
    List {
        json: bool,
    },
    /// Print the usage and stop.
    Help,
}

/// Locations and overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub top: PathBuf,
    pub esp: PathBuf,
    pub records: PathBuf,
    pub keys: PathBuf,
    pub lock: PathBuf,
    pub cache: PathBuf,
    pub cmdline: String,
    pub booted: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            top: PathBuf::from(DEFAULT_TOP),
            esp: PathBuf::from(DEFAULT_ESP),
            records: PathBuf::from(RECORDS_DIR),
            keys: PathBuf::from(DEFAULT_KEYS_DIR),
            lock: PathBuf::from(DEFAULT_LOCK),
            cache: PathBuf::from(DEFAULT_CACHE),
            cmdline: String::new(),
            booted: None,
        }
    }
}

/// A parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub options: Options,
}

/// The command line is wrong.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

/// Parse the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Invocation, UsageError> {
    let _ = args;
    todo!()
}

/// The exit code of a failed transaction.
pub fn exit_code(error: &TxError) -> u8 {
    let _ = error;
    todo!()
}

/// A committed transaction, result first.
pub fn render_report(report: &Report) -> String {
    let _ = report;
    todo!()
}

/// A failed transaction, result first.
pub fn render_error(error: &TxError) -> String {
    let _ = error;
    todo!()
}

/// One line of `list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListRow {
    pub counter: u64,
    pub kind: Option<TxKind>,
    pub source: Option<u64>,
    /// `None` until the boot is blessed or marked bad.
    pub boot: Option<BootVerdict>,
    pub booted: bool,
    pub default: bool,
}

/// The rows of `list`, oldest first.
pub fn list_rows(
    records: &BTreeMap<u64, Record>,
    esp: &EspState,
    booted: Option<u64>,
) -> Vec<ListRow> {
    let _ = (records, esp, booted);
    todo!()
}

/// The rows as a table, or as a JSON array.
pub fn render_list(rows: &[ListRow], json: bool) -> String {
    let _ = (rows, json);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::EspEntry;
    use crate::tx::PackageChange;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn command(list: &[&str]) -> Command {
        parse(&args(list)).unwrap().command
    }

    fn options(list: &[&str]) -> Options {
        parse(&args(list)).unwrap().options
    }

    fn usage(list: &[&str]) -> String {
        parse(&args(list)).unwrap_err().0
    }

    #[test]
    fn the_commands_parse() {
        assert_eq!(command(&["update"]), Command::Update);
        assert_eq!(
            command(&["install", "vim", "htop"]),
            Command::Install(args(&["vim", "htop"]))
        );
        assert_eq!(command(&["remove", "vim"]), Command::Remove(args(&["vim"])));
        assert_eq!(command(&["rollback"]), Command::Rollback(None));
        assert_eq!(command(&["rollback", "7"]), Command::Rollback(Some(7)));
        assert_eq!(command(&["list"]), Command::List { json: false });
        assert_eq!(command(&["list", "--json"]), Command::List { json: true });
    }

    #[test]
    fn help_is_a_request_not_an_error() {
        assert_eq!(command(&["--help"]), Command::Help);
        assert_eq!(command(&["update", "-h"]), Command::Help);
    }

    #[test]
    fn defaults_are_the_system_paths() {
        let o = options(&["update"]);
        assert_eq!(o.top, PathBuf::from("/run/tetsu/top"));
        assert_eq!(o.esp, PathBuf::from("/efi"));
        assert_eq!(o.records, PathBuf::from("/var/lib/tetsu/transactions"));
        assert_eq!(o.keys, PathBuf::from("/var/lib/tetsu/keys"));
        assert_eq!(o.lock, PathBuf::from("/run/tetsu/tx.lock"));
        assert_eq!(o.cache, PathBuf::from("/var/cache/pacman/pkg"));
        assert_eq!(o.booted, None);
    }

    #[test]
    fn options_come_before_or_after_the_command_in_both_spellings() {
        let o = options(&[
            "--top",
            "/t",
            "update",
            "--esp=/e",
            "--records",
            "/r",
            "--keys=/k",
            "--lock",
            "/l",
            "--cache=/c",
            "--booted",
            "4",
            "--cmdline=quiet ro",
        ]);
        assert_eq!(o.top, PathBuf::from("/t"));
        assert_eq!(o.esp, PathBuf::from("/e"));
        assert_eq!(o.records, PathBuf::from("/r"));
        assert_eq!(o.keys, PathBuf::from("/k"));
        assert_eq!(o.lock, PathBuf::from("/l"));
        assert_eq!(o.cache, PathBuf::from("/c"));
        assert_eq!(o.booted, Some(4));
        assert_eq!(o.cmdline, "quiet ro");
    }

    #[test]
    fn double_dash_ends_the_options() {
        assert_eq!(
            command(&["install", "--", "-weird-name"]),
            Command::Install(args(&["-weird-name"]))
        );
    }

    #[test]
    fn bad_command_lines_are_usage_errors() {
        assert!(usage(&[]).contains("command"));
        assert!(usage(&["frobnicate"]).contains("frobnicate"));
        assert!(usage(&["update", "extra"]).contains("extra"));
        assert!(usage(&["install"]).contains("package"));
        assert!(usage(&["remove"]).contains("package"));
        assert!(usage(&["rollback", "x"]).contains('x'));
        assert!(usage(&["rollback", "0"]).contains('0'));
        assert!(usage(&["rollback", "1", "2"]).contains('2'));
        assert!(usage(&["list", "x"]).contains('x'));
        assert!(usage(&["update", "--json"]).contains("--json"));
        assert!(usage(&["update", "--nope"]).contains("--nope"));
        assert!(usage(&["update", "--top"]).contains("--top"));
        assert!(usage(&["update", "--booted", "abc"]).contains("abc"));
        assert!(usage(&["install", "-x"]).contains("-x"));
    }

    #[test]
    fn exit_codes_tell_busy_from_failed() {
        assert_eq!(exit_code(&TxError::InProgress), EXIT_BUSY);
        assert_eq!(exit_code(&TxError::failed("x")), EXIT_FAILED);
        assert_eq!(
            exit_code(&TxError::PackageNotFound("x".into())),
            EXIT_FAILED
        );
        assert_eq!(exit_code(&TxError::NotUserPackage("x".into())), EXIT_FAILED);
        assert_eq!(exit_code(&TxError::SnapshotNotFound(3)), EXIT_FAILED);
    }

    fn report() -> Report {
        Report {
            counter: 5,
            kind: TxKind::Install,
            packages: args(&["htop"]),
            changes: vec![
                PackageChange {
                    name: "htop".into(),
                    from: None,
                    to: Some("3-1".into()),
                },
                PackageChange {
                    name: "vim".into(),
                    from: Some("9.0-1".into()),
                    to: Some("9.1-1".into()),
                },
                PackageChange {
                    name: "foo".into(),
                    from: Some("2-1".into()),
                    to: None,
                },
            ],
            log: args(&["DKMS modules built for kernel 6.1"]),
            warnings: args(&["retention left snapshots behind: x"]),
        }
    }

    #[test]
    fn a_report_leads_with_the_result() {
        let text = render_report(&report());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "install htop committed as snapshot 5");
        assert!(text.contains("  + htop 3-1"), "{text}");
        assert!(text.contains("  vim 9.0-1 -> 9.1-1"), "{text}");
        assert!(text.contains("  - foo 2-1"), "{text}");
        assert!(text.contains("DKMS modules built for kernel 6.1"), "{text}");
        assert!(
            lines.last().unwrap().starts_with("warning: retention"),
            "{text}"
        );
    }

    #[test]
    fn a_quiet_report_is_one_line() {
        let r = Report {
            counter: 2,
            kind: TxKind::Update,
            packages: vec![],
            changes: vec![],
            log: vec![],
            warnings: vec![],
        };
        assert_eq!(render_report(&r), "update committed as snapshot 2\n");
        let r = Report {
            kind: TxKind::Rollback,
            ..r
        };
        assert_eq!(render_report(&r), "rollback committed as snapshot 2\n");
    }

    #[test]
    fn an_error_leads_with_the_reason_then_the_proposal() {
        let e = TxError::Failed {
            reason: "update failed: boom".into(),
            proposal: Some("tx remove vim, then tx update".into()),
            log: args(&["error: boom", "second"]),
        };
        let text = render_error(&e);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "error: transaction failed: update failed: boom");
        assert!(text.contains("  error: boom\n  second\n"), "{text}");
        assert!(
            text.contains("try: tx remove vim, then tx update\n"),
            "{text}"
        );
        assert!(text.contains("system was not changed"), "{text}");
    }

    #[test]
    fn simple_errors_read_plainly() {
        assert_eq!(
            render_error(&TxError::InProgress),
            "error: another transaction is in progress\n"
        );
        assert!(
            render_error(&TxError::PackageNotFound("nope".into()))
                .starts_with("error: package not found: nope\n")
        );
        assert!(
            render_error(&TxError::SnapshotNotFound(9))
                .starts_with("error: snapshot 9 not found\n")
        );
        assert!(render_error(&TxError::SnapshotNotFound(0)).contains("no good snapshot"));
    }

    fn esp(names: &[&str]) -> EspState {
        let mut st = EspState::default();
        for n in names {
            let Some(FileKind::Entry(name)) = parse_file_name(n) else {
                panic!("{n}")
            };
            st.entries.push(EspEntry {
                name,
                path: PathBuf::from(n),
                size: 1,
            });
        }
        st
    }

    fn records() -> BTreeMap<u64, Record> {
        let rec = |counter, kind, source, boot| {
            (
                counter,
                Record {
                    counter,
                    kind,
                    source,
                    boot,
                    ..Record::default()
                },
            )
        };
        BTreeMap::from([
            rec(1, None, None, Some(BootVerdict::Good)),
            rec(2, Some(TxKind::Update), Some(1), Some(BootVerdict::Bad)),
            rec(3, Some(TxKind::Rollback), Some(1), None),
        ])
    }

    #[test]
    fn rows_mark_the_booted_and_the_default_snapshot() {
        let rows = list_rows(&records(), &esp(&["tetsu-1.efi", "tetsu-3+3.efi"]), Some(1));
        let flags: Vec<(u64, bool, bool)> = rows
            .iter()
            .map(|r| (r.counter, r.booted, r.default))
            .collect();
        assert_eq!(
            flags,
            [(1, true, false), (2, false, false), (3, false, true)]
        );
        assert_eq!(rows[1].kind, Some(TxKind::Update));
        assert_eq!(rows[1].boot, Some(BootVerdict::Bad));
        assert_eq!(rows[2].source, Some(1));
    }

    #[test]
    fn rows_without_a_booted_snapshot_mark_none() {
        let rows = list_rows(&records(), &esp(&["tetsu-1.efi"]), None);
        assert!(rows.iter().all(|r| !r.booted));
    }

    #[test]
    fn the_table_shows_the_verdict_and_the_marks() {
        let rows = list_rows(&records(), &esp(&["tetsu-1.efi", "tetsu-3+3.efi"]), Some(1));
        let text = render_list(&rows, false);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].starts_with("SNAPSHOT"), "{text}");
        let cells = |line: &str| {
            line.split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(cells(lines[1]), ["1", "-", "-", "good", "booted"]);
        assert_eq!(cells(lines[2]), ["2", "update", "1", "bad"]);
        assert_eq!(
            cells(lines[3]),
            ["3", "rollback", "1", "pending", "default"]
        );
    }

    #[test]
    fn an_empty_list_says_so() {
        assert_eq!(render_list(&[], false), "no transactions\n");
        assert_eq!(render_list(&[], true).trim(), "[]");
    }

    #[test]
    fn json_is_an_array_of_objects() {
        let rows = list_rows(&records(), &esp(&["tetsu-1.efi", "tetsu-3+3.efi"]), Some(1));
        let value: serde_json::Value = serde_json::from_str(&render_list(&rows, true)).unwrap();
        let all = value.as_array().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0]["counter"], 1);
        assert_eq!(all[0]["kind"], serde_json::Value::Null);
        assert_eq!(all[0]["booted"], true);
        assert_eq!(all[1]["kind"], "update");
        assert_eq!(all[1]["boot"], "bad");
        assert_eq!(all[2]["boot"], serde_json::Value::Null);
        assert_eq!(all[2]["default"], true);
    }
}
