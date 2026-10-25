//! Contract checker for the Tetsu bus IDL.
//!
//! Parses the Varlink IDL of `tetsu-bus` into an owned model and checks it
//! against the rules of `CONTRACT.md`: compatibility with the frozen
//! baseline, tag and doc rules, and the event fixtures. Nothing here is
//! linked into a daemon.

mod compat;
mod load;
mod model;
mod violation;

pub use compat::check_compat;
pub use load::{LoadError, load_dir, read_dir_sources};
pub use model::{
    Contract, CustomType, Doc, ErrorDef, Field, Interface, Method, Ty, TypeDef, Variant,
};
pub use violation::{Kind, Violation};
