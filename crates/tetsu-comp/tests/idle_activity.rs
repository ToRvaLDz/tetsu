//! Which input events count as user activity for idle: input from the
//! user does, a device that comes or goes and a switch (a lid) do not.
//! The events come from a test backend, so no seat or device is needed.

use std::path::PathBuf;

use smithay::backend::input::Switch;
use smithay::backend::input::{
    Device, DeviceCapability, Event, InputBackend, InputEvent, InputTime, KeyState,
    KeyboardKeyEvent, SwitchState, SwitchToggleEvent, UnusedEvent,
};
use smithay::input::keyboard::Keycode;
use tetsu_comp::idle::is_activity;

/// A backend whose devices, keys and switches the test makes.
#[derive(Debug)]
struct Fake;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FakeDevice;

impl Device for FakeDevice {
    fn id(&self) -> String {
        "fake".to_owned()
    }
    fn name(&self) -> String {
        "fake".to_owned()
    }
    fn has_capability(&self, _: DeviceCapability) -> bool {
        true
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

struct FakeKey;

impl Event<Fake> for FakeKey {
    fn time(&self) -> InputTime {
        InputTime::from_millis(0)
    }
    fn device(&self) -> FakeDevice {
        FakeDevice
    }
}

impl KeyboardKeyEvent<Fake> for FakeKey {
    fn key_code(&self) -> Keycode {
        Keycode::new(38)
    }
    fn state(&self) -> KeyState {
        KeyState::Pressed
    }
    fn count(&self) -> u32 {
        1
    }
}

/// A lid closing.
struct FakeLid;

impl Event<Fake> for FakeLid {
    fn time(&self) -> InputTime {
        InputTime::from_millis(0)
    }
    fn device(&self) -> FakeDevice {
        FakeDevice
    }
}

impl SwitchToggleEvent<Fake> for FakeLid {
    fn switch(&self) -> Option<Switch> {
        Some(Switch::Lid)
    }
    fn state(&self) -> SwitchState {
        SwitchState::On
    }
}

impl InputBackend for Fake {
    type Device = FakeDevice;
    type KeyboardKeyEvent = FakeKey;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = UnusedEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = FakeLid;
    type SpecialEvent = ();
}

#[test]
fn a_key_is_activity() {
    assert!(is_activity(&InputEvent::<Fake>::Keyboard {
        event: FakeKey
    }));
}

#[test]
fn a_device_added_or_removed_is_not_activity() {
    let added = InputEvent::<Fake>::DeviceAdded { device: FakeDevice };
    let removed = InputEvent::<Fake>::DeviceRemoved { device: FakeDevice };
    assert!(!is_activity(&added), "a device replug re-armed idle");
    assert!(!is_activity(&removed), "a device unplug re-armed idle");
}

#[test]
fn a_lid_switch_is_not_activity() {
    let lid = InputEvent::<Fake>::SwitchToggle { event: FakeLid };
    assert!(!is_activity(&lid), "a lid close re-armed idle");
}
