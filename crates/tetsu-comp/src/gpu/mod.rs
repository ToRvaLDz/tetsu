//! The pure GPU decisions of the tty backend: which device renders, how each
//! output is served, which formats are offered, whether explicit sync is on
//! and how a commit is gated. No DRM, GBM or EGL here; the glue in
//! `backend::udev` feeds these functions and acts on the answers.
//!
//! The policy is ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787` (GPL-3.0-or-later, like this
//! crate).

pub mod assign;
pub mod control;
pub mod formats;
pub mod pending;
pub mod select;
pub mod sync;
