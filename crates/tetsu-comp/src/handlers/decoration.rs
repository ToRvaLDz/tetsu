//! Window decorations: the compositor always answers server-side, on
//! `xdg-decoration` and on the KDE protocol (GTK3 and Qt). On
//! `xdg-decoration` the mode is only ever sent inside a configure; on KDE
//! the manager sends `Server` as the default mode on bind, then each
//! decoration object gets one `mode` event.

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgMode;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::WEnum;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::kde::decoration::KdeDecorationHandler;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
    Mode as KdeMode, OrgKdeKwinServerDecoration,
};

use crate::state::State;

/// Marks the mode in the pending state; after the first configure the
/// protocol owes the client an `xdg_surface.configure`, which Smithay skips
/// when the mode did not change.
fn answer_server_side(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(XdgMode::ServerSide));
    if toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
    }
}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Before the first commit the mode rides on the initial configure;
        // a decoration created later is answered at once.
        answer_server_side(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: XdgMode) {
        answer_server_side(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        answer_server_side(&toplevel);
    }
}

impl KdeDecorationHandler for State {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.decorations.kde
    }

    /// One answer per decoration object, whatever was asked: a client that
    /// asks again gets nothing, which ends any ping-pong.
    fn request_mode(
        &mut self,
        _surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        _mode: WEnum<KdeMode>,
    ) {
        let dh = &self.display_handle;
        let alive = |id: &ObjectId| dh.object_info(id.clone()).is_ok();
        if self.decorations.answered.first(decoration.id(), alive) {
            decoration.mode(KdeMode::Server);
        }
    }

    fn release(&mut self, decoration: &OrgKdeKwinServerDecoration, _surface: &WlSurface) {
        self.decorations.answered.forget(&decoration.id());
    }
}
