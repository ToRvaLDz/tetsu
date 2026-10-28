//! `SetManaged` planning: validation order, batch atomicity, the new
//! managed layer.

mod common;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tetsu_comp::config::layer::parse;
use tetsu_comp::config::merge::{Values, View, merge};
use tetsu_comp::config::set::{SetError, plan};

fn layer(text: &str) -> Values {
    parse(text).values
}

fn view(managed: &str, user: &str) -> (View, Values) {
    let managed = layer(managed);
    (merge(&Values::new(), &managed, &layer(user)), managed)
}

fn values(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn remove(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| (*k).to_owned()).collect()
}

fn run(
    managed: &str,
    user: &str,
    set: &[(&str, Value)],
    gone: &[&str],
) -> Result<Values, SetError> {
    let (view, managed) = view(managed, user);
    plan(&view, &managed, &values(set), &remove(gone))
}

/// The error as the wire has it, checked against the contract.
fn wire(err: &SetError) -> (String, Value) {
    let reply = err.reply();
    let found =
        tetsu_bus_contract::check_error(&common::contract(), &reply.error, &reply.parameters);
    assert!(found.is_empty(), "{}: {found:?}", reply.error);
    (reply.error, reply.parameters)
}

#[test]
fn a_set_lands_in_the_managed_layer_and_keeps_the_rest() {
    let next = run(
        "[layout]\ngaps_inner = 2\n",
        "",
        &[("layout.gaps_outer", json!(10))],
        &[],
    )
    .unwrap();
    assert_eq!(next["layout.gaps_inner"], toml::Value::Integer(2));
    assert_eq!(next["layout.gaps_outer"], toml::Value::Integer(10));
}

#[test]
fn a_remove_brings_the_lower_layer_back() {
    let next = run(
        "[layout]\ngaps_inner = 2\n",
        "",
        &[],
        &["layout.gaps_inner"],
    )
    .unwrap();
    assert!(next.is_empty());
}

#[test]
fn an_integer_is_accepted_where_a_float_is_wanted() {
    // 0 is out of range for the ratio; the type is what is under test.
    let err = run("", "", &[("layout.master_ratio", json!(1))], &[]).unwrap_err();
    assert!(matches!(err, SetError::InvalidValue { .. }), "{err:?}");
    assert!(wire(&err).1["reason"].as_str().unwrap().contains("outside"));
    let ok = run("", "", &[("layout.master_ratio", json!(0.7))], &[]).unwrap();
    assert_eq!(ok["layout.master_ratio"], toml::Value::Float(0.7));
}

#[test]
fn an_unknown_key_is_rejected() {
    let err = run("", "", &[("layout.nope", json!(1))], &[]).unwrap_err();
    let (name, params) = wire(&err);
    assert_eq!(name, "org.tetsu.comp.config.v1.UnknownKey");
    assert_eq!(params, json!({ "key": "layout.nope" }));
    let err = run("", "", &[], &["nope"]).unwrap_err();
    assert!(matches!(err, SetError::UnknownKey(_)));
}

#[test]
fn a_key_that_is_not_writable_names_its_method() {
    let err = run("", "", &[("render.device", json!("auto"))], &[]).unwrap_err();
    let (name, params) = wire(&err);
    assert_eq!(name, "org.tetsu.comp.config.v1.KeyNotWritable");
    assert_eq!(
        params,
        json!({
            "key": "render.device",
            "method": "org.tetsu.comp.render.v1.SetRenderDevice",
        })
    );
    let err = run("", "", &[("lock.command", json!(["x"]))], &[]).unwrap_err();
    assert_eq!(
        wire(&err).1,
        json!({ "key": "lock.command", "method": null })
    );
    // A remove is refused the same way.
    let err = run("", "", &[], &["trust.input"]).unwrap_err();
    assert!(matches!(err, SetError::KeyNotWritable { .. }));
}

#[test]
fn an_invalid_value_is_rejected_with_a_reason() {
    for (key, value) in [
        ("layout.gaps_inner", json!("wide")),
        ("layout.gaps_inner", json!(-1)),
        ("layout.default", json!("spiral")),
        ("layout.master_ratio", json!(5.0)),
        ("layout.gaps_outer", json!(null)),
        ("window_rule", json!([{ "app_id": "(" }])),
        ("window_rule", json!("foot")),
    ] {
        let err = run("", "", &[(key, value.clone())], &[]).unwrap_err();
        let (name, params) = wire(&err);
        assert_eq!(
            name, "org.tetsu.comp.config.v1.InvalidValue",
            "{key} {value}"
        );
        assert_eq!(params["key"], key);
        assert!(!params["reason"].as_str().unwrap().is_empty());
    }
}

#[test]
fn a_key_both_set_and_removed_is_invalid() {
    let err = run(
        "",
        "",
        &[("layout.gaps_inner", json!(1))],
        &["layout.gaps_inner"],
    )
    .unwrap_err();
    assert_eq!(
        wire(&err).1,
        json!({ "key": "layout.gaps_inner", "reason": "both set and removed" })
    );
}

#[test]
fn a_key_the_user_file_sets_rejects_the_whole_batch_and_lists_every_such_key() {
    let user = "[layout]\ngaps_outer = 3\nmaster_ratio = 0.6\n";
    let err = run(
        "",
        user,
        &[
            ("layout.master_ratio", json!(0.7)),
            ("layout.gaps_inner", json!(1)),
            ("layout.gaps_outer", json!(9)),
        ],
        &[],
    )
    .unwrap_err();
    let (name, params) = wire(&err);
    assert_eq!(name, "org.tetsu.comp.config.v1.OverriddenByUser");
    assert_eq!(
        params,
        json!({ "keys": ["layout.gaps_outer", "layout.master_ratio"] })
    );
}

#[test]
fn removing_a_dormant_managed_value_of_a_user_key_is_allowed() {
    let next = run(
        "[layout]\ngaps_outer = 3\n",
        "[layout]\ngaps_outer = 5\n",
        &[],
        &["layout.gaps_outer"],
    )
    .unwrap();
    assert!(next.is_empty());
}

#[test]
fn the_errors_come_in_the_documented_order() {
    // Unknown beats not writable beats invalid beats overridden.
    let user = "[layout]\ngaps_outer = 3\n";
    let all = [
        ("layout.gaps_outer", json!(1)),
        ("layout.gaps_inner", json!("x")),
        ("render.device", json!("auto")),
        ("zzz", json!(1)),
    ];
    let err = run("", user, &all, &[]).unwrap_err();
    assert!(matches!(err, SetError::UnknownKey(_)), "{err:?}");
    let err = run("", user, &all[..3], &[]).unwrap_err();
    assert!(matches!(err, SetError::KeyNotWritable { .. }), "{err:?}");
    let err = run("", user, &all[..2], &[]).unwrap_err();
    assert!(matches!(err, SetError::InvalidValue { .. }), "{err:?}");
    let err = run("", user, &all[..1], &[]).unwrap_err();
    assert!(matches!(err, SetError::OverriddenByUser(_)), "{err:?}");
}

#[test]
fn a_state_write_failure_reply_passes_the_contract() {
    let reply = tetsu_comp::config::set::state_write_failed("disk full");
    let found =
        tetsu_bus_contract::check_error(&common::contract(), &reply.error, &reply.parameters);
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(reply.error, "org.tetsu.comp.config.v1.StateWriteFailed");
}

#[test]
fn an_output_key_is_not_writable_through_set_or_remove() {
    let method = Some("org.tetsu.comp.output.v1.ConfigureOutputs".to_owned());
    let err = run("", "", &[("output.DP-1.scale", json!(1.5))], &[]).unwrap_err();
    assert_eq!(
        err,
        SetError::KeyNotWritable {
            key: "output.DP-1.scale".into(),
            method: method.clone()
        }
    );
    wire(&err);
    let err = run("", "", &[], &["output.DP-1.scale"]).unwrap_err();
    assert_eq!(
        err,
        SetError::KeyNotWritable {
            key: "output.DP-1.scale".into(),
            method
        }
    );
    wire(&err);
}

#[test]
fn the_idle_timeout_is_writable_unless_the_user_file_sets_it() {
    let next = run("", "", &[("lock.idle_timeout_s", json!(120))], &[]).unwrap();
    assert_eq!(next["lock.idle_timeout_s"], toml::Value::Integer(120));

    let err = run(
        "",
        "[lock]\nidle_timeout_s = 300\n",
        &[("lock.idle_timeout_s", json!(120))],
        &[],
    )
    .unwrap_err();
    let (name, params) = wire(&err);
    assert_eq!(name, "org.tetsu.comp.config.v1.OverriddenByUser");
    assert_eq!(params, json!({ "keys": ["lock.idle_timeout_s"] }));
}

#[test]
fn a_negative_or_too_short_idle_timeout_is_invalid() {
    for value in [-5, 1, 29] {
        let err = run("", "", &[("lock.idle_timeout_s", json!(value))], &[]).unwrap_err();
        let (name, params) = wire(&err);
        assert_eq!(name, "org.tetsu.comp.config.v1.InvalidValue", "{value}");
        assert_eq!(params["key"], "lock.idle_timeout_s");
        let reason = params["reason"].as_str().unwrap();
        assert!(reason.contains("lock.idle_timeout_s"), "{reason}");
        assert!(reason.contains("0 or 30..=86400"), "{reason}");
    }
    let next = run("", "", &[("lock.idle_timeout_s", json!(30))], &[]).unwrap();
    assert_eq!(next["lock.idle_timeout_s"], toml::Value::Integer(30));
}
