// SPDX-License-Identifier: Apache-2.0
//! The `fixed` wire argument type: signed 24.8 fixed-point, wire-encoded as a
//! raw `i32` (see `docs/protocol/00-conventions.md` 2.2). Used only by
//! `vitrin_shim_seat.motion`'s `x`/`y` in v0, so that later server-side
//! sub-pixel motion synthesis needs no signature change.

/// A raw 24.8 fixed-point value: 24 integer bits, 8 fraction bits, stored as
/// the exact `i32` that goes on the wire. Deliberately minimal -- just the
/// newtype plus `f64` conversion helpers, nothing fancier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Fixed(i32);

impl Fixed {
    /// Build a `Fixed` from its raw on-wire bit pattern.
    pub const fn from_bits(bits: i32) -> Self {
        Fixed(bits)
    }

    /// The raw on-wire bit pattern.
    pub const fn to_bits(self) -> i32 {
        self.0
    }

    /// Convert from a floating-point value, rounding to the nearest 1/256.
    ///
    /// Out-of-range input saturates: values beyond the representable
    /// `[i32::MIN, i32::MAX]` bit range clamp to the nearest bound, and NaN
    /// maps to 0 -- the defined behavior of Rust's float-to-int `as` cast,
    /// stated here so callers don't have to know that. The generated C
    /// header's `vitrin_fixed_from_double` clamps identically (C's raw cast
    /// would be UB there). This is an encode-side convenience fed only by
    /// trusted-core code, never by wire bytes, so saturating beats panicking:
    /// a coordinate clamp is recoverable, a crash in the compositor is not.
    pub fn from_f64(v: f64) -> Self {
        Fixed((v * 256.0).round() as i32)
    }

    /// Convert to a floating-point value.
    pub fn to_f64(self) -> f64 {
        f64::from(self.0) / 256.0
    }
}

impl From<i32> for Fixed {
    fn from(bits: i32) -> Self {
        Fixed::from_bits(bits)
    }
}

impl From<Fixed> for i32 {
    fn from(f: Fixed) -> Self {
        f.to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_bits() {
        for bits in [0, 1, -1, i32::MAX, i32::MIN, 256, -256, 12345] {
            assert_eq!(Fixed::from_bits(bits).to_bits(), bits);
        }
    }

    #[test]
    fn f64_conversion_is_reasonable() {
        assert_eq!(Fixed::from_f64(1.0).to_bits(), 256);
        assert_eq!(Fixed::from_f64(0.5).to_bits(), 128);
        assert_eq!(Fixed::from_f64(-1.0).to_bits(), -256);
        assert!((Fixed::from_bits(256).to_f64() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn f64_conversion_saturates_out_of_range_and_zeroes_nan() {
        // Pins the documented saturation semantics (and keeps them in lockstep
        // with the generated C header's clamping vitrin_fixed_from_double).
        assert_eq!(Fixed::from_f64(1e18).to_bits(), i32::MAX);
        assert_eq!(Fixed::from_f64(f64::INFINITY).to_bits(), i32::MAX);
        assert_eq!(Fixed::from_f64(-1e18).to_bits(), i32::MIN);
        assert_eq!(Fixed::from_f64(f64::NEG_INFINITY).to_bits(), i32::MIN);
        assert_eq!(Fixed::from_f64(f64::NAN).to_bits(), 0);
    }
}
