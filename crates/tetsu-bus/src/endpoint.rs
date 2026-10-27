//! Fixed-path endpoints: a client that knows where a service lives and who
//! must be running it (#28 item 6). The server is resolved from the
//! connection with the same pidfd method as a peer, and checked before the
//! first byte is sent, so a squatter on the path never sees a call.

use std::io;
use std::path::{Path, PathBuf};

use zlink::Connection;

pub use crate::access::ExpectedServer;
use crate::into_io;
use crate::peer::Peer;

type Socket = zlink::tokio::unix::Stream;

/// A socket path and the server expected behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    path: PathBuf,
    expected: ExpectedServer,
}

impl Endpoint {
    /// An endpoint at `path`, served by `expected`.
    pub fn new(path: impl Into<PathBuf>, expected: ExpectedServer) -> Self {
        Self {
            path: path.into(),
            expected,
        }
    }

    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The server that must answer there.
    pub fn expected(&self) -> &ExpectedServer {
        &self.expected
    }
}

/// Connects, then checks the server. A server that cannot be identified, or
/// is not the expected one, is `PermissionDenied`, and nothing was sent.
pub async fn connect_verified(endpoint: &Endpoint) -> io::Result<Connection<Socket>> {
    let mut connection = zlink::tokio::unix::connect(endpoint.path())
        .await
        .map_err(into_io)?;
    let credentials = connection.peer_credentials().await?.clone();
    let server = Peer::from_credentials(
        credentials.unix_user_id().as_raw(),
        credentials.unix_primary_group_id().as_raw(),
        credentials.process_id().as_raw_nonzero().get(),
        credentials.process_fd(),
    )?;
    if !endpoint.expected().matches(&server) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not served by the expected process (found {})",
                endpoint.path().display(),
                server.cgroup
            ),
        ));
    }
    Ok(connection)
}
