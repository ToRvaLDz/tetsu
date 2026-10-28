//! An `ext-idle-notify-v1` client for the end-to-end tests: it binds the
//! seat and the notifier and records `idled` and `resumed` per notification.
#![allow(dead_code)]

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::wl_registry::WlRegistry;
use smithay_client_toolkit::reexports::client::protocol::wl_seat::{self, WlSeat};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle};
use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// The longest wait for an event.
const TIMEOUT: Duration = Duration::from_secs(10);

/// One event of a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    Idled,
    Resumed,
}

/// A notification this client made, by its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note(usize);

#[derive(Default)]
struct App {
    events: Vec<(usize, Seen)>,
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// The notification's user data: its index.
impl Dispatch<ExtIdleNotificationV1, usize> for App {
    fn event(
        app: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        at: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let seen = match event {
            ext_idle_notification_v1::Event::Idled => Seen::Idled,
            ext_idle_notification_v1::Event::Resumed => Seen::Resumed,
            _ => return,
        };
        app.events.push((*at, seen));
    }
}

/// One connection with the seat and the idle notifier bound.
pub struct IdleClient {
    queue: EventQueue<App>,
    app: App,
    seat: WlSeat,
    notifier: ExtIdleNotifierV1,
    notifications: Vec<ExtIdleNotificationV1>,
}

impl IdleClient {
    /// Connects to the Wayland socket at `socket` and binds notifier v2.
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let seat = globals
            .bind::<WlSeat, _, _>(&qh, 1..=1, ())
            .expect("no wl_seat");
        let notifier = globals
            .bind::<ExtIdleNotifierV1, _, _>(&qh, 2..=2, ())
            .expect("no ext_idle_notifier_v1 v2");
        let mut app = App::default();
        queue.roundtrip(&mut app).unwrap();
        Self {
            queue,
            app,
            seat,
            notifier,
            notifications: Vec::new(),
        }
    }

    /// A notification after `timeout_ms` of idle (`get_idle_notification`).
    pub fn notification(&mut self, timeout_ms: u32) -> Note {
        let at = self.notifications.len();
        let qh = self.queue.handle();
        let n = self
            .notifier
            .get_idle_notification(timeout_ms, &self.seat, &qh, at);
        self.notifications.push(n);
        self.queue.roundtrip(&mut self.app).unwrap();
        Note(at)
    }

    /// The same, blind to inhibitors (`get_input_idle_notification`, v2).
    pub fn input_notification(&mut self, timeout_ms: u32) -> Note {
        let at = self.notifications.len();
        let qh = self.queue.handle();
        let n = self
            .notifier
            .get_input_idle_notification(timeout_ms, &self.seat, &qh, at);
        self.notifications.push(n);
        self.queue.roundtrip(&mut self.app).unwrap();
        Note(at)
    }

    /// The events of `note` so far.
    pub fn events(&mut self, note: Note) -> Vec<Seen> {
        self.queue.roundtrip(&mut self.app).unwrap();
        self.seen(note)
    }

    fn seen(&self, note: Note) -> Vec<Seen> {
        let events = self.app.events.iter();
        events
            .filter(|(at, _)| *at == note.0)
            .map(|(_, s)| *s)
            .collect()
    }

    /// Dispatches until `note` has as many events as `expected`, then
    /// checks they are `expected`.
    pub fn wait_for(&mut self, note: Note, expected: &[Seen]) {
        let start = Instant::now();
        while self.seen(note).len() < expected.len() {
            assert!(
                start.elapsed() < TIMEOUT,
                "timed out waiting for {expected:?}, got {:?}",
                self.seen(note)
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(self.seen(note), expected);
    }

    /// Round-trips for `span`, so a late event would show.
    pub fn settle(&mut self, span: Duration) {
        let start = Instant::now();
        while start.elapsed() < span {
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
