//! The headless backend: virtual outputs, no renderer, no input. Frame
//! callbacks come from a timer. It carries the CI tests.
//!
//! With `--headless-hotplug` it also reads hotplug commands from stdin, one
//! per line, so tests can plug and unplug outputs:
//!
//! - `add [<w>x<h>]`: a new output named `HEADLESS-<n>`, `n` counting up;
//! - `add-named <name> <w>x<h>`: an output with that name (a replug);
//! - `add-modes <name> <w>x<h>[,<w>x<h>...]`: an output with several
//!   modes (at most 16), all at 60 Hz; the first is preferred and current;
//! - `remove <name>`;
//! - `focus-next` / `focus-prev`: the Super+J / Super+K bind, run through
//!   the same path (the focus-to-bar latency test uses it);
//! - `activity`: user activity, through the path every input takes
//!   (`State::note_activity`), so tests can drive idle.
//!
//! A line that does not parse, or names nothing, is logged and ignored.

use std::io::{BufRead, Read};
use std::time::Duration;

use smithay::reexports::calloop::channel::{self, Event};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use tetsu_bus::output::Mode;

use crate::engine::Action as EngineAction;
use crate::input::ModKey;
use crate::lock::glue::View;
use crate::model::OutputInfo;
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
/// The most modes `add-modes` makes.
const MAX_MODES: usize = 16;

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
    /// An output with several modes; the first is preferred and current.
    AddModes {
        /// The connector name.
        name: String,
        /// The sizes of its modes.
        sizes: Vec<(i32, i32)>,
    },
    /// Unplugs the output with this name.
    Remove {
        /// The connector name.
        name: String,
    },
    /// The focus-next bind.
    FocusNext,
    /// The focus-prev bind.
    FocusPrev,
    /// User activity, as input would report it.
    Activity,
}

/// Reads one line, without its newline, converted lossily. Never buffers
/// more than `MAX_LINE + 1` bytes: a longer line comes back cut there (so
/// `parse_command` refuses it) and its rest is skipped. `None` at the end.
pub fn read_command_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let cap = MAX_LINE as u64 + 1;
    let mut buf = Vec::new();
    reader.by_ref().take(cap).read_until(b'\n', &mut buf)?;
    if buf.is_empty() {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() as u64 == cap {
        // Overlong: skip up to the newline, a bounded chunk at a time.
        loop {
            let mut skipped = Vec::new();
            reader.by_ref().take(cap).read_until(b'\n', &mut skipped)?;
            if skipped.last() != Some(&b'\n') && skipped.len() as u64 == cap {
                continue;
            }
            break;
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
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
        ["add-modes", name, sizes] => Ok(HotplugCommand::AddModes {
            name: (*name).to_owned(),
            sizes: parse_sizes(sizes)?,
        }),
        ["remove", name] => Ok(HotplugCommand::Remove {
            name: (*name).to_owned(),
        }),
        ["focus-next"] => Ok(HotplugCommand::FocusNext),
        ["focus-prev"] => Ok(HotplugCommand::FocusPrev),
        ["activity"] => Ok(HotplugCommand::Activity),
        [] => Err("empty line".to_owned()),
        _ => Err(format!("cannot parse {:?}", line.trim_end())),
    }
}

/// `<w>x<h>[,<w>x<h>...]`, at most 16 sizes.
fn parse_sizes(text: &str) -> Result<Vec<(i32, i32)>, String> {
    let sizes = text
        .split(',')
        .map(parse_size)
        .collect::<Result<Vec<_>, _>>()?;
    if sizes.len() > MAX_MODES {
        return Err(format!("more than {MAX_MODES} modes"));
    }
    Ok(sizes)
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

/// The length of one idle second, from `TETSU_COMP_HEADLESS_IDLE_UNIT_MS`.
pub fn idle_unit_of(_text: &str) -> Option<Duration> {
    None
}

/// Adds `count` virtual outputs, `HEADLESS-1..`, side by side, and starts
/// their frame timers. With `hotplug`, reads commands from stdin.
pub fn init(state: &mut State, count: u8, hotplug: bool) -> Result<(), Box<dyn std::error::Error>> {
    state.mod_key = ModKey::Super;
    for n in 1..=count {
        add_output(state, &output_name(n), &[OUTPUT_SIZE])?;
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
    sizes: &[(i32, i32)],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut info = OutputInfo::named(name);
    info.modes = sizes
        .iter()
        .enumerate()
        .map(|(at, (w, h))| Mode {
            width: i64::from(*w),
            height: i64::from(*h),
            refresh_mhz: i64::from(REFRESH_MHZ),
            preferred: at == 0,
        })
        .collect();
    let output = state.add_output(info);
    state
        .loop_handle
        .insert_source(Timer::from_duration(FRAME), move |_, (), state| {
            let Some(entry) = state.outputs.iter().find(|o| o.output == output) else {
                return TimeoutAction::Drop;
            };
            // A dark output has no frames.
            if !entry.enabled {
                return TimeoutAction::ToDuration(FRAME);
            }
            // No renderer: the tick presents the scene as it is now.
            let locked = matches!(state.view(&output), View::Locked(_));
            state.send_frames(&output);
            state.output_presented(&output, locked);
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
            let mut stdin = std::io::stdin().lock();
            loop {
                match read_command_line(&mut stdin) {
                    Ok(Some(line)) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => eprintln!("tetsu-comp: hotplug: cannot read stdin: {e}"),
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
            add_output(state, &name, &[size.unwrap_or(OUTPUT_SIZE)])
        }
        HotplugCommand::AddNamed { name, size } => {
            if exists(state, &name) {
                eprintln!("tetsu-comp: hotplug: output {name} exists already");
                return;
            }
            add_output(state, &name, &[size])
        }
        HotplugCommand::AddModes { name, sizes } => {
            if exists(state, &name) {
                eprintln!("tetsu-comp: hotplug: output {name} exists already");
                return;
            }
            add_output(state, &name, &sizes)
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
        HotplugCommand::FocusNext => {
            state.run_action(EngineAction::FocusNext);
            Ok(())
        }
        HotplugCommand::FocusPrev => {
            state.run_action(EngineAction::FocusPrev);
            Ok(())
        }
        HotplugCommand::Activity => {
            state.note_activity();
            Ok(())
        }
    };
    if let Err(e) = added {
        eprintln!("tetsu-comp: hotplug: {e}");
    }
}
