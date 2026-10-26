//! `org.tetsu.tx.v1` served by `tetsu-tx serve`.

use std::io;
use std::rc::Rc;
use std::sync::Arc;

use tetsu_bus::server::{self, Listener};

use self::backend::Backend;
use self::policy::Policy;
use self::service::TxService;

pub mod backend;
pub mod convert;
pub mod hub;
pub mod policy;
pub mod service;

/// Serves the interface on `listener` until it fails. Must run inside a
/// `tokio::task::LocalSet`, on a runtime that can spawn blocking tasks.
pub async fn serve<B: Backend, P: Policy>(
    listener: Listener,
    backend: Arc<B>,
    policy: Rc<P>,
) -> io::Result<()> {
    server::serve(listener, Rc::new(TxService::new(backend, policy))).await
}
