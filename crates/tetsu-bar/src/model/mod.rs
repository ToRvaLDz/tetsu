//! What the bar shows, and the one rule for when to draw it again.

pub mod workspaces;

use crate::sources::battery::Battery;
use crate::sources::network::Network;
use workspaces::WsCell;

/// Everything the bar draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    /// `HH:MM`.
    pub clock: String,
    /// `None` hides the segment.
    pub battery: Option<Battery>,
    /// The best link.
    pub network: Network,
    /// `None` hides the segment: no compositor, or it is not answering.
    pub workspaces: Option<Vec<WsCell>>,
}

/// One input to the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The clock label now.
    Tick(String),
    /// A battery sample.
    Battery(Option<Battery>),
    /// A network sample.
    Network(Network),
    /// The workspace strip, `None` when the compositor is not answering.
    Workspaces(Option<Vec<WsCell>>),
}

impl Bar {
    /// The bar after `change`, or `None` when nothing visible changed: the
    /// caller draws only on `Some`.
    pub fn apply(&self, change: Change) -> Option<Bar> {
        let next = match change {
            Change::Tick(clock) => Bar {
                clock,
                ..self.clone()
            },
            Change::Battery(battery) => Bar {
                battery,
                ..self.clone()
            },
            Change::Network(network) => Bar {
                network,
                ..self.clone()
            },
            Change::Workspaces(workspaces) => Bar {
                workspaces,
                ..self.clone()
            },
        };
        (next != *self).then_some(next)
    }
}
