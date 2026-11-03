//! wl_compositor, wl_shm and buffers: the first buffer maps a toplevel.

use std::sync::atomic::{AtomicBool, Ordering};

use smithay::backend::renderer::utils::{RendererSurfaceStateUserData, on_commit_buffer_handler};
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, SurfaceAttributes, add_blocker, add_pre_commit_hook,
};
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
    with_states,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm::{ShmHandler, ShmState};

use crate::gpu::sync::{BufferKind, Gate, commit_gate};
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

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, gate_commit);
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if let Some(tty) = self.tty.as_mut() {
            tty.early_import(surface);
        }
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
            }
            self.commit_layer(&root);
        }
        self.commit_toplevel(surface);
        self.popups.commit(surface);
    }
}

/// Holds a commit back until its buffer is ready: the client's explicit
/// acquire point, else the dmabuf's implicit fence (`gpu::sync::commit_gate`).
fn gate_commit(
    state: &mut State,
    _dh: &smithay::reexports::wayland_server::DisplayHandle,
    surface: &WlSurface,
) {
    let mut acquire_point = None;
    let dmabuf = with_states(surface, |data| {
        acquire_point.clone_from(
            &data
                .cached_state
                .get::<DrmSyncobjCachedState>()
                .pending()
                .acquire_point,
        );
        data.cached_state
            .get::<SurfaceAttributes>()
            .pending()
            .buffer
            .as_ref()
            .and_then(|assignment| match assignment {
                BufferAssignment::NewBuffer(buffer) => Some(get_dmabuf(buffer).ok().cloned()),
                BufferAssignment::Removed => None,
            })
    });
    let kind = match &dmabuf {
        None => BufferKind::None,
        Some(Some(_)) => BufferKind::Dmabuf,
        Some(None) => BufferKind::Shm,
    };
    let syncobj_enabled = state.syncobj_state.is_some();
    let gate = commit_gate(syncobj_enabled, acquire_point.is_some(), kind);
    match (gate, acquire_point, dmabuf.flatten()) {
        (Gate::AcquirePoint, Some(point), _) => {
            if let Ok((blocker, source)) = point.generate_blocker() {
                hold(state, surface, blocker, source);
            }
        }
        (Gate::DmabufFence, _, Some(dmabuf)) => {
            if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                hold(state, surface, blocker, source);
            }
        }
        _ => {}
    }
}

/// Blocks the commit until `source` fires, then lets the client's queue go on.
fn hold<B, S>(state: &mut State, surface: &WlSurface, blocker: B, source: S)
where
    B: smithay::wayland::compositor::Blocker + Send + 'static,
    S: smithay::reexports::calloop::EventSource<Event = (), Ret = Result<(), std::io::Error>>
        + 'static,
{
    let Some(client) = surface.client() else {
        return;
    };
    let registered = state.loop_handle.insert_source(source, move |(), _, data| {
        let dh = data.display_handle.clone();
        data.client_compositor_state(&client)
            .blocker_cleared(data, &dh);
        Ok(())
    });
    if registered.is_ok() {
        add_blocker(surface, blocker);
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

/// Whether `surface` has a buffer, and this is the first time anyone asks
/// after it got one.
pub(crate) fn first_buffer(surface: &WlSurface) -> bool {
    if !has_buffer(surface) {
        return false;
    }
    with_states(surface, |states| {
        let flag = states
            .data_map
            .get_or_insert_threadsafe(|| FirstBuffer(AtomicBool::new(false)));
        !flag.0.swap(true, Ordering::Relaxed)
    })
}

/// Set once the first buffer of a surface was reported.
struct FirstBuffer(AtomicBool);

pub(crate) fn has_buffer(surface: &WlSurface) -> bool {
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
