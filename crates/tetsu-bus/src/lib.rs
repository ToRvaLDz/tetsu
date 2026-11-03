//! Tetsu bus: Varlink interface definitions and shared client/server glue.
//!
//! The only crate that imports zlink (#15): daemons use these types and
//! functions, never zlink directly.

pub mod access;
pub mod cap;
pub mod config;
pub mod dynamic;
pub mod endpoint;
pub mod gate;
pub mod notify;
mod nullable;
pub mod output;
pub mod peer;
pub mod secrets;
pub mod server;
pub mod session;
pub mod tx;
pub mod window;
pub mod workspace;

fn into_io(e: zlink::Error) -> std::io::Error {
    match e {
        zlink::Error::Io(e) => e,
        zlink::Error::UnexpectedEof => std::io::ErrorKind::UnexpectedEof.into(),
        other => std::io::Error::other(other.to_string()),
    }
}
