//! The headless backend: virtual outputs, no renderer, no input. Frame
//! callbacks come from a timer. It carries the CI tests.
//!
//! With `--headless-hotplug` it also reads hotplug commands from stdin, one
//! per line, so tests can plug and unplug outputs:
//!
//! - `add [<w>x<h>]`: a new output named `HEADLESS-<n>`, `n` counting up;
//! - `add-named <name> <w>x<h>`: an output with that name (a replug);
//! - `remove <name>`.
//!
//! A line that does not parse, or names nothing, is logged and ignored.

use std::io::BufRead;
use std::time::Duration;

use smithay::reexports::calloop::channel::{self, Event};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::input::ModKey;
use crate::state::State;

/// Its size in pixels.
pub const OUTPUT_SIZE: (i32, i32) = (1920, 1080);
/// Its refresh rate in mHz.
pub const REFRESH_MHZ: i32 = 60_000;
/// Time between frame callbacks.
const FRAME: Duration = Duration::from_millis(16);
/// The largest side of a hotplugged output.
const MAX_SIDE: i32 = 16_384;
/// The longest hotplug line read.
const MAX_LINE: usize = 256;

/// A hotplug command read from stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotplugCommand {
    /// A new output with the next free `HEADLESS-<n>` name.
    Add {
        /// Its size; the default one when `None`.
        size: Option<(i32, i32)>,
    },
    /// An output with a name of the caller's choosing.
    AddNamed {
        /// The connector name.
        name: String,
        /// Its size.
        size: (i32, i32),
    },
    /// Unplugs the output with this name.
    Remove {
        /// The connector name.
        name: String,
    },
}

/// Parses one hotplug line; a trailing newline is fine.
pub fn parse_command(line: &str) -> Result<HotplugCommand, String> {
    if line.len() > MAX_LINE {
        return Err("line too long".to_owned());
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["add"] => Ok(HotplugCommand::Add { size: None }),
        ["add", size] => Ok(HotplugCommand::Add {
            size: Some(parse_size(size)?),
        }),
        ["add-named", name, size] => Ok(HotplugCommand::AddNamed {
            name: (*name).to_owned(),
            size: parse_size(size)?,
        }),
        ["remove", name] => Ok(HotplugCommand::Remove {
            name: (*name).to_owned(),
        }),
        [] => Err("empty line".to_owned()),
        _ => Err(format!("cannot parse {:?}", line.trim_end())),
    }
}

/// `<w>x<h>`, each side in `1..=16384`, digits only.
fn parse_size(text: &str) -> Result<(i32, i32), String> {
    let side = |part: &str| -> Result<i32, String> {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("bad size {text:?}"));
        }
        match part.parse::<i32>() {
            Ok(n) if (1..=MAX_SIDE).contains(&n) => Ok(n),
            _ => Err(format!("size {text:?} is out of range 1..={MAX_SIDE}")),
        }
    };
    let (w, h) = text
        .split_once('x')
        .ok_or_else(|| format!("bad size {text:?}"))?;
    Ok((side(w)?, side(h)?))
}

/// Adds `count` virtual outputs, `HEADLESS-1..`, side by side, and starts
/// their frame timers. With `hotplug`, reads commands from stdin.
pub fn init(state: &mut State, count: u8, hotplug: bool) -> Result<(), Box<dyn std::error::Error>> {
    state.mod_key = ModKey::Super;
    for n in 1..=count {
        add_output(state, &output_name(n), OUTPUT_SIZE)?;
    }
    if hotplug {
        listen(state, u32::from(count))?;
    }
    Ok(())
}

/// The name of the n-th virtual output.
pub fn output_name(n: u8) -> String {
    format!("HEADLESS-{n}")
}

/// Adds an output and the timer that sends its frame callbacks. The timer
/// ends with the output.
fn add_output(
    state: &mut State,
    name: &str,
    size: (i32, i32),
) -> Result<(), Box<dyn std::error::Error>> {
    let output = state.add_virtual_output(name, size.into(), REFRESH_MHZ);
    state
        .loop_handle
        .insert_source(Timer::from_duration(FRAME), move |_, (), state| {
            if !state.outputs.iter().any(|o| o.output == output) {
                return TimeoutAction::Drop;
            }
            state.send_frames(&output);
            TimeoutAction::ToDuration(FRAME)
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Reads stdin on a thread, so the loop never blocks, and acts on each
/// line in the loop.
fn listen(state: &mut State, mut counter: u32) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = channel::channel::<String>();
    std::thread::Builder::new()
        .name("tetsu-comp-hotplug".to_owned())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })?;
    state
        .loop_handle
        .insert_source(rx, move |event, (), state| {
            if let Event::Msg(line) = event {
                match parse_command(&line) {
                    Ok(command) => run_command(state, command, &mut counter),
                    Err(e) => eprintln!("tetsu-comp: hotplug: {e}"),
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn run_command(state: &mut State, command: HotplugCommand, counter: &mut u32) {
    let exists = |state: &State, name: &str| state.outputs.iter().any(|o| o.output.name() == name);
    let added = match command {
        HotplugCommand::Add { size } => {
            let name = loop {
                *counter += 1;
                let name = format!("HEADLESS-{counter}");
                if !exists(state, &name) {
                    break name;
                }
            };
            add_output(state, &name, size.unwrap_or(OUTPUT_SIZE))
        }
        HotplugCommand::AddNamed { name, size } => {
            if exists(state, &name) {
                eprintln!("tetsu-comp: hotplug: output {name} exists already");
                return;
            }
            add_output(state, &name, size)
        }
        HotplugCommand::Remove { name } => {
            let found = state
                .outputs
                .iter()
                .find(|o| o.output.name() == name)
                .map(|o| o.output.clone());
            match found {
                Some(output) => state.remove_output(&output),
                None => eprintln!("tetsu-comp: hotplug: no output {name}"),
            }
            Ok(())
        }
    };
    if let Err(e) = added {
        eprintln!("tetsu-comp: hotplug: {e}");
    }
}
