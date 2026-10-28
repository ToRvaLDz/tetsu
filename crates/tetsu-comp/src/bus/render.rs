//! The render side of the bus thread (#107): the `SetRenderDevice` and
//! `SetExplicitSync` commands that go to the calloop loop, and the mirror
//! of `render.v1` the reads serve.

use tetsu_bus::render::{GetRenderReply, Render, RenderEvent, RenderKind};
use tetsu_bus::server::ErrorReply;
use tokio::sync::oneshot;

use crate::settings::SyncMode;

/// The loop's answer to a Set call: whether the change waits for unlock.
pub type RenderAnswer = oneshot::Sender<Result<bool, ErrorReply>>;

/// A `SetRenderDevice` call, for the loop to run.
#[derive(Debug)]
pub struct SetRenderDevice {
    /// The device id; `None` returns to the automatic choice.
    pub target: Option<String>,
    /// Where the loop answers. Dropped without an answer: the compositor
    /// is going away.
    pub reply: RenderAnswer,
}

/// A `SetExplicitSync` call, for the loop to run.
#[derive(Debug)]
pub struct SetExplicitSync {
    /// The requested mode.
    pub mode: SyncMode,
    /// Where the loop answers, as for `SetRenderDevice`.
    pub reply: RenderAnswer,
}

/// The render state as the events built it.
#[derive(Debug, Clone, Default)]
pub struct RenderMirror {
    state: Render,
}

impl RenderMirror {
    /// Starts from a snapshot.
    pub fn new(state: Render) -> Self {
        Self { state }
    }

    /// For `GetRender`.
    pub fn reply(&self) -> GetRenderReply {
        GetRenderReply {
            render: self.state.clone(),
        }
    }

    /// The first event of a `WatchRender`.
    pub fn initial(&self) -> RenderEvent {
        RenderEvent::initial(self.state.clone())
    }

    /// Applies one event; `pending_dropped` carries no state.
    pub fn apply(&mut self, event: &RenderEvent) {
        if matches!(event.kind, RenderKind::Initial | RenderKind::Changed)
            && let Some(render) = &event.render
        {
            self.state = render.clone();
        }
    }
}
