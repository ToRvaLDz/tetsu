//! Tetsu bus: Varlink interface definitions and shared client/server glue.
//!
//! The only crate that imports zlink (#15): daemons use these types and
//! functions, never zlink directly.

pub mod notify;
pub mod peer;
pub mod server;
pub mod session;
pub mod tx;

fn into_io(e: zlink::Error) -> std::io::Error {
    match e {
        zlink::Error::Io(e) => e,
        zlink::Error::UnexpectedEof => std::io::ErrorKind::UnexpectedEof.into(),
        other => std::io::Error::other(other.to_string()),
    }
}
