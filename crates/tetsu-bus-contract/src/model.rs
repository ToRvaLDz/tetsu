//! Owned model of a parsed contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The comment lines of a member, `#` and leading blanks stripped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc(pub Vec<String>);

impl Doc {
    /// Lines that start with `@`: tag lines, trimmed.
    pub fn tag_lines(&self) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .map(|l| l.trim())
            .filter(|l| l.starts_with('@'))
    }

    /// Whether the doc carries exactly this tag on a line of its own.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tag_lines().any(|l| l == tag)
    }

    /// Whether there is any text beyond the tags.
    pub fn has_prose(&self) -> bool {
        self.0
            .iter()
            .map(|l| l.trim())
            .any(|l| !l.is_empty() && !l.starts_with('@'))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub doc: Doc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    pub doc: Doc,
}

/// A Varlink type. Inline enums and objects are kept in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Int,
    Float,
    String,
    /// The IDL's `object`: any JSON object.
    ForeignObject,
    Any,
    Optional(Box<Ty>),
    Array(Box<Ty>),
    Map(Box<Ty>),
    Custom(String),
    Enum(Vec<Variant>),
    Object(Vec<Field>),
}

impl Ty {
    pub fn is_optional(&self) -> bool {
        matches!(self, Ty::Optional(_))
    }

    /// The type without its outer `?`.
    pub fn unwrap_optional(&self) -> &Ty {
        match self {
            Ty::Optional(inner) => inner,
            other => other,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Bool => f.write_str("bool"),
            Ty::Int => f.write_str("int"),
            Ty::Float => f.write_str("float"),
            Ty::String => f.write_str("string"),
            Ty::ForeignObject => f.write_str("object"),
            Ty::Any => f.write_str("any"),
            Ty::Optional(t) => write!(f, "?{t}"),
            Ty::Array(t) => write!(f, "[]{t}"),
            Ty::Map(t) => write!(f, "[string]{t}"),
            Ty::Custom(n) => f.write_str(n),
            Ty::Enum(vs) => {
                let names: Vec<&str> = vs.iter().map(|v| v.name.as_str()).collect();
                write!(f, "({})", names.join(", "))
            }
            Ty::Object(fs) => {
                let items: Vec<String> =
                    fs.iter().map(|x| format!("{}: {}", x.name, x.ty)).collect();
                write!(f, "({})", items.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDef {
    Object(Vec<Field>),
    Enum(Vec<Variant>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomType {
    pub doc: Doc,
    pub def: TypeDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub doc: Doc,
    pub inputs: Vec<Field>,
    pub outputs: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDef {
    pub doc: Doc,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub doc: Doc,
    pub types: BTreeMap<String, CustomType>,
    pub methods: BTreeMap<String, Method>,
    pub errors: BTreeMap<String, ErrorDef>,
}

/// Every interface of a contract plus the streaming method list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contract {
    pub interfaces: BTreeMap<String, Interface>,
    /// Streaming (`more`) methods, as `<interface>.<Method>`.
    pub streaming: BTreeSet<String>,
}

impl Contract {
    /// Whether the method is on the streaming list.
    pub fn is_streaming(&self, interface: &str, method: &str) -> bool {
        self.streaming.contains(&format!("{interface}.{method}"))
    }
}
