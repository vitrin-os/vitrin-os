// SPDX-License-Identifier: MPL-2.0
//! A synthetic Smithay [`InputBackend`]: winit cannot run in CI, so the
//! generic [`super::intake_physical`] is driven with handcrafted host events
//! through the same `InputBackend` trait surface `WinitInput` provides.
//!
//! Compiled for `cargo test` **and** for a `physical-input-injector` build,
//! and that is the whole reason it is a module rather than a fixture inside
//! `super::tests`. Issue #212 asks for a test seam that feeds
//! `intake_physical`'s exact entry point and "never a second, weaker path";
//! the only way to reach that entry point without a winit event pump is to
//! hand it host events, so the injector needs the very types the unit tests
//! already hand it. Two copies would be two translations that could drift,
//! and the one that drifted would be the one no unit test covers.
//!
//! Nothing here mints an origin. `intake_physical` does that, in
//! [`super`], under [`super::SeatInput::physical`]'s module privacy —
//! unchanged, and deliberately so.

use smithay::backend::input::{
    AbsolutePositionEvent, AxisRelativeDirection, AxisSource, Device, DeviceCapability, Event,
    InputBackend, KeyboardKeyEvent, Keycode, PointerAxisEvent, PointerButtonEvent, UnusedEvent,
};

use smithay::backend::input as host;

use super::XKB_KEYCODE_OFFSET;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntheticDevice;

impl Device for SyntheticDevice {
    fn id(&self) -> String {
        "synthetic".into()
    }
    fn name(&self) -> String {
        "synthetic host device".into()
    }
    fn has_capability(&self, _capability: DeviceCapability) -> bool {
        true
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// Absolute pointer motion already in target coordinates (the
/// transform is the identity, mirroring how the nested window's
/// physical pixels are the view's pixels).
pub(crate) struct SyntheticMotion {
    pub x: f64,
    pub y: f64,
}

impl Event<SyntheticHost> for SyntheticMotion {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl AbsolutePositionEvent<SyntheticHost> for SyntheticMotion {
    fn x(&self) -> f64 {
        self.x
    }
    fn y(&self) -> f64 {
        self.y
    }
    fn x_transformed(&self, _width: i32) -> f64 {
        self.x
    }
    fn y_transformed(&self, _height: i32) -> f64 {
        self.y
    }
}

impl smithay::backend::input::PointerMotionAbsoluteEvent<SyntheticHost> for SyntheticMotion {}

pub(crate) struct SyntheticButton {
    pub code: u32,
    pub state: host::ButtonState,
}

impl Event<SyntheticHost> for SyntheticButton {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl PointerButtonEvent<SyntheticHost> for SyntheticButton {
    fn button_code(&self) -> u32 {
        self.code
    }
    fn state(&self) -> host::ButtonState {
        self.state
    }
}

/// Scroll with independent per-axis discrete (`v120`) and continuous
/// (pixel) amounts, `(vertical, horizontal)`.
pub(crate) struct SyntheticScroll {
    pub v120: (Option<f64>, Option<f64>),
    pub pixels: (Option<f64>, Option<f64>),
}

impl Event<SyntheticHost> for SyntheticScroll {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl PointerAxisEvent<SyntheticHost> for SyntheticScroll {
    fn amount(&self, axis: host::Axis) -> Option<f64> {
        match axis {
            host::Axis::Vertical => self.pixels.0,
            host::Axis::Horizontal => self.pixels.1,
        }
    }
    fn amount_v120(&self, axis: host::Axis) -> Option<f64> {
        match axis {
            host::Axis::Vertical => self.v120.0,
            host::Axis::Horizontal => self.v120.1,
        }
    }
    fn source(&self) -> AxisSource {
        if self.v120.0.is_some() || self.v120.1.is_some() {
            AxisSource::Wheel
        } else {
            AxisSource::Continuous
        }
    }
    fn relative_direction(&self, _axis: host::Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

pub(crate) struct SyntheticKey {
    pub evdev: u32,
    pub state: host::KeyState,
}

impl Event<SyntheticHost> for SyntheticKey {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl KeyboardKeyEvent<SyntheticHost> for SyntheticKey {
    fn key_code(&self) -> Keycode {
        // The xkb-domain keycode, exactly as Smithay's winit backend
        // produces it (evdev scancode + 8).
        (self.evdev + XKB_KEYCODE_OFFSET).into()
    }
    fn state(&self) -> host::KeyState {
        self.state
    }
    fn count(&self) -> u32 {
        1
    }
}

pub(crate) struct SyntheticHost;

impl InputBackend for SyntheticHost {
    type Device = SyntheticDevice;
    type KeyboardKeyEvent = SyntheticKey;
    type PointerAxisEvent = SyntheticScroll;
    type PointerButtonEvent = SyntheticButton;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = SyntheticMotion;
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
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = ();
}
