//! The config file: TOML text in, settings or a message naming the key.

use tetsu_comp::layout::{Gaps, LayoutKind};
use tetsu_comp::rules::{Pattern, Rule};
use tetsu_comp::settings::{DeviceChoice, Settings, SettingsError, SyncMode, parse};

fn err(text: &str) -> String {
    match parse(text) {
        Ok(settings) => panic!("accepted: {settings:?}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn an_empty_text_gives_the_defaults() {
    let settings = parse("").unwrap();
    assert_eq!(settings, Settings::default());
    assert_eq!(settings.layout, LayoutKind::Dwindle);
    assert_eq!(settings.gaps, Gaps::NONE);
    assert_eq!(settings.master_ratio, 0.55);
    assert!(settings.rules.is_empty());
}

#[test]
fn a_partial_layout_table_keeps_the_other_defaults() {
    let settings = parse("[layout]\ndefault = \"master\"\n").unwrap();
    assert_eq!(settings.layout, LayoutKind::Master);
    assert_eq!(settings.gaps, Gaps::NONE);
    assert_eq!(settings.master_ratio, 0.55);
}

#[test]
fn a_full_example_parses_into_the_settings() {
    let text = r#"
[layout]
default = "floating"
gaps_inner = 8
gaps_outer = 12
master_ratio = 0.6

[[window_rule]]
app_id = "^pavucontrol$"
floating = true
size = [640, 480]

[[window_rule]]
app_id = "firefox"
title = "Picture-in-Picture"
workspace = 3
output = "HEADLESS-2"
floating = false
"#;
    let settings = parse(text).unwrap();
    assert_eq!(settings.layout, LayoutKind::Floating);
    assert_eq!(
        settings.gaps,
        Gaps {
            outer: 12,
            inner: 8
        }
    );
    assert_eq!(settings.master_ratio, 0.6);
    assert_eq!(
        settings.rules,
        [
            Rule {
                app_id: Some(Pattern::new("^pavucontrol$").unwrap()),
                floating: Some(true),
                size: Some((640, 480)),
                ..Rule::default()
            },
            Rule {
                app_id: Some(Pattern::new("firefox").unwrap()),
                title: Some(Pattern::new("Picture-in-Picture").unwrap()),
                workspace: Some(3),
                output: Some("HEADLESS-2".into()),
                floating: Some(false),
                ..Rule::default()
            },
        ]
    );
}

#[test]
fn bad_toml_names_the_line() {
    let message = err("[layout\ndefault = 1");
    assert!(message.contains("line 1"), "{message}");
    assert!(matches!(
        parse("[layout\n").unwrap_err(),
        SettingsError::Toml(_)
    ));
}

#[test]
fn unknown_keys_are_refused() {
    assert!(err("[layout]\ncolour = 1\n").contains("colour"));
    assert!(err("[[window_rule]]\napp_id = \"a\"\nfloat = true\n").contains("float"));
    assert!(err("[theme]\n").contains("theme"));
}

#[test]
fn an_unknown_layout_names_the_key() {
    let message = err("[layout]\ndefault = \"spiral\"\n");
    assert!(message.contains("layout.default"), "{message}");
    assert!(message.contains("spiral"), "{message}");
}

#[test]
fn a_bad_regex_names_the_rule_and_the_key() {
    let message = err("[[window_rule]]\napp_id = \"ok\"\n\n[[window_rule]]\ntitle = \"(open\"\n");
    assert!(message.contains("window_rule[1].title"), "{message}");
}

#[test]
fn a_rule_without_a_matcher_is_refused() {
    let message = err("[[window_rule]]\nfloating = true\n");
    assert!(message.contains("window_rule[0]"), "{message}");
    assert!(message.contains("app_id"), "{message}");
}

#[test]
fn a_workspace_outside_one_to_nine_is_refused() {
    for value in [0, 10, -1] {
        let text = format!("[[window_rule]]\napp_id = \"a\"\nworkspace = {value}\n");
        let message = err(&text);
        assert!(message.contains("window_rule[0].workspace"), "{message}");
    }
    assert!(parse("[[window_rule]]\napp_id = \"a\"\nworkspace = 9\n").is_ok());
}

#[test]
fn a_size_must_be_two_positive_numbers() {
    for value in ["[0, 480]", "[640, -1]"] {
        let message = err(&format!(
            "[[window_rule]]\napp_id = \"a\"\nsize = {value}\n"
        ));
        assert!(message.contains("window_rule[0].size"), "{message}");
    }
    assert!(matches!(
        parse("[[window_rule]]\napp_id = \"a\"\nsize = [640]\n").unwrap_err(),
        SettingsError::Toml(_)
    ));
}

#[test]
fn the_master_ratio_must_be_between_a_tenth_and_nine_tenths() {
    for value in ["0.05", "0.95", "-1.0"] {
        let message = err(&format!("[layout]\nmaster_ratio = {value}\n"));
        assert!(message.contains("layout.master_ratio"), "{message}");
    }
    assert!(parse("[layout]\nmaster_ratio = 0.1\n").is_ok());
    assert!(parse("[layout]\nmaster_ratio = 0.9\n").is_ok());
}

#[test]
fn gaps_cannot_be_negative() {
    assert!(err("[layout]\ngaps_inner = -1\n").contains("layout.gaps_inner"));
    assert!(err("[layout]\ngaps_outer = -4\n").contains("layout.gaps_outer"));
}

#[test]
fn a_missing_file_is_an_error_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.toml");
    let e = tetsu_comp::settings::load(&path).unwrap_err();
    assert!(matches!(e, SettingsError::Io(_)));
    std::fs::write(&path, "[layout]\ngaps_outer = 3\n").unwrap();
    assert_eq!(tetsu_comp::settings::load(&path).unwrap().gaps.outer, 3);
}

#[test]
fn the_render_defaults_are_auto_auto_and_no_wait() {
    let render = parse("").unwrap().render;
    assert_eq!(render.device, DeviceChoice::Auto);
    assert_eq!(render.explicit_sync, SyncMode::Auto);
    assert!(!render.wait_for_frame_completion);
}

#[test]
fn a_render_table_parses_each_valid_value() {
    let text = "[render]\ndevice = \"/dev/dri/renderD129\"\nexplicit_sync = \"off\"\nwait_for_frame_completion = true\n";
    let render = parse(text).unwrap().render;
    assert_eq!(
        render.device,
        DeviceChoice::Node("/dev/dri/renderD129".into())
    );
    assert_eq!(render.explicit_sync, SyncMode::Off);
    assert!(render.wait_for_frame_completion);

    let render = parse("[render]\ndevice = \"auto\"\nexplicit_sync = \"on\"\n")
        .unwrap()
        .render;
    assert_eq!(render.device, DeviceChoice::Auto);
    assert_eq!(render.explicit_sync, SyncMode::On);
    assert_eq!(
        parse("[render]\nexplicit_sync = \"auto\"\n")
            .unwrap()
            .render
            .explicit_sync,
        SyncMode::Auto
    );
}

#[test]
fn a_bad_render_device_names_the_key() {
    for bad in ["card0", "/etc/passwd", "/dev/dri/../sda", "", "/dev/dri/"] {
        let message = err(&format!("[render]\ndevice = {bad:?}\n"));
        assert!(message.contains("render.device"), "{bad}: {message}");
    }
}

#[test]
fn an_unknown_sync_mode_names_the_key() {
    let message = err("[render]\nexplicit_sync = \"force\"\n");
    assert!(message.contains("render.explicit_sync"), "{message}");
    assert!(matches!(
        parse("[render]\nexplicit_sync = \"force\"\n"),
        Err(SettingsError::UnknownSyncMode(_))
    ));
}

#[test]
fn an_unknown_render_key_is_refused() {
    let message = err("[render]\nvrr = true\n");
    assert!(message.contains("vrr"), "{message}");
}

#[test]
fn the_lock_and_trust_defaults_are_swaylock_and_nothing() {
    let settings = parse("").unwrap();
    assert_eq!(settings.lock.command, ["swaylock"]);
    assert!(settings.trust.input.is_empty());
}

#[test]
fn a_lock_and_trust_table_parses() {
    let text = "[lock]\ncommand = [\"gtklock\", \"-d\"]\n\n[trust]\ninput = [[\"wtype\"], [\"/usr/bin/wayvnc\", \"--output=X\"]]\n";
    let settings = parse(text).unwrap();
    assert_eq!(settings.lock.command, ["gtklock", "-d"]);
    assert_eq!(
        settings.trust.input,
        [vec!["wtype"], vec!["/usr/bin/wayvnc", "--output=X"]]
    );
}

#[test]
fn an_empty_lock_command_means_no_relaunch() {
    let settings = parse("[lock]\ncommand = []\n").unwrap();
    assert!(settings.lock.command.is_empty());
}

#[test]
fn an_empty_argv_in_trust_input_is_refused_naming_the_key() {
    let message = err("[trust]\ninput = [[\"wtype\"], []]\n");
    assert!(message.contains("trust.input[1]"), "{message}");
    let message = err("[trust]\ninput = [[\"\"]]\n");
    assert!(message.contains("trust.input[0]"), "{message}");
}

#[test]
fn a_blank_lock_program_is_refused_naming_the_key() {
    let message = err("[lock]\ncommand = [\"\"]\n");
    assert!(message.contains("lock.command"), "{message}");
}

#[test]
fn unknown_lock_and_trust_keys_are_refused() {
    assert!(err("[lock]\nidle = 5\n").contains("idle"));
    assert!(err("[trust]\nall = true\n").contains("all"));
}

#[test]
fn the_decoration_defaults_are_two_pixels_and_the_two_constants() {
    use tetsu_comp::decor::{ACTIVE_COLOR, INACTIVE_COLOR};
    let d = parse("").unwrap().decorations;
    assert_eq!(d.width, 2);
    assert_eq!((d.active, d.inactive), (ACTIVE_COLOR, INACTIVE_COLOR));
}

#[test]
fn a_decorations_table_parses() {
    let d = parse("[decorations]\nwidth = 4\nactive_color = \"#ff0000\"\n")
        .unwrap()
        .decorations;
    assert_eq!(d.width, 4);
    assert_eq!(
        (d.active.r, d.active.g, d.active.b, d.active.a),
        (255, 0, 0, 255)
    );
}

#[test]
fn a_border_width_outside_zero_to_thirty_two_is_refused() {
    assert!(err("[decorations]\nwidth = -1\n").starts_with("decorations.width:"));
    assert!(err("[decorations]\nwidth = 33\n").starts_with("decorations.width:"));
    assert_eq!(
        parse("[decorations]\nwidth = 0\n")
            .unwrap()
            .decorations
            .width,
        0
    );
    assert_eq!(
        parse("[decorations]\nwidth = 32\n")
            .unwrap()
            .decorations
            .width,
        32
    );
}

#[test]
fn a_bad_decoration_colour_names_the_key() {
    assert!(
        err("[decorations]\nactive_color = \"blue\"\n").starts_with("decorations.active_color:")
    );
    assert!(
        err("[decorations]\ninactive_color = \"#12\"\n").starts_with("decorations.inactive_color:")
    );
}

#[test]
fn an_unknown_decorations_key_is_refused() {
    assert!(err("[decorations]\ntitle = true\n").contains("title"));
}

#[test]
fn the_idle_timeout_defaults_to_ten_minutes() {
    let lock = parse("").unwrap().lock;
    assert_eq!(lock.idle_timeout_s, 600);
    assert_eq!(
        lock.idle_timeout(),
        Some(std::time::Duration::from_secs(600))
    );
}

#[test]
fn an_idle_timeout_of_zero_turns_it_off() {
    let lock = parse("[lock]\nidle_timeout_s = 0\n").unwrap().lock;
    assert_eq!(lock.idle_timeout_s, 0);
    assert_eq!(lock.idle_timeout(), None);
    let lock = parse("[lock]\nidle_timeout_s = 30\n").unwrap().lock;
    assert_eq!(lock.idle_timeout_s, 30);
    assert_eq!(
        lock.idle_timeout(),
        Some(std::time::Duration::from_secs(30))
    );
}

#[test]
fn an_idle_timeout_outside_zero_or_30_to_a_day_is_refused_naming_the_range() {
    for text in [
        "[lock]\nidle_timeout_s = -1\n",
        "[lock]\nidle_timeout_s = 1\n",
        "[lock]\nidle_timeout_s = 29\n",
        "[lock]\nidle_timeout_s = 86401\n",
    ] {
        let message = err(text);
        assert!(message.contains("lock.idle_timeout_s"), "{message}");
        assert!(message.contains("0 or 30..=86400"), "{message}");
    }
    assert_eq!(
        parse("[lock]\nidle_timeout_s = 86400\n")
            .unwrap()
            .lock
            .idle_timeout_s,
        86_400
    );
}

#[test]
fn an_idle_timeout_that_is_not_a_number_is_refused_naming_the_key() {
    let message = err("[lock]\nidle_timeout_s = \"10m\"\n");
    assert!(message.contains("idle_timeout_s"), "{message}");
}

#[test]
fn the_satellite_defaults_to_xwayland_satellite_on_path() {
    let settings = parse("").unwrap();
    assert_eq!(settings.xwayland.satellite, "xwayland-satellite");
    assert_eq!(settings.xwayland.program(), Some("xwayland-satellite"));
}

#[test]
fn an_empty_satellite_turns_x11_off() {
    let settings = parse("[xwayland]\nsatellite = \"\"\n").unwrap();
    assert_eq!(settings.xwayland.program(), None);
    let settings = parse("[xwayland]\nsatellite = \"/opt/sat\"\n").unwrap();
    assert_eq!(settings.xwayland.program(), Some("/opt/sat"));
}

#[test]
fn a_blank_satellite_is_refused_naming_the_key() {
    let message = err("[xwayland]\nsatellite = \"  \"\n");
    assert!(message.contains("xwayland.satellite"), "{message}");
}

#[test]
fn unknown_xwayland_keys_are_refused() {
    assert!(err("[xwayland]\nshell = true\n").contains("shell"));
}
