//! Input from winit and libinput: keys to the focused surface, the pointer
//! to the surface under it, and a click focuses the window under it.
//!
//! Compositor binds: Ctrl+Alt+Backspace quits; Ctrl+Alt+F1..F12 switch VT
//! (tty only). Super combos are avoided, the host compositor grabs them.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent,
};
use smithay::desktop::WindowSurfaceType;
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState, keysyms};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::state::State;

/// Wheel clicks are 120 units; a click scrolls this many pixels.
const PIXELS_PER_CLICK: f64 = 15.0;
const CLICK_UNITS: f64 = 120.0;

/// What a compositor bind does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Stop the compositor.
    Quit,
    /// Switch to this VT.
    SwitchVt(i32),
}

/// The action bound to a key, if any.
pub fn bound_action(modifiers: &ModifiersState, keysym: Keysym) -> Option<Action> {
    if modifiers.ctrl && modifiers.alt && keysym == Keysym::BackSpace {
        return Some(Action::Quit);
    }
    // xkb turns Ctrl+Alt+F<n> into the XF86Switch_VT_<n> keysyms.
    let raw = keysym.raw();
    if (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&raw) {
        let vt = raw - keysyms::KEY_XF86Switch_VT_1 + 1;
        return i32::try_from(vt).ok().map(Action::SwitchVt);
    }
    None
}

impl State {
    /// Handles one input event from any backend.
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => self.on_key::<I>(&event),
            InputEvent::PointerMotion { event } => {
                let location = self.clamp_to_outputs(self.pointer_location() + event.delta());
                self.move_pointer(location, event.time());
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let Some(geometry) = self
                    .space
                    .outputs()
                    .next()
                    .and_then(|o| self.space.output_geometry(o))
                else {
                    return;
                };
                let location = event.position_transformed(geometry.size) + geometry.loc.to_f64();
                self.move_pointer(location, event.time());
            }
            InputEvent::PointerButton { event } => {
                self.on_button(event.button_code(), event.state(), event.time());
            }
            InputEvent::PointerAxis { event } => self.on_axis::<I>(&event),
            _ => {}
        }
    }

    fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat
            .get_pointer()
            .map_or_else(Point::default, |p| p.current_location())
    }

    /// Keeps `location` inside the union of the outputs.
    fn clamp_to_outputs(&self, location: Point<f64, Logical>) -> Point<f64, Logical> {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for geometry in self
            .space
            .outputs()
            .filter_map(|o| self.space.output_geometry(o))
        {
            let (x0, y0) = (f64::from(geometry.loc.x), f64::from(geometry.loc.y));
            let (x1, y1) = (
                x0 + f64::from(geometry.size.w),
                y0 + f64::from(geometry.size.h),
            );
            bounds = Some(match bounds {
                None => (x0, y0, x1, y1),
                Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
            });
        }
        match bounds {
            Some((x0, y0, x1, y1)) => (
                location.x.clamp(x0, (x1 - 1.0).max(x0)),
                location.y.clamp(y0, (y1 - 1.0).max(y0)),
            )
                .into(),
            None => location,
        }
    }

    fn move_pointer(&mut self, location: Point<f64, Logical>, time: InputTime) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    /// The surface under a point and its origin, for pointer focus.
    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (window, origin) = self.space.element_under(location)?;
        let (surface, offset) =
            window.surface_under(location - origin.to_f64(), WindowSurfaceType::ALL)?;
        Some((surface, (offset + origin).to_f64()))
    }

    fn on_key<I: InputBackend>(&mut self, event: &I::KeyboardKeyEvent) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let key_state = event.state();
        let action = keyboard.input::<Action, _>(
            self,
            event.key_code(),
            key_state,
            SERIAL_COUNTER.next_serial(),
            event.time(),
            |_, modifiers, handle| {
                let bound = (key_state == KeyState::Pressed)
                    .then(|| bound_action(modifiers, handle.modified_sym()))
                    .flatten();
                bound.map_or(FilterResult::Forward, FilterResult::Intercept)
            },
        );
        match action {
            Some(Action::Quit) => self.loop_signal.stop(),
            Some(Action::SwitchVt(vt)) => {
                if let Some(switch) = self.vt_switch.as_mut() {
                    switch(vt);
                }
            }
            None => {}
        }
    }

    fn on_button(&mut self, button: u32, button_state: ButtonState, time: InputTime) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
            let id = self
                .space
                .element_under(pointer.current_location())
                .and_then(|(window, _)| self.mapped.iter().find(|m| m.window == *window))
                .map(|m| m.id.clone());
            self.focus_window(id.as_deref());
        }
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: button_state,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    fn on_axis<I: InputBackend>(&mut self, event: &I::PointerAxisEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let amount = |axis| {
            event.amount(axis).unwrap_or_else(|| {
                event.amount_v120(axis).unwrap_or(0.0) * PIXELS_PER_CLICK / CLICK_UNITS
            })
        };
        let source = event.source();
        let mut frame = AxisFrame::new(event.time()).source(source);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let value = amount(axis);
            if value != 0.0 {
                frame = frame.value(axis, value);
                if let Some(discrete) = event.amount_v120(axis) {
                    frame = frame.v120(axis, discrete as i32);
                }
            }
            if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                frame = frame.stop(axis);
            }
        }
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}
