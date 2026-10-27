//! The compositor's config: three TOML layers (defaults, managed state,
//! user file), merged key by key, with diagnostics (#73).

pub mod glue;
pub mod keys;
pub mod layer;
pub mod merge;
pub mod paths;
pub mod set;
pub mod store;
