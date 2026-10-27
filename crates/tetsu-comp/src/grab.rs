//! The pointer grab that moves a floating window: started by Mod+left
//! press on it or by its xdg `move_request`, ended by the release of the
//! button.

use smithay::desktop::Window;
use smithay::input::SeatHandler;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::utils::{IsAlive, Logical, Point, Serial};

use crate::layout::Rect;
use crate::state::State;

/// Moves one floating window with the pointer.
pub struct MoveGrab {
    start_data: GrabStartData<State>,
    id: String,
    window: Window,
    /// Where the window was when the grab began.
    initial: Point<i32, Logical>,
    viewport: Rect,
}

impl MoveGrab {
    fn new(
        start_data: GrabStartData<State>,
        id: String,
        window: Window,
        initial: Point<i32, Logical>,
        viewport: Rect,
    ) -> Self {
        Self {
            start_data,
            id,
            window,
            initial,
            viewport,
        }
    }
}

impl State {
    /// Starts moving the floating window `id`, if it floats and is mapped.
    pub(crate) fn start_move_grab(
        &mut self,
        id: &str,
        start_data: GrabStartData<State>,
        serial: Serial,
    ) {
        // Locked: no client starts a grab, whatever serial it presents.
        if self.lock.is_locked() {
            return;
        }
        if !self.engine.is_floating(id) {
            return;
        }
        let Some(window) = self
            .mapped
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.window.clone())
        else {
            return;
        };
        let Some(initial) = self.space.element_location(&window) else {
            return;
        };
        let viewport = self
            .engine
            .model()
            .workspace_of(id)
            .and_then(|ws| self.engine.model().output_of(ws))
            .and_then(|out| {
                self.viewports()
                    .into_iter()
                    .find(|(o, _)| o == out)
                    .map(|(_, rect)| rect)
            });
        let Some(viewport) = viewport else { return };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        self.focus_window(Some(id));
        let grab = MoveGrab::new(start_data, id.to_owned(), window, initial, viewport);
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}

impl PointerGrab<State> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // No client has the pointer while the window moves.
        handle.motion(data, None, event);
        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let target = self.initial.to_f64() + (event.location - self.start_data.location);
        let target = target.to_i32_round::<i32>();
        let moved = data
            .engine
            .move_floating(&self.id, (target.x, target.y), self.viewport);
        // Senza rect la finestra non è più spostabile (nascosta, mossa): fine grab.
        let Some(rect) = moved else {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        data.space
            .map_element(self.window.clone(), (rect.x, rect.y), false);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut State) {}
}
