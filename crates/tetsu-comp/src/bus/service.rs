//! The Varlink methods of the window, workspace and output interfaces.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{ErrorReply, Handler, Request, Response};
use tetsu_bus::{output, session, window, workspace};

use super::hub::Hub;

/// Serves the reads of the four interfaces from the hub. They are open to every
/// caller (#22.8 gates mutating calls only, and there are none yet).
pub struct BusService {
    hub: Rc<RefCell<Hub>>,
}

impl BusService {
    /// A service over `hub`.
    pub fn new(hub: Rc<RefCell<Hub>>) -> Self {
        Self { hub }
    }
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
            // #97 serves it.
            "org.tetsu.comp.output.v1.ConfigureOutputs" => Response::Error(ErrorReply::new(
                "org.varlink.service.MethodNotImplemented",
                json!({ "method": request.method }),
            )),
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
