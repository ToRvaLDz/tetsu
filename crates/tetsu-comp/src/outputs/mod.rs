//! What an output is, without Smithay: the EDID fields, the mode list and
//! the geometry. The backends feed these, the model keeps the result.

pub mod edid;
pub mod geometry;
pub mod limits;
pub mod modes;
