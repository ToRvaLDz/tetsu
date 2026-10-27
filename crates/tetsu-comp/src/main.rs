//! tetsu-comp binary.

use std::path::Path;
use std::process::ExitCode;

use tetsu_comp::cli::{self, Cli, Parsed, USAGE};
use tetsu_comp::config::paths::Paths;
use tetsu_comp::config::store::Store;
use tetsu_comp::settings;

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
        Ok(Parsed::Run(cli)) => match load(&cli) {
            Ok(store) => run(&cli, store),
            Err(message) => {
                eprintln!("tetsu-comp: {message}");
                ExitCode::from(2)
            }
        },
    }
}

/// Reads the layers. An explicit `--config <file>` is the user layer and
/// is strict: any problem in it stops the start. The user file found by
/// XDG is lenient: its problems are logged and published.
fn load(cli: &Cli) -> Result<Store, String> {
    let paths = Paths::from_process_env();
    let Some(file) = &cli.config else {
        let store = Store::new(paths);
        log_problems(store.diagnostics(), store.user_path());
        return Ok(store);
    };
    settings::load(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let store = Store::new(paths.with_user(file.clone()));
    match store.diagnostics().first() {
        Some(d) => Err(format!("{}: {}", file.display(), d.message)),
        None => Ok(store),
    }
}

fn log_problems(diagnostics: &[tetsu_bus::config::Diagnostic], path: Option<&Path>) {
    let name = path.map_or_else(|| "user config".to_owned(), |p| p.display().to_string());
    for d in diagnostics {
        match d.line {
            Some(line) => eprintln!("tetsu-comp: {name}:{line}: {}", d.message),
            None => eprintln!("tetsu-comp: {name}: {}", d.message),
        }
    }
}

fn run(cli: &Cli, store: Store) -> ExitCode {
    match tetsu_comp::run::run(cli, store) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tetsu-comp: {e}");
            ExitCode::FAILURE
        }
    }
}
