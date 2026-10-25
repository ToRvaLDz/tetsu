//! Tag and doc rules of the current contract.

use crate::{Contract, Doc, ErrorDef, Field, Interface, Kind, Method, Ty, TypeDef, Violation};

/// The closed set of tags.
const TAGS: [&str; 5] = [
    "@destructive",
    "@transient",
    "@permanent",
    "@untrusted",
    "@deprecated",
];

const ON_METHOD: &[&str] = &["@destructive", "@deprecated"];
const ON_ERROR: &[&str] = &["@transient", "@permanent"];
const ON_PARAMETER: &[&str] = &["@untrusted"];
const ON_FIELD: &[&str] = &["@untrusted", "@deprecated"];
const ON_NOTHING: &[&str] = &[];

/// Every rule of CONTRACT.md's Tags and Shape sections the current IDL breaks.
pub fn check_rules(contract: &Contract) -> Vec<Violation> {
    let mut out = Vec::new();
    for iface in contract.interfaces.values() {
        check_interface(iface, &mut out);
    }
    check_streaming(contract, &mut out);
    out
}

/// A tag line must be exactly one tag of the closed set, on a member it may
/// mark, and appear once.
fn check_tags(doc: &Doc, allowed: &[&str], path: &str, out: &mut Vec<Violation>) {
    let mut seen: Vec<&str> = Vec::new();
    for line in doc.tag_lines() {
        if !TAGS.contains(&line) {
            out.push(Violation::new(
                Kind::UnknownTag,
                path,
                format!("`{line}` is not exactly one tag of the closed set"),
            ));
        } else if !allowed.contains(&line) {
            out.push(Violation::new(
                Kind::MisplacedTag,
                path,
                format!("{line} cannot mark this member"),
            ));
        } else if seen.contains(&line) {
            out.push(Violation::new(
                Kind::DuplicateTag,
                path,
                format!("{line} appears more than once"),
            ));
        }
        seen.push(line);
    }
}

fn check_interface(iface: &Interface, out: &mut Vec<Violation>) {
    let name = &iface.name;
    check_tags(&iface.doc, ON_NOTHING, name, out);
    if !iface.errors.contains_key("PermissionDenied") {
        out.push(Violation::new(
            Kind::MissingPermissionDenied,
            name,
            "interface does not declare the PermissionDenied error",
        ));
    }
    for (tname, t) in &iface.types {
        let path = format!("{name}.{tname}");
        check_tags(&t.doc, ON_NOTHING, &path, out);
        match &t.def {
            TypeDef::Object(fields) => check_fields(fields, &path, ON_FIELD, out),
            TypeDef::Enum(variants) => check_variants(variants, &path, out),
        }
    }
    for (mname, m) in &iface.methods {
        check_method(m, &format!("{name}.{mname}"), out);
    }
    for (ename, e) in &iface.errors {
        check_error(e, &format!("{name}.{ename}"), out);
    }
}

fn check_method(m: &Method, path: &str, out: &mut Vec<Violation>) {
    check_tags(&m.doc, ON_METHOD, path, out);
    if !m.doc.has_prose() {
        out.push(Violation::new(
            Kind::MissingDoc,
            path,
            "method has no doc comment",
        ));
    }
    let in_path = format!("{path}(in)");
    check_fields(&m.inputs, &in_path, ON_PARAMETER, out);
    for p in &m.inputs {
        if !p.doc.has_prose() {
            out.push(Violation::new(
                Kind::MissingDoc,
                format!("{in_path}.{}", p.name),
                "input parameter has no doc comment",
            ));
        }
    }
    check_fields(&m.outputs, &format!("{path}(out)"), ON_PARAMETER, out);
}

fn check_error(e: &ErrorDef, path: &str, out: &mut Vec<Violation>) {
    check_tags(&e.doc, ON_ERROR, path, out);
    let classes = e.doc.tag_lines().filter(|l| ON_ERROR.contains(l)).count();
    if classes != 1 {
        out.push(Violation::new(
            Kind::ErrorClass,
            path,
            format!("error must carry exactly one of @transient and @permanent, has {classes}"),
        ));
    }
    check_fields(&e.fields, path, ON_PARAMETER, out);
}

/// Checks the tags of `fields`, then those of every inline type below them.
fn check_fields(fields: &[Field], path: &str, allowed: &[&str], out: &mut Vec<Violation>) {
    for f in fields {
        let fpath = format!("{path}.{}", f.name);
        check_tags(&f.doc, allowed, &fpath, out);
        check_nested(&f.ty, &fpath, out);
    }
}

fn check_variants(variants: &[crate::Variant], path: &str, out: &mut Vec<Violation>) {
    for v in variants {
        check_tags(&v.doc, ON_NOTHING, &format!("{path}.{}", v.name), out);
    }
}

fn check_nested(ty: &Ty, path: &str, out: &mut Vec<Violation>) {
    match ty {
        Ty::Optional(t) => check_nested(t, path, out),
        Ty::Array(t) => check_nested(t, &format!("{path}[]"), out),
        Ty::Map(t) => check_nested(t, &format!("{path}[string]"), out),
        Ty::Object(fields) => check_fields(fields, path, ON_FIELD, out),
        Ty::Enum(variants) => check_variants(variants, path, out),
        _ => {}
    }
}

/// The streaming list names only existing methods and every `Watch*`.
fn check_streaming(contract: &Contract, out: &mut Vec<Violation>) {
    for entry in &contract.streaming {
        let exists = entry.rsplit_once('.').is_some_and(|(iface, method)| {
            contract
                .interfaces
                .get(iface)
                .is_some_and(|i| i.methods.contains_key(method))
        });
        if !exists {
            out.push(Violation::new(
                Kind::StreamingUnknown,
                entry,
                "streaming list names no existing method",
            ));
        }
    }
    for (iname, iface) in &contract.interfaces {
        for mname in iface.methods.keys().filter(|m| m.starts_with("Watch")) {
            if !contract.is_streaming(iname, mname) {
                out.push(Violation::new(
                    Kind::WatchNotStreaming,
                    format!("{iname}.{mname}"),
                    "Watch method is not on the streaming list",
                ));
            }
        }
    }
}
