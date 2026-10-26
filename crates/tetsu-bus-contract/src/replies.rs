//! Method replies and errors checked against the IDL.

use serde_json::Value;

use crate::value::{Checker, REPLY_KINDS};
use crate::{Contract, Field, Interface, Kind, Violation};

/// Prefix of the Varlink protocol errors every service may answer.
const PROTOCOL_ERRORS: &str = "org.varlink.service.";

/// Splits `<interface>.<Name>` at the last dot.
fn split(fqn: &str) -> Option<(&str, &str)> {
    fqn.rsplit_once('.')
}

fn check_fields(iface: &Interface, origin: &str, fields: &[Field], v: &Value) -> Vec<Violation> {
    let mut out = Vec::new();
    Checker {
        iface,
        origin,
        kinds: REPLY_KINDS,
        out: &mut out,
    }
    .object(fields, v, "");
    out
}

/// Checks the parameters of a reply to `method_fqn` (`<interface>.<Method>`)
/// against the method's outputs. Streamed replies of a method have the same
/// shape as its final one.
pub fn check_reply(contract: &Contract, method_fqn: &str, reply: &Value) -> Vec<Violation> {
    let found = split(method_fqn).and_then(|(iname, mname)| {
        let iface = contract.interfaces.get(iname)?;
        Some((iface, iface.methods.get(mname)?))
    });
    match found {
        Some((iface, method)) => check_fields(iface, method_fqn, &method.outputs, reply),
        None => vec![Violation::new(
            Kind::UnknownMethod,
            method_fqn,
            "the contract declares no such method",
        )],
    }
}

/// Checks the parameters of the error `error_fqn` (`<interface>.<Error>`)
/// against its declaration. `org.varlink.service.*` errors are protocol
/// errors, accepted whatever they carry.
pub fn check_error(contract: &Contract, error_fqn: &str, params: &Value) -> Vec<Violation> {
    if error_fqn.starts_with(PROTOCOL_ERRORS) {
        return Vec::new();
    }
    let found = split(error_fqn).and_then(|(iname, ename)| {
        let iface = contract.interfaces.get(iname)?;
        Some((iface, iface.errors.get(ename)?))
    });
    match found {
        Some((iface, error)) => check_fields(iface, error_fqn, &error.fields, params),
        None => vec![Violation::new(
            Kind::UnknownError,
            error_fqn,
            "the contract declares no such error",
        )],
    }
}
