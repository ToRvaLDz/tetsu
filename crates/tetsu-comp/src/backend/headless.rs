//! The headless backend: virtual outputs, no renderer, no input. Frame
//! callbacks come from a timer. It carries the CI tests.

use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::input::ModKey;
use crate::state::State;

/// Its size in pixels.
pub const OUTPUT_SIZE: (i32, i32) = (1920, 1080);
/// Its refresh rate in mHz.
pub const REFRESH_MHZ: i32 = 60_000;
/// Time between frame callbacks.
const FRAME: Duration = Duration::from_millis(16);

/// Adds `count` virtual outputs, `HEADLESS-1..`, side by side, and starts
/// their frame timers.
pub fn init(state: &mut State, count: u8) -> Result<(), Box<dyn std::error::Error>> {
    state.mod_key = ModKey::Super;
    for n in 1..=count {
        let output = state.add_output(&output_name(n), OUTPUT_SIZE.into(), REFRESH_MHZ);
        state
            .loop_handle
            .insert_source(Timer::from_duration(FRAME), move |_, (), state| {
                state.send_frames(&output);
                TimeoutAction::ToDuration(FRAME)
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The name of the n-th virtual output.
pub fn output_name(n: u8) -> String {
    format!("HEADLESS-{n}")
}
