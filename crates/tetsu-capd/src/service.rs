//! `org.tetsu.cap.v1` on the facade server.

use std::rc::Rc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tetsu_bus::cap::{self, CheckParams, IntentParams, SetParams};
use tetsu_bus::peer::{Peer, unit_name};
use tetsu_bus::server::{ErrorReply, Handler, Request, Response, StreamItem};
use tokio::sync::{mpsc, oneshot};

use crate::ask::NotifyChannel;
use crate::audit::AuditSink;
use crate::auth::{self, Call};
use crate::clock::Clock;
use crate::engine::{CapError, Engine};

/// How `Check` callers are authorised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckAuth {
    /// By unit and uid, see `auth`.
    Units,
    /// Everyone; for tests only.
    #[doc(hidden)]
    AllowAll,
}

/// Serves `org.tetsu.cap.v1` and the `org.varlink.service` basics.
pub struct CapService<N, C, A> {
    engine: Rc<Engine<N, C, A>>,
    own_uid: u32,
    check_auth: CheckAuth,
}

impl<N, C, A> CapService<N, C, A> {
    /// A service over an engine, run by the user `own_uid`.
    pub fn new(engine: Rc<Engine<N, C, A>>, own_uid: u32) -> Self {
        Self {
            engine,
            own_uid,
            check_auth: CheckAuth::Units,
        }
    }

    /// A service that lets any peer call `Check`. For tests; `main` never
    /// calls it.
    #[doc(hidden)]
    pub fn allow_all_checks(engine: Rc<Engine<N, C, A>>, own_uid: u32) -> Self {
        Self {
            check_auth: CheckAuth::AllowAll,
            ..Self::new(engine, own_uid)
        }
    }

    /// The engine.
    pub fn engine(&self) -> &Rc<Engine<N, C, A>> {
        &self.engine
    }
}

fn parse<T: DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

fn permission_denied() -> Response {
    Response::Error(ErrorReply::new(cap::PERMISSION_DENIED, json!({})))
}

fn invalid(parameter: &str) -> Response {
    Response::Error(ErrorReply::invalid_parameter(parameter))
}

fn error_reply(error: CapError) -> ErrorReply {
    match error {
        CapError::UnknownMethod(method) => {
            ErrorReply::new(cap::UNKNOWN_METHOD, json!({ "method": method }))
        }
        CapError::UnknownClass(class) => {
            ErrorReply::new(cap::UNKNOWN_CLASS, json!({ "class": class }))
        }
        CapError::OverriddenByUser(class) => {
            ErrorReply::new(cap::OVERRIDDEN_BY_USER, json!({ "class": class }))
        }
        CapError::InvalidParameter(parameter) => ErrorReply::invalid_parameter(parameter),
        CapError::Unavailable => {
            ErrorReply::new("org.varlink.service.ServiceUnavailable", json!({}))
        }
    }
}

/// Names the first bad parameter of a `Check` call.
fn check_parameter(parameters: &Value) -> &'static str {
    let peer = &parameters["peer"];
    if !(peer["pid"].is_i64() && peer["uid"].is_i64() && peer["unit"].is_string()) {
        "peer"
    } else if !parameters["method"].is_string() {
        "method"
    } else {
        "args"
    }
}

/// Names the first bad parameter of a `Set` call.
fn set_parameter(parameters: &Value) -> &'static str {
    if !parameters["class"].is_string() {
        "class"
    } else if !matches!(parameters["scope"], Value::Null | Value::String(_)) {
        "scope"
    } else {
        "state"
    }
}

impl<N, C, A> CapService<N, C, A>
where
    N: NotifyChannel + 'static,
    C: Clock + 'static,
    A: AuditSink + 'static,
{
    fn watch(&self, request: &Request) -> Response {
        if !request.more {
            return Response::Error(ErrorReply::new(
                "org.varlink.service.ExpectedMore",
                json!({}),
            ));
        }
        let (initial, mut events) = self.engine.watch();
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(StreamItem::Reply(json!({ "event": initial })));
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Some(event) => {
                            if tx.send(StreamItem::Reply(json!({ "event": event }))).is_err() {
                                return;
                            }
                        }
                        None => return,
                    },
                    () = tx.closed() => return,
                }
            }
        });
        Response::Stream(rx)
    }

    fn set(&self, peer: &Peer, parameters: &Value) -> Response {
        if !auth::allowed(peer, self.own_uid, Call::Set) {
            return permission_denied();
        }
        let Some(params) = parse::<SetParams>(parameters) else {
            return invalid(set_parameter(parameters));
        };
        match self.engine.set(&caller_unit(peer), &params) {
            Ok(()) => Response::Reply(json!({})),
            Err(e) => Response::Error(error_reply(e)),
        }
    }

    fn intent(&self, peer: &Peer, parameters: &Value) -> Response {
        let Some(params) = parse::<IntentParams>(parameters) else {
            let bad = if parameters["method"].is_string() {
                "reason"
            } else {
                "method"
            };
            return invalid(bad);
        };
        match self.engine.intent(i64::from(peer.pid), &params) {
            Ok(()) => Response::Reply(json!({})),
            Err(e) => Response::Error(error_reply(e)),
        }
    }

    /// The answer comes later, from a task that gives up when the caller
    /// closes its connection: dropping the engine future withdraws the ask.
    fn check(&self, peer: &Peer, parameters: &Value) -> Response {
        let allowed = self.check_auth == CheckAuth::AllowAll
            || auth::allowed(peer, self.own_uid, Call::Check);
        if !allowed {
            return permission_denied();
        }
        let params = match parse::<CheckParams>(parameters) {
            Some(params) if params.args.is_object() || params.args.is_null() => params,
            _ => return invalid(check_parameter(parameters)),
        };
        let (mut tx, rx) = oneshot::channel();
        let engine = self.engine.clone();
        let caller = caller_unit(peer);
        tokio::task::spawn_local(async move {
            tokio::select! {
                result = engine.check(&caller, &params) => {
                    let answer = match result {
                        Ok(reply) => Ok(json!(reply)),
                        Err(e) => Err(error_reply(e)),
                    };
                    let _ = tx.send(answer);
                }
                () = tx.closed() => {}
            }
        });
        Response::Pending(rx)
    }
}

/// The unit of the calling process, for the audit trail.
fn caller_unit(peer: &Peer) -> String {
    unit_name(&peer.cgroup).unwrap_or("unknown").to_owned()
}

impl<N, C, A> Handler for CapService<N, C, A>
where
    N: NotifyChannel + 'static,
    C: Clock + 'static,
    A: AuditSink + 'static,
{
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.tetsu.cap.v1.ListCapabilities" => {
                Response::Reply(json!({ "capabilities": self.engine.list() }))
            }
            "org.tetsu.cap.v1.WatchCapabilities" => self.watch(&request),
            "org.tetsu.cap.v1.Set" => self.set(&peer, &request.parameters),
            "org.tetsu.cap.v1.Intent" => self.intent(&peer, &request.parameters),
            "org.tetsu.cap.v1.Check" => self.check(&peer, &request.parameters),
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "Tetsu",
                "product": "tetsu-capd",
                "version": env!("CARGO_PKG_VERSION"),
                "url": env!("CARGO_PKG_REPOSITORY"),
                "interfaces": ["org.varlink.service", cap::INTERFACE],
            })),
            "org.varlink.service.GetInterfaceDescription" => {
                match request.parameters["interface"].as_str() {
                    Some(cap::INTERFACE) => Response::Reply(json!({ "description": cap::IDL })),
                    Some(other) => Response::Error(ErrorReply::new(
                        "org.varlink.service.InterfaceNotFound",
                        json!({ "interface": other }),
                    )),
                    None => invalid("interface"),
                }
            }
            other => Response::method_not_found(other),
        }
    }
}
