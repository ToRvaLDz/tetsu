//! tetsu-bar: a status bar on a layer surface (#60).
//!
//! It shows the workspaces of the compositor (from `WatchWorkspaces`), the
//! clock, the network and the battery. Drawing is software (#12 stack: SCTK,
//! tiny-skia, cosmic-text) and happens only when the shown state changes: on
//! a bus event, on the minute tick, or when a sampled source changes.
//!
//! Limitations, both deliberate for now:
//!
//! - Every output's bar lists the workspaces of every output. `output_id` is
//!   opaque and `output.v1` is not served yet, so a `wl_output` cannot be
//!   matched to the bus. Per-output filtering follows `output.v1`.
//! - Battery and network are polled every few seconds, so a cable or Wi-Fi
//!   change shows up late. Polling wakes the process without drawing.

pub mod bus;
pub mod clock;
pub mod model;
pub mod sources;
pub mod ui;
