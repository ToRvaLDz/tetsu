//! `org.tetsu.secrets.v1` on the facade server.
//!
//! Answers are metadata only. A rejected parameter is named, never echoed,
//! so a `@sensitive` input cannot come back in an error. Writes from the
//! agent unit are `PermissionDenied`, whatever they carry.

use std::rc::Rc;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::secrets::{self, SecretKind};
use tetsu_bus::server::{ErrorReply, Handler, Request, Response};

use crate::broker::{Broker, BrokerError, NewSecret};
use crate::caller::Caller;
use crate::model::{Host, KindSpec, ModelError, SecretId, ToolId, check_value};
use crate::vault::Secret;

/// Serves the interface over a [`Broker`].
pub struct SecretsService {
    broker: Rc<Broker>,
}

/// A rejected parameter: its name only.
type Invalid = &'static str;

impl SecretsService {
    /// A service answering from `broker`.
    pub fn new(broker: Rc<Broker>) -> Self {
        Self { broker }
    }

    async fn list(&self, caller: &Caller) -> Response {
        match self.broker.list(caller).await {
            Ok(secrets) => Response::Reply(json!({ "secrets": secrets })),
            Err(e) => broker_error(&e),
        }
    }

    async fn watch(&self, caller: &Caller) -> Response {
        match self.broker.watch(caller).await {
            Ok(stream) => Response::Stream(stream),
            Err(e) => broker_error(&e),
        }
    }

    async fn add(&self, caller: &Caller, mut parameters: Value) -> Response {
        let new = match parse_add(&mut parameters) {
            Ok(new) => new,
            Err(parameter) => return Response::Error(ErrorReply::invalid_parameter(parameter)),
        };
        match self.broker.add(caller, new).await {
            Ok(secret) => Response::Reply(json!({ "secret": secret })),
            Err(e) => broker_error(&e),
        }
    }

    async fn rotate(&self, caller: &Caller, mut parameters: Value) -> Response {
        let parsed = parse_id(&parameters).and_then(|id| Ok((id, take_value(&mut parameters)?)));
        let (id, value) = match parsed {
            Ok(parsed) => parsed,
            Err(parameter) => return Response::Error(ErrorReply::invalid_parameter(parameter)),
        };
        match self.broker.rotate(caller, &id, value).await {
            Ok(secret) => Response::Reply(json!({ "secret": secret })),
            Err(e) => broker_error(&e),
        }
    }

    async fn remove(&self, caller: &Caller, parameters: &Value) -> Response {
        let id = match parse_id(parameters) {
            Ok(id) => id,
            Err(parameter) => return Response::Error(ErrorReply::invalid_parameter(parameter)),
        };
        match self.broker.remove(caller, &id).await {
            Ok(()) => Response::Reply(json!({})),
            Err(e) => broker_error(&e),
        }
    }

    /// A write from the agent: audited, then refused whatever it carries.
    fn deny(&self, caller: &Caller, parameters: &Value) -> Response {
        let id = parse_id(parameters).ok();
        self.broker.deny_write(caller, id.as_ref());
        Response::Error(ErrorReply::new(secrets::PERMISSION_DENIED, json!({})))
    }
}

fn model_error(e: &ModelError) -> Invalid {
    e.field
}

fn parse_id(parameters: &Value) -> Result<SecretId, Invalid> {
    let text = parameters["id"].as_str().ok_or("id")?;
    SecretId::parse(text).map_err(|e| model_error(&e))
}

/// The `value` parameter, moved out of the request's own parameters so the
/// request holds no copy of it after this. Not a scrub of the process: the
/// transport read buffer and the copy made when the call is received are
/// not zeroized. `PR_SET_DUMPABLE=0` (set at startup) is what keeps other
/// same-uid processes from reading that memory.
fn take_value(parameters: &mut Value) -> Result<Secret, Invalid> {
    let Some(Value::String(text)) = parameters.get_mut("value") else {
        return Err("value");
    };
    let text = std::mem::take(text);
    let value = Secret::new(text);
    check_value(value.expose()).map_err(|e| model_error(&e))?;
    Ok(value)
}

fn optional_text(parameters: &Value, name: Invalid) -> Result<Option<&str>, Invalid> {
    match &parameters[name] {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text)),
        _ => Err(name),
    }
}

fn parse_add(parameters: &mut Value) -> Result<NewSecret, Invalid> {
    let id = parse_id(parameters)?;
    let kind: SecretKind =
        serde_json::from_value(parameters["kind"].clone()).map_err(|_| "kind")?;
    let header = optional_text(parameters, "header_name")?;
    let kind = KindSpec::from_wire(kind, header).map_err(|e| model_error(&e))?;
    let host = parameters["host"].as_str().ok_or("host")?;
    let host = Host::parse(host).map_err(|e| model_error(&e))?;
    let allowed_tools = match &parameters["allowed_tools"] {
        Value::Null => Vec::new(),
        Value::Array(list) => list
            .iter()
            .map(|t| {
                let text = t.as_str().ok_or("allowed_tools")?;
                ToolId::parse(text).map_err(|e| model_error(&e))
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("allowed_tools"),
    };
    let value = take_value(parameters)?;
    Ok(NewSecret {
        id,
        kind,
        host,
        allowed_tools,
        value,
    })
}

fn broker_error(e: &BrokerError) -> Response {
    let (name, parameters) = match e {
        BrokerError::NotFound(id) => (secrets::SECRET_NOT_FOUND, json!({ "id": id.as_str() })),
        BrokerError::Exists(id) => (secrets::SECRET_EXISTS, json!({ "id": id.as_str() })),
        BrokerError::Denied(_) => (secrets::PERMISSION_DENIED, json!({})),
        BrokerError::Unavailable => (secrets::VAULT_UNAVAILABLE, json!({})),
    };
    Response::Error(ErrorReply::new(name, parameters))
}

impl Handler for SecretsService {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        let caller = Caller::from_peer(&peer);
        let method = request.method.clone();
        let write = matches!(
            method.as_str(),
            "org.tetsu.secrets.v1.Add"
                | "org.tetsu.secrets.v1.Rotate"
                | "org.tetsu.secrets.v1.Remove"
        );
        if write && peer.agent {
            return self.deny(&caller, &request.parameters);
        }
        let parameters = request.parameters;
        match method.as_str() {
            "org.tetsu.secrets.v1.ListSecrets" => self.list(&caller).await,
            "org.tetsu.secrets.v1.WatchSecrets" => self.watch(&caller).await,
            "org.tetsu.secrets.v1.Add" => self.add(&caller, parameters).await,
            "org.tetsu.secrets.v1.Rotate" => self.rotate(&caller, parameters).await,
            "org.tetsu.secrets.v1.Remove" => self.remove(&caller, &parameters).await,
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "Tetsu",
                "product": "tetsu-secrets",
                "version": env!("CARGO_PKG_VERSION"),
                "url": env!("CARGO_PKG_REPOSITORY"),
                "interfaces": ["org.varlink.service", secrets::INTERFACE],
            })),
            "org.varlink.service.GetInterfaceDescription" => {
                match parameters["interface"].as_str() {
                    Some(secrets::INTERFACE) => {
                        Response::Reply(json!({ "description": secrets::IDL }))
                    }
                    Some(other) => Response::Error(ErrorReply::new(
                        "org.varlink.service.InterfaceNotFound",
                        json!({ "interface": other }),
                    )),
                    None => Response::Error(ErrorReply::invalid_parameter("interface")),
                }
            }
            other => Response::method_not_found(other),
        }
    }
}
