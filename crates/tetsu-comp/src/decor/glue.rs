//! The Smithay side of the decorations: the protocol states.

use std::collections::HashSet;

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;

/// The two decoration protocols and what the KDE one has answered.
pub struct DecorStates {
    /// `zxdg_decoration_manager_v1`; held to keep the global alive.
    pub xdg: XdgDecorationState,
    /// `org_kde_kwin_server_decoration_manager`, default mode Server.
    pub kde: KdeDecorationState,
    /// The KDE decoration objects that already got their one answer.
    pub answered: HashSet<ObjectId>,
}
