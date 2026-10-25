//! Tag and doc rules of the current contract.

use crate::{Contract, Doc, ErrorDef, Field, Interface, Kind, Method, Ty, TypeDef, Violation};

/// The closed set of tags.
const TAGS: [&str; 6] = [
    "@capability",
    "@destructive",
    "@transient",
    "@permanent",
    "@untrusted",
    "@deprecated",
];

const ON_METHOD: &[&str] = &["@capability", "@destructive", "@deprecated"];
const ON_ERROR: &[&str] = &["@transient", "@permanent"];
// Inputs, replies and error parameters are fields too, as in type members.
const ON_FIELD: &[&str] = &["@untrusted", "@deprecated"];
const ON_NOTHING: &[&str] = &[];

/// Methods that change no state besides the one they exist to change in
/// the capability service itself: they need no `@capability`.
const CAPABILITY_EXEMPT: &[&str] = &[
    "org.tetsu.cap.v1.Intent",
    "org.tetsu.cap.v1.Set",
    "org.tetsu.cap.v1.Check",
];

/// Method name prefixes of reads, which need no `@capability`.
const READ_PREFIXES: [&str; 3] = ["List", "Get", "Watch"];

/// Every rule of CONTRACT.md's Tags and Shape sections the current IDL breaks.
pub fn check_rules(contract: &Contract) -> Vec<Violation> {
    let mut out = Vec::new();
    for iface in contract.interfaces.values() {
        check_interface(iface, &mut out);
    }
    check_streaming(contract, &mut out);
    out
}

/// A tag line must be exactly one tag of the closed set (`@capability` also
/// carries its arguments), on a member it may mark, and appear once.
fn check_tags(doc: &Doc, allowed: &[&str], path: &str, out: &mut Vec<Violation>) {
    let mut seen: Vec<&str> = Vec::new();
    for line in doc.tag_lines() {
        let name = Doc::tag_name(line);
        if !TAGS.contains(&name) || (name != "@capability" && line != name) {
            out.push(Violation::new(
                Kind::UnknownTag,
                path,
                format!("`{line}` is not exactly one tag of the closed set"),
            ));
        } else if !allowed.contains(&name) {
            out.push(Violation::new(
                Kind::MisplacedTag,
                path,
                format!("{name} cannot mark this member"),
            ));
        } else if seen.contains(&name) {
            out.push(Violation::new(
                Kind::DuplicateTag,
                path,
                format!("{name} appears more than once"),
            ));
        }
        seen.push(name);
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
        check_capability(m, &format!("{name}.{mname}"), out);
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
    check_fields(&m.inputs, &in_path, ON_FIELD, out);
    for p in &m.inputs {
        if !p.doc.has_prose() {
            out.push(Violation::new(
                Kind::MissingDoc,
                format!("{in_path}.{}", p.name),
                "input parameter has no doc comment",
            ));
        }
    }
    check_fields(&m.outputs, &format!("{path}(out)"), ON_FIELD, out);
}

/// A state-changing method carries a well-formed `@capability`, whose scope
/// names one of its inputs; reads and the exempt methods may omit it.
fn check_capability(m: &Method, path: &str, out: &mut Vec<Violation>) {
    match m.doc.capability() {
        Some(Err(why)) => out.push(Violation::new(Kind::InvalidCapability, path, why)),
        Some(Ok(cap)) => {
            if let Some(scope) = cap.scope
                && !m.inputs.iter().any(|p| p.name == scope)
            {
                out.push(Violation::new(
                    Kind::InvalidCapability,
                    path,
                    format!("scope `{scope}` is not an input parameter"),
                ));
            }
        }
        None => {
            let name = path.rsplit('.').next().unwrap_or(path);
            let read = READ_PREFIXES.iter().any(|p| name.starts_with(p));
            if !read && !CAPABILITY_EXEMPT.contains(&path) {
                out.push(Violation::new(
                    Kind::MissingCapability,
                    path,
                    "state-changing method has no @capability",
                ));
            }
        }
    }
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
    check_fields(&e.fields, path, ON_FIELD, out);
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
