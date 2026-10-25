//! Tetsu notification server: `org.tetsu.notify.v1` for bus callers and
//! `org.freedesktop.Notifications` for applications (#23).

pub mod core;
pub mod daemon;
pub mod fdo;
pub mod icon;
pub mod lock;
pub mod markup;
pub mod ui;
pub mod varlink;
