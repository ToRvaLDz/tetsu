//! The Wayland side: Smithay client toolkit handlers for one layer surface
//! per output. No seat: the bar takes no input.

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

use super::{Reason, Ui};

/// The Wayland globals the surfaces need.
#[derive(Debug)]
pub struct Globals {
    pub registry: RegistryState,
    pub outputs: OutputState,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
}

impl CompositorHandler for Ui {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        self.set_scale(surface, factor);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Ui {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.globals.outputs
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_surface(output, qh);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.remove_output(&output);
    }
}

impl LayerShellHandler for Ui {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.remove_layer(layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.configured(layer, configure.new_size.0, Reason::Configure);
    }
}

impl ShmHandler for Ui {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.globals.shm
    }
}

impl ProvidesRegistryState for Ui {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.globals.registry
    }
    registry_handlers![OutputState];
}

delegate_registry!(Ui);
delegate_dispatch2!(Ui);
