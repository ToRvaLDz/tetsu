//! The bus thread: a current-thread tokio runtime that serves the window
//! and workspace interfaces. The calloop loop never waits on it: it hands
//! over deltas through an unbounded channel, and a supervisor restarts the
//! thread when it dies.

mod hub;
mod service;

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tetsu_bus::server;
use tokio::sync::mpsc;
use tokio::task::LocalSet;

pub use hub::{Hub, Mirror, Snapshot, WATCH_QUEUE};
pub use service::BusService;

use crate::model::Events;

/// Least time between two restarts of the bus thread.
const RESTART_INTERVAL: Duration = Duration::from_secs(1);

enum Message {
    Events(Events),
    Crash,
}

struct Live {
    tx: mpsc::UnboundedSender<Message>,
    thread: JoinHandle<()>,
}

/// The loop's handle on the bus thread.
pub struct Publisher {
    path: PathBuf,
    live: Option<Live>,
    last_attempt: Instant,
}

impl Publisher {
    /// Starts the bus thread, bound at `path`, serving `reset` until the
    /// first delta. Returns the bind result: a socket someone else serves,
    /// or a directory that is not ours, is an error here, and only here.
    pub fn spawn(path: &Path, reset: Snapshot) -> io::Result<Self> {
        let live = start(path, reset)?;
        Ok(Self {
            path: path.to_owned(),
            live: Some(live),
            last_attempt: Instant::now(),
        })
    }

    /// Hands deltas to the bus thread. Never blocks.
    pub fn publish(&self, events: Events) {
        if events.is_empty() {
            return;
        }
        if let Some(live) = &self.live {
            // A dead thread is noticed by `supervise`.
            let _ = live.tx.send(Message::Events(events));
        }
    }

    /// Restarts a dead bus thread with a fresh snapshot, at most once a
    /// second. Call it on the publish tick.
    pub fn supervise(&mut self, snapshot: impl FnOnce() -> Snapshot) {
        let dead = self.live.as_ref().is_none_or(|l| l.thread.is_finished());
        if !dead || self.last_attempt.elapsed() < RESTART_INTERVAL {
            return;
        }
        self.last_attempt = Instant::now();
        if let Some(old) = self.live.take()
            && old.thread.join().is_err()
        {
            eprintln!("tetsu-comp: the bus thread died; restarting it");
        }
        match start(&self.path, snapshot()) {
            Ok(live) => self.live = Some(live),
            Err(e) => eprintln!("tetsu-comp: cannot restart the bus thread: {e}"),
        }
    }

    /// Kills the bus thread, to test its supervision.
    #[doc(hidden)]
    pub fn crash_for_test(&self) {
        if let Some(live) = &self.live {
            let _ = live.tx.send(Message::Crash);
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        if let Some(live) = self.live.take() {
            drop(live.tx);
            let _ = live.thread.join();
        }
    }
}

fn start(path: &Path, reset: Snapshot) -> io::Result<Live> {
    let (tx, rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = std_mpsc::channel();
    let path = path.to_owned();
    let thread = std::thread::Builder::new()
        .name("tetsu-comp-bus".to_owned())
        .spawn(move || run(&path, &reset, rx, &ready_tx))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Live { tx, thread }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(io::Error::other("the bus thread died while starting"))
        }
    }
}

fn run(
    path: &Path,
    reset: &Snapshot,
    mut rx: mpsc::UnboundedReceiver<Message>,
    ready: &std_mpsc::Sender<io::Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    LocalSet::new().block_on(&runtime, async {
        let listener = match server::bind(path) {
            Ok(listener) => listener,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let _ = ready.send(Ok(()));
        let hub = Rc::new(RefCell::new(Hub::new(reset)));
        let service = Rc::new(BusService::new(hub.clone()));
        let mut serving = tokio::task::spawn_local(server::serve(listener, service));
        loop {
            tokio::select! {
                message = rx.recv() => match message {
                    Some(Message::Events(events)) => hub.borrow_mut().publish(&events),
                    Some(Message::Crash) => panic!("the bus thread was asked to crash"),
                    // The compositor is gone.
                    None => return,
                },
                result = &mut serving => {
                    eprintln!("tetsu-comp: the bus server stopped: {result:?}");
                    return;
                }
            }
        }
    });
}
