//! Smithay's protocol handlers on `State`.

mod compositor;
mod decoration;
mod dmabuf;
mod idle_notify;
mod layer_shell;
mod session_lock;
mod shell_misc;
mod syncobj;
mod virtual_input;
mod xdg_shell;

pub(crate) use compositor::has_buffer;

use crate::state::State;

smithay::delegate_dispatch2!(State);
