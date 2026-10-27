//! `ConfigureOutputs` and the apply path on the loop.

use crate::bus::ConfigureOutputs;
use crate::state::State;

impl State {
    /// Runs a `ConfigureOutputs` from the bus.
    pub fn handle_output_command(&mut self, command: ConfigureOutputs) {
        let _ = command.reply.send(Err(tetsu_bus::server::ErrorReply::new(
            "org.varlink.service.MethodNotImplemented",
            serde_json::json!({ "method": "org.tetsu.comp.output.v1.ConfigureOutputs" }),
        )));
    }
}
