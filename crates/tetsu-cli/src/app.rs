//! Entry point: parse, run as the real user on the real machine.

use std::process::ExitCode;

use tetsu_installer_core::entropy::SystemEntropy;
use tetsu_tx::boot::tool::SystemRunner;

use crate::cli::{self, Command, USAGE};
use crate::secureboot::{self, Context, Paths};

/// Exit status 0 done, 1 failed, 2 usage.
pub fn main(args: impl IntoIterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.into_iter().collect();
    let Some(command) = cli::parse(&args) else {
        eprintln!("tetsu: unknown command\n{USAGE}");
        return ExitCode::from(2);
    };
    let ctx = Context {
        paths: Paths::system(),
        is_root: rustix::process::geteuid().is_root(),
        runner: &SystemRunner,
        entropy: &SystemEntropy,
    };
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Enroll { without_microsoft } => secureboot::enroll(&ctx, without_microsoft),
        Command::Trust(cert) => secureboot::trust(&ctx, &cert),
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("tetsu: {message}");
            ExitCode::from(1)
        }
    }
}
