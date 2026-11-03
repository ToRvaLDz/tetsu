//! The outputs on the loop: the one apply path that takes the model's
//! target state to Smithay and the backend, and `ConfigureOutputs`.
//!
//! The model decides (see `prefs::targets` and `Model::configure_outputs`);
//! [`State::reapply_outputs`] then makes Smithay and the backend match it.
//! It runs at start, on hotplug, on a live reload and after a call.

use std::fmt;
use std::time::Duration;

use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode, Output, Scale};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::backend::GlobalId;
use tetsu_bus::output::OutputConfig;
use tetsu_bus::server::ErrorReply;

use super::configure::{ConfigureError, plan};
use super::geometry::to_smithay_transform;
use super::prefs::{self, apply_to_info};
use crate::bus::ConfigureOutputs;
use crate::config::set::state_write_failed;
use crate::model::OutputInfo;
use crate::state::State;

/// How long a darkened `wl_output` global stays listed, disabled, before it
/// is destroyed: a client binding at that very moment must not hit a
/// global that is gone.
const GLOBAL_REMOVE_DELAY: Duration = Duration::from_secs(10);

/// What the apply path did to an output, for the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputChange {
    /// Its current mode changed.
    Mode(String),
    /// It went dark.
    Disabled(String),
    /// It was lit again.
    Enabled(String),
}

/// The backend refused a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The output.
    pub id: String,
    /// Why.
    pub reason: String,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.reason)
    }
}

/// The Smithay mode of a wire mode.
pub fn smithay_mode(m: &tetsu_bus::output::Mode) -> Mode {
    Mode {
        size: (
            i32::try_from(m.width).unwrap_or(i32::MAX),
            i32::try_from(m.height).unwrap_or(i32::MAX),
        )
            .into(),
        refresh: i32::try_from(m.refresh_mhz).unwrap_or(i32::MAX),
    }
}

/// The Smithay scale of a factor.
pub fn smithay_scale(scale: f64) -> Scale {
    if scale == 1.0 {
        Scale::Integer(1)
    } else {
        Scale::Fractional(scale)
    }
}

/// Tells every layer surface of an output to close, and unmaps it.
pub fn close_layers(output: &Output) {
    let mut map = layer_map_for_output(output);
    let layers: Vec<_> = map.layers().cloned().collect();
    for layer in layers {
        layer.layer_surface().send_close();
        map.unmap_layer(&layer);
    }
}

impl State {
    /// The effective config values (empty before the config is loaded).
    fn config_values(&self) -> crate::config::merge::Values {
        self.config
            .as_ref()
            .map(|store| store.view().values())
            .unwrap_or_default()
    }

    /// The config's say on an output about to be added. Some output stays
    /// lit, whatever the config says.
    pub(crate) fn merge_output_prefs(&self, info: &mut OutputInfo) {
        apply_to_info(info, &self.config_values());
        info.enabled |= !self.outputs.iter().any(|o| o.enabled);
    }

    /// Disables a `wl_output` global now and destroys it a while later.
    pub(crate) fn retire_global(&mut self, global: GlobalId) {
        self.display_handle.disable_global::<State>(global.clone());
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(GLOBAL_REMOVE_DELAY),
            move |_, (), state| {
                state.display_handle.remove_global::<State>(global.clone());
                TimeoutAction::Drop
            },
        );
    }

    /// Takes the model's target state for every output (the config's say,
    /// leniently) and makes Smithay, the backend and the layout match.
    /// An error is the backend refusing a change; the rest was applied.
    pub fn reapply_outputs(&mut self) -> Result<(), Refused> {
        let outputs = self.engine.model().outputs_snapshot().outputs;
        let targets = prefs::targets(&outputs, &self.config_values());
        self.engine.configure_outputs(&targets);
        let changes = self.sync_outputs();
        let result = self.apply_backend(&changes);
        self.apply_output_layout();
        self.relayout();
        self.sync_focus();
        self.keep_pointer_on_screen();
        result
    }

    /// Makes the Smithay outputs match the model's; returns what changed.
    fn sync_outputs(&mut self) -> Vec<OutputChange> {
        let model = self.engine.model().outputs_snapshot().outputs;
        let mut changes = Vec::new();
        for at in 0..self.outputs.len() {
            let id = self.outputs[at].id.clone();
            let Some(want) = model.iter().find(|o| o.id == id) else {
                continue;
            };
            let was = self.outputs[at].enabled;
            match (was, want.enabled) {
                (true, false) => {
                    self.darken(at);
                    changes.push(OutputChange::Disabled(id));
                    continue;
                }
                (false, true) => {
                    self.light(at);
                    changes.push(OutputChange::Enabled(id.clone()));
                }
                (false, false) => continue,
                (true, true) => {}
            }
            if self.fit(at, want) && was {
                changes.push(OutputChange::Mode(id));
            }
        }
        changes
    }

    /// Sets mode, transform and scale of a lit output from the model's.
    /// Returns whether the mode changed.
    fn fit(&mut self, at: usize, want: &tetsu_bus::output::Output) -> bool {
        let output = self.outputs[at].output.clone();
        let mode = want
            .current_mode
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| want.modes.get(i))
            .map(smithay_mode);
        let transform = self.base_transform + to_smithay_transform(want.transform);
        let scale = smithay_scale(want.scale);
        let mode_changed = mode.is_some() && output.current_mode() != mode;
        let scale_changed = output.current_scale().fractional_scale() != scale.fractional_scale();
        if mode_changed || scale_changed || output.current_transform() != transform {
            output.change_current_state(
                mode.filter(|_| mode_changed),
                Some(transform),
                Some(scale),
                None,
            );
            layer_map_for_output(&output).arrange();
            self.lock_output_resized(&output);
        }
        mode_changed
    }

    /// Takes an output dark: it is unplugged as far as clients can tell,
    /// but stays listed.
    fn darken(&mut self, at: usize) {
        let entry = &mut self.outputs[at];
        entry.enabled = false;
        let (id, output, global) = (entry.id.clone(), entry.output.clone(), entry.global.take());
        close_layers(&output);
        self.space.unmap_output(&output);
        self.lock_output_removed(&id);
        if let Some(global) = global {
            self.retire_global(global);
        }
        eprintln!("tetsu-comp: output {} disabled", output.name());
    }

    /// Lights a dark output again: a new `wl_output` global appears.
    fn light(&mut self, at: usize) {
        let global = self.outputs[at]
            .output
            .create_global::<State>(&self.display_handle);
        let entry = &mut self.outputs[at];
        entry.enabled = true;
        entry.global = Some(global);
        let (id, name) = (entry.id.clone(), entry.output.name());
        self.lock.output_added(&id);
        eprintln!("tetsu-comp: output {name} enabled");
    }

    fn apply_backend(&mut self, changes: &[OutputChange]) -> Result<(), Refused> {
        self.apply_tty(changes)
    }

    /// Publishes the model's events now, ahead of a reply.
    fn publish_now(&mut self) {
        let mut events = self.engine.flush();
        events.session.extend(self.lock.flush());
        if let Some(publisher) = &self.publisher {
            publisher.publish(events);
        }
    }

    /// Runs a `ConfigureOutputs` from the bus: plans it, writes the managed
    /// file, applies it, publishes the events, then answers.
    pub fn handle_output_command(&mut self, command: ConfigureOutputs) {
        let ConfigureOutputs { configs, reply } = command;
        let answer = self.configure_outputs_now(&configs);
        self.publish_now();
        let _ = reply.send(answer);
    }

    fn configure_outputs_now(
        &mut self,
        configs: &std::collections::BTreeMap<String, OutputConfig>,
    ) -> Result<(), ErrorReply> {
        let Some(store) = self.config.as_mut() else {
            return Err(state_write_failed("the config is not loaded"));
        };
        // An edit saved a moment ago counts.
        let mut events = store.poll();
        let outputs = self.engine.model().outputs_snapshot().outputs;
        let planned = plan(&outputs, configs, store.view());
        let previous = store.managed_values().clone();
        let written = match &planned {
            Ok(planned) if !planned.managed.is_empty() => {
                let mut next = previous.clone();
                next.extend(planned.managed.clone());
                Some(store.write_managed(&next))
            }
            _ => None,
        };
        let failure = match (planned, written) {
            (Err(error), _) => Some(error.reply()),
            (Ok(_), Some(Err(e))) => Some(state_write_failed(&e.0)),
            (Ok(_), Some(Ok(more))) => {
                events.extend(more);
                None
            }
            (Ok(_), None) => None,
        };
        let applied = self.apply_config(events);
        if let Some(error) = failure {
            return Err(error);
        }
        applied.map_err(|refused| self.roll_back(&previous, &refused))
    }

    /// The backend refused what was just written: the managed layer goes
    /// back to what it was and the outputs follow.
    fn roll_back(
        &mut self,
        previous: &crate::config::merge::Values,
        refused: &Refused,
    ) -> ErrorReply {
        let events = self
            .config
            .as_mut()
            .and_then(|store| store.write_managed(previous).ok())
            .unwrap_or_default();
        if self.apply_config(events).is_err() {
            eprintln!("tetsu-comp: the outputs could not go back to what they were");
        }
        ConfigureError::Invalid {
            id: refused.id.clone(),
            reason: refused.reason.clone(),
        }
        .reply()
    }
}
