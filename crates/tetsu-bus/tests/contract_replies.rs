//! Method replies and errors checked against the IDL, so socket tests can
//! prove what a daemon sends is contract-valid.

use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_bus_contract::{Contract, Kind, check_error, check_reply, load_dir};

fn contract() -> Contract {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl");
    load_dir(&dir).expect("the bus contract loads")
}

fn transaction() -> Value {
    json!({
        "id": "tx-8",
        "kind": "install",
        "packages": ["htop"],
        "state": "succeeded",
        "started_at": "2026-01-15T09:30:00.000Z",
        "finished_at": "2026-01-15T09:31:12.481Z",
        "snapshot": "120",
        "changes": [{ "name": "htop", "from": null, "to": "3.4.1-1" }],
        "failure": null
    })
}

fn kinds(violations: &[tetsu_bus_contract::Violation]) -> Vec<Kind> {
    violations.iter().map(|v| v.kind).collect()
}

const LIST: &str = "org.tetsu.tx.v1.ListTransactions";

#[test]
fn a_valid_list_reply_passes() {
    let reply = json!({ "transactions": [transaction()] });
    assert_eq!(check_reply(&contract(), LIST, &reply), vec![]);
    let empty = json!({ "transactions": [] });
    assert_eq!(check_reply(&contract(), LIST, &empty), vec![]);
}

#[test]
fn a_transaction_with_an_unknown_field_fails() {
    let mut tx = transaction();
    tx["extra"] = json!(1);
    let found = check_reply(&contract(), LIST, &json!({ "transactions": [tx] }));
    assert_eq!(kinds(&found), vec![Kind::ReplyUnknownField]);
}

#[test]
fn a_transaction_missing_a_required_field_fails() {
    let mut tx = transaction();
    tx.as_object_mut().unwrap().remove("started_at");
    let found = check_reply(&contract(), LIST, &json!({ "transactions": [tx] }));
    assert_eq!(kinds(&found), vec![Kind::ReplyMissingField]);
}

#[test]
fn a_transaction_with_a_bad_enum_value_fails() {
    let mut tx = transaction();
    tx["state"] = json!("exploded");
    let found = check_reply(&contract(), LIST, &json!({ "transactions": [tx] }));
    assert_eq!(kinds(&found), vec![Kind::ReplyWrongType]);
}

#[test]
fn a_bad_default_boot_fails() {
    let bad = json!({ "booted": "1", "default": "2", "default_boot": "maybe" });
    let found = check_reply(&contract(), "org.tetsu.tx.v1.GetStatus", &bad);
    assert_eq!(kinds(&found), vec![Kind::ReplyWrongType]);
    let good = json!({ "booted": "1", "default": "2", "default_boot": "good" });
    assert_eq!(
        check_reply(&contract(), "org.tetsu.tx.v1.GetStatus", &good),
        vec![]
    );
}

#[test]
fn an_unknown_method_fails() {
    let found = check_reply(&contract(), "org.tetsu.tx.v1.Nope", &json!({}));
    assert_eq!(kinds(&found), vec![Kind::UnknownMethod]);
    let found = check_reply(&contract(), "org.nothing.v1.List", &json!({}));
    assert_eq!(kinds(&found), vec![Kind::UnknownMethod]);
}

#[test]
fn a_declared_error_passes() {
    let err = json!({ "id": "tx-9" });
    assert_eq!(
        check_error(&contract(), "org.tetsu.tx.v1.TransactionInProgress", &err),
        vec![]
    );
    assert_eq!(
        check_error(&contract(), "org.tetsu.tx.v1.PermissionDenied", &json!({})),
        vec![]
    );
}

#[test]
fn a_declared_error_with_bad_parameters_fails() {
    let found = check_error(
        &contract(),
        "org.tetsu.tx.v1.TransactionInProgress",
        &json!({}),
    );
    assert_eq!(kinds(&found), vec![Kind::ReplyMissingField]);
    let found = check_error(
        &contract(),
        "org.tetsu.tx.v1.TransactionInProgress",
        &json!({ "id": 3 }),
    );
    assert_eq!(kinds(&found), vec![Kind::ReplyWrongType]);
}

#[test]
fn an_undeclared_error_fails() {
    let found = check_error(&contract(), "org.tetsu.tx.v1.Exploded", &json!({}));
    assert_eq!(kinds(&found), vec![Kind::UnknownError]);
}

#[test]
fn a_notify_reply_passes() {
    let reply = json!({ "kind": "shown", "id": "n-19" });
    assert_eq!(
        check_reply(
            &contract(),
            "org.tetsu.notify.v1.Show",
            &json!({ "reply": reply })
        ),
        vec![],
        "Show returns its reply under its output name"
    );
}

#[test]
fn varlink_service_errors_are_protocol_errors() {
    for name in [
        "org.varlink.service.MethodNotFound",
        "org.varlink.service.MethodNotImplemented",
        "org.varlink.service.InvalidParameter",
        "org.varlink.service.ExpectedMore",
        "org.varlink.service.ServiceUnavailable",
    ] {
        assert_eq!(
            check_error(&contract(), name, &json!({ "method": "x" })),
            vec![],
            "{name}"
        );
    }
}
