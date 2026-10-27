//! tetsu-comp binary.

use std::process::ExitCode;

use tetsu_comp::cli::{self, Parsed, USAGE};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let hosted = ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()));
    match cli::parse(std::env::args_os().skip(1), hosted) {
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("tetsu-comp: {e}\n{USAGE}");
            ExitCode::from(2)
        }
        Ok(Parsed::Run(cli)) => match tetsu_comp::run::run(&cli) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("tetsu-comp: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
