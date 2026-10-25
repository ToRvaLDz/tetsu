//! The server side of the facade: one task per connection on a `LocalSet`.
//!
//! zlink's own `Server` stops reading a connection once it streams, so a
//! caller that goes away mid-stream would only be noticed at the next write.
//! Here the connection keeps being read while a reply streams: end of file
//! drops the stream, and the handler sees its sender close.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::path::Path;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use zlink::connection::{ReadConnection, WriteConnection};
use zlink::{Call, Connection, Listener as _, Reply};

use crate::peer::Peer;

type Socket = zlink::tokio::unix::Stream;

/// A bound server socket.
#[derive(Debug)]
pub struct Listener(zlink::tokio::unix::Listener);

/// A method call as the handler sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Fully qualified method name.
    pub method: String,
    /// The parameters object (`null` when absent).
    pub parameters: Value,
    /// The caller wants no reply.
    pub oneway: bool,
    /// The caller accepts a stream of replies.
    pub more: bool,
}

/// A Varlink error reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorReply {
    /// Fully qualified error name.
    pub error: String,
    /// The error parameters object.
    pub parameters: Value,
}

impl ErrorReply {
    /// An error with parameters.
    pub fn new(error: &str, parameters: Value) -> Self {
        Self {
            error: error.to_owned(),
            parameters,
        }
    }

    /// `org.varlink.service.InvalidParameter`.
    pub fn invalid_parameter(parameter: &str) -> Self {
        Self::new(
            "org.varlink.service.InvalidParameter",
            json!({ "parameter": parameter }),
        )
    }
}

/// One reply of a stream.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamItem {
    /// A reply with more to follow.
    Reply(Value),
    /// The final reply; the stream ends.
    Last(Value),
    /// An error; the stream ends.
    Error(ErrorReply),
}

/// What a handler answers.
#[derive(Debug)]
pub enum Response {
    /// One reply.
    Reply(Value),
    /// An error reply.
    Error(ErrorReply),
    /// A stream of replies. Dropping the receiver (the caller went away)
    /// closes the handler's sender.
    Stream(mpsc::UnboundedReceiver<StreamItem>),
}

impl Response {
    /// `org.varlink.service.MethodNotFound`.
    pub fn method_not_found(method: &str) -> Self {
        Self::Error(ErrorReply::new(
            "org.varlink.service.MethodNotFound",
            json!({ "method": method }),
        ))
    }
}

/// Serves calls. Runs on a `LocalSet`, so it needs no `Send`.
pub trait Handler: 'static {
    /// Answers one call from `peer`.
    fn call(&self, peer: Rc<Peer>, request: Request) -> impl Future<Output = Response>;
}

/// The method part of a call: everything but `oneway`, `more`, `upgrade`.
#[derive(Debug, Deserialize)]
struct RawMethod {
    method: String,
    #[serde(default)]
    parameters: Value,
}

/// Binds a session socket: the parent directory is created 0700 when
/// missing, a stale socket is replaced, the socket is 0600.
pub fn bind(path: &Path) -> io::Result<Listener> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => fs::remove_file(path)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = zlink::tokio::unix::bind(path).map_err(into_io)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(Listener(listener))
}

/// Accepts connections forever, one local task each. Must run inside a
/// `tokio::task::LocalSet`.
pub async fn serve<H: Handler>(mut listener: Listener, handler: Rc<H>) -> io::Result<()> {
    loop {
        let Some(connection) = listener.0.accept().await.map_err(into_io)? else {
            return Ok(());
        };
        let handler = handler.clone();
        tokio::task::spawn_local(async move {
            if let Err(e) = run_connection(connection, handler).await {
                eprintln!("tetsu-bus: connection closed: {e}");
            }
        });
    }
}

fn into_io(e: zlink::Error) -> io::Error {
    match e {
        zlink::Error::Io(e) => e,
        other => io::Error::other(other.to_string()),
    }
}

async fn identify(connection: &mut Connection<Socket>) -> io::Result<Peer> {
    let credentials = connection.peer_credentials().await?.clone();
    // A kernel without SO_PEERPIDFD (< 6.5) gives no pidfd: fail closed.
    let pidfd = credentials
        .process_fd()
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "no peer pidfd"))?;
    Peer::resolve(
        credentials.unix_user_id().as_raw(),
        credentials.unix_primary_group_id().as_raw(),
        credentials.process_id().as_raw_nonzero().get(),
        pidfd,
    )
}

async fn run_connection<H: Handler>(
    mut connection: Connection<Socket>,
    handler: Rc<H>,
) -> io::Result<()> {
    let peer = Rc::new(identify(&mut connection).await?);
    let (mut read, mut write) = connection.split();
    // Calls pipelined while a reply streamed, answered in order afterwards.
    let mut queued = VecDeque::new();
    loop {
        let call = match queued.pop_front() {
            Some(call) => call,
            None => match receive(&mut read).await {
                Some(call) => call,
                None => return Ok(()),
            },
        };
        let response = handler.call(peer.clone(), call.clone()).await;
        if call.oneway {
            continue;
        }
        match response {
            Response::Reply(value) => send_reply(&mut write, value, false).await?,
            Response::Error(error) => send_error(&mut write, &error).await?,
            Response::Stream(_) if !call.more => {
                let error = ErrorReply::new("org.varlink.service.ExpectedMore", json!({}));
                send_error(&mut write, &error).await?;
            }
            Response::Stream(stream) => {
                if !relay(stream, &mut read, &mut write, &mut queued).await? {
                    return Ok(());
                }
            }
        }
    }
}

/// Sends a stream's replies while still reading the connection. Returns
/// `false` when the caller went away (the stream is dropped with it).
async fn relay(
    mut stream: mpsc::UnboundedReceiver<StreamItem>,
    read: &mut ReadConnection<<Socket as zlink::connection::Socket>::ReadHalf>,
    write: &mut WriteConnection<<Socket as zlink::connection::Socket>::WriteHalf>,
    queued: &mut VecDeque<Request>,
) -> io::Result<bool> {
    loop {
        tokio::select! {
            item = stream.recv() => match item {
                Some(StreamItem::Reply(value)) => send_reply(write, value, true).await?,
                Some(StreamItem::Last(value)) => {
                    send_reply(write, value, false).await?;
                    return Ok(true);
                }
                Some(StreamItem::Error(error)) => {
                    send_error(write, &error).await?;
                    return Ok(true);
                }
                // The handler dropped its sender without a final reply.
                None => return Ok(true),
            },
            call = receive(read) => match call {
                Some(call) => queued.push_back(call),
                None => return Ok(false),
            },
        }
    }
}

/// The next call, or `None` once the caller is gone or sent garbage.
async fn receive(
    read: &mut ReadConnection<<Socket as zlink::connection::Socket>::ReadHalf>,
) -> Option<Request> {
    let (call, _fds) = read.receive_call::<RawMethod>().await.ok()?;
    let call: Call<RawMethod> = call;
    Some(Request {
        oneway: call.oneway(),
        more: call.more(),
        method: call.method().method.clone(),
        parameters: call.method().parameters.clone(),
    })
}

async fn send_reply(
    write: &mut WriteConnection<<Socket as zlink::connection::Socket>::WriteHalf>,
    value: Value,
    continues: bool,
) -> io::Result<()> {
    let parameters = (!value.is_null()).then_some(value);
    let reply = Reply::new(parameters).set_continues(continues.then_some(true));
    write.send_reply(&reply, Vec::new()).await.map_err(into_io)
}

async fn send_error(
    write: &mut WriteConnection<<Socket as zlink::connection::Socket>::WriteHalf>,
    error: &ErrorReply,
) -> io::Result<()> {
    write.send_error(error, Vec::new()).await.map_err(into_io)
}
