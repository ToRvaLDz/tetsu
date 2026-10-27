//! Repository tasks: `cargo xtask <task>`.

use std::path::PathBuf;
use std::process::ExitCode;

use tetsu_bus_contract::{BlessError, bless, load_dir, polkit_policy};

const USAGE: &str = "usage: cargo xtask bless-contract | polkit-policy <out>";

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

/// Scrive il file polkit generato dall'IDL in `out`.
fn polkit_policy_task(out: &str) -> ExitCode {
    let contract = match load_dir(&idl_dir()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("polkit-policy: cannot load the IDL: {e}");
            return ExitCode::FAILURE;
        }
    };
    let xml = match polkit_policy(&contract) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("polkit-policy: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(out, xml) {
        eprintln!("polkit-policy: cannot write {out}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("bless-contract"), None) => bless_contract(),
        (Some("polkit-policy"), Some(out)) if args.next().is_none() => polkit_policy_task(&out),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
