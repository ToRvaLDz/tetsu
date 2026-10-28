//! What the bus thread hands to the calloop loop, which owns the layers
//! and the outputs: one channel for every mutating call.

use std::collections::BTreeMap;
use std::sync::Arc;

use tetsu_bus::output::OutputConfig;
use tetsu_bus::server::ErrorReply;
use tokio::sync::oneshot;

use super::config::SetManaged;
use super::render::{SetExplicitSync, SetRenderDevice};

/// A `ConfigureOutputs` call, for the loop to run.
#[derive(Debug)]
pub struct ConfigureOutputs {
    /// Output id to the configuration for that output.
    pub configs: BTreeMap<String, OutputConfig>,
    /// Where the loop answers. Dropped without an answer: the compositor
    /// is going away.
    pub reply: oneshot::Sender<Result<(), ErrorReply>>,
}

/// A call for the loop.
#[derive(Debug)]
pub enum Command {
    /// `config.v1.SetManaged`.
    SetManaged(SetManaged),
    /// `output.v1.ConfigureOutputs`.
    ConfigureOutputs(ConfigureOutputs),
    /// `render.v1.SetRenderDevice`.
    SetRenderDevice(SetRenderDevice),
    /// `render.v1.SetExplicitSync`.
    SetExplicitSync(SetExplicitSync),
}

/// Hands a command to the loop; gives it back when the loop is gone.
/// Callable from the bus thread.
pub type CommandSink = Arc<dyn Fn(Command) -> Result<(), Command> + Send + Sync>;
