//! tetsu-comp: the Tetsu compositor.
//!
//! Serves `org.tetsu.comp.{window,workspace,output,session,config}.v1`
//! through the `tetsu-bus` facade, behind the capability gate (`output` reads
//! and `ConfigureOutputs`; `config` is the three-layer config), `zwlr_layer_shell_v1`,
//! `ext-session-lock-v1` and `ext-idle-notify-v1` (`idle`: after the idle
//! timeout the locker starts). Windows get server-side decorations (`decor`): the
//! mode is always server-side and the compositor draws a border, never on a
//! locked output. Trust (`trust`) is decided when a client connects:
//! only clients the compositor starts see the virtual input protocols or may
//! take over a dead lock (`lock`). X11 apps run through xwayland-satellite,
//! started on the first X11 connection (`xwayland`).

pub mod backend;
pub mod bus;
pub mod cli;
pub mod config;
pub mod decor;
pub mod engine;
pub mod gpu;
pub mod grab;
pub mod idle;
pub mod input;
pub mod layout;
pub mod lock;
pub mod model;
pub mod outputs;
pub mod render;
pub mod rules;
pub mod run;
pub mod settings;
pub mod socket;
pub mod state;
pub mod trust;
pub mod xwayland;

mod handlers;
