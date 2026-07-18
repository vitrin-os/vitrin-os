//! Presentation backends for the trusted core.
//!
//! Two backends exist. The nested [`winit`] backend runs the core as a client
//! of the host compositor, presenting one host window (P1.3.1). The
//! [`headless`] backend drives a fixed-size virtual output composited in
//! software, its framebuffer retained in memory for capture (P1.3.2). `main`
//! selects between them with `--nested` / `--headless`. Both backends present
//! the same realm views; nothing outside this module may depend on which one
//! is running.

pub mod headless;
pub mod winit;
