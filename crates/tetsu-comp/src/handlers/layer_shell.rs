//! wlr-layer-shell: bars, panels, backgrounds and lock-like overlays. The
//! arrangement and the exclusive zones are Smithay's `LayerMap`; this file
//! picks the output, announces the surface and lays the windows out again
//! when the zone changes.

use smithay::desktop::{LayerSurface, PopupKind, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::PopupSurface;

use crate::state::State;

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let Some(output) = self.output_for_layer(wl_output.as_ref()) else {
            // No output to put it on: the client may try again when one appears.
            surface.send_close();
            return;
        };
        eprintln!("tetsu-comp: layer {namespace} on {}", output.name());
        let layer = LayerSurface::new(surface, namespace);
        if let Err(e) = layer_map_for_output(&output).map_layer(&layer) {
            eprintln!("tetsu-comp: cannot map a layer surface: {e}");
            layer.layer_surface().send_close();
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        let positioner = popup.with_pending_state(|state| state.positioner);
        popup.with_pending_state(|state| state.geometry = positioner.get_geometry());
        if let Err(e) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            eprintln!("tetsu-comp: cannot track a layer popup: {e}");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let found = self.outputs.iter().find_map(|entry| {
            let map = layer_map_for_output(&entry.output);
            let layer = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            layer.map(|layer| (entry.output.clone(), layer))
        });
        let Some((output, layer)) = found else {
            return;
        };
        layer_map_for_output(&output).unmap_layer(&layer);
        self.relayout();
        self.sync_focus();
    }
}

impl State {
    /// The output a new layer surface goes on: the one it names, else the
    /// focused one, else the first lit one. A dark output takes none. `None`
    /// when it names an output that is gone or dark, or there is none.
    fn output_for_layer(&self, wanted: Option<&WlOutput>) -> Option<Output> {
        if let Some(wanted) = wanted {
            let output = Output::from_resource(wanted)?;
            return self
                .outputs
                .iter()
                .any(|o| o.enabled && o.output == output)
                .then_some(output);
        }
        let focused = self
            .engine
            .model()
            .focused_output()
            .and_then(|id| self.outputs.iter().find(|o| o.enabled && o.id == id));
        focused
            .or_else(|| self.outputs.iter().find(|o| o.enabled))
            .map(|o| o.output.clone())
    }

    /// A layer surface committed: arrange, answer the first commit with its
    /// configure, and lay the windows out again when the zone changed.
    pub(crate) fn commit_layer(&mut self, surface: &WlSurface) {
        let Some(output) = self.outputs.iter().map(|o| o.output.clone()).find(|o| {
            layer_map_for_output(o)
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .is_some()
        }) else {
            return;
        };
        let (zone_changed, namespace) = {
            let mut map = layer_map_for_output(&output);
            let before = map.non_exclusive_zone();
            // Arrange before the first configure, so it carries the size.
            map.arrange();
            let configured = with_states(surface, |states| {
                states.data_map.get::<LayerSurfaceData>().is_some_and(|d| {
                    d.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .initial_configure_sent
                })
            });
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL);
            if let Some(layer) = layer
                && !configured
            {
                layer.layer_surface().send_configure();
            }
            let namespace = layer.map(|l| l.namespace().to_owned());
            (map.non_exclusive_zone() != before, namespace)
        };
        if let Some(namespace) = namespace
            && crate::handlers::compositor::first_buffer(surface)
        {
            eprintln!("tetsu-comp: layer {namespace} mapped on {}", output.name());
        }
        if zone_changed {
            self.relayout();
        }
        self.sync_focus();
    }
}
