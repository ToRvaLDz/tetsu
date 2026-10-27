//! A raw `org_kde_kwin_server_decoration` client, as GTK3 and Qt are.
#![allow(dead_code)]

use std::os::unix::net::UnixStream;
use std::path::Path;

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_misc::server_decoration::client::org_kde_kwin_server_decoration::{
    self as decoration, OrgKdeKwinServerDecoration,
};
use wayland_protocols_misc::server_decoration::client::org_kde_kwin_server_decoration_manager::{
    self as manager, OrgKdeKwinServerDecorationManager,
};

pub use decoration::Mode;

#[derive(Default)]
struct Seen {
    default_mode: Option<WEnum<manager::Mode>>,
    modes: Vec<WEnum<Mode>>,
}

impl Dispatch<WlRegistry, GlobalListContents> for Seen {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCompositor, ()> for Seen {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: <WlCompositor as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for Seen {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: <WlSurface as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<OrgKdeKwinServerDecorationManager, ()> for Seen {
    fn event(
        seen: &mut Self,
        _: &OrgKdeKwinServerDecorationManager,
        event: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let manager::Event::DefaultMode { mode } = event {
            seen.default_mode = Some(mode);
        }
    }
}

impl Dispatch<OrgKdeKwinServerDecoration, ()> for Seen {
    fn event(
        seen: &mut Self,
        _: &OrgKdeKwinServerDecoration,
        event: decoration::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let decoration::Event::Mode { mode } = event {
            seen.modes.push(mode);
        }
    }
}

/// One connection that binds the KDE manager.
pub struct KdeClient {
    _conn: Connection,
    queue: EventQueue<Seen>,
    seen: Seen,
    globals: GlobalList,
    manager: OrgKdeKwinServerDecorationManager,
    surfaces: Vec<WlSurface>,
}

impl KdeClient {
    pub fn connect(socket: &Path) -> Self {
        let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<Seen>(&conn).unwrap();
        let qh = queue.handle();
        let manager = globals
            .bind::<OrgKdeKwinServerDecorationManager, _, _>(&qh, 1..=1, ())
            .expect("org_kde_kwin_server_decoration_manager is not advertised");
        let mut seen = Seen::default();
        queue.roundtrip(&mut seen).unwrap();
        Self {
            _conn: conn,
            queue,
            seen,
            globals,
            manager,
            surfaces: Vec::new(),
        }
    }

    /// The mode the manager announced on bind.
    pub fn default_mode(&self) -> Option<WEnum<manager::Mode>> {
        self.seen.default_mode
    }

    /// Creates a decoration for a new surface, asks for `Client` and returns
    /// every mode the compositor answered with.
    pub fn ask_for_client_side(&mut self) -> Vec<WEnum<Mode>> {
        let qh = self.queue.handle();
        let compositor = self
            .globals
            .bind::<WlCompositor, _, _>(&qh, 1..=1, ())
            .unwrap();
        let surface = compositor.create_surface(&qh, ());
        let decoration = self.manager.create(&surface, &qh, ());
        self.surfaces.push(surface);
        self.queue.roundtrip(&mut self.seen).unwrap();
        decoration.request_mode(Mode::Client);
        self.queue.roundtrip(&mut self.seen).unwrap();
        self.queue.roundtrip(&mut self.seen).unwrap();
        std::mem::take(&mut self.seen.modes)
    }
}
