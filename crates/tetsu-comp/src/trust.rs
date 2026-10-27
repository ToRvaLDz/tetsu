//! Who a Wayland client is, decided when it connects and fixed for its
//! lifetime. Only clients the compositor starts itself are trusted.

/// How much a client is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trust {
    /// Every client that comes through the listening socket, and any client
    /// whose trust is unknown.
    #[default]
    Untrusted,
    /// The session locker the compositor started: it may take over a
    /// defunct lock.
    Locker,
    /// A helper from `[trust] input`: it may bind the virtual input
    /// protocols.
    Input,
}
