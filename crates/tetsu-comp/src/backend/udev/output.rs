//! Per card: a `DrmOutputManager` with the allocator and formats from
//! `gpu::assign` and `gpu::formats`, and the connectors lit at their
//! preferred mode.
//!
//! An output that cannot be created with the full modifier list is retried
//! with the invalid modifier inside Smithay's `DrmOutputManager` itself, so
//! there is no retry here.
//! Overlay planes are off everywhere (Smithay #2119, #2154).

use std::error::Error;

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
        state
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
        });
    }
    Ok(())
}

/// Lights each connected connector on a free CRTC, at its preferred mode.
pub(super) fn light_connectors(state: &mut State, tty: &mut Tty) -> Result<(), Box<dyn Error>> {
    for index in 0..tty.devices.len() {
        light_device(state, tty, index)?;
    }
    Ok(())
}

fn light_device(state: &mut State, tty: &mut Tty, index: usize) -> Result<(), Box<dyn Error>> {
    let node = tty.devices[index].node;
    let plan = tty.devices[index].plan;
    let device_render = tty.devices[index].render_node.unwrap_or(tty.render_node);
    let resources = tty.devices[index].manager.device().resource_handles()?;
    let mut used: Vec<crtc::Handle> = Vec::new();
    for handle in resources.connectors() {
        let info = tty.devices[index]
            .manager
            .device()
            .get_connector(*handle, true)?;
        if info.state() != connector::State::Connected || info.modes().is_empty() {
            continue;
        }
        let Some(crtc) = free_crtc(
            tty.devices[index].manager.device(),
            &resources,
            &info,
            &used,
        ) else {
            eprintln!("tetsu-comp: no free CRTC for {info}");
            continue;
        };
        let drm_mode = *info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(&info.modes()[0]);
        let mode = Mode::from(drm_mode);
        let name = connector_name(&info);
        let output = state.add_virtual_output(&name, mode.size, mode.refresh);
        let mut planes = tty.devices[index]
            .manager
            .device()
            .planes(&crtc)
            .map_err(|e| format!("cannot read the planes of {name}: {e}"))?;
        planes.overlay = Vec::new();
        let mut renderer = tty
            .gpus
            .single_renderer(&device_render)
            .map_err(|e| format!("no renderer for {name}: {e}"))?;
        let drm_output = tty.devices[index]
            .manager
            .lock()
            .initialize_output::<_, Elements<'_>>(
                crtc,
                drm_mode,
                &[*handle],
                &output,
                Some(planes),
                &mut renderer,
                &DrmOutputRenderElements::default(),
            )
            .map_err(|e| format!("cannot light {name}: {e}"))?;
        drop(renderer);
        used.push(crtc);
        println!(
            "tetsu-comp: output {name} on {node}, cross-gpu={}",
            plan.cross_gpu
        );
        let target = smithay::backend::drm::DrmNode::from_dev_id(plan.target)?;
        let feedback = feedback::for_head(tty, &plan, target, &drm_output);
        tty.heads.push(Head {
            node,
            crtc,
            output,
            drm_output,
            target,
            feedback,
        });
    }
    Ok(())
}

/// The CRTC a connector already drives, else any free one it can use.
fn free_crtc(
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
