//! One config layer: TOML text in, valid values and per-key diagnostics out.

use tetsu_comp::config::keys::{KEYS, lookup};
use tetsu_comp::config::layer::parse;

#[test]
fn every_key_is_known_with_its_writable_flag_and_method() {
    let expected: [(&str, bool, Option<&str>); 14] = [
        ("layout.default", true, None),
        ("layout.gaps_inner", true, None),
        ("layout.gaps_outer", true, None),
        ("layout.master_ratio", true, None),
        ("render.wait_for_frame_completion", true, None),
        ("window_rule", true, None),
        ("decorations.width", true, None),
        ("decorations.active_color", true, None),
        ("decorations.inactive_color", true, None),
        ("lock.idle_timeout_s", true, None),
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
    assert!(!lookup("lock.idle_timeout_s").unwrap().runs_commands);
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

const CONFIGURE: &str = "org.tetsu.comp.output.v1.ConfigureOutputs";

#[test]
fn an_output_table_validates_into_per_field_keys() {
    let parsed = parse(
        "[output.DP-1]\nenabled = true\nscale = 1.25\ntransform = \"rotate_90\"\n\
         mode = { width = 3840, height = 2160, refresh_mhz = 60000 }\n\
         position = { x = -1920, y = 0 }\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let keys: Vec<_> = parsed.values.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "output.DP-1.enabled",
            "output.DP-1.mode",
            "output.DP-1.position",
            "output.DP-1.scale",
            "output.DP-1.transform",
        ]
    );
    assert_eq!(parsed.values["output.DP-1.scale"], toml::Value::Float(1.25));
}

#[test]
fn a_bad_output_entry_gives_one_diagnostic_and_keeps_the_rest() {
    let cases = [
        (
            "[output.DP-1]\nbogus = 1\nscale = 2.0\n",
            "output.DP-1.bogus",
        ),
        ("[output.\"DP.1\"]\nscale = 2.0\n", "output.DP.1"),
        ("[output.\"\"]\nscale = 2.0\n", "output."),
        (
            "[output.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]\nscale = 2.0\n",
            "output.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        ("[output.DP-1]\nscale = 0\n", "output.DP-1.scale"),
        (
            "[output.DP-1]\ntransform = \"sideways\"\n",
            "output.DP-1.transform",
        ),
        (
            "[output.DP-1]\nmode = { width = 1920, height = 1080 }\n",
            "output.DP-1.mode",
        ),
    ];
    for (text, key) in cases {
        let parsed = parse(&format!("{text}[layout]\ngaps_outer = 3\n"));
        assert_eq!(
            parsed.diagnostics.len(),
            1,
            "{text}: {:?}",
            parsed.diagnostics
        );
        let d = &parsed.diagnostics[0];
        assert_eq!(d.key.as_deref(), Some(key), "{text}");
        assert!(d.line.is_some(), "{text}");
        assert!(parsed.values.contains_key("layout.gaps_outer"), "{text}");
        assert!(
            !parsed.values.contains_key(key),
            "{text}: bad value was kept"
        );
    }
}

#[test]
fn a_bad_output_field_does_not_cost_its_neighbours() {
    let parsed = parse("[output.DP-1]\nscale = 0\ntransform = \"rotate_90\"\n");
    assert_eq!(parsed.diagnostics.len(), 1);
    assert!(parsed.values.contains_key("output.DP-1.transform"));
}

#[test]
fn a_non_table_output_is_one_diagnostic() {
    let parsed = parse("output = 3\n[layout]\ngaps_outer = 3\n");
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].key.as_deref(), Some("output"));
    assert!(parsed.values.contains_key("layout.gaps_outer"));
    let parsed = parse("[output]\nDP-1 = 3\n");
    assert_eq!(parsed.diagnostics.len(), 1);
}

#[test]
fn an_output_key_is_looked_up_and_is_not_writable() {
    let key = lookup("output.DP-1.scale").expect("the key is known");
    assert!(!key.writable);
    assert_eq!(key.method, Some(CONFIGURE));
    assert!(!key.runs_commands);
    assert!(lookup("output.DP-1.nope").is_none());
    assert!(lookup("output.DP.1.scale").is_none());
    assert!(lookup("output.scale").is_none());
}

#[test]
fn a_bad_decoration_colour_keeps_the_valid_width() {
    let parsed = parse("[decorations]\nwidth = 4\ninactive_color = \"grey\"\n");
    assert_eq!(parsed.values.len(), 1);
    assert_eq!(parsed.values["decorations.width"], toml::Value::Integer(4));
    assert_eq!(parsed.diagnostics.len(), 1);
    let d = &parsed.diagnostics[0];
    assert_eq!(
        (d.line, d.key.as_deref()),
        (Some(3), Some("decorations.inactive_color"))
    );
}

#[test]
fn settings_read_the_decoration_keys_and_fall_back_on_bad_ones() {
    use std::collections::BTreeMap;
    use tetsu_comp::decor::{ACTIVE_COLOR, Color};
    use tetsu_comp::settings::Settings;

    let mut values = BTreeMap::new();
    values.insert("decorations.width".to_owned(), toml::Value::Integer(6));
    values.insert(
        "decorations.active_color".to_owned(),
        toml::Value::String("#ff000080".into()),
    );
    values.insert(
        "decorations.inactive_color".to_owned(),
        toml::Value::String("#101010".into()),
    );
    let d = Settings::from_values(&values).decorations;
    assert_eq!(d.width, 6);
    assert_eq!(
        d.active,
        Color {
            r: 255,
            g: 0,
            b: 0,
            a: 0x80
        }
    );
    assert_eq!(
        d.inactive,
        Color {
            r: 16,
            g: 16,
            b: 16,
            a: 255
        }
    );

    values.insert("decorations.width".to_owned(), toml::Value::Integer(99));
    values.insert(
        "decorations.active_color".to_owned(),
        toml::Value::String("blue".into()),
    );
    let d = Settings::from_values(&values).decorations;
    assert_eq!(d.width, 2);
    assert_eq!(d.active, ACTIVE_COLOR);
}
