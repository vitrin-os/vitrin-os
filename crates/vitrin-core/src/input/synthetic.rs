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
    GestureBeginEvent, GestureEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
    InputBackend, KeyboardKeyEvent, Keycode, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent, UnusedEvent,
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

/// Relative pointer motion, with the accelerated and unaccelerated deltas
/// held independently — they are two different numbers on a real device and
/// [`super::SeatInputKind::RelativeMotion`] carries both, so a fixture that
/// tied them together could not catch the one bug worth catching here (a
/// translation that copies one into the other's field).
pub(crate) struct SyntheticRelativeMotion {
    pub dx: f64,
    pub dy: f64,
    pub dx_unaccel: f64,
    pub dy_unaccel: f64,
}

impl Event<SyntheticHost> for SyntheticRelativeMotion {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl PointerMotionEvent<SyntheticHost> for SyntheticRelativeMotion {
    fn delta_x(&self) -> f64 {
        self.dx
    }
    fn delta_y(&self) -> f64 {
        self.dy
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.dx_unaccel
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.dy_unaccel
    }
}

/// A gesture begin. One type serves swipe and pinch, exactly as the wire's
/// one `gesture_begin` does: Smithay's `GestureSwipeBeginEvent` and
/// `GesturePinchBeginEvent` are both empty marker traits over
/// [`GestureBeginEvent`], so the two host events differ only in which
/// `InputEvent` variant carries them.
pub(crate) struct SyntheticGestureBegin {
    pub fingers: u32,
}

impl Event<SyntheticHost> for SyntheticGestureBegin {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl GestureBeginEvent<SyntheticHost> for SyntheticGestureBegin {
    fn fingers(&self) -> u32 {
        self.fingers
    }
}

impl GestureSwipeBeginEvent<SyntheticHost> for SyntheticGestureBegin {}
impl GesturePinchBeginEvent<SyntheticHost> for SyntheticGestureBegin {}

/// A gesture end, likewise shared. `cancelled` is the libinput flag the
/// intake turns into `gesture_state`.
pub(crate) struct SyntheticGestureEnd {
    pub cancelled: bool,
}

impl Event<SyntheticHost> for SyntheticGestureEnd {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl GestureEndEvent<SyntheticHost> for SyntheticGestureEnd {
    fn cancelled(&self) -> bool {
        self.cancelled
    }
}

impl GestureSwipeEndEvent<SyntheticHost> for SyntheticGestureEnd {}
impl GesturePinchEndEvent<SyntheticHost> for SyntheticGestureEnd {}

/// A swipe's motion: the centre delta since the gesture's previous event.
pub(crate) struct SyntheticSwipeUpdate {
    pub dx: f64,
    pub dy: f64,
}

impl Event<SyntheticHost> for SyntheticSwipeUpdate {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl GestureSwipeUpdateEvent<SyntheticHost> for SyntheticSwipeUpdate {
    fn delta_x(&self) -> f64 {
        self.dx
    }
    fn delta_y(&self) -> f64 {
        self.dy
    }
}

/// A pinch's motion. The four quantities are independent here for the same
/// reason they are independent on the wire: `scale` is absolute since the
/// begin while the other three are deltas, and a fixture that derived one
/// from another could not catch a translation that swapped them.
pub(crate) struct SyntheticPinchUpdate {
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    pub rotation: f64,
}

impl Event<SyntheticHost> for SyntheticPinchUpdate {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> SyntheticDevice {
        SyntheticDevice
    }
}

impl GesturePinchUpdateEvent<SyntheticHost> for SyntheticPinchUpdate {
    fn delta_x(&self) -> f64 {
        self.dx
    }
    fn delta_y(&self) -> f64 {
        self.dy
    }
    fn scale(&self) -> f64 {
        self.scale
    }
    fn rotation(&self) -> f64 {
        self.rotation
    }
}

pub(crate) struct SyntheticHost;

impl InputBackend for SyntheticHost {
    type Device = SyntheticDevice;
    type KeyboardKeyEvent = SyntheticKey;
    type PointerAxisEvent = SyntheticScroll;
    type PointerButtonEvent = SyntheticButton;
    type PointerMotionEvent = SyntheticRelativeMotion;
    type PointerMotionAbsoluteEvent = SyntheticMotion;
    type GestureSwipeBeginEvent = SyntheticGestureBegin;
    type GestureSwipeUpdateEvent = SyntheticSwipeUpdate;
    type GestureSwipeEndEvent = SyntheticGestureEnd;
    type GesturePinchBeginEvent = SyntheticGestureBegin;
    type GesturePinchUpdateEvent = SyntheticPinchUpdate;
    type GesturePinchEndEvent = SyntheticGestureEnd;
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
