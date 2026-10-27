//! What an output is, without Smithay: the EDID fields, the mode list and
//! the geometry. The backends feed these, the model keeps the result.
//! `configure` and `prefs` are the pure part of `ConfigureOutputs`.

pub mod configure;
pub mod edid;
pub mod geometry;
pub mod limits;
pub mod modes;
pub mod prefs;
