//! wl_compositor, wl_shm and buffers: the first buffer maps a toplevel.

use smithay::backend::renderer::utils::{RendererSurfaceStateUserData, on_commit_buffer_handler};
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
    with_states,
};
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm::{ShmHandler, ShmState};

use crate::state::{ClientState, State};

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Every client is inserted with a `ClientState`.
        match client.get_data::<ClientState>() {
            Some(data) => &data.compositor_state,
            None => unreachable!("a client without ClientState"),
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
            }
        }
        self.commit_toplevel(surface);
        self.popups.commit(surface);
    }
}

impl State {
    /// A toplevel's first commit gets its configure; the first one with a
    /// buffer maps it.
    fn commit_toplevel(&mut self, surface: &WlSurface) {
        let Some(at) = self
            .unmapped
            .iter()
            .position(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
        else {
            return;
        };
        let Some(toplevel) = self.unmapped[at].toplevel().cloned() else {
            return;
        };
        let configured = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .is_some_and(|d| {
                    d.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .initial_configure_sent
                })
        });
        if !configured {
            self.send_initial_configure(&toplevel);
        } else if has_buffer(surface) {
            let window = self.unmapped.remove(at);
            let pid = self.client_pid(surface);
            self.map_window(window, pid);
        }
    }
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .is_some_and(|d| {
                d.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .buffer()
                    .is_some()
            })
    })
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
