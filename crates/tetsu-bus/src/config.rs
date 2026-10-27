//! `org.tetsu.comp.config.v1`: the wire types, shaped exactly as the IDL.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zlink::Call;

use crate::cap::ClientError;
use crate::into_io;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.comp.config.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.comp.config.v1.varlink");
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.comp.config.v1.PermissionDenied";
/// The fully qualified `SetManaged`, the method the gate asks capd about.
pub const SET_MANAGED: &str = "org.tetsu.comp.config.v1.SetManaged";

/// The fully qualified `GetConfig`.
pub const GET_CONFIG: &str = "org.tetsu.comp.config.v1.GetConfig";

/// `ConfigEntry.origin`: the layer the effective value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The image or built-in defaults.
    Default,
    /// The managed state.
    Managed,
    /// The user file.
    User,
}

/// `ConfigEntry`. The nullable fields are always written, as `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigEntry {
    /// Dotted path, for example `layout.gaps_outer`.
    pub key: String,
    /// The effective value.
    pub value: Value,
    /// Which layer it comes from.
    pub origin: Origin,
    /// The default layer's value, if it sets the key.
    #[serde(default)]
    pub default_value: Option<Value>,
    /// The managed layer's value, if it sets the key.
    #[serde(default)]
    pub managed_value: Option<Value>,
    /// The user layer's value, if it sets the key.
    #[serde(default)]
    pub user_value: Option<Value>,
    /// Whether `SetManaged` accepts the key.
    pub writable: bool,
    /// For a key that is not writable, the method that writes it.
    #[serde(default)]
    pub method: Option<String>,
}

/// `Diagnostic`: a problem found when the user file was last loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// 1-based line in the user file.
    #[serde(default)]
    pub line: Option<i64>,
    /// The key it is about.
    #[serde(default)]
    pub key: Option<String>,
    /// What is wrong.
    pub message: String,
}

/// `ConfigEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigKind {
    /// The first event of a stream.
    Initial,
    /// One key changed.
    Changed,
    /// A key went away.
    Removed,
    /// The diagnostics of the user file.
    Diagnostics,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchConfig`. Absent fields are omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigEvent {
    /// Which event.
    pub kind: ConfigKind,
    /// `initial`: every known key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<ConfigEntry>>,
    /// `changed`: the full new state of one key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<ConfigEntry>,
    /// `removed`: the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// `initial`, `diagnostics`: the full list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
}

impl ConfigEvent {
    fn of(kind: ConfigKind) -> Self {
        Self {
            kind,
            entries: None,
            entry: None,
            key: None,
            diagnostics: None,
        }
    }

    /// `initial`.
    pub fn initial(entries: Vec<ConfigEntry>, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            entries: Some(entries),
            diagnostics: Some(diagnostics),
            ..Self::of(ConfigKind::Initial)
        }
    }

    /// `changed`.
    pub fn changed(entry: ConfigEntry) -> Self {
        Self {
            entry: Some(entry),
            ..Self::of(ConfigKind::Changed)
        }
    }

    /// `removed`.
    pub fn removed(key: &str) -> Self {
        Self {
            key: Some(key.to_owned()),
            ..Self::of(ConfigKind::Removed)
        }
    }

    /// `diagnostics`.
    pub fn diagnostics(diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            diagnostics: Some(diagnostics),
            ..Self::of(ConfigKind::Diagnostics)
        }
    }
}

/// The reply of `GetConfig`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GetConfigReply {
    /// Every known key.
    pub entries: Vec<ConfigEntry>,
    /// The problems of the user file.
    pub diagnostics: Vec<Diagnostic>,
}

/// The parameters of `SetManaged`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetManagedParams {
    /// Key to the new managed value.
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
    /// Keys whose managed value is removed.
    #[serde(default)]
    pub remove: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct GetConfigCall {
    method: &'static str,
}

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
    #[serde(default)]
    parameters: Value,
}

/// Connects to the compositor socket and calls `GetConfig`.
pub async fn get_config(path: &Path) -> Result<GetConfigReply, ClientError> {
    let mut connection = zlink::tokio::unix::connect(path).await.map_err(into_io)?;
    let call = Call::new(GetConfigCall { method: GET_CONFIG });
    connection
        .send_call(&call, Vec::new())
        .await
        .map_err(into_io)?;
    let (reply, _fds) = connection
        .receive_reply::<GetConfigReply, RemoteError>()
        .await
        .map_err(into_io)?;
    match reply {
        Ok(reply) => reply
            .into_parameters()
            .ok_or_else(|| io::Error::other("GetConfig reply without parameters").into()),
        Err(e) => Err(ClientError::Remote {
            error: e.error,
            parameters: e.parameters,
        }),
    }
}
