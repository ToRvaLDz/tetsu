//! X11 for the tests: whether xwayland-satellite and Xwayland are around,
//! and a probe window made with x11rb.
#![allow(dead_code)]

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// What a skipped test prints.
pub const SKIPPED: &str = "skipped: xwayland-satellite or Xwayland not on PATH";

/// The first X11 connection starts satellite and Xwayland: allow for it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether `program` is an executable file in a `PATH` directory.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Whether the X11 tests can run. When they cannot, prints `SKIPPED` and
/// returns false; with `TETSU_REQUIRE_XWAYLAND=1` it fails instead.
pub fn require() -> bool {
    if on_path("xwayland-satellite") && on_path("Xwayland") {
        return true;
    }
    if std::env::var_os("TETSU_REQUIRE_XWAYLAND").is_some_and(|v| v == "1") {
        panic!("TETSU_REQUIRE_XWAYLAND=1 but {SKIPPED}");
    }
    eprintln!("{SKIPPED}");
    false
}

/// Connects to `display`, giving up after `CONNECT_TIMEOUT`.
pub fn connect(display: &str) -> Result<RustConnection, String> {
    let (sender, receiver) = mpsc::channel();
    let name = display.to_owned();
    std::thread::spawn(move || {
        let result = RustConnection::connect(Some(&name))
            .map(|(conn, _)| conn)
            .map_err(|e| e.to_string());
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(CONNECT_TIMEOUT)
        .map_err(|_| format!("no X11 connection to {display} in {CONNECT_TIMEOUT:?}"))?
}

/// A mapped X11 window and its connection.
pub struct ProbeWindow {
    pub conn: RustConnection,
    pub window: Window,
}

impl ProbeWindow {
    /// Destroys the window.
    pub fn destroy(self) {
        self.conn.destroy_window(self.window).unwrap();
        self.conn.flush().unwrap();
    }
}

/// Maps a 200x150 window on `display` with WM_CLASS `class` and WM_NAME
/// `title`.
pub fn probe_window(display: &str, class: &str, title: &str) -> ProbeWindow {
    let conn = connect(display).unwrap();
    probe_window_on(conn, class, title)
}

/// Like `probe_window`, on a connection already made.
pub fn probe_window_on(conn: RustConnection, class: &str, title: &str) -> ProbeWindow {
    let screen = &conn.setup().roots[0];
    let (root, visual, pixel) = (screen.root, screen.root_visual, screen.white_pixel);
    let window = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        200,
        150,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux::new().background_pixel(pixel),
    )
    .unwrap();
    let wm_class = format!("{class}\0{class}\0");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        wm_class.as_bytes(),
    )
    .unwrap();
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.flush().unwrap();
    ProbeWindow { conn, window }
}
