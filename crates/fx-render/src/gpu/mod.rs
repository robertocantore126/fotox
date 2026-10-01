//! GPU tile atlas and compositor.

pub mod atlas;
// AUDIT-FIX(P1): startup adapter memory probe.
pub mod compositor;
pub mod hardware;
pub mod viewport;

pub use compositor::{CompositeError, CompositorConfig, CompositorStats, GpuCompositor, TileOutcome};
pub use viewport::ViewportRenderer;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod viewport_tests;
