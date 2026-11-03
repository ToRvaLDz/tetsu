//! The Varlink methods of the window, workspace, output, session and
//! config interfaces.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use tetsu_bus::config::{self, SetManagedParams};
use tetsu_bus::output::ConfigureOutputsParams;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{ErrorReply, Handler, Request, Response};
use tetsu_bus::{output, session, window, workspace};
use tokio::sync::oneshot;

use super::command::{Command, CommandSink, ConfigureOutputs};
use super::config::SetManaged;
use super::hub::Hub;
use crate::config::set;

/// Serves the reads of the five interfaces from the hub, and `SetManaged`
/// and `ConfigureOutputs` through the sink to the loop. The gate in front
/// of it (#28) decides who may call the mutating methods.
pub struct BusService {
    hub: Rc<RefCell<Hub>>,
    commands: Option<CommandSink>,
}

impl BusService {
    /// A service over `hub`. `commands` reaches the loop; without it
    /// the mutating methods are not implemented.
    pub fn new(hub: Rc<RefCell<Hub>>, commands: Option<CommandSink>) -> Self {
        Self { hub, commands }
    }

    /// `SetManaged`: the loop owns the layers, so the call goes there and
    /// its answer comes back later.
    fn set_managed(&self, request: &Request) -> Response {
        let Some(sink) = self.commands.clone() else {
            return not_implemented(&request.method);
        };
        let params: SetManagedParams = match serde_json::from_value(request.parameters.clone()) {
            Ok(params) => params,
            Err(_) => return Response::Error(ErrorReply::invalid_parameter("values")),
        };
        let (reply, answer) = oneshot::channel();
        let command = Command::SetManaged(SetManaged {
            values: params.values,
            remove: params.remove.unwrap_or_default(),
            reply,
        });
        defer(&sink, command, answer)
    }

    /// `ConfigureOutputs`: the loop owns the outputs and the managed
    /// file, so the call goes there like `SetManaged`.
    fn configure_outputs(&self, request: &Request) -> Response {
        let Some(sink) = self.commands.clone() else {
            return not_implemented(&request.method);
        };
        let params: ConfigureOutputsParams =
            match serde_json::from_value(request.parameters.clone()) {
                Ok(params) => params,
                Err(_) => return Response::Error(ErrorReply::invalid_parameter("configs")),
            };
        let (reply, answer) = oneshot::channel();
        let command = Command::ConfigureOutputs(ConfigureOutputs {
            configs: params.configs,
            reply,
        });
        defer(&sink, command, answer)
    }
}

/// Sends a command to the loop and answers when the loop does. The error
/// of a write that cannot happen (the loop is gone) is `StateWriteFailed`.
fn defer(
    sink: &CommandSink,
    command: Command,
    answer: oneshot::Receiver<Result<(), ErrorReply>>,
) -> Response {
    let stopping = || Response::Error(set::state_write_failed("the compositor is stopping"));
    if sink(command).is_err() {
        return stopping();
    }
    let (tx, rx) = oneshot::channel();
    tokio::task::spawn_local(async move {
        let response = match answer.await {
            Ok(Ok(())) => Response::Reply(json!({})),
            Ok(Err(error)) => Response::Error(error),
            Err(_) => Response::Error(set::state_write_failed("the compositor is stopping")),
        };
        let _ = tx.send(response);
    });
    Response::Deferred(rx)
}

fn not_implemented(method: &str) -> Response {
    Response::Error(ErrorReply::new(
        "org.varlink.service.MethodNotImplemented",
        json!({ "method": method }),
    ))
}

fn value<T: serde::Serialize>(reply: &T) -> Response {
    Response::Reply(serde_json::to_value(reply).unwrap_or(Value::Null))
}

impl Handler for BusService {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.tetsu.comp.window.v1.ListWindows" => value(&self.hub.borrow().mirror().windows()),
            "org.tetsu.comp.window.v1.WatchWindows" => {
                Response::BoundedStream(self.hub.borrow_mut().watch_windows())
            }
            "org.tetsu.comp.workspace.v1.ListWorkspaces" => {
                value(&self.hub.borrow().mirror().workspaces())
            }
            "org.tetsu.comp.workspace.v1.WatchWorkspaces" => {
                Response::BoundedStream(self.hub.borrow_mut().watch_workspaces())
            }
            "org.tetsu.comp.output.v1.ListOutputs" => value(&self.hub.borrow().mirror().outputs()),
            "org.tetsu.comp.output.v1.WatchOutputs" => {
                Response::BoundedStream(self.hub.borrow_mut().watch_outputs())
            }
            "org.tetsu.comp.session.v1.GetSession" => value(&self.hub.borrow().mirror().session()),
            "org.tetsu.comp.session.v1.WatchSession" => {
                Response::BoundedStream(self.hub.borrow_mut().watch_session())
            }
            "org.tetsu.comp.config.v1.GetConfig" => value(&self.hub.borrow().mirror().config()),
            "org.tetsu.comp.config.v1.WatchConfig" => {
                Response::BoundedStream(self.hub.borrow_mut().watch_config())
            }
            "org.tetsu.comp.config.v1.SetManaged" => self.set_managed(&request),
            "org.tetsu.comp.output.v1.ConfigureOutputs" => self.configure_outputs(&request),
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "Tetsu",
                "product": "tetsu-comp",
                "version": env!("CARGO_PKG_VERSION"),
                "url": env!("CARGO_PKG_REPOSITORY"),
                "interfaces": [
                    "org.varlink.service",
                    window::INTERFACE,
                    workspace::INTERFACE,
                    output::INTERFACE,
                    session::INTERFACE,
                    config::INTERFACE,
                ],
            })),
            "org.varlink.service.GetInterfaceDescription" => {
                match request.parameters["interface"].as_str() {
                    Some(window::INTERFACE) => {
                        Response::Reply(json!({ "description": window::IDL }))
                    }
                    Some(workspace::INTERFACE) => {
                        Response::Reply(json!({ "description": workspace::IDL }))
                    }
                    Some(output::INTERFACE) => {
                        Response::Reply(json!({ "description": output::IDL }))
                    }
                    Some(session::INTERFACE) => {
                        Response::Reply(json!({ "description": session::IDL }))
                    }
                    Some(config::INTERFACE) => {
                        Response::Reply(json!({ "description": config::IDL }))
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
