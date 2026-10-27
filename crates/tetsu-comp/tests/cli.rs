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
fn the_config_path_and_the_output_count_are_options() {
    let cli = run(&["--config", "/etc/tetsu.toml"], false).unwrap();
    assert_eq!(cli.config, Some("/etc/tetsu.toml".into()));
    let cli = run(&["--config=rel.toml", "--backend=headless"], false).unwrap();
    assert_eq!(cli.config, Some("rel.toml".into()));
    assert_eq!(run(&[], false).unwrap().config, None);

    assert_eq!(run(&[], false).unwrap().headless_outputs, 1);
    let cli = run(&["--backend", "headless", "--headless-outputs", "2"], false).unwrap();
    assert_eq!(cli.headless_outputs, 2);
    let cli = run(&["--headless-outputs=4", "--backend=headless"], false).unwrap();
    assert_eq!(cli.headless_outputs, 4);
}

#[test]
fn bad_values_for_the_new_options_are_errors() {
    assert_eq!(
        run(&["--config"], false).unwrap_err(),
        CliError::MissingValue("--config")
    );
    assert_eq!(
        run(&["--headless-outputs"], false).unwrap_err(),
        CliError::MissingValue("--headless-outputs")
    );
    for bad in ["0", "5", "two", "-1", ""] {
        assert_eq!(
            run(&["--backend", "headless", "--headless-outputs", bad], false).unwrap_err(),
            CliError::BadNumber("--headless-outputs", bad.into()),
            "{bad:?}"
        );
    }
}

#[test]
fn extra_headless_outputs_need_the_headless_backend() {
    for backend in ["winit", "tty"] {
        assert_eq!(
            run(&["--backend", backend, "--headless-outputs", "2"], false).unwrap_err(),
            CliError::HeadlessOnly
        );
    }
    // The default backend counts too.
    assert_eq!(
        run(&["--headless-outputs", "2"], true).unwrap_err(),
        CliError::HeadlessOnly
    );
}

#[test]
fn help_is_not_a_run() {
    assert!(matches!(parse(args(&["--help"]), false), Ok(Parsed::Help)));
    assert!(matches!(parse(args(&["-h"]), true), Ok(Parsed::Help)));
}
