//! Trust, end to end and headless: the virtual input protocols are visible
//! and bindable only to clients the compositor started itself.

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use std::path::Path;

use common::comp::Comp;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;

const ROLE: &str = "TETSU_COMP_TEST_ROLE";
const OUT: &str = "TETSU_COMP_TEST_OUT";
const KEYBOARD: &str = "zwp_virtual_keyboard_manager_v1";
const POINTER: &str = "zwlr_virtual_pointer_manager_v1";

/// A client state that only watches the registry.
struct Probe;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// The helper the compositor starts: it writes its globals to the file in
/// `TETSU_COMP_TEST_OUT`. It is a test of this binary, run on purpose by
/// `trusted_helper_sees_both_globals`; alone it returns at once.
#[test]
#[ignore = "run by the compositor under test"]
fn helper_trusted() {
    if std::env::var(ROLE).as_deref() != Ok("trusted") {
        return;
    }
    let out = std::env::var(OUT).expect("TETSU_COMP_TEST_OUT");
    // WAYLAND_SOCKET first, as every Wayland client does.
    let conn = Connection::connect_to_env().expect("connect_to_env");
    let (globals, _queue) = registry_queue_init::<Probe>(&conn).expect("registry");
    let names: Vec<String> = globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|g| g.interface)
        .collect();
    std::fs::write(out, names.join("\n")).unwrap();
}

fn config_for(helper_args: &[&str]) -> String {
    let exe = std::env::current_exe().unwrap();
    let mut argv = vec![exe.to_string_lossy().into_owned()];
    argv.extend(helper_args.iter().map(|a| (*a).to_owned()));
    let quoted: Vec<String> = argv.iter().map(|a| format!("{a:?}")).collect();
    format!("[trust]\ninput = [[{}]]\n", quoted.join(", "))
}

fn wait_for_file(path: &Path) -> String {
    let start = std::time::Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "the helper wrote nothing"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn an_untrusted_client_sees_neither_virtual_input_manager() {
    let comp = Comp::start();
    let globals = wayland::globals(&comp.wayland_socket());
    assert!(!globals.iter().any(|g| g == KEYBOARD), "{globals:?}");
    assert!(!globals.iter().any(|g| g == POINTER), "{globals:?}");
}

#[test]
fn guessed_global_names_never_bind_a_virtual_input_manager() {
    let comp = Comp::start();
    for name in 1..=64u32 {
        for (interface, keyboard) in [(KEYBOARD, true), (POINTER, false)] {
            let stream = std::os::unix::net::UnixStream::connect(comp.wayland_socket()).unwrap();
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
            let qh = queue.handle();
            if keyboard {
                let _ = globals
                    .registry()
                    .bind::<ZwpVirtualKeyboardManagerV1, _, _>(name, 1, &qh, ());
            } else {
                let _ = globals
                    .registry()
                    .bind::<ZwlrVirtualPointerManagerV1, _, _>(name, 1, &qh, ());
            }
            // A bind of a global that is not listed for this client is a
            // protocol error, whatever the name: the connection dies.
            let result = queue.roundtrip(&mut Probe);
            assert!(
                result.is_err(),
                "{interface} bound as global {name}: {result:?}"
            );
        }
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[test]
fn trusted_helper_sees_both_globals() {
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("globals");
    let config = config_for(&["--exact", "helper_trusted", "--ignored", "--nocapture"]);
    let comp = Comp::start_with_env(
        &[],
        Some(&config),
        &[(ROLE, "trusted"), (OUT, file.to_str().unwrap())],
    );
    let globals = wait_for_file(&file);
    assert!(globals.lines().any(|g| g == KEYBOARD), "{globals}");
    assert!(globals.lines().any(|g| g == POINTER), "{globals}");
    // The same compositor still hides them from everyone else.
    let others = wayland::globals(&comp.wayland_socket());
    assert!(!others.iter().any(|g| g == KEYBOARD), "{others:?}");
    assert!(!others.iter().any(|g| g == POINTER), "{others:?}");
}
