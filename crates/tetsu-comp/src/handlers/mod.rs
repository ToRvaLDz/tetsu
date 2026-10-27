//! Smithay's protocol handlers on `State`.

mod compositor;
mod dmabuf;
mod shell_misc;
mod syncobj;
mod xdg_shell;

use crate::state::State;

smithay::delegate_dispatch2!(State);
