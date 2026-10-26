//! The installer from start to finish on the real machine: preflight and
//! questions (or an answer file), summary, install, cleanup.

use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tetsu_installer_core::answers::Answers;
use tetsu_installer_core::entropy::SystemEntropy;
use tetsu_installer_core::env::Env;
use tetsu_installer_core::error::InstallError;
use tetsu_installer_core::frontend::Frontend;
use tetsu_installer_core::install::InstallTarget;
use tetsu_installer_core::install::target::DEFAULT_RUN_DIR;
use tetsu_installer_core::preflight::secure_boot_state;
use tetsu_tx::boot::space::StatvfsProbe;
use tetsu_tx::boot::tool::SystemRunner;

use crate::answer_file::AnswerFile;
use crate::cli::{self, Command, Options, USAGE};
use crate::collect::{Collected, collect};
use crate::install::{Machine, Report, install};
use crate::locate::{self, Source};
use crate::probe::MountProbe;
use crate::system;
use crate::text::TextFrontend;
use crate::tty;

const LOG_NAME: &str = "install.log";
const CMDLINE: &str = "/proc/cmdline";

/// Warning printed whenever an answer file is in use.
pub const FILE_WARNING: &str = "Warning: an answer file is in use. It may hold the disk passphrase \
and an API key in plain text, so use it for tests only. It is not copied to the installed system.";

/// Entry point: parse the command line and run. Exit status 0 installed,
/// 1 failed or aborted, 2 usage.
pub fn main(args: impl IntoIterator<Item = String>) -> ExitCode {
    match cli::parse(args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Run(opts)) => match run(&opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                eprintln!("tetsu-installer: {msg}");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("tetsu-installer: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(opts: &Options) -> Result<(), String> {
    let stdin = io::stdin();
    let fe = RefCell::new(
        TextFrontend::new(stdin.lock(), io::stdout()).hide_secrets(tty::stdin_is_tty()),
    );
    let run_dir = Path::new(DEFAULT_RUN_DIR);
    let runner = SystemRunner::new();
    let file = answer_file(opts, run_dir, &runner)?;
    if file.is_some() {
        fe.borrow_mut().message(FILE_WARNING);
    }
    let probe = MountProbe::new(&runner, run_dir);
    let env = Env {
        root: PathBuf::from("/"),
        runner: &runner,
        probe: &probe,
        entropy: &SystemEntropy,
        boot_partition: system::boot_partition(&runner, &opts.medium),
    };
    let Collected {
        answers,
        trust_cert,
    } = collect(&mut *fe.borrow_mut(), &env, file.as_ref()).map_err(|e| e.to_string())?;

    let payload = system::payload_path(&opts.medium);
    if !payload.is_file() {
        return Err(format!(
            "{}: the system image is missing",
            payload.display()
        ));
    }
    let mut target = target(&answers, &payload, run_dir);
    target.trust_cert = trust_cert;
    target.console_args = system::console_args(&fs::read_to_string(CMDLINE).unwrap_or_default());
    // From here on Ctrl+C is held until the next safe point.
    let cancel = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&cancel))
        .map_err(|e| format!("cannot install the Ctrl+C handler: {e}"))?;
    // The tools of the install run in their own process group, so Ctrl+C
    // reaches the handler above and not the tool that is running.
    let report = install(
        &fe,
        &Machine {
            runner: &SystemRunner::own_group(),
            space: &StatvfsProbe,
            entropy: &SystemEntropy,
            cancel: &cancel,
        },
        &answers,
        &target,
    );
    finish(&mut *fe.borrow_mut(), run_dir, report)
}

/// The answer file, from `--answers` or from the machine, parsed.
fn answer_file(
    opts: &Options,
    run_dir: &Path,
    runner: &SystemRunner,
) -> Result<Option<AnswerFile>, String> {
    let text = match &opts.answers {
        Some(path) => {
            Some(fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?)
        }
        None => {
            let cmdline = fs::read_to_string(CMDLINE).unwrap_or_default();
            let source = Source {
                root: Path::new("/"),
                cmdline: &cmdline,
                run_dir,
            };
            locate::load(&source, runner).map_err(|e| e.to_string())?
        }
    };
    text.map(|t| AnswerFile::parse(&t).map_err(|e| e.to_string()))
        .transpose()
}

fn target(answers: &Answers, payload: &Path, run_dir: &Path) -> InstallTarget {
    let mut target = InstallTarget::new(
        &answers.disk.path,
        payload,
        &run_dir.join("work"),
        secure_boot_state(Path::new("/")),
    );
    target.microsoft_certs = system::microsoft_certs(Path::new("/"));
    target
}

/// Report how the install ended and turn it into the exit result.
fn finish(fe: &mut dyn Frontend, run_dir: &Path, report: Report) -> Result<(), String> {
    let log = write_log(run_dir, &report.log);
    let log_note = match &log {
        Ok(path) => format!("The log is in {}.", path.display()),
        Err(e) => format!("The log could not be written: {e}"),
    };
    if let Some(problems) = &report.cleanup_problems {
        fe.message("The disk was cleaned up: mounts released and signatures wiped.");
        for p in problems {
            fe.message(&format!("Cleanup problem: {p}"));
        }
    }
    if let Some(problems) = &report.release_problems {
        for p in problems {
            fe.message(&format!("Release problem: {p}"));
        }
    }
    if report.disk_kept {
        let aborted = matches!(report.result, Err(InstallError::Cancelled));
        fe.message(kept_message(aborted));
    }
    match report.result {
        Ok(()) => {
            fe.message(&format!(
                "Installation complete. Remove the installation medium and reboot. {log_note}"
            ));
            Ok(())
        }
        Err(InstallError::Cancelled) => Err(format!("install aborted. {log_note}")),
        Err(e) => Err(format!("install failed: {e}. {log_note}")),
    }
}

/// Said when the disk is kept because the keys are already enrolled.
fn kept_message(aborted: bool) -> &'static str {
    if aborted {
        "You aborted after the Secure Boot keys were enrolled in the firmware, so the disk \
         was kept as it is, not wiped. The install is incomplete. To start over, reset the \
         keys in the firmware setup first."
    } else {
        "The install failed after the Secure Boot keys were enrolled in the firmware, so the \
         disk was kept as it is, not wiped. Look at the log; to start over, reset the keys \
         in the firmware setup first."
    }
}

/// Write the install log (descriptions only, no secrets) under the run
/// directory, readable by root only; returns its path.
pub fn write_log(run_dir: &Path, lines: &[String]) -> io::Result<PathBuf> {
    let path = run_dir.join(LOG_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    for line in lines {
        writeln!(file, "{line}")?;
    }
    file.sync_all()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_kept_disk_message_fits_an_abort_and_a_failure() {
        let abort = kept_message(true);
        assert!(abort.contains("You aborted") && !abort.contains("Look at the log"));
        let failure = kept_message(false);
        assert!(failure.contains("failed") && failure.contains("Look at the log"));
    }

    #[test]
    fn the_log_is_written_private_and_replaces_an_older_one() {
        let t = tempfile::tempdir().unwrap();
        write_log(t.path(), &["old".into(), "older".into()]).unwrap();
        let path = write_log(t.path(), &["[disk] wipe".into(), "[boot] uki".into()]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[disk] wipe\n[boot] uki\n"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_log_in_a_missing_directory_is_an_error_not_a_panic() {
        let t = tempfile::tempdir().unwrap();
        assert!(write_log(&t.path().join("gone"), &[]).is_err());
    }
}
