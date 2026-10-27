//! A broker over a real vault file in a tempdir, a recording audit sink and
//! a fixed clock, served on a socket.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use tetsu_bus::server;
use tetsu_secrets::audit::{AuditEntry, AuditSink};
use tetsu_secrets::broker::{Broker, Clock};
use tetsu_secrets::service::SecretsService;
use tetsu_secrets::store::Store;
use tetsu_secrets::vault::Secret;
use tokio::task::LocalSet;

pub const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// The value every test hides; nothing a daemon says may contain it.
pub const SENTINEL: &str = "S3NT1NEL-SENTINEL";

#[derive(Default)]
pub struct Recording {
    pub entries: RefCell<Vec<AuditEntry>>,
    pub fail: Cell<bool>,
}

impl AuditSink for Recording {
    fn record(&self, entry: &AuditEntry) -> io::Result<()> {
        if self.fail.get() {
            return Err(io::Error::other("journal down"));
        }
        self.entries.borrow_mut().push(entry.clone());
        Ok(())
    }
}

pub struct Fixed(pub Cell<u64>);

impl Clock for Fixed {
    fn now_secs(&self) -> u64 {
        self.0.get()
    }
}

pub struct Rig {
    pub broker: Rc<Broker>,
    pub sink: Rc<Recording>,
    pub clock: Rc<Fixed>,
    pub dir: tempfile::TempDir,
}

pub async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("vault/vault.keyring"), &Secret::new(KEY))
        .await
        .unwrap();
    let sink = Rc::new(Recording::default());
    let clock = Rc::new(Fixed(Cell::new(1_700_000_000)));
    let broker = Rc::new(Broker::new(store, sink.clone(), clock.clone()));
    Rig {
        broker,
        sink,
        clock,
        dir,
    }
}

/// Runs `test` against a service bound in a fresh directory.
pub fn with_server<F, Fut>(test: F)
where
    F: FnOnce(PathBuf, Rig) -> Fut,
    Fut: Future<Output = ()>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let rig = rig().await;
        let path = rig.dir.path().join("run/tetsu/org.tetsu.secrets.v1");
        let listener = server::bind(&path).unwrap();
        let service = Rc::new(SecretsService::new(rig.broker.clone()));
        let server = tokio::task::spawn_local(server::serve(listener, service));
        test(path, rig).await;
        server.abort();
    });
}
