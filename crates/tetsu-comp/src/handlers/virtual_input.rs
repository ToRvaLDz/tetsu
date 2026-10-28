//! `zwp_virtual_keyboard_v1` and `zwlr_virtual_pointer_v1`. The globals are
//! visible only to `Trust::Input` clients (see `State::new`); what they send
//! goes through the same paths as real input.
//!
//! A virtual keyboard's own keymap is not activated: its keycodes are read
//! with the seat's keymap, and its `modifiers` requests are ignored.

use smithay::backend::input::{InputEvent, KeyboardKeyEvent};
use smithay::input::keyboard::FilterResult;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::virtual_keyboard::{VirtualKeyboardBackend, VirtualKeyboardHandler};
use smithay::wayland::virtual_pointer::{VirtualPointerBackend, VirtualPointerHandler};

use crate::state::State;

impl VirtualKeyboardHandler for State {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        self.note_activity();
        let InputEvent::Keyboard { event } = event else {
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let device = smithay::backend::input::Event::device(&event);
        // Never a compositor bind: a virtual key goes to the focus.
        keyboard.input_from_source::<(), _>(
            device.source(),
            self,
            event.key_code(),
            event.state(),
            SERIAL_COUNTER.next_serial(),
            smithay::backend::input::Event::time(&event),
            |_, _, _| FilterResult::Forward,
        );
    }
}

impl VirtualPointerHandler for State {
    fn process_virtual_pointer_event(&mut self, event: InputEvent<VirtualPointerBackend>) {
        self.process_input_event(event);
    }
}
