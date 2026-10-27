//! The merge of the three layers: nearest wins, key by key.

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::json;
use tetsu_bus::config::{GetConfigReply, Origin};
use tetsu_comp::config::keys::KEYS;
use tetsu_comp::config::layer::parse;
use tetsu_comp::config::merge::{View, diff, merge};
use tetsu_comp::config::paths::Paths;
use tetsu_comp::layout::Gaps;
use tetsu_comp::settings::Settings;

fn layer(text: &str) -> BTreeMap<String, toml::Value> {
    let parsed = parse(text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    parsed.values
}

fn view(defaults: &str, managed: &str, user: &str) -> View {
    merge(&layer(defaults), &layer(managed), &layer(user))
}

#[test]
fn with_no_layer_every_key_has_its_built_in_value() {
    let view = view("", "", "");
    let entries = view.entries();
    assert_eq!(entries.len(), KEYS.len());
    for (entry, key) in entries.iter().zip(KEYS) {
        assert_eq!(entry.key, key.name);
        assert_eq!(entry.origin, Origin::Default, "{}", key.name);
        assert_eq!(entry.default_value.as_ref(), Some(&entry.value));
        assert_eq!(entry.managed_value, None);
        assert_eq!(entry.user_value, None);
        assert_eq!(entry.writable, key.writable);
        assert_eq!(entry.method.as_deref(), key.method);
    }
    assert_eq!(
        view.entry("layout.default").unwrap().value,
        json!("dwindle")
    );
    assert_eq!(Settings::from_values(&view.values()), Settings::default());
}

#[test]
fn the_nearest_layer_wins_key_by_key() {
    let view = view(
        "[layout]\ngaps_inner = 1\n",
        "[layout]\ngaps_inner = 2\ngaps_outer = 3\n",
        "[layout]\ngaps_inner = 4\n",
    );
    let inner = view.entry("layout.gaps_inner").unwrap();
    assert_eq!(inner.origin, Origin::User);
    assert_eq!(inner.value, json!(4));
    assert_eq!(inner.default_value, Some(json!(1)));
    assert_eq!(inner.managed_value, Some(json!(2)));
    assert_eq!(inner.user_value, Some(json!(4)));
    let outer = view.entry("layout.gaps_outer").unwrap();
    assert_eq!(outer.origin, Origin::Managed);
    assert_eq!(outer.value, json!(3));
    assert_eq!(outer.default_value, Some(json!(0)));
    assert_eq!(outer.user_value, None);
    let settings = Settings::from_values(&view.values());
    assert_eq!(settings.gaps, Gaps { inner: 4, outer: 3 });
}

#[test]
fn the_image_file_is_over_the_built_in_values_and_still_a_default() {
    let view = view("[layout]\ndefault = \"master\"\n", "", "");
    let entry = view.entry("layout.default").unwrap();
    assert_eq!(entry.origin, Origin::Default);
    assert_eq!(entry.value, json!("master"));
    assert_eq!(entry.default_value, Some(json!("master")));
}

#[test]
fn a_managed_command_key_is_ignored() {
    let managed = layer("[trust]\ninput = [[\"evil\"]]\n[lock]\ncommand = [\"evil\"]\n");
    let view = merge(&BTreeMap::new(), &managed, &BTreeMap::new());
    for key in ["lock.command", "trust.input"] {
        let entry = view.entry(key).unwrap();
        assert_eq!(entry.origin, Origin::Default, "{key}");
        assert_eq!(entry.managed_value, None, "{key}");
    }
    assert_eq!(
        view.entry("lock.command").unwrap().value,
        json!(["swaylock"])
    );
}

#[test]
fn a_user_command_key_applies() {
    let view = view("", "", "[lock]\ncommand = [\"mylock\"]\n");
    assert_eq!(view.entry("lock.command").unwrap().origin, Origin::User);
    let settings = Settings::from_values(&view.values());
    assert_eq!(settings.lock.command, ["mylock"]);
}

#[test]
fn window_rules_are_replaced_as_a_whole() {
    let view = view(
        "[[window_rule]]\napp_id = \"a\"\n[[window_rule]]\napp_id = \"b\"\n",
        "[[window_rule]]\napp_id = \"c\"\n",
        "",
    );
    let entry = view.entry("window_rule").unwrap();
    assert_eq!(entry.origin, Origin::Managed);
    assert_eq!(entry.value, json!([{ "app_id": "c" }]));
    assert_eq!(Settings::from_values(&view.values()).rules.len(), 1);
}

#[test]
fn the_user_keys_are_listed_sorted() {
    let view = view(
        "",
        "",
        "[layout]\nmaster_ratio = 0.6\ndefault = \"master\"\n",
    );
    assert_eq!(view.user_keys(), ["layout.default", "layout.master_ratio"]);
}

#[test]
fn the_entries_pass_the_get_config_contract() {
    let view = view(
        "[layout]\ngaps_inner = 1\n",
        "[layout]\ngaps_outer = 3\n",
        "[[window_rule]]\napp_id = \"x\"\n",
    );
    let reply = GetConfigReply {
        entries: view.entries().to_vec(),
        diagnostics: vec![],
    };
    common::check(
        &common::contract(),
        "org.tetsu.comp.config.v1.GetConfig",
        &serde_json::to_value(&reply).unwrap(),
    );
}

#[test]
fn a_diff_has_a_changed_for_each_key_that_differs_only() {
    let before = view("", "", "");
    let after = view("", "[layout]\ngaps_outer = 10\n", "");
    let events = diff(&before, &after);
    assert_eq!(events.len(), 1);
    let entry = events[0].entry.as_ref().unwrap();
    assert_eq!(entry.key, "layout.gaps_outer");
    assert_eq!(entry.origin, Origin::Managed);
    assert!(diff(&after, &after).is_empty());
}

fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| OsString::from(*v))
    }
}

#[test]
fn the_paths_follow_xdg_with_home_fallbacks() {
    let paths = Paths::from_env(env(&[("HOME", "/home/u")]));
    assert_eq!(paths.defaults, PathBuf::from("/usr/share/tetsu/comp.toml"));
    assert_eq!(
        paths.managed,
        Some(PathBuf::from("/home/u/.local/state/tetsu/comp.toml"))
    );
    assert_eq!(
        paths.user,
        Some(PathBuf::from("/home/u/.config/tetsu/comp.toml"))
    );
    let paths = Paths::from_env(env(&[
        ("XDG_CONFIG_HOME", "/c"),
        ("XDG_STATE_HOME", "/s"),
        ("TETSU_COMP_DEFAULTS_FILE", "/d.toml"),
        ("HOME", "/home/u"),
    ]));
    assert_eq!(paths.defaults, PathBuf::from("/d.toml"));
    assert_eq!(paths.managed, Some(PathBuf::from("/s/tetsu/comp.toml")));
    assert_eq!(paths.user, Some(PathBuf::from("/c/tetsu/comp.toml")));
}

#[test]
fn with_no_home_the_user_and_managed_layers_are_absent() {
    let paths = Paths::from_env(env(&[]));
    assert_eq!(paths.managed, None);
    assert_eq!(paths.user, None);
    let paths = Paths::from_env(env(&[("HOME", "relative")]));
    assert_eq!(paths.user, None);
}

#[test]
fn an_explicit_user_file_replaces_the_user_layer_path() {
    let paths = Paths::from_env(env(&[("HOME", "/home/u")])).with_user("/x/y.toml".into());
    assert_eq!(paths.user, Some(PathBuf::from("/x/y.toml")));
}
