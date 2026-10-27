//! The real notification service: `org.tetsu.notify.v1` at its fixed path.

use std::io;
use std::path::{Path, PathBuf};

use tetsu_bus::notify::{self, NotifyReply, ShowParams, ShowStream};

use crate::ask::{NotifyChannel, ShowEvents};

/// The notification service of this session.
#[derive(Debug, Clone)]
pub struct BusNotify {
    path: PathBuf,
}

impl BusNotify {
    /// A client for the server at `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The fixed path under the runtime directory.
    pub fn in_runtime_dir(runtime: &Path) -> Self {
        Self::new(runtime.join("tetsu").join(notify::INTERFACE))
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
        notify::show(&self.path, &params).await
    }

    async fn notice(&self, params: ShowParams) -> io::Result<()> {
        notify::show_oneway(&self.path, &params).await
    }
}
