//! The Wayland side: Smithay client toolkit handlers for the popup and panel
//! layer surfaces, the pointer, and the one key the panel takes (Esc).

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle, WEnum};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

use super::Ui;

/// `BTN_LEFT` from linux/input-event-codes.h.
const BTN_LEFT: u32 = 0x110;
/// `KEY_ESC` from linux/input-event-codes.h: `wl_keyboard` sends evdev codes.
const KEY_ESC: u32 = 1;

/// The Wayland globals the surfaces need.
#[derive(Debug)]
pub struct Globals {
    pub registry: RegistryState,
    pub seats: SeatState,
    pub outputs: OutputState,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub keyboard: Option<wl_keyboard::WlKeyboard>,
}

impl CompositorHandler for Ui {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        self.set_scale(surface, factor, qh);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Ui {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.globals.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Ui {
    fn closed(&mut self, _: &Connection, qh: &QueueHandle<Self>, layer: &LayerSurface) {
        self.surface_closed(layer, qh);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.configured(layer, configure.new_size, qh);
    }
}

impl SeatHandler for Ui {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.globals.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.globals.pointer.is_none() {
            self.globals.pointer = self.globals.seats.get_pointer(qh, &seat).ok();
        }
        if capability == Capability::Keyboard && self.globals.keyboard.is_none() {
            self.globals.keyboard = Some(seat.get_keyboard(qh, EscOnly));
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(pointer) = self.globals.pointer.take()
        {
            pointer.release();
        }
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.globals.keyboard.take()
        {
            keyboard.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for Ui {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            match &event.kind {
                PointerEventKind::Press { button, .. } if *button == BTN_LEFT => {
                    self.press(&event.surface, x, y, qh);
                }
                PointerEventKind::Axis { vertical, .. } => {
                    self.scroll(&event.surface, vertical.absolute as f32, qh);
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for Ui {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.globals.shm
    }
}

/// The keyboard: the panel closes on Esc, every other key is ignored. No
/// keymap is loaded since no text is ever typed.
#[derive(Debug)]
pub struct EscOnly;

impl Dispatch2<wl_keyboard::WlKeyboard, Ui> for EscOnly {
    fn event(
        &self,
        ui: &mut Ui,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &Connection,
        qh: &QueueHandle<Ui>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => ui.keyboard_focus = Some(surface.id()),
            wl_keyboard::Event::Leave { .. } => ui.keyboard_focus = None,
            wl_keyboard::Event::Key {
                key: KEY_ESC,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => ui.escape(qh),
            _ => {}
        }
    }
}

impl ProvidesRegistryState for Ui {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.globals.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(Ui);
delegate_dispatch2!(Ui);
