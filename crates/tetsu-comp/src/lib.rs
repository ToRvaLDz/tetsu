//! tetsu-comp: the Tetsu compositor.
//!
//! Serves `org.tetsu.comp.{window,workspace,output}.v1` through the
//! `tetsu-bus` facade (output reads only for now) and `zwlr_layer_shell_v1`.

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
