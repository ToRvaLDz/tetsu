//! Input from winit and libinput: keys to the focused surface, the pointer
//! to the surface under it, and a click focuses the window under it.
//!
//! Compositor binds: Ctrl+Alt+Backspace quits; Ctrl+Alt+F1..F12 switch VT
//! (tty only); the layout binds use Super on a VT and in tests, and
//! Ctrl+Alt nested, because the host compositor grabs Super.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent,
};
use smithay::desktop::{WindowSurfaceType, layer_map_for_output};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState, keysyms};
use smithay::input::pointer::{AxisFrame, ButtonEvent, GrabStartData, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::Layer as WlrLayer;

use crate::engine::Action as EngineAction;
use crate::state::State;

/// Linux evdev code of the left mouse button.
const BTN_LEFT: u32 = 0x110;
/// Wheel clicks are 120 units; a click scrolls this many pixels.
const PIXELS_PER_CLICK: f64 = 15.0;
const CLICK_UNITS: f64 = 120.0;

/// The modifier of the layout binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKey {
    /// Super (tty and headless).
    Super,
    /// Ctrl+Alt (nested: the host compositor grabs Super).
    CtrlAlt,
}

impl ModKey {
    fn held(self, modifiers: &ModifiersState) -> bool {
        match self {
            Self::Super => modifiers.logo && !modifiers.ctrl && !modifiers.alt,
            Self::CtrlAlt => modifiers.ctrl && modifiers.alt && !modifiers.logo,
        }
    }
}

/// What a compositor bind does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// Stop the compositor.
    Quit,
    /// Switch to this VT.
    SwitchVt(i32),
    /// An action of the layout engine.
    Engine(EngineAction),
}

/// The layout bind of a key (without the modifier check). Shifted letters
/// arrive upper-case; digits arrive unshifted, the glue passes the raw
/// keysyms of the key.
fn engine_bind(shift: bool, raw: u32) -> Option<EngineAction> {
    let lower = if (keysyms::KEY_A..=keysyms::KEY_Z).contains(&raw) {
        raw + (keysyms::KEY_a - keysyms::KEY_A)
    } else {
        raw
    };
    let digit = || {
        (keysyms::KEY_1..=keysyms::KEY_9)
            .contains(&lower)
            .then(|| u8::try_from(lower - keysyms::KEY_1 + 1).ok())
            .flatten()
    };
    match (shift, lower) {
        (false, keysyms::KEY_j) => Some(EngineAction::FocusNext),
        (false, keysyms::KEY_k) => Some(EngineAction::FocusPrev),
        (true, keysyms::KEY_j) => Some(EngineAction::SwapNext),
        (true, keysyms::KEY_k) => Some(EngineAction::SwapPrev),
        (true, keysyms::KEY_Return) => Some(EngineAction::PromoteMaster),
        (false, keysyms::KEY_h) => Some(EngineAction::RatioDelta(-1)),
        (false, keysyms::KEY_l) => Some(EngineAction::RatioDelta(1)),
        (false, keysyms::KEY_space) => Some(EngineAction::CycleLayout),
        (false, keysyms::KEY_f) => Some(EngineAction::ToggleFloating),
        (false, keysyms::KEY_q) => Some(EngineAction::Close),
        (false, _) => digit().map(EngineAction::FocusWorkspace),
        (true, _) => digit().map(EngineAction::MoveToWorkspace),
    }
}

/// The action bound to a key, if any.
pub fn bound_action(mod_key: ModKey, modifiers: &ModifiersState, keysym: Keysym) -> Option<Bound> {
    if modifiers.ctrl && modifiers.alt && keysym == Keysym::BackSpace {
        return Some(Bound::Quit);
    }
    // xkb turns Ctrl+Alt+F<n> into the XF86Switch_VT_<n> keysyms.
    let raw = keysym.raw();
    if (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&raw) {
        let vt = raw - keysyms::KEY_XF86Switch_VT_1 + 1;
        return i32::try_from(vt).ok().map(Bound::SwitchVt);
    }
    if !mod_key.held(modifiers) {
        return None;
    }
    engine_bind(modifiers.shift, raw).map(Bound::Engine)
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

    pub(crate) fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat
            .get_pointer()
            .map_or_else(Point::default, |p| p.current_location())
    }

    /// After an output went away: moves the pointer back inside the
    /// outputs that are left.
    pub(crate) fn keep_pointer_on_screen(&mut self) {
        let location = self.pointer_location();
        let clamped = self.clamp_to_outputs(location);
        if clamped != location {
            let time = u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);
            self.move_pointer(clamped, InputTime::from_millis(time));
        }
    }

    /// Keeps `location` inside the union of the outputs.
    pub(crate) fn clamp_to_outputs(&self, location: Point<f64, Logical>) -> Point<f64, Logical> {
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

    pub(crate) fn move_pointer(&mut self, location: Point<f64, Logical>, time: InputTime) {
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

    /// The surface under a point and its origin, for pointer focus: the
    /// overlay and top layers, then the windows, then the bottom and
    /// background layers.
    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(location).next().cloned();
        let on_layer = |layer: WlrLayer| {
            let output = output.as_ref()?;
            let geometry = self.space.output_geometry(output)?;
            let map = layer_map_for_output(output);
            let relative = location - geometry.loc.to_f64();
            let surface = map.layer_under(layer, relative)?;
            let placed = map.layer_geometry(surface)?;
            let (found, offset) =
                surface.surface_under(relative - placed.loc.to_f64(), WindowSurfaceType::ALL)?;
            Some((found, (offset + placed.loc + geometry.loc).to_f64()))
        };
        let on_window = || {
            let (window, origin) = self.space.element_under(location)?;
            let (surface, offset) =
                window.surface_under(location - origin.to_f64(), WindowSurfaceType::ALL)?;
            Some((surface, (offset + origin).to_f64()))
        };
        on_layer(WlrLayer::Overlay)
            .or_else(|| on_layer(WlrLayer::Top))
            .or_else(on_window)
            .or_else(|| on_layer(WlrLayer::Bottom))
            .or_else(|| on_layer(WlrLayer::Background))
    }

    fn on_key<I: InputBackend>(&mut self, event: &I::KeyboardKeyEvent) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let key_state = event.state();
        let mod_key = self.mod_key;
        let action = keyboard.input::<Bound, _>(
            self,
            event.key_code(),
            key_state,
            SERIAL_COUNTER.next_serial(),
            event.time(),
            |_, modifiers, handle| {
                // The raw syms of the key (digits unshifted), then the modified one
                // (xkb's VT keysyms only exist there).
                let bound = (key_state == KeyState::Pressed)
                    .then(|| {
                        handle
                            .raw_syms()
                            .iter()
                            .copied()
                            .chain([handle.modified_sym()])
                            .find_map(|sym| bound_action(mod_key, modifiers, sym))
                    })
                    .flatten();
                bound.map_or(FilterResult::Forward, FilterResult::Intercept)
            },
        );
        match action {
            Some(Bound::Quit) => self.loop_signal.stop(),
            Some(Bound::SwitchVt(vt)) => {
                if let Some(switch) = self.vt_switch.as_mut() {
                    switch(vt);
                }
            }
            Some(Bound::Engine(action)) => self.run_action(action),
            None => {}
        }
    }

    /// Whether the layout modifier is held right now.
    fn mod_held(&self) -> bool {
        self.seat
            .get_keyboard()
            .is_some_and(|k| self.mod_key.held(&k.modifier_state()))
    }

    /// Runs a layout bind and follows it: layout, focus, close request.
    fn run_action(&mut self, action: EngineAction) {
        let effect = self.engine.apply(action, &self.viewports());
        if effect.relayout {
            self.relayout();
        }
        self.sync_focus();
        let closing = effect.close.and_then(|id| {
            self.mapped
                .iter()
                .find(|m| m.id == id)
                .and_then(|m| m.window.toplevel().cloned())
        });
        if let Some(toplevel) = closing {
            toplevel.send_close();
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
            if let Some(id) = id.as_deref()
                && button == BTN_LEFT
                && self.mod_held()
            {
                let start_data = GrabStartData {
                    focus: None,
                    button,
                    location: pointer.current_location(),
                };
                self.start_move_grab(id, start_data, SERIAL_COUNTER.next_serial());
                if pointer.is_grabbed() {
                    return;
                }
            }
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
