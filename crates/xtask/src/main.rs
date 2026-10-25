//! Repository tasks: `cargo xtask <task>`.

use std::path::PathBuf;
use std::process::ExitCode;

use tetsu_bus_contract::{BlessError, bless};

const USAGE: &str = "usage: cargo xtask bless-contract";

/// The IDL directory of tetsu-bus, found from this crate's own location so
/// it does not depend on the working directory.
fn idl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl")
}

fn bless_contract() -> ExitCode {
    match bless(&idl_dir()) {
        Ok(changed) if changed.is_empty() => {
            println!("baseline already matches the current IDL, nothing to bless");
            ExitCode::SUCCESS
        }
        Ok(changed) => {
            println!("blessed {} file(s) into idl/baseline/:", changed.len());
            for name in changed {
                println!("  {name}");
            }
            println!("commit them as `chore(contract): bless v1 baseline`");
            ExitCode::SUCCESS
        }
        Err(BlessError::Violations(violations)) => {
            eprintln!("refusing to bless, the baseline is untouched:");
            for v in violations {
                eprintln!("  {v}");
            }
            ExitCode::FAILURE
        }
        Err(e @ BlessError::Io(_)) => {
            // The message says what state the baseline was left in.
            eprintln!("bless failed: {e}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("refusing to bless, the baseline is untouched: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("bless-contract"), None) => bless_contract(),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
