//! The daemon task: owns the state machine, takes commands from the bus
//! adapters and the screen, and carries out its effects.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::json;
use tetsu_bus::notify::{PendingEvent, ShowParams};
use tetsu_bus::server::StreamItem;
use tokio::sync::{mpsc, oneshot};

use crate::core::{Caller, Core, Effect, FdoShow, NoteId, ShowError, View};

/// How often time is fed to the state machine (expiry, collapse, arming).
const TICK: Duration = Duration::from_millis(100);

/// What the daemon is asked to do.
#[derive(Debug)]
pub enum Command {
    /// Varlink `Show`; `stream` is absent for a oneway call.
    ShowTetsu {
        /// The caller, from its credentials.
        caller: Caller,
        /// The call parameters.
        params: ShowParams,
        /// Called oneway.
        oneway: bool,
        /// Where the replies go.
        stream: Option<mpsc::UnboundedSender<StreamItem>>,
        /// The new note, or why not.
        done: oneshot::Sender<Result<NoteId, ShowError>>,
    },
    /// Varlink `Close`.
    CloseTetsu {
        /// The calling process.
        pid: i32,
        /// The id from `shown`.
        id: String,
        /// Done, or not found.
        done: oneshot::Sender<Result<(), ShowError>>,
    },
    /// The caller of a streaming `Show` disconnected.
    CallerGone(NoteId),
    /// Varlink `GetPending`.
    GetPending(oneshot::Sender<i64>),
    /// Varlink `WatchPending`.
    WatchPending(mpsc::UnboundedSender<StreamItem>),
    /// Varlink `OpenPanel`.
    OpenPanel,
    /// freedesktop `Notify`.
    ShowFdo {
        /// The decoded call.
        show: FdoShow,
        /// The id returned to the client.
        done: oneshot::Sender<u32>,
    },
    /// freedesktop `CloseNotification`.
    CloseFdo(u32),
    /// The user clicked an action.
    Click {
        /// The note.
        note: NoteId,
        /// The action id.
        action: String,
    },
    /// The user dismissed a note.
    Dismiss(NoteId),
    /// The panel became visible.
    PanelOpened,
    /// The session lock state.
    Lock(bool),
}

/// What the screen is told.
#[derive(Debug, Clone, PartialEq)]
pub enum UiMsg {
    /// Draw this.
    View(View),
    /// Open the panel.
    OpenPanel,
}

/// The daemon task.
#[derive(Debug)]
pub struct Daemon {
    core: Core,
    commands: mpsc::UnboundedReceiver<Command>,
    ui: mpsc::UnboundedSender<UiMsg>,
    fdo_signals: Option<mpsc::UnboundedSender<Effect>>,
    streams: HashMap<NoteId, mpsc::UnboundedSender<StreamItem>>,
    watchers: Vec<mpsc::UnboundedSender<StreamItem>>,
    drawn: Option<View>,
}

impl Daemon {
    /// A daemon drawing through `ui`, and the sender for its commands. It
    /// starts locked: nothing is answerable until a lock source says
    /// otherwise.
    pub fn new(ui: mpsc::UnboundedSender<UiMsg>) -> (Self, mpsc::UnboundedSender<Command>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut core = Core::new();
        core.set_locked(true, Instant::now());
        let daemon = Self {
            core,
            commands: rx,
            ui,
            fdo_signals: None,
            streams: HashMap::new(),
            watchers: Vec::new(),
            drawn: None,
        };
        (daemon, tx)
    }

    /// Sends freedesktop `ActionInvoked` and `NotificationClosed` effects to
    /// `signals`.
    pub fn with_fdo_signals(mut self, signals: mpsc::UnboundedSender<Effect>) -> Self {
        self.fdo_signals = Some(signals);
        self
    }

    /// Runs until every command sender is gone.
    pub async fn run(mut self) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.handle(command, Instant::now()),
                    None => return,
                },
                _ = tick.tick() => {
                    let effects = self.core.tick(Instant::now());
                    self.apply(effects);
                }
            }
            self.draw();
        }
    }

    fn handle(&mut self, command: Command, now: Instant) {
        match command {
            Command::ShowTetsu {
                caller,
                params,
                oneway,
                stream,
                done,
            } => match self.core.show_tetsu(&caller, params, oneway, now) {
                Ok((note, effects)) => {
                    if let Some(stream) = stream {
                        self.streams.insert(note, stream);
                    }
                    self.apply(effects);
                    let _ = done.send(Ok(note));
                }
                Err(e) => {
                    let _ = done.send(Err(e));
                }
            },
            Command::CloseTetsu { pid, id, done } => {
                let result = self.core.close_tetsu(pid, &id, now).map(|e| self.apply(e));
                let _ = done.send(result);
            }
            Command::CallerGone(note) => {
                self.streams.remove(&note);
                let effects = self.core.caller_gone(note, now);
                self.apply(effects);
            }
            Command::GetPending(done) => {
                let _ = done.send(self.core.pending_count());
            }
            Command::WatchPending(stream) => {
                let initial = PendingEvent::initial(self.core.pending_count());
                if stream
                    .send(StreamItem::Reply(json!({ "event": initial })))
                    .is_ok()
                {
                    self.watchers.push(stream);
                }
            }
            Command::OpenPanel => {
                let _ = self.ui.send(UiMsg::OpenPanel);
            }
            Command::ShowFdo { show, done } => {
                let (id, effects) = self.core.show_fdo(show, now);
                self.apply(effects);
                let _ = done.send(id);
            }
            Command::CloseFdo(id) => {
                let effects = self.core.close_fdo(id, now);
                self.apply(effects);
            }
            Command::Click { note, action } => {
                // A refused click (locked, not armed, gone) does nothing.
                if let Ok(effects) = self.core.click(note, &action, now) {
                    self.apply(effects);
                }
            }
            Command::Dismiss(note) => {
                if let Ok(effects) = self.core.dismiss(note, now) {
                    self.apply(effects);
                }
            }
            Command::PanelOpened => self.core.panel_opened(now),
            Command::Lock(locked) => {
                let effects = self.core.set_locked(locked, now);
                self.apply(effects);
            }
        }
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Reply { note, reply, last } => {
                    let message = json!({ "reply": reply });
                    let item = if last {
                        StreamItem::Last(message)
                    } else {
                        StreamItem::Reply(message)
                    };
                    if let Some(stream) = self.streams.get(&note) {
                        let _ = stream.send(item);
                    }
                    if last {
                        self.streams.remove(&note);
                    }
                }
                Effect::PendingChanged(count) => {
                    let message = json!({ "event": PendingEvent::changed(count) });
                    self.watchers
                        .retain(|w| w.send(StreamItem::Reply(message.clone())).is_ok());
                }
                fdo @ (Effect::FdoActionInvoked { .. } | Effect::FdoClosed { .. }) => {
                    if let Some(signals) = &self.fdo_signals {
                        let _ = signals.send(fdo);
                    }
                }
            }
        }
    }

    /// Tells the screen about a changed view.
    fn draw(&mut self) {
        let view = self.core.view(Instant::now());
        if self.drawn.as_ref() != Some(&view) {
            let _ = self.ui.send(UiMsg::View(view.clone()));
            self.drawn = Some(view);
        }
    }
}
