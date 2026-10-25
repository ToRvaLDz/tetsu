//! Loading a contract from IDL sources.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use zlink_idl::{Comment, CustomType as ZCustom, Type as ZType};

use crate::model::{
    Contract, CustomType, Doc, ErrorDef, Field, Interface, Method, Ty, TypeDef, Variant,
};

/// Name of the streaming list next to the IDL files.
pub const STREAMING_LIST: &str = "streaming.list";

/// A source that cannot be read or parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(pub String);

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

fn doc<'a>(comments: impl Iterator<Item = &'a Comment<'a>>) -> Doc {
    Doc(comments.map(|c| c.text().trim().to_string()).collect())
}

fn variants<'a>(vs: impl Iterator<Item = &'a zlink_idl::EnumVariant<'a>>) -> Vec<Variant> {
    vs.map(|v| Variant {
        name: v.name().to_string(),
        doc: doc(v.comments()),
    })
    .collect()
}

fn fields<'a>(fs: impl Iterator<Item = &'a zlink_idl::Field<'a>>) -> Vec<Field> {
    fs.map(|f| Field {
        name: f.name().to_string(),
        ty: ty(f.ty()),
        doc: doc(f.comments()),
    })
    .collect()
}

fn ty(t: &ZType<'_>) -> Ty {
    match t {
        ZType::Bool => Ty::Bool,
        ZType::Int => Ty::Int,
        ZType::Float => Ty::Float,
        ZType::String => Ty::String,
        ZType::ForeignObject => Ty::ForeignObject,
        ZType::Any => Ty::Any,
        ZType::Optional(inner) => Ty::Optional(Box::new(ty(inner))),
        ZType::Array(inner) => Ty::Array(Box::new(ty(inner))),
        ZType::Map(inner) => Ty::Map(Box::new(ty(inner))),
        ZType::Custom(name) => Ty::Custom((*name).to_string()),
        ZType::Enum(vs) => Ty::Enum(variants(vs.iter())),
        ZType::Object(fs) => Ty::Object(fields(fs.iter())),
    }
}

fn interface(name_for_errors: &str, source: &str) -> Result<Interface, LoadError> {
    let parsed = zlink_idl::Interface::try_from(source)
        .map_err(|e| LoadError(format!("{name_for_errors}: {e}")))?;
    let mut out = Interface {
        name: parsed.name().to_string(),
        doc: doc(parsed.comments()),
        types: BTreeMap::new(),
        methods: BTreeMap::new(),
        errors: BTreeMap::new(),
    };
    let dup = |what: &str, n: &str| LoadError(format!("{name_for_errors}: duplicate {what} {n}"));
    for t in parsed.custom_types() {
        let (n, custom) = match t {
            ZCustom::Object(o) => (
                o.name(),
                CustomType {
                    doc: doc(o.comments()),
                    def: TypeDef::Object(fields(o.fields())),
                },
            ),
            ZCustom::Enum(e) => (
                e.name(),
                CustomType {
                    doc: doc(e.comments()),
                    def: TypeDef::Enum(variants(e.variants())),
                },
            ),
        };
        if out.types.insert(n.to_string(), custom).is_some() {
            return Err(dup("type", n));
        }
    }
    for m in parsed.methods() {
        let method = Method {
            doc: doc(m.comments()),
            inputs: fields(m.inputs()),
            outputs: fields(m.outputs()),
        };
        if out.methods.insert(m.name().to_string(), method).is_some() {
            return Err(dup("method", m.name()));
        }
    }
    for e in parsed.errors() {
        let error = ErrorDef {
            doc: doc(e.comments()),
            fields: fields(e.fields()),
        };
        if out.errors.insert(e.name().to_string(), error).is_some() {
            return Err(dup("error", e.name()));
        }
    }
    Ok(out)
}

/// The entries of a streaming list: one `<interface>.<Method>` per line,
/// `#` starts a comment.
fn streaming(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

impl Contract {
    /// Builds a contract from `(source name, IDL text)` pairs and the text
    /// of a streaming list.
    pub fn from_sources(
        sources: &[(&str, &str)],
        streaming_list: &str,
    ) -> Result<Contract, LoadError> {
        let mut interfaces = BTreeMap::new();
        for (name, text) in sources {
            let parsed = interface(name, text)?;
            let iface_name = parsed.name.clone();
            if interfaces.insert(iface_name.clone(), parsed).is_some() {
                return Err(LoadError(format!(
                    "{name}: duplicate interface {iface_name}"
                )));
            }
        }
        Ok(Contract {
            interfaces,
            streaming: streaming(streaming_list),
        })
    }
}

/// Reads every `*.varlink` file of `dir` (not recursively), sorted by name,
/// as `(file name, text)`.
pub fn read_dir_sources(dir: &Path) -> Result<Vec<(String, String)>, LoadError> {
    let io = |e: std::io::Error, p: &Path| LoadError(format!("{}: {e}", p.display()));
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io(e, dir))? {
        let entry = entry.map_err(|e| io(e, dir))?;
        if !entry.path().is_file() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|n| LoadError(format!("{}: file name {n:?} is not UTF-8", dir.display())))?;
        if name.ends_with(".varlink") {
            names.push(name);
        }
    }
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let path = dir.join(&n);
            let text = fs::read_to_string(&path).map_err(|e| io(e, &path))?;
            Ok((n, text))
        })
        .collect()
}

/// Loads every `*.varlink` of `dir` plus its `streaming.list`.
pub fn load_dir(dir: &Path) -> Result<Contract, LoadError> {
    let sources = read_dir_sources(dir)?;
    let list_path = dir.join(STREAMING_LIST);
    let list = fs::read_to_string(&list_path)
        .map_err(|e| LoadError(format!("{}: {e}", list_path.display())))?;
    let borrowed: Vec<(&str, &str)> = sources
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    Contract::from_sources(&borrowed, &list)
}
