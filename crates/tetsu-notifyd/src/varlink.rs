//! `org.tetsu.notify.v1` on the facade server.

use std::rc::Rc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tetsu_bus::notify::{self, CloseParams, ShowParams};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{ErrorReply, Handler, Request, Response};
use tokio::sync::{mpsc, oneshot};

use crate::core::{Caller, ShowError};
use crate::daemon::Command;

/// The optional `Show` parameters, checked one by one to name a bad one.
const SHOW_OPTIONALS: [&str; 4] = ["body", "urgency", "actions", "timeout_ms"];

/// Serves `org.tetsu.notify.v1` and the `org.varlink.service` basics.
#[derive(Debug, Clone)]
pub struct NotifyService {
    commands: mpsc::UnboundedSender<Command>,
}

impl NotifyService {
    /// A service feeding the daemon through `commands`.
    pub fn new(commands: mpsc::UnboundedSender<Command>) -> Self {
        Self { commands }
    }

    async fn ask<T>(&self, command: impl FnOnce(oneshot::Sender<T>) -> Command) -> Option<T> {
        let (done, answer) = oneshot::channel();
        self.commands.send(command(done)).ok()?;
        answer.await.ok()
    }

    async fn show(&self, peer: &Peer, request: Request) -> Response {
        if !request.oneway && !request.more {
            return Response::Error(ErrorReply::new(
                "org.varlink.service.ExpectedMore",
                json!({}),
            ));
        }
        let params: ShowParams = match parse_show(&request.parameters) {
            Ok(params) => params,
            Err(parameter) => return Response::Error(ErrorReply::invalid_parameter(parameter)),
        };
        let caller = Caller {
            pid: peer.pid,
            sender: peer.sender_name(),
            agent: peer.agent,
        };
        let (stream, replies) = if request.oneway {
            (None, None)
        } else {
            let (tx, rx) = mpsc::unbounded_channel();
            (Some(tx), Some(rx))
        };
        let watch = stream.clone();
        let oneway = request.oneway;
        let shown = self
            .ask(|done| Command::ShowTetsu {
                caller,
                params,
                oneway,
                stream,
                done,
            })
            .await;
        match (shown, watch, replies) {
            (Some(Ok(note)), Some(watch), Some(replies)) => {
                // The facade drops `replies` when the caller goes away.
                let commands = self.commands.clone();
                tokio::task::spawn_local(async move {
                    watch.closed().await;
                    let _ = commands.send(Command::CallerGone(note));
                });
                Response::Stream(replies)
            }
            (Some(Ok(_)), _, _) => Response::Reply(Value::Null),
            (Some(Err(e)), _, _) => Response::Error(show_error(e, "")),
            (None, _, _) => unavailable(),
        }
    }

    async fn close(&self, peer: &Peer, parameters: &Value) -> Response {
        let Ok(CloseParams { id }) = parse(parameters) else {
            return Response::Error(ErrorReply::invalid_parameter("id"));
        };
        let pid = peer.pid;
        let target = id.clone();
        match self
            .ask(|done| Command::CloseTetsu {
                pid,
                id: target,
                done,
            })
            .await
        {
            Some(Ok(())) => Response::Reply(json!({})),
            Some(Err(e)) => Response::Error(show_error(e, &id)),
            None => unavailable(),
        }
    }
}

impl Handler for NotifyService {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        let method = request.method.clone();
        match method.as_str() {
            "org.tetsu.notify.v1.Show" => self.show(&peer, request).await,
            "org.tetsu.notify.v1.Close" => self.close(&peer, &request.parameters).await,
            "org.tetsu.notify.v1.GetPending" => match self.ask(Command::GetPending).await {
                Some(count) => Response::Reply(json!({ "count": count })),
                None => unavailable(),
            },
            "org.tetsu.notify.v1.WatchPending" => {
                let (tx, rx) = mpsc::unbounded_channel();
                match self.commands.send(Command::WatchPending(tx)) {
                    Ok(()) => Response::Stream(rx),
                    Err(_) => unavailable(),
                }
            }
            "org.tetsu.notify.v1.OpenPanel" => match self.commands.send(Command::OpenPanel) {
                Ok(()) => Response::Reply(json!({})),
                Err(_) => unavailable(),
            },
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "Tetsu",
                "product": "tetsu-notifyd",
                "version": env!("CARGO_PKG_VERSION"),
                "url": env!("CARGO_PKG_REPOSITORY"),
                "interfaces": ["org.varlink.service", notify::INTERFACE],
            })),
            "org.varlink.service.GetInterfaceDescription" => {
                match request.parameters["interface"].as_str() {
                    Some(notify::INTERFACE) => {
                        Response::Reply(json!({ "description": notify::IDL }))
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

fn parse<T: DeserializeOwned>(value: &Value) -> serde_json::Result<T> {
    serde_json::from_value(value.clone())
}

/// Parses `Show` parameters, naming the first bad parameter on failure.
fn parse_show(parameters: &Value) -> Result<ShowParams, &'static str> {
    parse(parameters).map_err(|_| {
        if !parameters["title"].is_string() {
            return "title";
        }
        SHOW_OPTIONALS
            .into_iter()
            .find(|name| {
                let mut only = json!({ "title": "" });
                only[*name] = parameters[*name].clone();
                parse::<ShowParams>(&only).is_err()
            })
            .unwrap_or("title")
    })
}

fn show_error(error: ShowError, id: &str) -> ErrorReply {
    match error {
        ShowError::PermissionDenied => ErrorReply::new(notify::PERMISSION_DENIED, json!({})),
        ShowError::InvalidParameter(parameter) => ErrorReply::invalid_parameter(parameter),
        ShowError::NotFound => ErrorReply::new(notify::NOTIFICATION_NOT_FOUND, json!({ "id": id })),
    }
}

/// The daemon task is gone; only seen while shutting down.
fn unavailable() -> Response {
    Response::Error(ErrorReply::new(
        "org.varlink.service.ServiceUnavailable",
        json!({}),
    ))
}
