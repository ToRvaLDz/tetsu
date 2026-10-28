//! The Varlink methods of the window, workspace, output, session, config
//! and render interfaces.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use tetsu_bus::config::{self, SetManagedParams};
use tetsu_bus::output::ConfigureOutputsParams;
use tetsu_bus::peer::Peer;
use tetsu_bus::render::{
    self as render_v1, SetExplicitSyncParams, SetRenderDeviceParams, SetRenderReply,
};
use tetsu_bus::server::{ErrorReply, Handler, Request, Response};
use tetsu_bus::{output, session, window, workspace};
use tokio::sync::oneshot;

use super::command::{Command, CommandSink, ConfigureOutputs};
use super::config::SetManaged;
use super::hub::Hub;
use super::render::{SetExplicitSync, SetRenderDevice};
use crate::config::set;
use crate::gpu::control::RenderError;
use crate::settings::SyncMode;

/// Serves the reads of the six interfaces from the hub, and `SetManaged`,
/// `ConfigureOutputs` and the render Set calls through the sink to the
/// loop. The gate in front
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
        defer(&sink, command, answer, empty, config_stopping)
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
        defer(&sink, command, answer, empty, config_stopping)
    }

    /// `SetRenderDevice`: the loop owns the devices and the managed file.
    fn set_render_device(&self, request: &Request) -> Response {
        let Some(sink) = self.commands.clone() else {
            return not_implemented(&request.method);
        };
        let params: SetRenderDeviceParams = match parameters(request) {
            Some(params) => params,
            None => return Response::Error(ErrorReply::invalid_parameter("device_id")),
        };
        let (reply, answer) = oneshot::channel();
        let command = Command::SetRenderDevice(SetRenderDevice {
            target: params.device_id,
            reply,
        });
        defer(&sink, command, answer, pending, render_stopping)
    }

    /// `SetExplicitSync`, like `SetRenderDevice`.
    fn set_explicit_sync(&self, request: &Request) -> Response {
        let Some(sink) = self.commands.clone() else {
            return not_implemented(&request.method);
        };
        let mode = match parameters::<SetExplicitSyncParams>(request).map(|p| p.mode) {
            Some(render_v1::SyncMode::Auto) => SyncMode::Auto,
            Some(render_v1::SyncMode::On) => SyncMode::On,
            Some(render_v1::SyncMode::Off) => SyncMode::Off,
            Some(render_v1::SyncMode::Unknown) | None => {
                return Response::Error(ErrorReply::invalid_parameter("mode"));
            }
        };
        let (reply, answer) = oneshot::channel();
        let command = Command::SetExplicitSync(SetExplicitSync { mode, reply });
        defer(&sink, command, answer, pending, render_stopping)
    }
}

/// The parameters of a call; absent ones read as an empty object.
fn parameters<T: serde::de::DeserializeOwned>(request: &Request) -> Option<T> {
    let raw = if request.parameters.is_null() {
        json!({})
    } else {
        request.parameters.clone()
    };
    serde_json::from_value(raw).ok()
}

/// The reply of a call that answers `{}`.
fn empty((): ()) -> Value {
    json!({})
}

/// The reply of a render Set call.
fn pending(pending: bool) -> Value {
    serde_json::to_value(SetRenderReply { pending }).unwrap_or(Value::Null)
}

fn config_stopping() -> ErrorReply {
    set::state_write_failed("the compositor is stopping")
}

fn render_stopping() -> ErrorReply {
    RenderError::StateWriteFailed("the compositor is stopping".to_owned()).reply()
}

/// Sends a command to the loop and answers when the loop does, `ok`
/// turning its value into the reply. A loop that is gone answers
/// `stopping`, the interface's `StateWriteFailed`.
fn defer<T: 'static>(
    sink: &CommandSink,
    command: Command,
    answer: oneshot::Receiver<Result<T, ErrorReply>>,
    ok: fn(T) -> Value,
    stopping: fn() -> ErrorReply,
) -> Response {
    if sink(command).is_err() {
        return Response::Error(stopping());
    }
    let (tx, rx) = oneshot::channel();
    tokio::task::spawn_local(async move {
        let response = match answer.await {
            Ok(Ok(value)) => Response::Reply(ok(value)),
            Ok(Err(error)) => Response::Error(error),
            Err(_) => Response::Error(stopping()),
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
            render_v1::GET_RENDER => value(&self.hub.borrow().mirror().render()),
            render_v1::WATCH_RENDER => {
                Response::BoundedStream(self.hub.borrow_mut().watch_render())
            }
            render_v1::SET_RENDER_DEVICE => self.set_render_device(&request),
            render_v1::SET_EXPLICIT_SYNC => self.set_explicit_sync(&request),
            // The stats come with #108.
            render_v1::LIST_RENDER_STATS | render_v1::WATCH_RENDER_STATS => {
                not_implemented(&request.method)
            }
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
                    render_v1::INTERFACE,
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
                    Some(render_v1::INTERFACE) => {
                        Response::Reply(json!({ "description": render_v1::IDL }))
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
