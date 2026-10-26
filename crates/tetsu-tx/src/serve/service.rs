//! The handler of `org.tetsu.tx.v1`: parses the calls, asks the policy,
//! runs a transaction on a blocking thread and fans its progress out to the
//! caller and to the watchers.

use std::fmt::Write as _;
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{ErrorReply, Handler, Request, Response, StreamItem};
use tetsu_bus::tx as wire;
use tokio::sync::mpsc::{self, UnboundedSender};

use super::backend::Backend;
use super::convert;
use super::hub::Hub;
use super::policy::Policy;
use crate::app::now_secs;
use crate::tx::TxError;
use crate::tx::engine::{EngineEvent, EnginePhase, Request as TxRequest};

/// Random bytes in a transaction id.
const ID_BYTES: usize = 16;

/// A write call, as asked.
enum Ask {
    Update,
    Install(Vec<String>),
    Remove(Vec<String>),
    /// The snapshot id as the caller wrote it.
    Rollback(Option<String>),
}

impl Ask {
    /// The capability the IDL tags the method with.
    fn class(&self) -> &'static str {
        match self {
            Self::Update => "system.update",
            Self::Install(_) => "packages.install",
            Self::Remove(_) => "packages.remove",
            Self::Rollback(_) => "system.rollback",
        }
    }

    /// The engine's request; a rollback id that names no snapshot is a
    /// `SnapshotNotFound` here, before anything runs.
    fn request(self) -> Result<TxRequest, ErrorReply> {
        Ok(match self {
            Self::Update => TxRequest::Update,
            Self::Install(packages) => TxRequest::Install(packages),
            Self::Remove(packages) => TxRequest::Remove(packages),
            Self::Rollback(None) => TxRequest::Rollback(None),
            Self::Rollback(Some(id)) => TxRequest::Rollback(Some(convert::parse_snapshot_id(&id)?)),
        })
    }
}

/// A call's parameters; absent ones are an empty object. `bad` names the
/// parameter to blame when they do not parse.
fn parse<T: DeserializeOwned>(parameters: &Value, bad: &str) -> Result<T, ErrorReply> {
    let value = if parameters.is_null() {
        json!({})
    } else {
        parameters.clone()
    };
    serde_json::from_value(value).map_err(|_| ErrorReply::invalid_parameter(bad))
}

fn packages(parameters: &Value) -> Result<Vec<String>, ErrorReply> {
    let wire::PackagesParams { packages } = parse(parameters, "packages")?;
    if packages.is_empty() {
        return Err(ErrorReply::invalid_parameter("packages"));
    }
    Ok(packages)
}

fn parse_ask(method: &str, parameters: &Value) -> Result<Ask, ErrorReply> {
    match method {
        "org.tetsu.tx.v1.Update" => Ok(Ask::Update),
        "org.tetsu.tx.v1.Install" => packages(parameters).map(Ask::Install),
        "org.tetsu.tx.v1.Remove" => packages(parameters).map(Ask::Remove),
        _ => {
            let wire::RollbackParams { to } = parse(parameters, "to")?;
            Ok(Ask::Rollback(to))
        }
    }
}

fn unavailable() -> Response {
    Response::Error(ErrorReply::new(
        "org.varlink.service.ServiceUnavailable",
        json!({}),
    ))
}

/// A read that failed: logged, and the caller is told the service cannot
/// answer.
fn read_failed(what: &str, why: &str) -> Response {
    eprintln!("tetsu-tx: {what} failed: {why}");
    unavailable()
}

/// `tx-` and 16 random bytes in hex.
fn new_id() -> io::Result<String> {
    let mut bytes = [0u8; ID_BYTES];
    let mut filled = 0;
    while filled < bytes.len() {
        filled +=
            rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::empty())?;
    }
    let mut id = String::from("tx-");
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    Ok(id)
}

fn phase_of(phase: EnginePhase) -> wire::Phase {
    match phase {
        EnginePhase::Packages => wire::Phase::Packages,
        EnginePhase::BootEntry => wire::Phase::BootEntry,
        EnginePhase::Commit => wire::Phase::Commit,
    }
}

/// Serves `org.tetsu.tx.v1` and the `org.varlink.service` basics.
pub struct TxService<B, P> {
    backend: Arc<B>,
    policy: Rc<P>,
    hub: Rc<Hub>,
}

impl<B: Backend, P: Policy> TxService<B, P> {
    /// A service over `backend`, with `policy` deciding the write calls.
    pub fn new(backend: Arc<B>, policy: Rc<P>) -> Self {
        Self {
            backend,
            policy,
            hub: Rc::new(Hub::new()),
        }
    }

    /// Runs a blocking read of the backend off the runtime thread.
    async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&B) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || read(&backend))
            .await
            .unwrap_or_else(|e| Err(format!("the read thread failed: {e}")))
    }

    async fn write(&self, peer: &Peer, request: Request) -> Response {
        if !request.more {
            return Response::Error(ErrorReply::new(
                "org.varlink.service.ExpectedMore",
                json!({}),
            ));
        }
        let ask = match parse_ask(&request.method, &request.parameters) {
            Ok(ask) => ask,
            Err(e) => return Response::Error(e),
        };
        if !self.policy.allows(peer, ask.class()).await {
            return Response::Error(ErrorReply::new(wire::PERMISSION_DENIED, json!({})));
        }
        let asked = match &ask {
            Ask::Rollback(to) => to.clone(),
            _ => None,
        };
        let tx_request = match ask.request() {
            Ok(request) => request,
            Err(e) => return Response::Error(e),
        };
        let Ok(id) = new_id() else {
            return unavailable();
        };
        // No await between taking the slot and spawning its driver.
        if let Err(running) = self.hub.reserve(&id) {
            return Response::Error(ErrorReply::new(
                wire::TRANSACTION_IN_PROGRESS,
                json!({ "id": running }),
            ));
        }
        let (caller, stream) = mpsc::unbounded_channel();
        let job = Job {
            id,
            request: tx_request,
            asked,
            caller,
        };
        // Not tied to the connection: the transaction runs to the end
        // whoever started it.
        tokio::task::spawn_local(drive(self.backend.clone(), self.hub.clone(), job));
        Response::Stream(stream)
    }

    async fn list(&self, parameters: &Value) -> Response {
        let wire::ListParams { limit } = match parse(parameters, "limit") {
            Ok(params) => params,
            Err(e) => return Response::Error(e),
        };
        let limit = match limit.map(usize::try_from) {
            Some(Ok(n)) => Some(n),
            Some(Err(_)) => return Response::Error(ErrorReply::invalid_parameter("limit")),
            None => None,
        };
        // Snapshot the hub first: a transaction that commits after the scan
        // and leaves the hub before the read returns is then still listed.
        let known = self.hub.known();
        let records = match self.read(|b| b.transactions()).await {
            Ok(records) => records,
            Err(why) => return read_failed("listing the transactions", &why),
        };
        let committed: Vec<wire::Transaction> =
            records.iter().filter_map(convert::from_record).collect();
        // One entry per id: a record wins over the hub's copy.
        let mut all: Vec<wire::Transaction> = known
            .into_iter()
            .filter(|t| !committed.iter().any(|c| c.id == t.id))
            .collect();
        all.extend(committed);
        // Stable: transactions of the same second keep their order.
        all.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        all.truncate(limit.unwrap_or(usize::MAX));
        Response::Reply(json!({ "transactions": all }))
    }

    async fn user_packages(&self) -> Response {
        match self.read(|b| b.user_packages()).await {
            Ok(names) => {
                let packages: Vec<wire::UserPackage> = names
                    .into_iter()
                    .map(|name| wire::UserPackage {
                        name,
                        source: wire::PackageSource::Repo,
                        aur_commit: None,
                    })
                    .collect();
                Response::Reply(json!({ "packages": packages }))
            }
            Err(why) => read_failed("reading the user layer", &why),
        }
    }

    async fn status(&self) -> Response {
        match self.read(|b| b.status()).await {
            Ok(view) => Response::Reply(json!(wire::Status {
                booted: view.booted.to_string(),
                default: view.default.to_string(),
                default_boot: convert::default_boot(view.default_boot),
            })),
            Err(why) => read_failed("reading the status", &why),
        }
    }

    fn watch(&self) -> Response {
        let (watcher, stream) = mpsc::unbounded_channel();
        self.hub.watch(watcher);
        Response::Stream(stream)
    }
}

/// One transaction, from the call to its end.
struct Job {
    id: String,
    request: TxRequest,
    /// The snapshot id the caller named, for `SnapshotNotFound`.
    asked: Option<String>,
    caller: UnboundedSender<StreamItem>,
}

/// Tells the watchers and the caller (if still there) the phase.
fn announce(hub: &Hub, job: &Job, phase: wire::Phase) {
    let progress = wire::Progress::phase(phase);
    hub.progress(&job.id, progress.clone());
    let reply = wire::TxReply::progress(progress);
    let _ = job.caller.send(StreamItem::Reply(json!(reply)));
}

/// Runs `job` on a blocking thread and relays what it reports until it
/// ends. Never stops early: a caller that left only stops receiving.
async fn drive<B: Backend>(backend: Arc<B>, hub: Rc<Hub>, job: Job) {
    let started = now_secs();
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let blocking = {
        let (backend, id, request) = (backend, job.id.clone(), job.request.clone());
        tokio::task::spawn_blocking(move || {
            let observer = move |event: EngineEvent| {
                let _ = events_tx.send(event);
            };
            backend.run(&id, &request, &observer)
        })
    };
    let mut begun: Option<wire::Transaction> = None;
    while let Some(event) = events.recv().await {
        match event {
            EngineEvent::Begun { .. } => {
                let transaction = convert::running(&job.id, &job.request, started);
                hub.started(transaction.clone());
                begun = Some(transaction);
                announce(&hub, &job, wire::Phase::Snapshot);
            }
            EngineEvent::Phase(phase) => announce(&hub, &job, phase_of(phase)),
        }
    }
    let result = blocking.await.unwrap_or_else(|e| {
        Err(TxError::failed(format!(
            "the transaction thread failed: {e}"
        )))
    });
    let ended = now_secs();
    match (result, begun) {
        (Ok(report), _) => {
            let transaction = convert::finished(&job.id, &report, started, ended);
            hub.finished(transaction.clone());
            let reply = wire::TxReply::finished(transaction);
            let _ = job.caller.send(StreamItem::Last(json!(reply)));
        }
        (Err(error), Some(running)) => {
            let failed = convert::failed(&running, ended, &convert::failure_text(&error));
            hub.finished(failed);
            let reply = convert::error_reply(&job.id, &error, job.asked.as_deref());
            let _ = job.caller.send(StreamItem::Error(reply));
        }
        (Err(error), None) => {
            // It never began: no transaction existed, nothing to tell.
            hub.release();
            let id = match error {
                // The lock is held by another process: no id to give.
                TxError::InProgress => "",
                _ => &job.id,
            };
            let reply = convert::error_reply(id, &error, job.asked.as_deref());
            let _ = job.caller.send(StreamItem::Error(reply));
        }
    }
}

impl<B: Backend, P: Policy> Handler for TxService<B, P> {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.tetsu.tx.v1.Update"
            | "org.tetsu.tx.v1.Install"
            | "org.tetsu.tx.v1.Remove"
            | "org.tetsu.tx.v1.Rollback" => self.write(&peer, request).await,
            "org.tetsu.tx.v1.ListTransactions" => self.list(&request.parameters).await,
            "org.tetsu.tx.v1.ListUserPackages" => self.user_packages().await,
            "org.tetsu.tx.v1.GetStatus" => self.status().await,
            "org.tetsu.tx.v1.WatchTransactions" => self.watch(),
            // #46 replaces these two arms.
            "org.tetsu.tx.v1.ReviewAurPackages" | "org.tetsu.tx.v1.InstallFromAur" => {
                Response::Error(ErrorReply::new(
                    "org.varlink.service.MethodNotImplemented",
                    json!({ "method": request.method }),
                ))
            }
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "Tetsu",
                "product": "tetsu-tx",
                "version": env!("CARGO_PKG_VERSION"),
                "url": env!("CARGO_PKG_REPOSITORY"),
                "interfaces": ["org.varlink.service", wire::INTERFACE],
            })),
            "org.varlink.service.GetInterfaceDescription" => {
                match request.parameters["interface"].as_str() {
                    Some(wire::INTERFACE) => Response::Reply(json!({ "description": wire::IDL })),
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
