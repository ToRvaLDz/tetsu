//! `org.tetsu.secrets.v1` on the facade server.
//!
//! Answers are metadata only. A rejected parameter is named, never echoed,
//! so a `@sensitive` input cannot come back in an error. Writes from the
//! agent unit are `PermissionDenied`, whatever they carry.

use std::rc::Rc;

use tetsu_bus::peer::Peer;
use tetsu_bus::server::{Handler, Request, Response};

use crate::broker::Broker;

/// Serves the interface over a [`Broker`].
pub struct SecretsService {
    broker: Rc<Broker>,
}

impl SecretsService {
    /// A service answering from `broker`.
    pub fn new(broker: Rc<Broker>) -> Self {
        Self { broker }
    }
}

impl Handler for SecretsService {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        let _ = &self.broker;
        Response::method_not_found(&request.method)
    }
}
