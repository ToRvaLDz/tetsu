//! Smithay's protocol handlers on `State`.

mod compositor;
mod shell_misc;
mod xdg_shell;

use crate::state::State;

smithay::delegate_dispatch2!(State);
