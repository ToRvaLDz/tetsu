//! The Tetsu installer: a text frontend, the answer file and the glue
//! around `tetsu-installer-core`.

pub mod answer_file;
pub mod app;
pub mod cleanup;
pub mod cli;
pub mod collect;
pub mod install;
pub mod isolated;
pub mod locate;
pub mod probe;
pub mod system;
pub mod text;
pub mod tty;
