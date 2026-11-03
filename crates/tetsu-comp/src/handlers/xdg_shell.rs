//! xdg-shell: toplevels become windows of the model.

use smithay::desktop::{PopupKind, Window};
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat};
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::model::WindowUpdate;
use crate::state::{State, toplevel_text};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.unmapped.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.unmap_window(surface.wl_surface());
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| state.geometry = positioner.get_geometry());
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    /// A floating window can be moved by its client's request; tiles ignore it.
    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        // Only a request that follows a click on the window counts.
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };
        let Some(id) = self
            .mapped
            .iter()
            .find(|m| {
                m.window
                    .toplevel()
                    .is_some_and(|t| t.wl_surface() == surface.wl_surface())
            })
            .map(|m| m.id.clone())
        else {
            return;
        };
        self.start_move_grab(&id, start_data, serial);
    }

    // Fullscreen and maximize are declined: the window keeps its tile.
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        surface.send_configure();
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        surface.send_configure();
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.update_text(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.update_text(&surface);
    }
}

impl State {
    /// Copies a mapped window's title and app_id into the model.
    fn update_text(&mut self, surface: &ToplevelSurface) {
        let Some(id) = self
            .mapped
            .iter()
            .find(|m| {
                m.window
                    .toplevel()
                    .is_some_and(|t| t.wl_surface() == surface.wl_surface())
            })
            .map(|m| m.id.clone())
        else {
            // Not mapped yet: the model reads both when it maps.
            return;
        };
        let (app_id, title) = toplevel_text(surface.wl_surface());
        self.engine.update(
            &id,
            WindowUpdate {
                app_id: Some(app_id),
                title: Some(title),
            },
        );
    }
}
