//! The `tetsu-secrets` subcommands: `serve`, the broker the user unit runs,
//! and `init`, the first-login key creation.
//!
//! `serve` reads `HOME`, `XDG_RUNTIME_DIR` and `CREDENTIALS_DIRECTORY`; the
//! audit socket is the journal's unless `TETSU_SECRETS_JOURNAL_SOCKET`
//! says otherwise (the user manager sets the broker's environment; the
//! agent cannot).

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use rustix::process::{DumpableBehavior, set_dumpable_behavior};
use tetsu_bus::server;
use tetsu_tx::boot::tool::SystemRunner;
use tokio::runtime::Runtime;
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::LocalSet;

use crate::audit::{Action, AuditEntry, AuditSink, JOURNAL_SOCKET, JournalSink, Outcome};
use crate::broker::{Broker, NewSecret, SystemClock};
use crate::caller::Caller;
use crate::init::{Seal, init, system_random};
use crate::model::{Host, KindSpec, SecretId};
use crate::paths::{Paths, socket};
use crate::service::SecretsService;
use crate::staging::import_staged;
use crate::store::Store;
use crate::unlock::vault_key;
use crate::vault::{Secret, Vault, VaultError};

/// The environment `serve` runs in.
#[derive(Debug, Clone)]
pub struct Env {
    /// The user's home directory.
    pub home: PathBuf,
    /// `$XDG_RUNTIME_DIR`.
    pub runtime_dir: PathBuf,
    /// `$CREDENTIALS_DIRECTORY`, when the unit provides one.
    pub credentials_dir: Option<PathBuf>,
    /// The journal's native-protocol socket.
    pub journal_socket: PathBuf,
}

impl Env {
    /// Reads the process environment.
    pub fn from_process() -> Result<Self, String> {
        let need = |name: &str| {
            env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} is not set"))
        };
        Ok(Self {
            home: need("HOME")?,
            runtime_dir: need("XDG_RUNTIME_DIR")?,
            credentials_dir: env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from),
            journal_socket: env::var_os("TETSU_SECRETS_JOURNAL_SOCKET")
                .map_or_else(|| PathBuf::from(JOURNAL_SOCKET), PathBuf::from),
        })
    }
}

#[derive(Debug, thiserror::Error)]
enum DaemonError {
    #[error("cannot unlock the vault: {0}")]
    Unlock(String),
    #[error("the audit journal is not writable: {0}")]
    Journal(std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// What the installer stages: provider name to its upstream binding.
fn provider_binding(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "anthropic" => Some(("header:x-api-key", "api.anthropic.com")),
        "openai" => Some(("bearer", "api.openai.com")),
        "google" => Some(("header:x-goog-api-key", "generativelanguage.googleapis.com")),
        _ => None,
    }
}

/// The staged-key importer over the broker.
struct Importer<'a> {
    runtime: &'a Runtime,
    broker: &'a Broker,
    caller: Caller,
}

impl Vault for Importer<'_> {
    fn store_api_key(&mut self, provider: &str, key: &Secret) -> Result<(), VaultError> {
        let (kind, host) = provider_binding(provider)
            .ok_or_else(|| VaultError("no upstream host is known for this provider".into()))?;
        let bad = |e: crate::model::ModelError| VaultError(e.to_string());
        let new = NewSecret {
            id: SecretId::parse(provider).map_err(bad)?,
            kind: KindSpec::parse(kind).map_err(bad)?,
            host: Host::parse(host).map_err(bad)?,
            allowed_tools: Vec::new(),
            value: key.clone(),
        };
        self.runtime
            .block_on(self.broker.add(&self.caller, new))
            .map(|_| ())
            .map_err(|e| VaultError(e.to_string()))
    }
}

/// Runs the broker until SIGTERM or SIGINT.
pub fn serve(env: &Env) -> ExitCode {
    match run(env) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tetsu-secrets: {e}");
            ExitCode::FAILURE
        }
    }
}

fn unlock_entry(outcome: Outcome, reason: Option<String>) -> AuditEntry {
    let mut entry = AuditEntry::new(Action::Unlock, outcome, Caller::this_process());
    entry.reason = reason;
    entry
}

fn run(env: &Env) -> Result<(), DaemonError> {
    // Same-uid processes, the agent among them, cannot read this memory.
    set_dumpable_behavior(DumpableBehavior::NotDumpable)
        .map_err(|e| DaemonError::Other(format!("cannot clear the dumpable flag: {e}")))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| DaemonError::Other(e.to_string()))?;
    let sink: Rc<dyn AuditSink> = Rc::new(JournalSink::new(&env.journal_socket));
    let paths = Paths::new(&env.home);

    let unlocked = vault_key(env.credentials_dir.as_deref())
        .map_err(|e| e.to_string())
        .and_then(|key| {
            runtime
                .block_on(Store::open(&paths.vault(), &key))
                .map_err(|e| e.to_string())
        });
    let store = match unlocked {
        Ok(store) => store,
        Err(reason) => {
            // Best effort: the broker is exiting either way.
            let _ = sink.record(&unlock_entry(Outcome::Failed, Some(reason.clone())));
            return Err(DaemonError::Unlock(reason));
        }
    };
    sink.record(&unlock_entry(Outcome::Ok, None))
        .map_err(DaemonError::Journal)?;

    let broker = Rc::new(Broker::new(store, sink, Rc::new(SystemClock)));
    import_staged_key(env, &runtime, &broker);

    let _enter = runtime.enter();
    // Socket activation passes the socket; started by hand, bind the path.
    let listener = match server::activated() {
        Ok(Some(listener)) => listener,
        Ok(None) => server::bind(&socket(&env.runtime_dir))
            .map_err(|e| DaemonError::Other(format!("cannot bind the socket: {e}")))?,
        Err(e) => {
            return Err(DaemonError::Other(format!(
                "cannot take the socket from systemd: {e}"
            )));
        }
    };
    let mut term =
        signal(SignalKind::terminate()).map_err(|e| DaemonError::Other(e.to_string()))?;
    let mut int = signal(SignalKind::interrupt()).map_err(|e| DaemonError::Other(e.to_string()))?;
    let service = Rc::new(SecretsService::new(broker));
    LocalSet::new().block_on(&runtime, async move {
        tokio::select! {
            served = server::serve(listener, service) => {
                served.map_err(|e| DaemonError::Other(e.to_string()))
            }
            _ = term.recv() => Ok(()),
            _ = int.recv() => Ok(()),
        }
    })
}

/// Imports the key the installer staged, if any. A failure is reported and
/// does not stop the broker: the staged file stays for another try.
fn import_staged_key(env: &Env, runtime: &Runtime, broker: &Broker) {
    let mut importer = Importer {
        runtime,
        broker,
        caller: Caller::this_process(),
    };
    match import_staged(&env.home, &mut importer) {
        Ok(Some(imported)) => eprintln!(
            "tetsu-secrets: imported the staged {} key",
            imported.provider
        ),
        Ok(None) => {}
        Err(e) => eprintln!("tetsu-secrets: staged key not imported: {e}"),
    }
}

/// `tetsu-secrets init [--seal auto|host]`.
pub fn init_command(args: &[String]) -> ExitCode {
    let seal = match args {
        [] => Seal::Auto,
        [flag, value] if flag == "--seal" => match value.as_str() {
            "auto" => Seal::Auto,
            "host" => Seal::Host,
            _ => return usage_error("--seal takes auto or host"),
        },
        _ => return usage_error("usage: tetsu-secrets init [--seal auto|host]"),
    };
    let Some(home) = env::var_os("HOME").filter(|h| !h.is_empty()) else {
        return usage_error("HOME is not set");
    };
    let paths = Paths::new(&PathBuf::from(home));
    match init(&paths, seal, &SystemRunner::new(), &mut system_random) {
        Ok(path) => {
            println!("sealed a new vault key in {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("tetsu-secrets: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("tetsu-secrets: {message}");
    ExitCode::from(2)
}
