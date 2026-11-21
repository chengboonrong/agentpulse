//! Desktop notifications through UserNotifications.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use block2::RcBlock;
use objc2::rc::autoreleasepool;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};

static AUTHORIZED: OnceLock<()> = OnceLock::new();
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Shows a notification. Does nothing but log when the process has no app
/// bundle (`cargo run`, tests), where UserNotifications would throw.
/// Failures are logged, never returned.
pub fn notify(title: &str, body: &str) {
    // The long-lived collector thread has no pool of its own to drain autoreleased objects.
    autoreleasepool(|_| notify_in_pool(title, body))
}

fn notify_in_pool(title: &str, body: &str) {
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        tracing::info!(title, body, "notification (no app bundle)");
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    AUTHORIZED.get_or_init(|| {
        let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
            // SAFETY: the system passes a valid NSError or null.
            if let Some(e) = unsafe { error.as_ref() } {
                tracing::warn!("notification authorization failed: {e}");
            } else if !granted.as_bool() {
                tracing::warn!("notification permission not granted");
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    });

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let id = format!(
        "agentpulse-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&id),
        &content,
        None,
    );
    let done = RcBlock::new(|error: *mut NSError| {
        // SAFETY: the system passes a valid NSError or null.
        if let Some(e) = unsafe { error.as_ref() } {
            tracing::warn!("could not show notification: {e}");
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&done));
}

#[cfg(test)]
mod tests {
    #[test]
    fn notify_without_a_bundle_is_a_no_op() {
        super::notify("t", "b");
    }
}
