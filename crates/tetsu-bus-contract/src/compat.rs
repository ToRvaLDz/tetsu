//! Compatibility of the current contract with the frozen baseline.
//!
//! The current IDL must be a compatible superset of the baseline, as
//! CONTRACT.md's Evolution section defines: additive changes pass, anything
//! listed as breaking is a violation.

use crate::{Contract, Doc, ErrorDef, Field, Interface, Kind, Method, Ty, TypeDef, Variant};
use crate::{CustomType, Violation};

/// Where a list of fields sits; decides which changes are allowed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A field of a type, inline or custom.
    Member,
    /// A method input: going from required to optional is allowed.
    Input,
    Output,
    ErrorParam,
}

struct Ctx<'a> {
    out: &'a mut Vec<Violation>,
}

impl Ctx<'_> {
    fn push(&mut self, kind: Kind, path: &str, detail: impl Into<String>) {
        self.out.push(Violation::new(kind, path, detail));
    }
}

/// Every way `current` is not a compatible superset of `baseline`.
pub fn check_compat(baseline: &Contract, current: &Contract) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut ctx = Ctx { out: &mut out };
    for (name, base) in &baseline.interfaces {
        match current.interfaces.get(name) {
            None => ctx.push(Kind::InterfaceRemoved, name, "interface removed"),
            Some(cur) => compare_interface(baseline, current, base, cur, &mut ctx),
        }
    }
    out
}

fn compare_interface(
    baseline: &Contract,
    current: &Contract,
    base: &Interface,
    cur: &Interface,
    ctx: &mut Ctx<'_>,
) {
    let iface = &base.name;
    for (name, b) in &base.types {
        let path = format!("{iface}.{name}");
        match cur.types.get(name) {
            None => ctx.push(Kind::TypeRemoved, &path, "type removed"),
            Some(c) => compare_type(b, c, &path, ctx),
        }
    }
    for (name, b) in &base.methods {
        let path = format!("{iface}.{name}");
        match cur.methods.get(name) {
            None => ctx.push(Kind::MethodRemoved, &path, "method removed"),
            Some(c) => {
                compare_method(b, c, &path, ctx);
                let was = baseline.is_streaming(iface, name);
                let is = current.is_streaming(iface, name);
                if was != is {
                    let (from, to) = if was {
                        ("streaming", "plain")
                    } else {
                        ("plain", "streaming")
                    };
                    ctx.push(
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
            None => ctx.push(Kind::ErrorRemoved, &path, "error removed"),
            Some(c) => compare_error(b, c, &path, ctx),
        }
    }
}

fn compare_type(base: &CustomType, cur: &CustomType, path: &str, ctx: &mut Ctx<'_>) {
    match (&base.def, &cur.def) {
        (TypeDef::Object(b), TypeDef::Object(c)) => compare_fields(b, c, path, Role::Member, ctx),
        (TypeDef::Enum(b), TypeDef::Enum(c)) => compare_variants(b, c, path, ctx),
        (b, _) => {
            let (from, to) = match b {
                TypeDef::Object(_) => ("object", "enum"),
                TypeDef::Enum(_) => ("enum", "object"),
            };
            ctx.push(
                Kind::Retyped,
                path,
                format!("type changed from {from} to {to}"),
            );
        }
    }
}

fn compare_method(base: &Method, cur: &Method, path: &str, ctx: &mut Ctx<'_>) {
    if base.doc.has_tag("@destructive") && !cur.doc.has_tag("@destructive") {
        ctx.push(Kind::TagRemoved, path, "@destructive removed");
    }
    compare_fields(
        &base.inputs,
        &cur.inputs,
        &format!("{path}(in)"),
        Role::Input,
        ctx,
    );
    compare_fields(
        &base.outputs,
        &cur.outputs,
        &format!("{path}(out)"),
        Role::Output,
        ctx,
    );
}

/// The retry class an error is tagged with, if any.
fn error_class(doc: &Doc) -> Option<&str> {
    doc.tag_lines()
        .find(|l| *l == "@transient" || *l == "@permanent")
}

fn compare_error(base: &ErrorDef, cur: &ErrorDef, path: &str, ctx: &mut Ctx<'_>) {
    if let Some(was) = error_class(&base.doc)
        && error_class(&cur.doc) != Some(was)
    {
        let now = error_class(&cur.doc).unwrap_or("no marker");
        ctx.push(Kind::ErrorClassChanged, path, format!("{was} became {now}"));
    }
    compare_fields(&base.fields, &cur.fields, path, Role::ErrorParam, ctx);
}

fn compare_variants(base: &[Variant], cur: &[Variant], path: &str, ctx: &mut Ctx<'_>) {
    for b in base {
        if !cur.iter().any(|c| c.name == b.name) {
            ctx.push(
                Kind::EnumValueRemoved,
                &format!("{path}.{}", b.name),
                "enum value removed",
            );
        }
    }
}

fn compare_fields(base: &[Field], cur: &[Field], path: &str, role: Role, ctx: &mut Ctx<'_>) {
    for b in base {
        let fpath = format!("{path}.{}", b.name);
        let Some(c) = cur.iter().find(|c| c.name == b.name) else {
            ctx.push(Kind::FieldRemoved, &fpath, "removed");
            continue;
        };
        if b.doc.has_tag("@untrusted") && !c.doc.has_tag("@untrusted") {
            ctx.push(Kind::TagRemoved, &fpath, "@untrusted removed");
        }
        compare_ty(&b.ty, &c.ty, &fpath, role == Role::Input, ctx);
    }
    for c in cur {
        if !base.iter().any(|b| b.name == c.name) && !c.ty.is_optional() {
            ctx.push(
                Kind::NewRequired,
                &format!("{path}.{}", c.name),
                format!("new field must be optional, is {}", c.ty),
            );
        }
    }
}

/// `relax`: this is an input, so required -> optional is allowed here.
fn compare_ty(base: &Ty, cur: &Ty, path: &str, relax: bool, ctx: &mut Ctx<'_>) {
    let (b_opt, c_opt) = (base.is_optional(), cur.is_optional());
    if b_opt != c_opt && !(relax && !b_opt && c_opt) {
        let (from, to) = if b_opt {
            ("optional", "required")
        } else {
            ("required", "optional")
        };
        ctx.push(
            Kind::OptionalityChanged,
            path,
            format!("changed from {from} to {to}"),
        );
    }
    compare_inner(base.unwrap_optional(), cur.unwrap_optional(), path, ctx);
}

fn compare_inner(base: &Ty, cur: &Ty, path: &str, ctx: &mut Ctx<'_>) {
    match (base, cur) {
        (Ty::Array(b), Ty::Array(c)) => compare_ty(b, c, &format!("{path}[]"), false, ctx),
        (Ty::Map(b), Ty::Map(c)) => compare_ty(b, c, &format!("{path}[string]"), false, ctx),
        (Ty::Enum(b), Ty::Enum(c)) => compare_variants(b, c, path, ctx),
        (Ty::Object(b), Ty::Object(c)) => compare_fields(b, c, path, Role::Member, ctx),
        (b, c) if b == c => {}
        (b, c) => ctx.push(Kind::Retyped, path, format!("retyped from {b} to {c}")),
    }
}
