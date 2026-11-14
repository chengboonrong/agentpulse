//! macOS system APIs (AppKit, IOKit, Foundation's URL loading) that GPUI does not expose. No GPUI dependency.

mod fetch;
mod gpu;
mod status_item;

pub use fetch::{Response, fetch, request};
pub use gpu::gpu_utilization;
pub use objc2::MainThreadMarker;
pub use status_item::{MenuAction, MenuImage, MenuLine, StatusItem, WIDEST_TITLE};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
}

/// Turns on or off Secure Event Input, so no other process can read the keys
/// typed into a secure field. Calls must be balanced: each `true` with one `false`.
pub fn secure_event_input(on: bool) {
    // SAFETY: public Carbon calls with no arguments; the system counts them.
    unsafe {
        if on {
            EnableSecureEventInput();
        } else {
            DisableSecureEventInput();
        }
    }
}

/// The Accessibility "Reduce motion" setting. GPUI does not read it on macOS.
pub fn reduce_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
