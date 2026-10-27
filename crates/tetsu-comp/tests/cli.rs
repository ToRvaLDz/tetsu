//! The command line: argv and the host session in, a plan out.

use std::ffi::OsString;

use tetsu_comp::cli::{Backend, CliError, Parsed, parse};

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn run(list: &[&str], hosted: bool) -> Result<tetsu_comp::cli::Cli, CliError> {
    match parse(args(list), hosted)? {
        Parsed::Run(cli) => Ok(cli),
        Parsed::Help => panic!("unexpected help"),
    }
}

#[test]
fn the_backend_defaults_to_winit_inside_a_session_and_tty_outside() {
    assert_eq!(run(&[], true).unwrap().backend, Backend::Winit);
    assert_eq!(run(&[], false).unwrap().backend, Backend::Tty);
}

#[test]
fn the_backend_can_be_named() {
    for (name, backend) in [
        ("winit", Backend::Winit),
        ("tty", Backend::Tty),
        ("headless", Backend::Headless),
    ] {
        assert_eq!(run(&["--backend", name], false).unwrap().backend, backend);
        let joined = format!("--backend={name}");
        assert_eq!(run(&[&joined], true).unwrap().backend, backend);
    }
}

#[test]
fn what_follows_a_double_dash_is_the_client_command() {
    let cli = run(&["--backend", "headless", "--", "foot", "-e", "top"], false).unwrap();
    assert_eq!(cli.command, args(&["foot", "-e", "top"]));
    assert!(run(&[], false).unwrap().command.is_empty());
    // Options after the double dash belong to the client.
    let cli = run(&["--", "foot", "--backend", "x"], false).unwrap();
    assert_eq!(cli.command, args(&["foot", "--backend", "x"]));
}

#[test]
fn bad_arguments_are_errors() {
    assert_eq!(
        run(&["--backend", "wayland"], false).unwrap_err(),
        CliError::UnknownBackend("wayland".into())
    );
    assert_eq!(
        run(&["--backend"], false).unwrap_err(),
        CliError::MissingValue("--backend")
    );
    assert_eq!(
        run(&["--frobnicate"], false).unwrap_err(),
        CliError::UnknownOption("--frobnicate".into())
    );
    assert_eq!(
        run(&["foot"], false).unwrap_err(),
        CliError::UnknownOption("foot".into())
    );
}

#[test]
fn help_is_not_a_run() {
    assert!(matches!(parse(args(&["--help"]), false), Ok(Parsed::Help)));
    assert!(matches!(parse(args(&["-h"]), true), Ok(Parsed::Help)));
}
