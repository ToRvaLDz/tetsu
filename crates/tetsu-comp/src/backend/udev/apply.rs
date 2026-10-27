//! The tty backend applying output changes.

use crate::outputs::glue::{OutputChange, Refused};
use crate::state::State;

impl State {
    /// Takes the changes to the displays. Nothing to do without a tty.
    pub(crate) fn apply_tty(&mut self, _changes: &[OutputChange]) -> Result<(), Refused> {
        Ok(())
    }
}
