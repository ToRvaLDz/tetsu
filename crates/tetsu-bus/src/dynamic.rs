//! A generic Varlink client: calls any method by its fully qualified name
//! with JSON parameters and returns JSON. It lets a caller that learns its
//! methods at run time (the agent) talk to any service without importing
//! zlink (#15).
//!
//! Errors are [`ClientError`] of the capability client: `Io` for the socket
//! or the protocol, `Remote` for an error reply of the service.

use std::fmt;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zlink::{Call, Connection};

pub use crate::cap::ClientError;
use crate::endpoint::{Endpoint, connect_verified};
use crate::into_io;

/// The method that lists the interfaces a service speaks.
const GET_INFO: &str = "org.varlink.service.GetInfo";

type Socket = zlink::tokio::unix::Stream;

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
    #[serde(default)]
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct Body<'a> {
    method: &'a str,
    parameters: &'a Value,
}

/// What `org.varlink.service.GetInfo` tells that the bus uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Info {
    /// The interfaces the service speaks, fully qualified.
    #[serde(default)]
    pub interfaces: Vec<String>,
}

/// One reply of a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The reply parameters; `{}` when the reply carried none.
    pub value: Value,
    /// Whether more replies follow.
    pub continues: bool,
}

/// A connection to a service, for calls by name.
#[derive(Debug)]
pub struct Client {
    connection: Connection<Socket>,
}

impl Client {
    /// Connects to a service socket.
    pub async fn connect(path: &Path) -> Result<Self, ClientError> {
        let connection = zlink::tokio::unix::connect(path).await.map_err(into_io)?;
        Ok(Self { connection })
    }

    /// Connects to a service at a fixed path, after checking who serves it.
    /// A wrong server is `PermissionDenied` and receives nothing.
    pub async fn connect_to(endpoint: &Endpoint) -> Result<Self, ClientError> {
        let connection = connect_verified(endpoint).await?;
        Ok(Self { connection })
    }

    async fn send(&mut self, method: &str, parameters: &Value, more: bool) -> io::Result<()> {
        let call = Call::new(Body { method, parameters }).set_more(more);
        self.connection
            .send_call(&call, Vec::new())
            .await
            .map_err(into_io)?;
        Ok(())
    }

    /// Reads one reply: its parameters (`{}` when absent) and `continues`.
    async fn receive<R: DeserializeOwned + fmt::Debug + Default>(
        &mut self,
    ) -> Result<(R, bool), ClientError> {
        let (reply, _fds) = self
            .connection
            .receive_reply::<R, RemoteError>()
            .await
            .map_err(into_io)?;
        match reply {
            Ok(reply) => {
                let continues = reply.continues() == Some(true);
                Ok((reply.into_parameters().unwrap_or_default(), continues))
            }
            Err(e) => Err(ClientError::Remote {
                error: e.error,
                parameters: e.parameters,
            }),
        }
    }

    /// One call with one reply.
    pub async fn call(&mut self, method: &str, parameters: &Value) -> Result<Value, ClientError> {
        self.send(method, parameters, false).await?;
        Ok(empty_object(self.receive::<Value>().await?.0))
    }

    /// A call that accepts a stream of replies. Consumes the connection:
    /// dropping the stream closes it, which cancels the call.
    pub async fn call_more(
        mut self,
        method: &str,
        parameters: &Value,
    ) -> Result<DynStream, ClientError> {
        self.send(method, parameters, true).await?;
        Ok(DynStream {
            client: self,
            ended: false,
        })
    }

    /// `org.varlink.service.GetInfo`.
    pub async fn get_info(&mut self) -> Result<Info, ClientError> {
        self.send(GET_INFO, &Value::Object(Default::default()), false)
            .await?;
        let (value, _) = self.receive::<Value>().await?;
        serde_json::from_value(empty_object(value))
            .map_err(|e| io::Error::other(format!("malformed GetInfo reply: {e}")).into())
    }
}

/// A reply without parameters reads as `null`; the contract's empty reply
/// is `{}`.
fn empty_object(value: Value) -> Value {
    if value.is_null() {
        Value::Object(Default::default())
    } else {
        value
    }
}

/// An open stream of replies.
#[derive(Debug)]
pub struct DynStream {
    client: Client,
    ended: bool,
}

impl DynStream {
    /// The next reply, or `None` once the last one was returned. End of
    /// file before the last reply is an `UnexpectedEof` error.
    pub async fn next(&mut self) -> Result<Option<Item>, ClientError> {
        if self.ended {
            return Ok(None);
        }
        let (value, continues) = self.client.receive::<Value>().await?;
        self.ended = !continues;
        Ok(Some(Item {
            value: empty_object(value),
            continues,
        }))
    }
}
