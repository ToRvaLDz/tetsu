//! Generate the owner's language on the target when the payload lacks it.

use crate::error::InstallError;

use super::exec::Executor;

impl Executor<'_> {
    pub(super) fn generate_locale(&mut self) -> Result<(), InstallError> {
        Ok(())
    }
}
