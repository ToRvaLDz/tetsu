//! Per card: a `DrmOutputManager` with the allocator and formats from
//! `gpu::assign` and `gpu::formats`, and the connectors lit at their
//! preferred mode.
//!
//! An output that cannot be created with the full modifier list is retried
//! with the invalid modifier inside Smithay's `DrmOutputManager` itself, so
//! there is no retry here.
//! Overlay planes are off everywhere (Smithay #2119, #2154).

use std::error::Error;

use crate::model::OutputInfo;
use crate::outputs::edid;
use crate::outputs::modes::from_list;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags};
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmEvent};
use smithay::output::Mode;
use smithay::reexports::drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc};

use super::device::{Opened, connector_name};
use super::frame::Elements;
use super::{Device, Head, Tty, feedback};
use crate::gpu::assign::{Alloc, assign};
use crate::gpu::formats::{color_formats, render_formats};
use crate::state::State;

/// Builds each card's output manager and registers its VBlank source.
pub(super) fn build_devices(
    state: &mut State,
    tty: &mut Tty,
    opened: Vec<Opened>,
) -> Result<(), Box<dyn Error>> {
    let render_id = tty.render_node.dev_id();
    for o in opened {
        let plan = assign(o.render_node.map(|n| n.dev_id()), render_id);
        let allocator = match plan.allocator {
            Alloc::Own => GbmAllocator::new(
                o.gbm.clone(),
                GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
            ),
            Alloc::RenderDevice => tty
                .devices
                .iter()
                .find(|d| d.render_node == Some(tty.render_node))
                .map(|d| d.manager.allocator().clone())
                .ok_or("a display-only device needs the render device first")?,
        };
        let exporter = GbmFramebufferExporter::new(o.gbm.clone(), o.render_node.into());
        let mut renderer = tty
            .gpus
            .single_renderer(&o.render_node.unwrap_or(tty.render_node))
            .map_err(|e| format!("no renderer for {}: {e}", o.node))?;
        let all: Vec<Format> = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied()
            .collect();
        drop(renderer);
        let formats: FormatSet = render_formats(&all, plan.linear_only).into_iter().collect();
        let colors: Vec<Fourcc> = color_formats(plan.cross_gpu).to_vec();
        let manager =
            DrmOutputManager::new(o.drm, allocator, exporter, Some(o.gbm), colors, formats);
        let node = o.node;
        let events = state
            .loop_handle
            .insert_source(o.events, move |event, _, state| match event {
                DrmEvent::VBlank(crtc) => state.frame_done(node, crtc),
                DrmEvent::Error(e) => eprintln!("tetsu-comp: drm {node}: {e}"),
            })
            .map_err(|e| e.to_string())?;
        tty.devices.push(Device {
            node,
            render_node: o.render_node,
            plan,
            manager,
            events,
        });
    }
    Ok(())
}

/// Lights each connected connector on a free CRTC, at its preferred mode.
/// One that fails is skipped with a message: the others still light.
pub(super) fn light_connectors(state: &mut State, tty: &mut Tty) -> Result<(), Box<dyn Error>> {
    for index in 0..tty.devices.len() {
        light_device(state, tty, index)?;
    }
    Ok(())
}

/// Lights every connected connector of a card that has no head yet; returns
/// the CRTCs it lit.
fn light_device(
    state: &mut State,
    tty: &mut Tty,
    index: usize,
) -> Result<Vec<crtc::Handle>, Box<dyn Error>> {
    let node = tty.devices[index].node;
    let resources = tty.devices[index].manager.device().resource_handles()?;
    let mut used: Vec<crtc::Handle> = tty
        .heads
        .iter()
        .filter(|h| h.node == node)
        .map(|h| h.crtc)
        .collect();
    let mut lit = Vec::new();
    for handle in resources.connectors() {
        if tty
            .heads
            .iter()
            .any(|h| h.node == node && h.connector == *handle)
        {
            continue;
        }
        match light_connector(state, tty, index, *handle, &used) {
            Ok(Some(crtc)) => {
                used.push(crtc);
                lit.push(crtc);
            }
            Ok(None) => {}
            Err(e) => eprintln!("tetsu-comp: {node}: {e}"),
        }
    }
    Ok(lit)
}

/// Lights one connector if it is connected and a CRTC is free; returns the
/// CRTC. Whatever fails after the output exists removes it again.
fn light_connector(
    state: &mut State,
    tty: &mut Tty,
    index: usize,
    handle: connector::Handle,
    used: &[crtc::Handle],
) -> Result<Option<crtc::Handle>, Box<dyn Error>> {
    let device = tty.devices[index].manager.device();
    let info = device.get_connector(handle, true)?;
    if info.state() != connector::State::Connected || info.modes().is_empty() {
        return Ok(None);
    }
    let resources = device.resource_handles()?;
    let Some(crtc) = free_crtc(device, &resources, &info, used) else {
        eprintln!("tetsu-comp: no free CRTC for {info}");
        return Ok(None);
    };
    let preferred = *info
        .modes()
        .iter()
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(&info.modes()[0]);
    let name = connector_name(&info);
    // An output of that name already listed: a dark one stays dark, a lit
    // one without a head (recovered after the last display went) is lit now.
    let known = state
        .outputs
        .iter()
        .find(|o| o.output.name() == name)
        .map(|o| (o.output.clone(), o.enabled));
    let (output, added) = match known {
        Some((_, false)) => return Ok(None),
        Some((output, true)) => (output, false),
        None => {
            let described = describe(device, handle, &info, &name, preferred);
            (state.add_output(described), true)
        }
    };
    if !state
        .outputs
        .iter()
        .any(|o| o.output == output && o.enabled)
    {
        println!("tetsu-comp: output {name} is disabled by the config");
        return Ok(None);
    }
    let drm_mode = configured_mode(&info, &output).unwrap_or(preferred);
    match light_head(tty, index, handle, crtc, drm_mode, &output) {
        Ok(()) => {
            let node = tty.devices[index].node;
            println!("tetsu-comp: output {name} connected on {node}");
            Ok(Some(crtc))
        }
        Err(e) => {
            if added {
                state.remove_output(&output);
            }
            Err(format!("cannot light {name}: {e}").into())
        }
    }
}

/// The connector's mode that the output is set to (the config's choice
/// when the connector offers it).
pub(super) fn configured_mode(
    info: &connector::Info,
    output: &smithay::output::Output,
) -> Option<smithay::reexports::drm::control::Mode> {
    let wanted = output.current_mode()?;
    info.modes()
        .iter()
        .copied()
        .find(|m| Mode::from(*m) == wanted)
}

/// The part of lighting that can fail once the output exists.
pub(super) fn light_head(
    tty: &mut Tty,
    index: usize,
    handle: connector::Handle,
    crtc: crtc::Handle,
    drm_mode: smithay::reexports::drm::control::Mode,
    output: &smithay::output::Output,
) -> Result<(), Box<dyn Error>> {
    let node = tty.devices[index].node;
    let plan = tty.devices[index].plan;
    let device_render = tty.devices[index].render_node.unwrap_or(tty.render_node);
    let mut planes = tty.devices[index]
        .manager
        .device()
        .planes(&crtc)
        .map_err(|e| format!("cannot read the planes: {e}"))?;
    planes.overlay = Vec::new();
    let mut renderer = tty
        .gpus
        .single_renderer(&device_render)
        .map_err(|e| format!("no renderer: {e}"))?;
    let drm_output = tty.devices[index]
        .manager
        .lock()
        .initialize_output::<_, Elements<'_>>(
            crtc,
            drm_mode,
            &[handle],
            output,
            Some(planes),
            &mut renderer,
            &DrmOutputRenderElements::default(),
        )
        .map_err(|e| e.to_string())?;
    drop(renderer);
    println!(
        "tetsu-comp: output {} on {node}, cross-gpu={}",
        output.name(),
        plan.cross_gpu
    );
    let target = smithay::backend::drm::DrmNode::from_dev_id(plan.target)?;
    let feedback = feedback::for_head(tty, &plan, target, &drm_output);
    tty.heads.push(Head {
        node,
        crtc,
        connector: handle,
        output: output.clone(),
        drm_output,
        target,
        feedback,
    });
    Ok(())
}

/// What the model keeps of a connector: name, EDID fields and modes.
fn describe(
    device: &DrmDevice,
    handle: connector::Handle,
    info: &connector::Info,
    name: &str,
    current: smithay::reexports::drm::control::Mode,
) -> OutputInfo {
    let raw: Vec<_> = info
        .modes()
        .iter()
        .map(|m| {
            let mode = Mode::from(*m);
            (
                mode.size.w,
                mode.size.h,
                mode.refresh,
                m.mode_type().contains(ModeTypeFlags::PREFERRED),
            )
        })
        .collect();
    let now = Mode::from(current);
    let (modes, current_mode) = from_list(&raw, (now.size.w, now.size.h, now.refresh));
    let edid = read_edid(device, handle).map(|blob| edid::parse(&blob));
    let edid = edid.unwrap_or_default();
    OutputInfo {
        make: edid.make,
        model: edid.model,
        serial: edid.serial,
        modes,
        current_mode,
        ..OutputInfo::named(name)
    }
}

/// The connector's EDID blob, if it has one.
fn read_edid(device: &DrmDevice, handle: connector::Handle) -> Option<Vec<u8>> {
    let set = device.get_properties(handle).ok()?;
    let (props, values) = set.as_props_and_values();
    props.iter().zip(values).find_map(|(prop, value)| {
        let info = device.get_property(*prop).ok()?;
        if info.name().to_str() != Ok("EDID") {
            return None;
        }
        let blob = info.value_type().convert_value(*value).as_blob()?;
        device.get_property_blob(blob).ok()
    })
}

impl State {
    /// A card sent a uevent: connectors may have come or gone.
    pub(super) fn card_changed(&mut self, device_id: u64) {
        self.with_tty(|state, tty| {
            if let Some(index) = tty
                .devices
                .iter()
                .position(|d| d.node.dev_id() == device_id)
            {
                rescan(state, tty, index);
            }
        });
    }

    /// Looks at every card again (after a session resume).
    pub(super) fn rescan_all(&mut self) {
        self.with_tty(|state, tty| {
            for index in 0..tty.devices.len() {
                rescan(state, tty, index);
            }
        });
    }

    /// A card went away: its heads go with it.
    pub(super) fn card_removed(&mut self, device_id: u64) {
        self.with_tty(|state, tty| {
            let Some(index) = tty
                .devices
                .iter()
                .position(|d| d.node.dev_id() == device_id)
            else {
                return;
            };
            let device = tty.devices.remove(index);
            let lost_render = device.render_node == Some(tty.render_node);
            if lost_render {
                eprintln!(
                    "tetsu-comp: the render device {} went away; the outputs it served cannot be drawn until a restart",
                    device.node
                );
            }
            // Without the render device no head can be drawn: drop them
            // all, or each would retry and log every frame.
            drop_heads(state, tty, |h| lost_render || h.node == device.node);
            forget_dark(state, tty);
            state.loop_handle.remove(device.events);
            if let Some(render) = device.render_node {
                tty.gpus.as_mut().remove_node(&render);
                // The rendering device stays listed: it is still the one.
                let _ = state.render.remove(&super::render::device_id(&render));
            }
            eprintln!("tetsu-comp: card {} removed", device.node);
        });
    }

    /// Runs `f` with the tty backend taken out of the state, then puts it back.
    pub(super) fn with_tty(&mut self, f: impl FnOnce(&mut State, &mut Tty)) {
        let Some(mut tty) = self.tty.take() else {
            return;
        };
        self.tty_taken = true;
        f(self, &mut tty);
        self.tty = Some(tty);
        self.tty_taken = false;
        // Changes made by a nested reapply while the tty was away.
        if !self.tty_queue.is_empty()
            && let Err(refused) = self.apply_tty(&[])
        {
            eprintln!("tetsu-comp: {refused}");
        }
    }
}

/// Drops the heads that `gone` selects, and their outputs.
fn drop_heads(state: &mut State, tty: &mut Tty, gone: impl Fn(&Head) -> bool) {
    let (dropped, kept): (Vec<Head>, Vec<Head>) = std::mem::take(&mut tty.heads)
        .into_iter()
        .partition(|h| gone(h));
    tty.heads = kept;
    for head in dropped {
        println!(
            "tetsu-comp: output {} disconnected from {}",
            head.output.name(),
            head.node
        );
        state.remove_output(&head.output);
    }
}

/// Whether some card has a connected connector called `name`.
pub(super) fn connected_named(tty: &Tty, name: &str) -> bool {
    tty.devices.iter().any(|d| {
        let device = d.manager.device();
        let Ok(resources) = device.resource_handles() else {
            return false;
        };
        resources.connectors().iter().any(|handle| {
            device.get_connector(*handle, true).is_ok_and(|info| {
                info.state() == connector::State::Connected
                    && !info.modes().is_empty()
                    && connector_name(&info) == name
            })
        })
    })
}

/// A dark output has no head: it goes when its connector is unplugged.
fn forget_dark(state: &mut State, tty: &Tty) {
    let gone: Vec<_> = state
        .outputs
        .iter()
        .filter(|o| !o.enabled && !connected_named(tty, &o.output.name()))
        .map(|o| o.output.clone())
        .collect();
    for output in gone {
        println!("tetsu-comp: disabled output {} disconnected", output.name());
        state.remove_output(&output);
    }
}

/// Matches a card's heads with its connectors: unplugged ones go, new ones
/// light and get their first frame.
fn rescan(state: &mut State, tty: &mut Tty, index: usize) {
    let node = tty.devices[index].node;
    let device = tty.devices[index].manager.device();
    let connected = |handle: connector::Handle| {
        device
            .get_connector(handle, true)
            .is_ok_and(|i| i.state() == connector::State::Connected && !i.modes().is_empty())
    };
    let unplugged: Vec<connector::Handle> = tty
        .heads
        .iter()
        .filter(|h| h.node == node && !connected(h.connector))
        .map(|h| h.connector)
        .collect();
    drop_heads(state, tty, |h| {
        h.node == node && unplugged.contains(&h.connector)
    });
    forget_dark(state, tty);
    match light_device(state, tty, index) {
        Ok(lit) => {
            for crtc in lit {
                state
                    .loop_handle
                    .insert_idle(move |state| state.render_crtc(node, crtc));
            }
        }
        Err(e) => eprintln!("tetsu-comp: cannot look at {node}: {e}"),
    }
}

/// The CRTC a connector already drives, else any free one it can use.
pub(super) fn free_crtc(
    device: &DrmDevice,
    resources: &smithay::reexports::drm::control::ResourceHandles,
    info: &connector::Info,
    used: &[crtc::Handle],
) -> Option<crtc::Handle> {
    let encoders: Vec<_> = info
        .encoders()
        .iter()
        .filter_map(|e| device.get_encoder(*e).ok())
        .collect();
    let current = encoders
        .iter()
        .filter_map(|e| e.crtc())
        .find(|c| !used.contains(c));
    current.or_else(|| {
        encoders
            .iter()
            .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
            .find(|c| !used.contains(c))
    })
}
