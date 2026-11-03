//! `org.tetsu.cap.v1`: the wire types, shaped exactly as the IDL, and a
//! client for the methods any caller may use.

use std::fmt;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zlink::{Call, Connection};

use crate::endpoint::{Endpoint, connect_verified};
use crate::into_io;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.cap.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.cap.v1.varlink");
/// `UnknownClass (class: string)`.
pub const UNKNOWN_CLASS: &str = "org.tetsu.cap.v1.UnknownClass";
/// `UnknownMethod (method: string)`.
pub const UNKNOWN_METHOD: &str = "org.tetsu.cap.v1.UnknownMethod";
/// `OverriddenByUser (class: string)`.
pub const OVERRIDDEN_BY_USER: &str = "org.tetsu.cap.v1.OverriddenByUser";
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.cap.v1.PermissionDenied";

/// `Capability.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Not chosen yet; asked like `ask`.
    Unset,
    /// Allowed without a word.
    Automatic,
    /// Allowed, and the user is told.
    Notify,
    /// Asked every time.
    Ask,
    /// Refused.
    Off,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// The states `Set` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// Allowed without a word.
    Automatic,
    /// Allowed, and the user is told.
    Notify,
    /// Asked every time.
    Ask,
    /// Refused.
    Off,
}

impl From<Grant> for State {
    fn from(grant: Grant) -> Self {
        match grant {
            Grant::Automatic => Self::Automatic,
            Grant::Notify => Self::Notify,
            Grant::Ask => Self::Ask,
            Grant::Off => Self::Off,
        }
    }
}

impl Grant {
    /// The wire and file name of the state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Notify => "notify",
            Self::Ask => "ask",
            Self::Off => "off",
        }
    }
}

/// `Capability.layer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// The image defaults.
    Image,
    /// Written through `Set`.
    Managed,
    /// The user's own file.
    User,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `Capability`. Nullable fields are written as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// The class.
    pub class: String,
    /// Null for a grant on the whole class.
    #[serde(default)]
    pub scope: Option<String>,
    /// How a call of the class is decided.
    pub state: State,
    /// The layer that sets the state; null when `unset`.
    #[serde(default)]
    pub layer: Option<Layer>,
    /// Whether the class holds a `@destructive` method.
    pub destructive: bool,
    /// The fully qualified methods of the class.
    pub methods: Vec<String>,
    /// Counts since the service started.
    pub asks: i64,
    /// Approved asks.
    pub approvals: i64,
    /// Denied asks.
    pub denials: i64,
    /// Asks held back by a suppression.
    pub suppressions: i64,
    /// Null when never used.
    #[serde(default)]
    pub last_used_at: Option<String>,
    /// Null when asks are not suppressed.
    #[serde(default)]
    pub suppressed_until_at: Option<String>,
}

/// `CapabilityEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Every capability.
    Initial,
    /// One capability changed.
    Changed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `CapabilityEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityEvent {
    /// Which event.
    pub kind: EventKind,
    /// `initial`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<Capability>>,
    /// `changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<Capability>,
}

/// `Peer`: the process behind a `Check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckPeer {
    /// Process id.
    pub pid: i64,
    /// Unix user id.
    pub uid: i64,
    /// The systemd unit of the process.
    pub unit: String,
}

/// The parameters of `Check`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckParams {
    /// The process that made the call.
    pub peer: CheckPeer,
    /// The fully qualified method called.
    pub method: String,
    /// The arguments of the call.
    #[serde(default)]
    pub args: Value,
}

/// `Check.decision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// The call may run.
    Allow,
    /// The call is refused.
    Deny,
}

/// `Check.denied_by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeniedBy {
    /// The class is off.
    Off,
    /// The user said no.
    User,
    /// A recent refusal still holds.
    Suppressed,
    /// No decision could be made; refused.
    Unavailable,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// The reply of `Check`. `denied_by` is written as `null` on allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReply {
    /// The decision.
    pub decision: Decision,
    /// Why, when denied.
    #[serde(default)]
    pub denied_by: Option<DeniedBy>,
}

impl CheckReply {
    /// An allow.
    pub fn allow() -> Self {
        Self {
            decision: Decision::Allow,
            denied_by: None,
        }
    }

    /// A deny.
    pub fn deny(by: DeniedBy) -> Self {
        Self {
            decision: Decision::Deny,
            denied_by: Some(by),
        }
    }
}

/// The parameters of `Set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetParams {
    /// The class to set.
    pub class: String,
    /// The scope; null for the whole class.
    #[serde(default)]
    pub scope: Option<String>,
    /// Null removes the managed value.
    #[serde(default)]
    pub state: Option<Grant>,
}

/// The parameters of `Intent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentParams {
    /// The fully qualified method about to be called.
    pub method: String,
    /// Why, in the caller's words. Untrusted.
    pub reason: String,
}

/// Why a client call failed.
#[derive(Debug)]
pub enum ClientError {
    /// The socket, or the protocol.
    Io(io::Error),
    /// The service answered with an error.
    Remote {
        /// Fully qualified error name.
        error: String,
        /// The error parameters.
        parameters: Value,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Remote { error, .. } => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
    #[serde(default)]
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct Body<'a, P> {
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<&'a P>,
}

#[derive(Debug, Deserialize)]
struct Capabilities {
    capabilities: Vec<Capability>,
}

#[derive(Debug, Deserialize)]
struct Event {
    event: CapabilityEvent,
}

type Socket = zlink::tokio::unix::Stream;

/// A connection to the capability service.
#[derive(Debug)]
pub struct Client {
    connection: Connection<Socket>,
}

impl Client {
    /// Connects to the service socket.
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

    async fn send<P: Serialize + fmt::Debug>(
        &mut self,
        method: &'static str,
        parameters: Option<&P>,
        more: bool,
    ) -> Result<(), ClientError> {
        let call = Call::new(Body { method, parameters }).set_more(more);
        self.connection
            .send_call(&call, Vec::new())
            .await
            .map_err(into_io)?;
        Ok(())
    }

    /// Reads one reply: the parameters and whether more follow.
    async fn receive<R: DeserializeOwned + fmt::Debug>(
        &mut self,
    ) -> Result<(Option<R>, bool), ClientError> {
        let (reply, _fds) = self
            .connection
            .receive_reply::<R, RemoteError>()
            .await
            .map_err(into_io)?;
        match reply {
            Ok(reply) => {
                let continues = reply.continues() == Some(true);
                Ok((reply.into_parameters(), continues))
            }
            Err(e) => Err(ClientError::Remote {
                error: e.error,
                parameters: e.parameters,
            }),
        }
    }

    /// `ListCapabilities`.
    pub async fn list(&mut self) -> Result<Vec<Capability>, ClientError> {
        self.send::<Value>("org.tetsu.cap.v1.ListCapabilities", None, false)
            .await?;
        let (reply, _) = self.receive::<Capabilities>().await?;
        reply
            .map(|r| r.capabilities)
            .ok_or_else(|| io::Error::other("ListCapabilities reply without parameters").into())
    }

    /// `Check`. Any error, a malformed reply included, is an `Err`: the
    /// caller must treat it as a refusal.
    pub async fn check(&mut self, params: &CheckParams) -> Result<CheckReply, ClientError> {
        self.send("org.tetsu.cap.v1.Check", Some(params), false)
            .await?;
        let (reply, _) = self.receive::<CheckReply>().await?;
        reply.ok_or_else(|| io::Error::other("Check reply without parameters").into())
    }

    /// `Set`.
    pub async fn set(&mut self, params: &SetParams) -> Result<(), ClientError> {
        self.send("org.tetsu.cap.v1.Set", Some(params), false)
            .await?;
        self.receive::<Value>().await?;
        Ok(())
    }

    /// `Intent`.
    pub async fn intent(&mut self, params: &IntentParams) -> Result<(), ClientError> {
        self.send("org.tetsu.cap.v1.Intent", Some(params), false)
            .await?;
        self.receive::<Value>().await?;
        Ok(())
    }

    /// Starts `WatchCapabilities`. Consumes the connection.
    pub async fn watch(mut self) -> Result<CapWatch, ClientError> {
        self.send::<Value>("org.tetsu.cap.v1.WatchCapabilities", None, true)
            .await?;
        Ok(CapWatch {
            client: self,
            ended: false,
        })
    }
}

/// An open `WatchCapabilities` stream.
#[derive(Debug)]
pub struct CapWatch {
    client: Client,
    ended: bool,
}

impl CapWatch {
    /// The next event. After the final reply, or once the service went
    /// away, an `UnexpectedEof` error.
    pub async fn next(&mut self) -> Result<CapabilityEvent, ClientError> {
        if self.ended {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "watch ended").into());
        }
        let (reply, continues) = self.client.receive::<Event>().await?;
        self.ended = !continues;
        reply
            .map(|r| r.event)
            .ok_or_else(|| io::Error::other("WatchCapabilities reply without parameters").into())
    }
}
