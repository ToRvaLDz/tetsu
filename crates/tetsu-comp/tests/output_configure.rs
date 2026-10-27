//! The `ConfigureOutputs` planner and the output prefs: pure functions of
//! the outputs, the call and the config layers.

mod common;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tetsu_bus::output::{Mode, Output, OutputConfig, Transform};
use tetsu_comp::config::layer::parse;
use tetsu_comp::config::merge::{Values, View, merge};
use tetsu_comp::outputs::configure::{ConfigureError, Planned, Target, plan};
use tetsu_comp::outputs::prefs::targets;

fn modes() -> Vec<Mode> {
    let mode = |width, height, preferred| Mode {
        width,
        height,
        refresh_mhz: 60000,
        preferred,
    };
    vec![mode(1920, 1080, true), mode(1280, 720, false)]
}

fn output(id: &str, name: &str, x: i64, y: i64) -> Output {
    Output {
        id: id.into(),
        name: name.into(),
        make: None,
        model: None,
        serial: None,
        enabled: true,
        modes: modes(),
        current_mode: Some(0),
        scale: 1.0,
        transform: Transform::Normal,
        x,
        y,
    }
}

fn two() -> Vec<Output> {
    vec![
        output("out-1", "HEADLESS-1", 0, 0),
        output("out-2", "HEADLESS-2", 1920, 0),
    ]
}

fn layer(text: &str) -> Values {
    let parsed = parse(text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    parsed.values
}

fn view(managed: &str, user: &str) -> View {
    merge(&Values::new(), &layer(managed), &layer(user))
}

fn configs(pairs: &[(&str, Value)]) -> BTreeMap<String, OutputConfig> {
    pairs
        .iter()
        .map(|(id, v)| ((*id).to_owned(), serde_json::from_value(v.clone()).unwrap()))
        .collect()
}

fn run(
    outputs: &[Output],
    call: &[(&str, Value)],
    managed: &str,
    user: &str,
) -> Result<Planned, ConfigureError> {
    plan(outputs, &configs(call), &view(managed, user))
}

/// The error on the wire, checked against the contract.
fn wire(err: &ConfigureError) -> (String, Value) {
    let reply = err.reply();
    let found =
        tetsu_bus_contract::check_error(&common::contract(), &reply.error, &reply.parameters);
    assert!(found.is_empty(), "{}: {found:?}", reply.error);
    (reply.error, reply.parameters)
}

fn invalid(result: Result<Planned, ConfigureError>) -> (String, String) {
    match result.expect_err("the call is refused") {
        err @ ConfigureError::Invalid { .. } => {
            let (name, params) = wire(&err);
            assert_eq!(name, "org.tetsu.comp.output.v1.InvalidConfiguration");
            (
                params["id"].as_str().unwrap().to_owned(),
                params["reason"].as_str().unwrap().to_owned(),
            )
        }
        other => panic!("expected InvalidConfiguration, got {other:?}"),
    }
}

#[test]
fn an_unknown_id_wins_over_a_bad_value_on_another_id() {
    let err = run(
        &two(),
        &[("out-1", json!({ "scale": 0 })), ("out-9", json!({}))],
        "",
        "",
    )
    .unwrap_err();
    assert_eq!(err, ConfigureError::NotFound("out-9".into()));
    let (name, params) = wire(&err);
    assert_eq!(name, "org.tetsu.comp.output.v1.OutputNotFound");
    assert_eq!(params, json!({ "id": "out-9" }));
}

#[test]
fn an_unknown_mode_is_refused() {
    let call = [(
        "out-1",
        json!({ "mode": { "width": 1024, "height": 768, "refresh_mhz": 60000 } }),
    )];
    let (id, reason) = invalid(run(&two(), &call, "", ""));
    assert_eq!(id, "out-1");
    assert!(reason.contains("1024x768"), "{reason}");
}

#[test]
fn a_scale_outside_the_bounds_is_refused() {
    for scale in [json!(0), json!(0.1), json!(9), json!(-1)] {
        let (id, reason) = invalid(run(&two(), &[("out-2", json!({ "scale": scale }))], "", ""));
        assert_eq!(id, "out-2");
        assert!(reason.contains("scale"), "{reason}");
    }
    let ok = run(&two(), &[("out-2", json!({ "scale": 0.25 }))], "", "");
    assert!(ok.is_ok());
    let ok = run(&two(), &[("out-2", json!({ "scale": 8 }))], "", "");
    assert!(ok.is_ok());
}

#[test]
fn an_unknown_transform_is_refused() {
    let (id, reason) = invalid(run(
        &two(),
        &[("out-1", json!({ "transform": "skewed" }))],
        "",
        "",
    ));
    assert_eq!(id, "out-1");
    assert!(reason.contains("transform"), "{reason}");
}

#[test]
fn a_position_out_of_range_is_refused() {
    for call in [json!({ "x": 2_000_000 }), json!({ "y": -2_000_000 })] {
        let (id, reason) = invalid(run(&two(), &[("out-1", call)], "", ""));
        assert_eq!(id, "out-1");
        assert!(reason.contains("position"), "{reason}");
    }
}

#[test]
fn enabling_an_output_with_no_modes_is_refused() {
    let mut outputs = two();
    outputs[1].enabled = false;
    outputs[1].modes.clear();
    outputs[1].current_mode = None;
    let (id, reason) = invalid(run(
        &outputs,
        &[("out-2", json!({ "enabled": true }))],
        "",
        "",
    ));
    assert_eq!(id, "out-2");
    assert!(reason.contains("no mode"), "{reason}");
}

#[test]
fn a_call_that_leaves_no_output_enabled_names_the_first_that_disables() {
    let call = [
        ("out-1", json!({ "enabled": false })),
        ("out-2", json!({ "enabled": false })),
    ];
    let (id, reason) = invalid(run(&two(), &call, "", ""));
    assert_eq!(id, "out-1");
    assert!(reason.contains("no output"), "{reason}");
    let one = [("out-2", json!({ "enabled": false }))];
    assert!(run(&two(), &one, "", "").is_ok());
}

#[test]
fn two_pinned_outputs_that_overlap_are_refused_naming_the_other() {
    let call = [
        ("out-1", json!({ "x": 0, "y": 0 })),
        ("out-2", json!({ "x": 1000, "y": 0 })),
    ];
    let (id, reason) = invalid(run(&two(), &call, "", ""));
    assert_eq!(id, "out-2");
    assert!(reason.contains("out-1"), "{reason}");
    let touching = [
        ("out-1", json!({ "x": 0, "y": 0 })),
        ("out-2", json!({ "x": 1920, "y": 0 })),
    ];
    assert!(run(&two(), &touching, "", "").is_ok());
}

#[test]
fn a_pin_over_a_disabled_output_is_fine() {
    let call = [
        ("out-1", json!({ "x": 0, "y": 0, "enabled": false })),
        ("out-2", json!({ "x": 0, "y": 0 })),
    ];
    assert!(run(&two(), &call, "", "").is_ok());
}

#[test]
fn the_first_failure_wins_in_the_documented_order() {
    // Invalid (out-1 scale) comes before the user override of out-2.
    let call = [
        ("out-1", json!({ "scale": 0 })),
        ("out-2", json!({ "scale": 2 })),
    ];
    let user = "[output.HEADLESS-2]\nscale = 1.5\n";
    let (id, _) = invalid(run(&two(), &call, "", user));
    assert_eq!(id, "out-1");
}

#[test]
fn the_user_file_names_the_keys_it_overrides_sorted_and_only_those() {
    let user = "[output.HEADLESS-2]\nscale = 1.5\nposition = { x = 0, y = 0 }\n";
    let call = [("out-2", json!({ "scale": 2, "x": 5, "y": 5 }))];
    let err = run(&two(), &call, "", user).unwrap_err();
    assert_eq!(
        err,
        ConfigureError::OverriddenByUser(vec![
            "output.HEADLESS-2.position".into(),
            "output.HEADLESS-2.scale".into()
        ])
    );
    let (name, params) = wire(&err);
    assert_eq!(name, "org.tetsu.comp.output.v1.OverriddenByUser");
    assert_eq!(
        params,
        json!({ "keys": ["output.HEADLESS-2.position", "output.HEADLESS-2.scale"] })
    );
    // A user-set field the call does not touch passes.
    let moves = [("out-2", json!({ "transform": "rotate_90" }))];
    assert!(run(&two(), &moves, "", user).is_ok());
}

#[test]
fn the_managed_values_are_the_non_null_fields_keyed_by_connector() {
    let call = [
        (
            "out-2",
            json!({ "scale": 2.0, "x": 0, "y": 1080, "enabled": null, "transform": "rotate_90",
                    "mode": { "width": 1280, "height": 720, "refresh_mhz": 60000 } }),
        ),
        ("out-1", json!({ "enabled": true })),
    ];
    let planned = run(&two(), &call, "", "").unwrap();
    let mut expected = Values::new();
    let table = |pairs: &[(&str, i64)]| {
        toml::Value::Table(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), toml::Value::Integer(*v)))
                .collect(),
        )
    };
    expected.insert(
        "output.HEADLESS-1.enabled".into(),
        toml::Value::Boolean(true),
    );
    expected.insert("output.HEADLESS-2.scale".into(), toml::Value::Float(2.0));
    expected.insert(
        "output.HEADLESS-2.transform".into(),
        toml::Value::String("rotate_90".into()),
    );
    expected.insert(
        "output.HEADLESS-2.position".into(),
        table(&[("x", 0), ("y", 1080)]),
    );
    expected.insert(
        "output.HEADLESS-2.mode".into(),
        table(&[("width", 1280), ("height", 720), ("refresh_mhz", 60000)]),
    );
    assert_eq!(planned.managed, expected);
}

#[test]
fn x_alone_persists_the_position_with_the_current_y() {
    let outputs = vec![
        output("out-1", "HEADLESS-1", 0, 0),
        output("out-2", "HEADLESS-2", 1920, 7),
    ];
    let planned = run(&outputs, &[("out-2", json!({ "x": 5000 }))], "", "").unwrap();
    let at = &planned.managed["output.HEADLESS-2.position"];
    assert_eq!(at["x"].as_integer(), Some(5000));
    assert_eq!(at["y"].as_integer(), Some(7));
}

#[test]
fn an_empty_call_or_empty_configs_persist_nothing() {
    let planned = run(&two(), &[], "", "").unwrap();
    assert!(planned.managed.is_empty());
    let planned = run(&two(), &[("out-1", json!({}))], "", "").unwrap();
    assert!(planned.managed.is_empty());
}

#[test]
fn a_connector_name_a_key_cannot_hold_is_refused() {
    let mut outputs = two();
    outputs[0].name = "DP.1".into();
    let (id, reason) = invalid(run(&outputs, &[("out-1", json!({ "scale": 2 }))], "", ""));
    assert_eq!(id, "out-1");
    assert!(reason.contains("DP.1"), "{reason}");
}

fn target(id: &str, enabled: bool, mode: Option<usize>) -> Target {
    Target {
        id: id.into(),
        enabled,
        mode,
        scale: 1.0,
        transform: Transform::Normal,
        pin: None,
    }
}

#[test]
fn with_no_config_every_output_is_automatic() {
    let mut outputs = two();
    outputs[1].current_mode = Some(1);
    let got = targets(&outputs, &Values::new());
    assert_eq!(
        got,
        [
            target("out-1", true, Some(0)),
            target("out-2", true, Some(0))
        ]
    );
}

#[test]
fn an_unknown_mode_in_the_config_keeps_the_current_one() {
    let mut outputs = two();
    outputs[0].current_mode = Some(1);
    let values =
        layer("[output.HEADLESS-1]\nmode = { width = 1024, height = 768, refresh_mhz = 60000 }\n");
    assert_eq!(targets(&outputs, &values)[0].mode, Some(1));
}

#[test]
fn when_every_output_is_disabled_the_first_stays_enabled() {
    let values =
        layer("[output.HEADLESS-1]\nenabled = false\n[output.HEADLESS-2]\nenabled = false\n");
    let got = targets(&two(), &values);
    assert!(got[0].enabled);
    assert!(!got[1].enabled);
}

#[test]
fn an_overlapping_pin_falls_back_to_automatic() {
    let values = layer(
        "[output.HEADLESS-1]\nposition = { x = 0, y = 0 }\n\
         [output.HEADLESS-2]\nposition = { x = 100, y = 100 }\n",
    );
    let got = targets(&two(), &values);
    assert_eq!(got[0].pin, Some((0, 0)));
    assert_eq!(got[1].pin, None);
}

/// The targets a call plans are the ones the prefs give once it is written.
fn consistent(outputs: &[Output], call: &[(&str, Value)], managed: &str) {
    let planned = run(outputs, call, managed, "").unwrap();
    let mut after = layer(managed);
    after.extend(planned.managed.clone());
    let merged = merge(&Values::new(), &after, &Values::new());
    assert_eq!(targets(outputs, &merged.values()), planned.targets);
}

#[test]
fn the_planned_targets_equal_the_prefs_after_the_write() {
    let mode = json!({ "width": 1280, "height": 720, "refresh_mhz": 60000 });
    consistent(&two(), &[("out-1", json!({ "mode": mode }))], "");
    consistent(
        &two(),
        &[("out-2", json!({ "scale": 2.0, "x": 0, "y": 1080 }))],
        "",
    );
    consistent(&two(), &[("out-2", json!({ "enabled": false }))], "");
    let mut disabled = two();
    disabled[1].enabled = false;
    disabled[1].current_mode = None;
    consistent(
        &disabled,
        &[("out-2", json!({ "enabled": true }))],
        "[output.HEADLESS-2]\nenabled = false\n",
    );
}

#[test]
fn a_planned_mode_change_names_the_mode_index() {
    let mode = json!({ "width": 1280, "height": 720, "refresh_mhz": 60000 });
    let planned = run(&two(), &[("out-1", json!({ "mode": mode }))], "", "").unwrap();
    assert_eq!(planned.targets[0].mode, Some(1));
    assert_eq!(planned.targets[1].mode, Some(0));
}
