//! JSON Schema (a subset) from Varlink types.
//!
//! Custom types are inlined and recursive types are refused. An optional
//! `?T` is `anyOf [T, null]` and is not in `required`; objects are closed
//! with `additionalProperties: false`. Field prose, tags stripped, becomes
//! `description`.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{Map, Value, json};
use tetsu_bus_contract::{Doc, Field, Interface, Ty, TypeDef};

/// Why a type has no schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The type refers to itself.
    Recursive(String),
    /// The type is not defined in the interface.
    Unknown(String),
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recursive(n) => write!(f, "type `{n}` is recursive"),
            Self::Unknown(n) => write!(f, "type `{n}` is not defined"),
        }
    }
}

impl std::error::Error for SchemaError {}

/// The prose of a doc: tag lines dropped, the lines of a paragraph joined
/// by a space and the paragraphs by a blank line.
pub fn prose(doc: &Doc) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut open = false;
    for line in doc.0.iter().map(|l| l.trim()) {
        if line.starts_with('@') {
            continue;
        }
        if line.is_empty() {
            open = false;
        } else if open {
            let last = paragraphs.last_mut().expect("an open paragraph");
            last.push(' ');
            last.push_str(line);
        } else {
            paragraphs.push(line.to_owned());
            open = true;
        }
    }
    paragraphs.join("\n\n")
}

/// The object schema of a list of fields.
pub fn fields_schema(interface: &Interface, fields: &[Field]) -> Result<Value, SchemaError> {
    object(interface, fields, &mut BTreeSet::new())
}

fn object(
    interface: &Interface,
    fields: &[Field],
    inlining: &mut BTreeSet<String>,
) -> Result<Value, SchemaError> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        let mut schema = ty(interface, &field.ty, inlining)?;
        let text = prose(&field.doc);
        if !text.is_empty() {
            schema["description"] = Value::String(text);
        }
        if !field.ty.is_optional() {
            required.push(Value::String(field.name.clone()));
        }
        properties.insert(field.name.clone(), schema);
    }
    let mut out = Map::new();
    out.insert("type".to_owned(), json!("object"));
    out.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        out.insert("required".to_owned(), Value::Array(required));
    }
    out.insert("additionalProperties".to_owned(), json!(false));
    Ok(Value::Object(out))
}

fn variants(vs: &[tetsu_bus_contract::Variant]) -> Value {
    let names: Vec<&str> = vs.iter().map(|v| v.name.as_str()).collect();
    json!({ "type": "string", "enum": names })
}

fn ty(
    interface: &Interface,
    t: &Ty,
    inlining: &mut BTreeSet<String>,
) -> Result<Value, SchemaError> {
    Ok(match t {
        Ty::Bool => json!({ "type": "boolean" }),
        Ty::Int => json!({ "type": "integer" }),
        Ty::Float => json!({ "type": "number" }),
        Ty::String => json!({ "type": "string" }),
        Ty::ForeignObject => json!({ "type": "object" }),
        Ty::Any => json!({}),
        Ty::Optional(inner) => {
            json!({ "anyOf": [ty(interface, inner, inlining)?, { "type": "null" }] })
        }
        Ty::Array(inner) => json!({ "type": "array", "items": ty(interface, inner, inlining)? }),
        Ty::Map(inner) => {
            json!({ "type": "object", "additionalProperties": ty(interface, inner, inlining)? })
        }
        Ty::Enum(vs) => variants(vs),
        Ty::Object(fields) => object(interface, fields, inlining)?,
        Ty::Custom(name) => {
            let def = interface
                .types
                .get(name)
                .ok_or_else(|| SchemaError::Unknown(name.clone()))?;
            if !inlining.insert(name.clone()) {
                return Err(SchemaError::Recursive(name.clone()));
            }
            let schema = match &def.def {
                TypeDef::Object(fields) => object(interface, fields, inlining),
                TypeDef::Enum(vs) => Ok(variants(vs)),
            };
            inlining.remove(name);
            schema?
        }
    })
}

/// The paths of the `@untrusted` fields of `fields`, e.g. `windows[].title`.
/// A type that refers to itself is not followed a second time.
pub fn untrusted_paths(interface: &Interface, fields: &[Field]) -> Vec<String> {
    let mut out = Vec::new();
    collect(interface, fields, "", &mut BTreeSet::new(), &mut out);
    out
}

fn collect(
    interface: &Interface,
    fields: &[Field],
    prefix: &str,
    inlining: &mut BTreeSet<String>,
    out: &mut Vec<String>,
) {
    for field in fields {
        let path = format!("{prefix}{}", field.name);
        if field.doc.has_tag("@untrusted") {
            out.push(path);
        } else {
            descend(interface, &field.ty, &path, inlining, out);
        }
    }
}

fn descend(
    interface: &Interface,
    t: &Ty,
    path: &str,
    inlining: &mut BTreeSet<String>,
    out: &mut Vec<String>,
) {
    match t {
        Ty::Optional(inner) => descend(interface, inner, path, inlining, out),
        Ty::Array(inner) => descend(interface, inner, &format!("{path}[]"), inlining, out),
        Ty::Map(inner) => descend(interface, inner, &format!("{path}{{}}"), inlining, out),
        Ty::Object(fields) => collect(interface, fields, &format!("{path}."), inlining, out),
        Ty::Custom(name) => {
            if let Some(TypeDef::Object(fields)) = interface.types.get(name).map(|d| &d.def)
                && inlining.insert(name.clone())
            {
                collect(interface, fields, &format!("{path}."), inlining, out);
                inlining.remove(name);
            }
        }
        _ => {}
    }
}
