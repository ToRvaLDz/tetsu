//! tetsu-comp: the Tetsu compositor.
//!
//! Serves `org.tetsu.comp.{window,workspace,output,session}.v1` through the
//! `tetsu-bus` facade (output reads only for now), `zwlr_layer_shell_v1` and
//! `ext-session-lock-v1`. Trust (`trust`) is decided when a client connects:
//! only clients the compositor starts see the virtual input protocols or may
//! take over a dead lock (`lock`).

pub mod backend;
pub mod bus;
pub mod cli;
pub mod engine;
pub mod gpu;
pub mod grab;
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

mod handlers;
