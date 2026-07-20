//! piwiplay TUI library surface.
//!
//! The frontend is split into a library (these modules) and a thin `main.rs`
//! binary. Exposing the modules as a lib lets integration tests render real
//! frames with ratatui's `TestBackend` (see `tests/render.rs`) without a
//! terminal — which is how the TUI is regression-tested.

pub mod app;
pub mod fs_browser;
pub mod theme;
pub mod ui;
pub mod widgets;
