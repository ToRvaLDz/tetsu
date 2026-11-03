//! The tty backend taking `ConfigureOutputs` to the displays: a mode
//! change is a modeset on the head, a disable drops the head (freeing its
//! CRTC), an enable lights the connector again on a free CRTC.
//!
//! Glue over Smithay's DRM objects: it is checked by review and by the
//! human checks H1 and H2, not by the headless tests.

use std::error::Error;

use smithay::backend::drm::output::DrmOutputRenderElements;
use smithay::output::Output;
use smithay::reexports::drm::control::{Device as ControlDevice, connector};

use super::Tty;
use super::device::connector_name;
use super::frame::Elements;
use super::output::{configured_mode, free_crtc, light_head};
use crate::outputs::glue::{OutputChange, Refused};
use crate::outputs::queue::ChangeSink;
use crate::state::State;

fn refuse(id: &str, reason: String) -> Refused {
    Refused {
        id: id.to_owned(),
        reason,
    }
}

/// The tty backend as the place the changes go to.
struct Live<'a> {
    state: &'a mut State,
    tty: &'a mut Tty,
}

impl ChangeSink for Live<'_> {
    fn apply(&mut self, changes: &[OutputChange]) -> Result<(), Refused> {
        let mut first_refusal = None;
        for change in changes {
            let (id, result) = match change {
                OutputChange::Mode(id) => (id, self.state.tty_mode(self.tty, id)),
                OutputChange::Disabled(id) => (id, self.state.tty_disable(self.tty, id)),
                OutputChange::Enabled(id) => (id, self.state.tty_enable(self.tty, id)),
            };
            if let Err(reason) = result {
                eprintln!("tetsu-comp: {id}: {reason}");
                first_refusal.get_or_insert_with(|| refuse(id, reason));
            }
        }
        first_refusal.map_or(Ok(()), Err)
    }
}

impl State {
    /// Takes the changes to the displays, Disabled first, then Mode, then
    /// Enabled. Without a tty there is nothing to do; while the tty is taken
    /// out (a rescan is running) they wait and go once it is back.
    pub(crate) fn apply_tty(&mut self, changes: &[OutputChange]) -> Result<(), Refused> {
        if self.tty.is_none() && !self.tty_taken {
            return Ok(());
        }
        let mut queue = std::mem::take(&mut self.tty_queue);
        let mut result = Ok(());
        if self.tty.is_none() {
            result = queue.submit(changes, None);
        } else {
            self.with_tty(|state, tty| {
                result = queue.submit(changes, Some(&mut Live { state, tty }));
            });
        }
        // Whatever was queued meanwhile comes after.
        queue.absorb(std::mem::take(&mut self.tty_queue));
        self.tty_queue = queue;
        result
    }

    fn output_of(&self, id: &str) -> Option<Output> {
        self.outputs
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.output.clone())
    }

    /// A modeset on the head of `id`, then a frame.
    fn tty_mode(&mut self, tty: &mut Tty, id: &str) -> Result<(), String> {
        let output = self.output_of(id).ok_or("no such output")?;
        let wanted = output.current_mode().ok_or("the output has no mode")?;
        let at = tty
            .heads
            .iter()
            .position(|h| h.output == output)
            .ok_or("the output has no head")?;
        let (index, node, crtc, handle) = {
            let head = &tty.heads[at];
            let index = tty
                .devices
                .iter()
                .position(|d| d.node == head.node)
                .ok_or("the card is gone")?;
            (index, head.node, head.crtc, head.connector)
        };
        let info = tty.devices[index]
            .manager
            .device()
            .get_connector(handle, true)
            .map_err(|e| e.to_string())?;
        let drm_mode = configured_mode(&info, &output)
            .ok_or_else(|| format!("the display offers no {}x{}", wanted.size.w, wanted.size.h))?;
        let render = tty.devices[index].render_node.unwrap_or(tty.render_node);
        let mut renderer = tty
            .gpus
            .single_renderer(&render)
            .map_err(|e| format!("no renderer: {e}"))?;
        tty.heads[at]
            .drm_output
            .use_mode(
                drm_mode,
                &mut renderer,
                &DrmOutputRenderElements::<_, Elements<'_>>::default(),
            )
            .map_err(|e| {
                format!(
                    "the display refused {}x{}@{}: {e}",
                    wanted.size.w, wanted.size.h, wanted.refresh
                )
            })?;
        drop(renderer);
        self.loop_handle
            .insert_idle(move |state| state.render_crtc(node, crtc));
        Ok(())
    }

    /// Drops the head of `id`: its CRTC is free and the display goes dark.
    fn tty_disable(&mut self, tty: &mut Tty, id: &str) -> Result<(), String> {
        let output = self.output_of(id).ok_or("no such output")?;
        tty.heads.retain(|h| h.output != output);
        Ok(())
    }

    /// Lights the connector of `id` again, at the output's mode.
    fn tty_enable(&mut self, tty: &mut Tty, id: &str) -> Result<(), String> {
        let output = self.output_of(id).ok_or("no such output")?;
        if tty.heads.iter().any(|h| h.output == output) {
            return Ok(());
        }
        let name = output.name();
        light_named(tty, &output, &name)
            .map(|(node, crtc)| {
                self.loop_handle
                    .insert_idle(move |state| state.render_crtc(node, crtc));
            })
            .map_err(|e| e.to_string())
    }
}

/// Finds the connected connector called `name` and lights it for `output`.
fn light_named(
    tty: &mut Tty,
    output: &Output,
    name: &str,
) -> Result<
    (
        smithay::backend::drm::DrmNode,
        smithay::reexports::drm::control::crtc::Handle,
    ),
    Box<dyn Error>,
> {
    for index in 0..tty.devices.len() {
        let node = tty.devices[index].node;
        let device = tty.devices[index].manager.device();
        let resources = device.resource_handles()?;
        for handle in resources.connectors() {
            let info = device.get_connector(*handle, true)?;
            if info.state() != connector::State::Connected
                || info.modes().is_empty()
                || connector_name(&info) != name
            {
                continue;
            }
            let used: Vec<_> = tty
                .heads
                .iter()
                .filter(|h| h.node == node)
                .map(|h| h.crtc)
                .collect();
            let crtc = free_crtc(device, &resources, &info, &used)
                .ok_or_else(|| format!("no free CRTC for {info}"))?;
            let mode = configured_mode(&info, output).unwrap_or(info.modes()[0]);
            light_head(tty, index, *handle, crtc, mode, output)?;
            return Ok((node, crtc));
        }
    }
    Err(format!("no connected connector {name}").into())
}
