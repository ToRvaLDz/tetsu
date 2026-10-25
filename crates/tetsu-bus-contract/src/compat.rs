//! Compatibility of the current contract with the frozen baseline.
//!
//! The current IDL must be a compatible superset of the baseline, as
//! CONTRACT.md's Evolution section defines: additive changes pass, anything
//! listed as breaking is a violation.

use crate::{
    Capability, Contract, Doc, ErrorDef, Field, Interface, Kind, Method, Ty, TypeDef, Variant,
};
use crate::{CustomType, Violation};

/// Appends a violation.
fn push(out: &mut Vec<Violation>, kind: Kind, path: &str, detail: impl Into<String>) {
    out.push(Violation::new(kind, path, detail));
}

/// Every way `current` is not a compatible superset of `baseline`.
pub fn check_compat(baseline: &Contract, current: &Contract) -> Vec<Violation> {
    let mut out = Vec::new();
    for (name, base) in &baseline.interfaces {
        match current.interfaces.get(name) {
            None => push(&mut out, Kind::InterfaceRemoved, name, "interface removed"),
            Some(cur) => compare_interface(baseline, current, base, cur, &mut out),
        }
    }
    out
}

fn compare_interface(
    baseline: &Contract,
    current: &Contract,
    base: &Interface,
    cur: &Interface,
    out: &mut Vec<Violation>,
) {
    let iface = &base.name;
    for (name, b) in &base.types {
        let path = format!("{iface}.{name}");
        match cur.types.get(name) {
            None => push(out, Kind::TypeRemoved, &path, "type removed"),
            Some(c) => compare_type(b, c, &path, out),
        }
    }
    for (name, b) in &base.methods {
        let path = format!("{iface}.{name}");
        match cur.methods.get(name) {
            None => push(out, Kind::MethodRemoved, &path, "method removed"),
            Some(c) => {
                compare_method(b, c, &path, out);
                let was = baseline.is_streaming(iface, name);
                let is = current.is_streaming(iface, name);
                if was != is {
                    let (from, to) = if was {
                        ("streaming", "plain")
                    } else {
                        ("plain", "streaming")
                    };
                    push(
                        out,
                        Kind::StreamingChanged,
                        &path,
                        format!("method turned from {from} to {to}"),
                    );
                }
            }
        }
    }
    for (name, b) in &base.errors {
        let path = format!("{iface}.{name}");
        match cur.errors.get(name) {
            None => push(out, Kind::ErrorRemoved, &path, "error removed"),
            Some(c) => compare_error(b, c, &path, out),
        }
    }
}

fn compare_type(base: &CustomType, cur: &CustomType, path: &str, out: &mut Vec<Violation>) {
    match (&base.def, &cur.def) {
        (TypeDef::Object(b), TypeDef::Object(c)) => compare_fields(b, c, path, false, out),
        (TypeDef::Enum(b), TypeDef::Enum(c)) => compare_variants(b, c, path, out),
        (b, _) => {
            let (from, to) = match b {
                TypeDef::Object(_) => ("object", "enum"),
                TypeDef::Enum(_) => ("enum", "object"),
            };
            push(
                out,
                Kind::Retyped,
                path,
                format!("type changed from {from} to {to}"),
            );
        }
    }
}

fn compare_method(base: &Method, cur: &Method, path: &str, out: &mut Vec<Violation>) {
    if base.doc.has_tag("@destructive") && !cur.doc.has_tag("@destructive") {
        push(out, Kind::TagRemoved, path, "@destructive removed");
    }
    compare_capability(&base.doc, &cur.doc, path, out);
    compare_fields(&base.inputs, &cur.inputs, &format!("{path}(in)"), true, out);
    compare_fields(
        &base.outputs,
        &cur.outputs,
        &format!("{path}(out)"),
        false,
        out,
    );
}

/// A method that had a `@capability` keeps its class and scope. Adding the
/// tag to a method without one is additive. A tag that does not parse is
/// the rules' business, not a compatibility break.
fn compare_capability(base: &Doc, cur: &Doc, path: &str, out: &mut Vec<Violation>) {
    let Some(Ok(was)) = base.capability() else {
        return;
    };
    match cur.capability() {
        Some(Ok(now)) if now == was => {}
        Some(Ok(now)) => push(
            out,
            Kind::CapabilityChanged,
            path,
            format!("@capability {} became {}", describe(&was), describe(&now)),
        ),
        _ => push(
            out,
            Kind::CapabilityChanged,
            path,
            format!("@capability {} removed", describe(&was)),
        ),
    }
}

fn describe(cap: &Capability) -> String {
    match &cap.scope {
        Some(scope) => format!("{} scope={scope}", cap.class),
        None => cap.class.clone(),
    }
}

/// The retry class an error is tagged with, if any.
fn error_class(doc: &Doc) -> Option<&str> {
    doc.tag_lines()
        .find(|l| *l == "@transient" || *l == "@permanent")
}

fn compare_error(base: &ErrorDef, cur: &ErrorDef, path: &str, out: &mut Vec<Violation>) {
    if let Some(was) = error_class(&base.doc)
        && error_class(&cur.doc) != Some(was)
    {
        let now = error_class(&cur.doc).unwrap_or("no marker");
        push(
            out,
            Kind::ErrorClassChanged,
            path,
            format!("{was} became {now}"),
        );
    }
    compare_fields(&base.fields, &cur.fields, path, false, out);
}

fn compare_variants(base: &[Variant], cur: &[Variant], path: &str, out: &mut Vec<Violation>) {
    for b in base {
        if !cur.iter().any(|c| c.name == b.name) {
            push(
                out,
                Kind::EnumValueRemoved,
                &format!("{path}.{}", b.name),
                "enum value removed",
            );
        }
    }
}

/// `relax_input`: these are method inputs, where required -> optional is allowed.
fn compare_fields(
    base: &[Field],
    cur: &[Field],
    path: &str,
    relax_input: bool,
    out: &mut Vec<Violation>,
) {
    for b in base {
        let fpath = format!("{path}.{}", b.name);
        let Some(c) = cur.iter().find(|c| c.name == b.name) else {
            push(out, Kind::FieldRemoved, &fpath, "removed");
            continue;
        };
        if b.doc.has_tag("@untrusted") && !c.doc.has_tag("@untrusted") {
            push(out, Kind::TagRemoved, &fpath, "@untrusted removed");
        }
        compare_ty(&b.ty, &c.ty, &fpath, relax_input, out);
    }
    for c in cur {
        if !base.iter().any(|b| b.name == c.name) && !c.ty.is_optional() {
            push(
                out,
                Kind::NewRequired,
                &format!("{path}.{}", c.name),
                format!("new field must be optional, is {}", c.ty),
            );
        }
    }
}

/// `relax`: this is an input, so required -> optional is allowed here.
fn compare_ty(base: &Ty, cur: &Ty, path: &str, relax: bool, out: &mut Vec<Violation>) {
    let (b_opt, c_opt) = (base.is_optional(), cur.is_optional());
    if b_opt != c_opt && !(relax && !b_opt && c_opt) {
        let (from, to) = if b_opt {
            ("optional", "required")
        } else {
            ("required", "optional")
        };
        push(
            out,
            Kind::OptionalityChanged,
            path,
            format!("changed from {from} to {to}"),
        );
    }
    compare_inner(base.unwrap_optional(), cur.unwrap_optional(), path, out);
}

fn compare_inner(base: &Ty, cur: &Ty, path: &str, out: &mut Vec<Violation>) {
    match (base, cur) {
        (Ty::Array(b), Ty::Array(c)) => compare_ty(b, c, &format!("{path}[]"), false, out),
        (Ty::Map(b), Ty::Map(c)) => compare_ty(b, c, &format!("{path}[string]"), false, out),
        (Ty::Enum(b), Ty::Enum(c)) => compare_variants(b, c, path, out),
        (Ty::Object(b), Ty::Object(c)) => compare_fields(b, c, path, false, out),
        (b, c) if b == c => {}
        (b, c) => push(out, Kind::Retyped, path, format!("retyped from {b} to {c}")),
    }
}
