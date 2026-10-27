//! The real notification service: `org.tetsu.notify.v1` at its fixed path.

use std::io;
use std::path::Path;

use tetsu_bus::endpoint::{Endpoint, ExpectedServer};
use tetsu_bus::notify::{self, NotifyReply, ShowParams, ShowStream};

use crate::ask::{NotifyChannel, ShowEvents};

/// The unit that must serve the notifications of the session.
const NOTIFYD_UNIT: &str = "tetsu-notifyd.service";

/// The notification service of this session. A server that is not the
/// expected one gets nothing: the show fails, as if the service were down.
#[derive(Debug, Clone)]
pub struct BusNotify {
    endpoint: Endpoint,
}

impl BusNotify {
    /// A client for the server behind `endpoint`.
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// The fixed path under the runtime directory, expecting
    /// `tetsu-notifyd.service` under the manager of this user.
    pub fn in_runtime_dir(runtime: &Path) -> Self {
        let expected = ExpectedServer::UserUnit {
            uid: rustix::process::geteuid().as_raw(),
            unit: NOTIFYD_UNIT.to_owned(),
        };
        let path = runtime.join("tetsu").join(notify::INTERFACE);
        Self::new(Endpoint::new(path, expected))
    }
}

impl ShowEvents for ShowStream {
    async fn next(&mut self) -> io::Result<NotifyReply> {
        ShowStream::next(self).await
    }
}

impl NotifyChannel for BusNotify {
    type Events = ShowStream;

    async fn show(&self, params: ShowParams) -> io::Result<ShowStream> {
        notify::show_at(&self.endpoint, &params).await
    }

    async fn notice(&self, params: ShowParams) -> io::Result<()> {
        notify::show_oneway_at(&self.endpoint, &params).await
    }
}
