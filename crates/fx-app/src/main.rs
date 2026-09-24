//! Fotox desktop app. Implemented in milestone M0 (docs/tasks/M0.md).
//!
//! Structure to build (port from reference/graphite-desktop/src, see docs/GRAPHITE.md):
//! * `main.rs`: `UiContext::setup()` FIRST (CEF helper processes re-enter
//!   main and must exit immediately), then wgpu, winit, UI start.
//! * `app.rs`: winit `ApplicationHandler`: window events, UI events,
//!   engine outputs, redraw scheduling.
//! * `render.rs`: final composite pass, viewport texture + UI texture →
//!   swapchain (port of Graphite's `render/state.rs` + shader).
//! * `input.rs`: routes pointer events. Over the viewport rect and not
//!   captured by the UI → engine (`fx_engine::PointerInput`), otherwise →
//!   CEF (`UiCommand::Input`).
//! * `bridge.rs`: UI messages (fx-protocol frames) ↔ engine.

fn main() {
	eprintln!("fotox: the desktop shell is built in milestone M0 (docs/tasks/M0.md)");
}
