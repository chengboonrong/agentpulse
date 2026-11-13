//! macOS system APIs (AppKit, IOKit) that GPUI does not expose. No GPUI dependency.

mod gpu;
mod status_item;

pub use gpu::gpu_utilization;
pub use objc2::MainThreadMarker;
pub use status_item::{MenuAction, MenuLine, StatusItem, WIDEST_TITLE};

/// The Accessibility "Reduce motion" setting. GPUI does not read it on macOS.
pub fn reduce_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
