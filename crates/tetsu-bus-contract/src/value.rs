//! Checking a JSON value against IDL types.

use serde_json::Value;

use crate::{Field, Interface, Kind, Ty, TypeDef, Variant, Violation};

/// The violation kinds a check reports for a value that disagrees with the IDL.
#[derive(Clone, Copy)]
pub(crate) struct ValueKinds {
    pub unknown_field: Kind,
    pub missing_field: Kind,
    pub wrong_type: Kind,
}

/// Kinds for the event fixtures.
pub(crate) const FIXTURE_KINDS: ValueKinds = ValueKinds {
    unknown_field: Kind::FixtureUnknownField,
    missing_field: Kind::FixtureMissingField,
    wrong_type: Kind::FixtureWrongType,
};

/// Kinds for method replies and errors.
pub(crate) const REPLY_KINDS: ValueKinds = ValueKinds {
    unknown_field: Kind::ReplyUnknownField,
    missing_field: Kind::ReplyMissingField,
    wrong_type: Kind::ReplyWrongType,
};

/// Checks JSON values against the types of one interface.
pub(crate) struct Checker<'a> {
    pub iface: &'a Interface,
    /// What the violations name as their path: a fixture path or a method.
    pub origin: &'a str,
    pub kinds: ValueKinds,
    pub out: &'a mut Vec<Violation>,
}

impl Checker<'_> {
    fn push(&mut self, kind: Kind, detail: String) {
        self.out.push(Violation::new(kind, self.origin, detail));
    }

    fn wrong(&mut self, path: &str, expected: &str, got: &Value) {
        let got = match got {
            Value::Null => "null",
            Value::Bool(_) => "a bool",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        };
        self.push(
            self.kinds.wrong_type,
            format!("{path}: expected {expected}, got {got}"),
        );
    }

    pub fn object(&mut self, fields: &[Field], v: &Value, path: &str) {
        let Value::Object(map) = v else {
            return self.wrong(path, "an object", v);
        };
        for key in map.keys() {
            if !fields.iter().any(|f| &f.name == key) {
                self.push(
                    self.kinds.unknown_field,
                    format!("{}: field not in the IDL", join(path, key)),
                );
            }
        }
        for f in fields {
            let fpath = join(path, &f.name);
            match map.get(&f.name) {
                Some(value) => self.value(&f.ty, value, &fpath),
                None if f.ty.is_optional() => {}
                None => self.push(
                    self.kinds.missing_field,
                    format!("{fpath}: required field is missing"),
                ),
            }
        }
    }

    pub fn value(&mut self, ty: &Ty, v: &Value, path: &str) {
        match ty {
            Ty::Bool if !v.is_boolean() => self.wrong(path, "a bool", v),
            Ty::Int if !v.is_i64() => self.wrong(path, "an int", v),
            Ty::Float if !v.is_number() => self.wrong(path, "a float", v),
            Ty::String if !v.is_string() => self.wrong(path, "a string", v),
            Ty::ForeignObject if !v.is_object() => self.wrong(path, "an object", v),
            // `any` carries any JSON value; null only where the field is optional.
            Ty::Any if v.is_null() => self.wrong(path, "a JSON value", v),
            Ty::Optional(inner) => {
                if !v.is_null() {
                    self.value(inner, v, path);
                }
            }
            Ty::Array(inner) => match v {
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        self.value(inner, item, &format!("{path}[{i}]"));
                    }
                }
                _ => self.wrong(path, "an array", v),
            },
            Ty::Map(inner) => match v {
                Value::Object(map) => {
                    for (k, item) in map {
                        self.value(inner, item, &join(path, k));
                    }
                }
                _ => self.wrong(path, "an object", v),
            },
            Ty::Enum(variants) => self.enum_value(variants, v, path),
            Ty::Object(fields) => self.object(fields, v, path),
            Ty::Custom(name) => match self.iface.types.get(name).map(|t| &t.def) {
                Some(TypeDef::Object(fields)) => self.object(fields, v, path),
                Some(TypeDef::Enum(variants)) => self.enum_value(variants, v, path),
                None => self.push(
                    self.kinds.wrong_type,
                    format!("{path}: type {name} is not declared"),
                ),
            },
            _ => {}
        }
    }

    fn enum_value(&mut self, variants: &[Variant], v: &Value, path: &str) {
        match v.as_str() {
            Some(s) if variants.iter().any(|x| x.name == s) => {}
            Some(s) => self.push(
                self.kinds.wrong_type,
                format!("{path}: `{s}` is not a declared enum value"),
            ),
            None => self.wrong(path, "an enum value (string)", v),
        }
    }
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}
