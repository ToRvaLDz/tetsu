//! The command line: argv and the host session in, a plan out.

use std::ffi::OsString;

use tetsu_comp::backend::headless::{HotplugCommand, parse_command};
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

#[test]
fn the_hotplug_hook_needs_the_headless_backend() {
    let cli = run(&["--backend", "headless", "--headless-hotplug"], false).unwrap();
    assert!(cli.headless_hotplug);
    assert!(
        !run(&["--backend", "headless"], false)
            .unwrap()
            .headless_hotplug
    );
    for backend in ["winit", "tty"] {
        assert_eq!(
            run(&["--backend", backend, "--headless-hotplug"], false).unwrap_err(),
            CliError::HeadlessOnly
        );
    }
    assert_eq!(
        run(&["--headless-hotplug"], true).unwrap_err(),
        CliError::HeadlessOnly
    );
}

#[test]
fn the_headless_only_message_names_both_options() {
    let message = CliError::HeadlessOnly.to_string();
    assert!(message.contains("--headless-outputs"), "{message}");
    assert!(message.contains("--headless-hotplug"), "{message}");
    assert!(message.contains("--backend headless"), "{message}");
}

#[test]
fn hotplug_commands_parse() {
    assert_eq!(parse_command("add"), Ok(HotplugCommand::Add { size: None }));
    assert_eq!(
        parse_command("add 1280x720"),
        Ok(HotplugCommand::Add {
            size: Some((1280, 720))
        })
    );
    assert_eq!(
        parse_command("add 800x600\n"),
        Ok(HotplugCommand::Add {
            size: Some((800, 600))
        })
    );
    assert_eq!(
        parse_command("add-named HEADLESS-1 1920x1080\r\n"),
        Ok(HotplugCommand::AddNamed {
            name: "HEADLESS-1".into(),
            size: (1920, 1080)
        })
    );
    assert_eq!(
        parse_command("remove HEADLESS-2\n"),
        Ok(HotplugCommand::Remove {
            name: "HEADLESS-2".into()
        })
    );
}

#[test]
fn focus_control_lines_parse() {
    assert_eq!(parse_command("focus-next"), Ok(HotplugCommand::FocusNext));
    assert_eq!(
        parse_command("focus-prev\r\n"),
        Ok(HotplugCommand::FocusPrev)
    );
    for bad in ["focus-next x", "focus", "focus-sideways"] {
        assert!(parse_command(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn hotplug_sides_are_one_to_sixteen_thousand_three_hundred_eighty_four() {
    for ok in ["1x1", "16384x16384", "1x16384"] {
        assert!(parse_command(&format!("add {ok}")).is_ok(), "{ok}");
    }
    for bad in [
        "0x0",
        "0x10",
        "10x0",
        "16385x10",
        "10x16385",
        "-1x5",
        "1280",
        "1280x",
        "x720",
        "axb",
        "1280x720x3",
        "1280 720",
        "+5x5",
    ] {
        assert!(parse_command(&format!("add {bad}")).is_err(), "{bad:?}");
    }
}

#[test]
fn bad_hotplug_lines_are_errors() {
    for bad in [
        "",
        "\n",
        "bogus",
        "add-named",
        "add-named DP-1",
        "add-named DP-1 0x0",
        "add-named DP-1 1920x1080 extra",
        "remove",
        "remove a b",
        "add 1920x1080 extra",
        "ADD",
    ] {
        assert!(parse_command(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_hotplug_line_is_read_lossily_and_capped() {
    use std::io::Cursor;
    use tetsu_comp::backend::headless::read_command_line;

    let mut long = vec![b'a'; 100_000];
    long.extend_from_slice(b"\nadd\n\xff\xfe\nremove DP-1");
    let mut reader = Cursor::new(long);
    // An overlong line is cut at the cap plus one, the rest is skipped.
    let first = read_command_line(&mut reader).unwrap().unwrap();
    assert_eq!(first.len(), 257);
    assert!(parse_command(&first).is_err());
    assert_eq!(
        read_command_line(&mut reader).unwrap().as_deref(),
        Some("add")
    );
    // Bytes that are not UTF-8 do not end the stream.
    let bad = read_command_line(&mut reader).unwrap().unwrap();
    assert!(parse_command(&bad).is_err());
    // The last line has no newline.
    assert_eq!(
        read_command_line(&mut reader).unwrap().as_deref(),
        Some("remove DP-1")
    );
    assert_eq!(read_command_line(&mut reader).unwrap(), None);
}

#[test]
fn add_modes_names_an_output_and_its_sizes() {
    assert_eq!(
        parse_command("add-modes HEADLESS-9 1920x1080,1280x720\n"),
        Ok(HotplugCommand::AddModes {
            name: "HEADLESS-9".into(),
            sizes: vec![(1920, 1080), (1280, 720)],
        })
    );
    assert_eq!(
        parse_command("add-modes DP-1 800x600"),
        Ok(HotplugCommand::AddModes {
            name: "DP-1".into(),
            sizes: vec![(800, 600)],
        })
    );
}

#[test]
fn add_modes_refuses_an_empty_list_a_bad_size_and_too_many_modes() {
    let too_many = format!("add-modes DP-1 {}", vec!["800x600"; 17].join(","));
    let sixteen = format!("add-modes DP-1 {}", vec!["800x600"; 16].join(","));
    for line in [
        "add-modes",
        "add-modes DP-1",
        "add-modes DP-1 ",
        "add-modes DP-1 1920x1080,",
        "add-modes DP-1 ,1920x1080",
        "add-modes DP-1 1920x1080,0x0",
        "add-modes DP-1 1920x1080,abc",
        "add-modes DP-1 1920x1080 extra",
        too_many.as_str(),
    ] {
        assert!(parse_command(line).is_err(), "{line:?} should be refused");
    }
    assert!(parse_command(&sixteen).is_ok());
}
