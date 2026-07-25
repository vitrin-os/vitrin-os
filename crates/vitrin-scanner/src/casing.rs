// SPDX-License-Identifier: Apache-2.0
//! Identifier-casing helpers shared by codegen backends.
//!
//! The IDL uses snake_case throughout (Wayland-style) for interface, message,
//! arg, enum, and entry names. Rust module and field names already match that
//! convention verbatim; only *type*-position names (message structs, enum
//! types) and bitfield constant names need conversion.

/// snake_case -> PascalCase, e.g. "frame_ready" -> "FrameReady".
pub fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// snake_case -> SCREAMING_SNAKE_CASE, e.g. "actuate_pointer" -> "ACTUATE_POINTER".
/// Names in this IDL are already snake_case, so this is just an uppercase pass.
pub fn to_screaming_snake(s: &str) -> String {
    s.to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_basic() {
        assert_eq!(to_pascal_case("frame_ready"), "FrameReady");
        assert_eq!(to_pascal_case("move"), "Move");
        assert_eq!(to_pascal_case("type"), "Type");
        assert_eq!(to_pascal_case("get_realm"), "GetRealm");
        assert_eq!(to_pascal_case("xrgb8888"), "Xrgb8888");
    }

    #[test]
    fn screaming_snake_basic() {
        assert_eq!(to_screaming_snake("actuate_pointer"), "ACTUATE_POINTER");
        assert_eq!(to_screaming_snake("observe"), "OBSERVE");
    }
}
