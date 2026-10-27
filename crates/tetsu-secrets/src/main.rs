//! tetsu-secrets binary: `serve` (the broker) and `init` (first login).

use std::process::ExitCode;

use tetsu_secrets::daemon::{Env, init_command, serve};

const USAGE: &str = "usage: tetsu-secrets serve | init [--seal auto|host]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") if args.len() == 1 => match Env::from_process() {
            Ok(env) => serve(&env),
            Err(e) => {
                eprintln!("tetsu-secrets: {e}");
                ExitCode::FAILURE
            }
        },
        Some("init") => init_command(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
