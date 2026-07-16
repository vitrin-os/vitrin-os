//! Tiny formatting helpers shared by both codegen backends (`rust_gen.rs`,
//! `c_gen.rs`). Nothing here is IDL-specific logic -- it's just string-
//! building plumbing so each backend's `gen_*` functions read as a flat
//! sequence of `buf.line(...)` calls instead of `write!(...).unwrap()` noise,
//! plus the one numeric-literal-rendering convention both backends want to
//! agree on (so the same enum value reads identically -- hex vs decimal --
//! whether a human is looking at the generated Rust or the generated C).

/// A line-oriented string builder.
#[derive(Default)]
pub struct Buf(pub String);

impl Buf {
    pub fn line(&mut self, s: impl AsRef<str>) {
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }

    pub fn blank(&mut self) {
        self.0.push('\n');
    }
}

/// Render a `u32` as either a decimal or `0x` hex literal. Hex above a
/// threshold where decimal would be unreadable -- which happens to match
/// every hex-spelled entry in `protocol/vitrin-v0.xml` (the DRM fourcc codes
/// in `vitrin_view.format`) -- rather than preserving the IDL's original
/// lexical spelling verbatim. Shared so the same enum value reads identically
/// (hex vs decimal) in both the generated Rust and the generated C header.
pub fn format_u32_literal(value: u32) -> String {
    if value > 0xffff {
        format!("0x{value:x}")
    } else {
        format!("{value}")
    }
}

/// Escape a summary string for embedding in a single-line doc comment
/// (`///` in Rust, `/* ... */` in C). Beyond newline flattening, a literal
/// `*/` must be defused: inside a generated C block comment it would
/// terminate the comment early, turning the rest of the summary into
/// (probably uncompilable, possibly semantically live) C code with no
/// generation-time error. `*\/` renders near-identically and is inert in
/// both comment syntaxes.
pub fn doc_text(s: &str) -> String {
    s.replace('\n', " ").replace('\r', "").replace("*/", "*\\/")
}
