//! The render devices of the backends without a GPU of their own (#107).
//!
//! Headless and winit report one synthetic device, `headless-0` or
//! `winit-0` (node `/dev/dri/<id>`, which does not exist), wired to every
//! output and chosen automatically. A switch there is a relabel, so it is
//! always hot.
//!
//! With `--headless-hotplug`, stdin lines starting with `gpu` add fake
//! devices, so tests can switch between them:
//!
//! - `gpu add <name>`: a device `<name>` at `/dev/dri/<name>`;
//! - `gpu add <name> fail`: the same, but every hot switch to it fails;
//! - `gpu remove <name>`: the device goes; the rendering one and the
//!   synthetic one cannot be removed.
//!
//! A name is 1 to 32 characters of `a-z`, `0-9` and `-`. A line that does
//! not parse, or names nothing, is logged and ignored.

use crate::gpu::control::{DeviceFacts, RenderCtl, Switch, Target};
use crate::state::State;

/// The synthetic device of the headless backend.
pub const HEADLESS_DEVICE: &str = "headless-0";
/// The synthetic device of the winit backend.
pub const WINIT_DEVICE: &str = "winit-0";
/// The longest device name.
const MAX_NAME: usize = 32;

/// A `gpu` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuCommand {
    /// Adds a fake device.
    Add {
        /// Its id.
        name: String,
        /// Every hot switch to it fails.
        fail: bool,
    },
    /// Removes a fake device.
    Remove {
        /// Its id.
        name: String,
    },
}

fn name(text: &str) -> Result<String, String> {
    let fits = !text.is_empty()
        && text.len() <= MAX_NAME
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if fits {
        Ok(text.to_owned())
    } else {
        Err(format!("bad device name {text:?}"))
    }
}

/// Parses one `gpu` line.
pub fn parse(line: &str) -> Result<GpuCommand, String> {
    // Not written yet.
    let _ = name;
    Err(format!("cannot parse {:?}", line.trim_end()))
}

/// Starts the render state with the synthetic device `id`.
pub fn start(state: &mut State, id: &str) {
    let driver = id.split('-').next().unwrap_or(id);
    state.render = RenderCtl::synthetic(id, driver);
}

/// Runs one `gpu` line.
pub fn run(state: &mut State, line: &str) {
    match parse(line) {
        Ok(GpuCommand::Add { name, fail }) => {
            let facts = DeviceFacts {
                fails: fail,
                ..DeviceFacts::new(&name, &format!("/dev/dri/{name}"), "headless")
            };
            if !state.render.add(facts) {
                eprintln!("tetsu-comp: hotplug: device {name} exists already");
            }
        }
        Ok(GpuCommand::Remove { name }) => {
            if let Err(e) = state.render.remove(&name) {
                eprintln!("tetsu-comp: hotplug: cannot remove device {name}: {e}");
            }
        }
        Err(e) => eprintln!("tetsu-comp: hotplug: {e}"),
    }
}

/// A switch on a backend without a renderer: a relabel, unless the
/// device is a fake one marked to fail.
pub fn switch(ctl: &RenderCtl, target: &Target) -> Switch {
    let _ = (ctl, target);
    Switch::Failed("not written yet".to_owned())
}
