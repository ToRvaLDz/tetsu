//! What is drawn: the space over a clear colour, or the locked scene.

use crate::lock::{LockMachine, Scene};

/// The background behind the windows.
pub const CLEAR_COLOR: [f32; 4] = [0.08, 0.08, 0.10, 1.0];
/// The opaque colour of a locked output.
pub const LOCK_COLOR: [f32; 4] = [0.02, 0.02, 0.02, 1.0];

/// What `output` may show: the one place that decides it.
pub fn scene(lock: &LockMachine, output: &str) -> Scene {
    lock.scene(output)
}
