//! An `xdg-decoration` object created after the toplevel's first configure,
//! on its own event queue next to a `WlClient`: sctk only decorates a window
//! as it creates it.
#![allow(dead_code)]

use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::wl_registry::WlRegistry;
use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use smithay_client_toolkit::reexports::protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
};

use crate::wayland::{Toplevel, WlClient};

pub use zxdg_toplevel_decoration_v1::Mode;

/// The modes the late decorations were configured with, by surface.
#[derive(Default)]
struct Seen {
    modes: Vec<(ObjectId, WEnum<Mode>)>,
}

impl Dispatch<WlRegistry, GlobalListContents> for Seen {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for Seen {
    fn event(
        _: &mut Self,
        _: &ZxdgDecorationManagerV1,
        _: <ZxdgDecorationManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// The decoration's user data: the surface it decorates.
impl Dispatch<ZxdgToplevelDecorationV1, ObjectId> for Seen {
    fn event(
        seen: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            seen.modes.push((surface.clone(), mode));
        }
    }
}

/// Late decorations for the toplevels of one `WlClient`, on their own queue.
pub struct LateDecorations {
    queue: EventQueue<Seen>,
    seen: Seen,
    manager: ZxdgDecorationManagerV1,
}

impl LateDecorations {
    /// Binds the decoration manager on `wl`'s connection.
    pub fn bind(wl: &WlClient) -> Self {
        let (globals, mut queue) = registry_queue_init::<Seen>(wl.connection()).unwrap();
        let manager = globals
            .bind::<ZxdgDecorationManagerV1, _, _>(&queue.handle(), 1..=1, ())
            .expect("no xdg decoration");
        let mut seen = Seen::default();
        queue.roundtrip(&mut seen).unwrap();
        Self {
            queue,
            seen,
            manager,
        }
    }

    /// Creates the toplevel's decoration now, after its first configure.
    pub fn decorate(&mut self, top: &Toplevel) {
        let qh = self.queue.handle();
        self.manager
            .get_toplevel_decoration(top.xdg_toplevel(), &qh, top.surface_id());
        self.queue.roundtrip(&mut self.seen).unwrap();
    }

    /// The modes the toplevel's late decoration was configured with.
    pub fn modes(&mut self, top: &Toplevel) -> Vec<WEnum<Mode>> {
        self.queue.roundtrip(&mut self.seen).unwrap();
        let id = top.surface_id();
        let seen = self.seen.modes.iter();
        seen.filter(|(s, _)| *s == id).map(|(_, m)| *m).collect()
    }
}
