//! Contract checker for the Tetsu bus IDL.
//!
//! Parses the Varlink IDL of `tetsu-bus` into an owned model and checks it
//! against the rules of `CONTRACT.md`: compatibility with the frozen
//! baseline, tag and doc rules, and the event fixtures. Nothing here is
//! linked into a daemon.

mod bless;
mod compat;
mod fixtures;
mod load;
mod model;
mod rules;
mod violation;

pub use bless::{BlessError, bless};
pub use compat::check_compat;
pub use fixtures::{EventType, Fixture, check_fixtures, event_types, read_fixtures};
pub use load::{LoadError, load_dir, read_dir_sources};
pub use model::{
    Contract, CustomType, Doc, ErrorDef, Field, Interface, Method, Ty, TypeDef, Variant,
};
pub use rules::check_rules;
pub use violation::{Kind, Violation};
