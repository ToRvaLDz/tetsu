//! `org.tetsu.secrets.v1`: the wire types, shaped exactly as the IDL, and a
//! client for the calls that do not stream.
//!
//! No type here can hold a secret value except [`Sensitive`], which
//! redacts itself in `Debug`, has no `Display` and is zeroized on drop.
//! Every client call runs under a `NoSubscriber`, so the facade never
//! traces a message body.

use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::instrument::WithSubscriber as _;
use tracing::subscriber::NoSubscriber;
use zeroize::Zeroize as _;
use zlink::{Call, Connection};

use crate::into_io;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.secrets.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.secrets.v1.varlink");
/// `SecretNotFound (id: string)`.
pub const SECRET_NOT_FOUND: &str = "org.tetsu.secrets.v1.SecretNotFound";
/// `SecretExists (id: string)`.
pub const SECRET_EXISTS: &str = "org.tetsu.secrets.v1.SecretExists";
/// `VaultUnavailable ()`.
pub const VAULT_UNAVAILABLE: &str = "org.tetsu.secrets.v1.VaultUnavailable";
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.secrets.v1.PermissionDenied";

/// A secret value on the wire: redacted in `Debug`, no `Display`, zeroized
/// on drop.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sensitive(String);

impl Sensitive {
    /// Wraps a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value; the caller must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Sensitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sensitive(***)")
    }
}

impl Drop for Sensitive {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// `SecretInfo.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    /// `Authorization: Bearer <value>`.
    Bearer,
    /// The value in a named header.
    Header,
    /// HTTP basic authentication.
    Basic,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One secret: its binding and counters, never its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretInfo {
    /// The name.
    pub id: String,
    /// How the value is presented upstream.
    pub kind: SecretKind,
    /// `header`: the header name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_name: Option<String>,
    /// The only upstream host the value may go to.
    pub host: String,
    /// Tool ids that may receive the value.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// When the secret was added.
    pub created_at: String,
    /// When the value was last replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<String>,
    /// When the broker last used it since it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    /// Uses since the broker started.
    #[serde(default)]
    pub use_count: i64,
}

/// `SecretEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretEventKind {
    /// The first event; carries `secrets`.
    Initial,
    /// A secret was added; carries `secret`.
    Added,
    /// A secret changed; carries `secret`.
    Changed,
    /// A secret was removed; carries `id`.
    Removed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchSecrets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretEvent {
    /// Which event.
    pub kind: SecretEventKind,
    /// `initial`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<SecretInfo>>,
    /// `added`, `changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretInfo>,
    /// `removed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl SecretEvent {
    fn of(kind: SecretEventKind) -> Self {
        Self {
            kind,
            secrets: None,
            secret: None,
            id: None,
        }
    }

    /// `initial`.
    pub fn initial(secrets: Vec<SecretInfo>) -> Self {
        Self {
            secrets: Some(secrets),
            ..Self::of(SecretEventKind::Initial)
        }
    }

    /// `added`.
    pub fn added(secret: SecretInfo) -> Self {
        Self {
            secret: Some(secret),
            ..Self::of(SecretEventKind::Added)
        }
    }

    /// `changed`.
    pub fn changed(secret: SecretInfo) -> Self {
        Self {
            secret: Some(secret),
            ..Self::of(SecretEventKind::Changed)
        }
    }

    /// `removed`.
    pub fn removed(id: &str) -> Self {
        Self {
            id: Some(id.to_owned()),
            ..Self::of(SecretEventKind::Removed)
        }
    }
}

/// The parameters of `Add`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddParams {
    /// The name.
    pub id: String,
    /// How the value is presented upstream.
    pub kind: SecretKind,
    /// `header`: the header name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_name: Option<String>,
    /// The upstream host.
    pub host: String,
    /// Tool ids that may receive the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// The value.
    pub value: Sensitive,
}

/// The parameters of `Rotate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateParams {
    /// The secret.
    pub id: String,
    /// The new value.
    pub value: Sensitive,
}

/// The parameters of `Remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveParams {
    /// The secret.
    pub id: String,
}

/// What a client call failed with.
#[derive(Debug)]
pub enum ClientError {
    /// The socket or the framing failed.
    Io(io::Error),
    /// The server answered with a Varlink error: its fully qualified name
    /// and parameters (an id or a parameter name, never a value).
    Remote {
        /// Fully qualified error name.
        name: String,
        /// The error parameters.
        parameters: Value,
    },
    /// The reply did not have the shape of the IDL.
    Protocol(String),
}

impl ClientError {
    /// The remote error name, when the server sent one.
    pub fn remote_name(&self) -> Option<&str> {
        match self {
            Self::Remote { name, .. } => Some(name),
            _ => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Remote { name, .. } => write!(f, "{name}"),
            Self::Protocol(why) => write!(f, "unexpected reply: {why}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Debug, Serialize)]
struct CallBody<P> {
    method: &'static str,
    parameters: P,
}

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
    #[serde(default)]
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct ListReply {
    secrets: Vec<SecretInfo>,
}

#[derive(Debug, Deserialize)]
struct SecretReply {
    secret: SecretInfo,
}

/// A connection to the secrets broker. Its calls never stream.
#[derive(Debug)]
pub struct Client {
    connection: Connection<zlink::tokio::unix::Stream>,
}

impl Client {
    /// Connects to the broker socket.
    pub async fn connect(path: &Path) -> io::Result<Self> {
        let connection = zlink::tokio::unix::connect(path)
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(into_io)?;
        Ok(Self { connection })
    }

    /// `ListSecrets`.
    pub async fn list(&mut self) -> Result<Vec<SecretInfo>, ClientError> {
        let reply = self.call("ListSecrets", json!({})).await?;
        parse::<ListReply>(reply).map(|r| r.secrets)
    }

    /// `Add`. The value is sent and never returned.
    pub async fn add(&mut self, params: &AddParams) -> Result<SecretInfo, ClientError> {
        let reply = self.call("Add", params).await?;
        parse::<SecretReply>(reply).map(|r| r.secret)
    }

    /// `Rotate`.
    pub async fn rotate(&mut self, params: &RotateParams) -> Result<SecretInfo, ClientError> {
        let reply = self.call("Rotate", params).await?;
        parse::<SecretReply>(reply).map(|r| r.secret)
    }

    /// `Remove`.
    pub async fn remove(&mut self, id: &str) -> Result<(), ClientError> {
        let params = RemoveParams { id: id.to_owned() };
        self.call("Remove", &params).await.map(|_| ())
    }

    async fn call<P: Serialize + fmt::Debug>(
        &mut self,
        method: &'static str,
        parameters: P,
    ) -> Result<Value, ClientError> {
        let call = Call::new(CallBody { method, parameters });
        let sent = self
            .connection
            .call_method::<_, Value, RemoteError>(&call, Vec::new())
            .with_subscriber(NoSubscriber::default())
            .await;
        match sent {
            Ok((Ok(reply), _fds)) => Ok(reply.into_parameters().unwrap_or(Value::Null)),
            Ok((Err(error), _fds)) => Err(ClientError::Remote {
                name: error.error,
                parameters: error.parameters,
            }),
            Err(zlink::Error::VarlinkService(e)) => Err(service_error(&e)),
            Err(e) => Err(ClientError::Io(into_io(e))),
        }
    }
}

/// An `org.varlink.service` error as a remote error.
fn service_error(e: &zlink::varlink_service::Error<'_>) -> ClientError {
    use zlink::varlink_service::Error;
    let (name, parameters) = match e {
        Error::InvalidParameter { parameter } => (
            "org.varlink.service.InvalidParameter",
            json!({ "parameter": parameter }),
        ),
        Error::MethodNotFound { method } => (
            "org.varlink.service.MethodNotFound",
            json!({ "method": method }),
        ),
        Error::PermissionDenied => ("org.varlink.service.PermissionDenied", json!({})),
        Error::ExpectedMore => ("org.varlink.service.ExpectedMore", json!({})),
        _ => ("org.varlink.service.Error", json!({})),
    };
    ClientError::Remote {
        name: name.to_owned(),
        parameters,
    }
}

fn parse<T: serde::de::DeserializeOwned>(reply: Value) -> Result<T, ClientError> {
    serde_json::from_value(reply).map_err(|e| ClientError::Protocol(e.to_string()))
}
