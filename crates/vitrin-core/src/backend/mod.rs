//! Presentation backends for the trusted core.
//!
//! P1.3.1 ships only the nested [`winit`] backend. The headless backend
//! (virtual output at a fixed size, framebuffer retained for capture —
//! P1.3.2) will be a sibling module here, selected by `--headless` in
//! `main`. Both backends present the same realm views; nothing outside this
//! module may depend on which one is running.

pub mod winit;
