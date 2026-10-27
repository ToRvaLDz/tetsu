//! The config file: TOML text in, settings or a message naming the key.

use tetsu_comp::layout::{Gaps, LayoutKind};
use tetsu_comp::rules::{Pattern, Rule};
use tetsu_comp::settings::{Settings, SettingsError, parse};

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
