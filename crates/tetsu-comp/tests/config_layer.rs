//! One config layer: TOML text in, valid values and per-key diagnostics out.

use tetsu_comp::config::keys::{KEYS, lookup};
use tetsu_comp::config::layer::parse;

#[test]
fn every_key_is_known_with_its_writable_flag_and_method() {
    let expected: [(&str, bool, Option<&str>); 10] = [
        ("layout.default", true, None),
        ("layout.gaps_inner", true, None),
        ("layout.gaps_outer", true, None),
        ("layout.master_ratio", true, None),
        ("render.wait_for_frame_completion", true, None),
        ("window_rule", true, None),
        (
            "render.device",
            false,
            Some("org.tetsu.comp.render.v1.SetRenderDevice"),
        ),
        (
            "render.explicit_sync",
            false,
            Some("org.tetsu.comp.render.v1.SetExplicitSync"),
        ),
        ("lock.command", false, None),
        ("trust.input", false, None),
    ];
    assert_eq!(KEYS.len(), expected.len());
    for (name, writable, method) in expected {
        let key = lookup(name).unwrap_or_else(|| panic!("{name} is unknown"));
        assert_eq!(key.writable, writable, "{name}");
        assert_eq!(key.method, method, "{name}");
    }
    assert!(lookup("layout.nope").is_none());
}

#[test]
fn command_keys_are_recognised() {
    for name in ["lock.command", "trust.input"] {
        assert!(lookup(name).unwrap().runs_commands, "{name}");
    }
    assert!(!lookup("layout.default").unwrap().runs_commands);
}

#[test]
fn a_good_file_gives_its_values_and_no_diagnostics() {
    let parsed = parse("[layout]\ngaps_inner = 8\ndefault = \"master\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(parsed.values.len(), 2);
    assert_eq!(parsed.values["layout.gaps_inner"], toml::Value::Integer(8));
    assert_eq!(
        parsed.values["layout.default"],
        toml::Value::String("master".into())
    );
}

#[test]
fn an_empty_text_is_an_empty_layer() {
    let parsed = parse("");
    assert!(parsed.values.is_empty());
    assert!(parsed.diagnostics.is_empty());
}

#[test]
fn a_syntax_error_gives_one_diagnostic_with_a_line_and_no_key() {
    let parsed = parse("[layout]\ngaps_inner =\n");
    assert!(parsed.values.is_empty());
    assert_eq!(parsed.diagnostics.len(), 1);
    let d = &parsed.diagnostics[0];
    assert_eq!(d.line, Some(2));
    assert_eq!(d.key, None);
    assert!(!d.message.is_empty());
}

#[test]
fn an_unknown_key_is_dropped_and_the_others_are_kept() {
    let parsed = parse("[layout]\ngaps_inner = 5\nbogus = 1\n");
    assert_eq!(parsed.values.len(), 1);
    assert!(parsed.values.contains_key("layout.gaps_inner"));
    assert_eq!(parsed.diagnostics.len(), 1);
    let d = &parsed.diagnostics[0];
    assert_eq!((d.line, d.key.as_deref()), (Some(3), Some("layout.bogus")));
    assert!(d.message.contains("unknown key"), "{}", d.message);
}

#[test]
fn an_unknown_table_is_dropped_with_its_line() {
    let parsed = parse("\n[nope]\nx = 1\n");
    assert!(parsed.values.is_empty());
    let d = &parsed.diagnostics[0];
    assert_eq!((d.line, d.key.as_deref()), (Some(2), Some("nope")));
}

#[test]
fn a_bad_value_is_dropped_with_the_existing_message() {
    let parsed = parse("[layout]\nmaster_ratio = 5.0\ngaps_outer = 2\n");
    assert_eq!(parsed.values.len(), 1);
    assert!(parsed.values.contains_key("layout.gaps_outer"));
    let d = &parsed.diagnostics[0];
    assert_eq!(
        (d.line, d.key.as_deref()),
        (Some(2), Some("layout.master_ratio"))
    );
    assert!(
        d.message.starts_with("layout.master_ratio: 5 is outside"),
        "{}",
        d.message
    );
}

#[test]
fn a_value_of_the_wrong_type_is_dropped() {
    let parsed = parse("[layout]\ngaps_inner = \"wide\"\n");
    assert!(parsed.values.is_empty());
    let d = &parsed.diagnostics[0];
    assert_eq!(d.key.as_deref(), Some("layout.gaps_inner"));
    assert!(d.message.starts_with("layout.gaps_inner:"), "{}", d.message);
}

#[test]
fn an_integer_is_a_valid_master_ratio_only_in_range() {
    let parsed = parse("[layout]\nmaster_ratio = 1\n");
    assert!(parsed.values.is_empty());
    assert_eq!(parsed.diagnostics.len(), 1);
}

#[test]
fn a_bad_window_rule_drops_the_whole_array() {
    let text = "[[window_rule]]\napp_id = \"foot\"\n\n[[window_rule]]\napp_id = \"(\"\n";
    let parsed = parse(text);
    assert!(!parsed.values.contains_key("window_rule"));
    let d = &parsed.diagnostics[0];
    assert_eq!(d.key.as_deref(), Some("window_rule"));
    assert_eq!(d.line, Some(4));
    assert!(
        d.message.starts_with("window_rule[1].app_id: bad regex"),
        "{}",
        d.message
    );
}

#[test]
fn good_window_rules_are_one_array_value() {
    let parsed = parse("[[window_rule]]\napp_id = \"foot\"\nfloating = true\n");
    assert!(parsed.diagnostics.is_empty());
    let rules = parsed.values["window_rule"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
}

#[test]
fn render_and_command_keys_are_validated() {
    let parsed = parse(
        "[render]\ndevice = \"/etc/passwd\"\nexplicit_sync = \"maybe\"\n\
         [lock]\ncommand = [\" \"]\n[trust]\ninput = [[\"helper\"]]\n",
    );
    let keys: Vec<_> = parsed
        .diagnostics
        .iter()
        .filter_map(|d| d.key.as_deref())
        .collect();
    assert_eq!(
        keys,
        ["render.device", "render.explicit_sync", "lock.command"]
    );
    assert!(parsed.values.contains_key("trust.input"));
}
