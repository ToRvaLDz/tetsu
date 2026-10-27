//! The headless backend: one virtual output, no renderer, no input. Frame
//! callbacks come from a timer. It carries the CI tests.

use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::state::State;

/// The virtual output's name.
pub const OUTPUT_NAME: &str = "HEADLESS-1";
/// Its size in pixels.
pub const OUTPUT_SIZE: (i32, i32) = (1920, 1080);
/// Its refresh rate in mHz.
pub const REFRESH_MHZ: i32 = 60_000;
/// Time between frame callbacks.
const FRAME: Duration = Duration::from_millis(16);

/// Adds the virtual output and starts the frame timer.
pub fn init(state: &mut State) -> Result<(), Box<dyn std::error::Error>> {
    let output = state.add_output(OUTPUT_NAME, OUTPUT_SIZE.into(), REFRESH_MHZ);
    state
        .loop_handle
        .insert_source(Timer::from_duration(FRAME), move |_, (), state| {
            state.send_frames(&output);
            TimeoutAction::ToDuration(FRAME)
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
