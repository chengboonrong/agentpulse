//! One HTTPS GET through NSURLSession, for the update check.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::autoreleasepool;
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration,
};

/// GETs `url` and returns the body of an HTTP 200 response, or a short message.
/// Blocks until the response or `timeout_s` seconds; call it off the main thread.
/// An ephemeral session: no cookies, credentials or cache are stored on disk.
/// Sends `User-Agent: <user_agent>`, the GitHub `Accept` type and
/// `Accept-Language: en`, so the user's locale is not sent; macOS adds
/// `Accept-Encoding`.
pub fn fetch(url: &str, user_agent: &str, timeout_s: f64) -> Result<Vec<u8>, String> {
    // A background thread has no pool of its own to drain autoreleased objects.
    autoreleasepool(|_| fetch_in_pool(url, user_agent, timeout_s))
}

fn fetch_in_pool(url: &str, user_agent: &str, timeout_s: f64) -> Result<Vec<u8>, String> {
    let ns_url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("bad URL")?;
    let request = NSMutableURLRequest::requestWithURL(&ns_url);
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str(user_agent)),
        &NSString::from_str("User-Agent"),
    );
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str("application/vnd.github+json")),
        &NSString::from_str("Accept"),
    );
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str("en")),
        &NSString::from_str("Accept-Language"),
    );
    request.setTimeoutInterval(timeout_s);
    let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    // The whole transfer, not just the gap between packets.
    config.setTimeoutIntervalForResource(timeout_s);
    let session = NSURLSession::sessionWithConfiguration(&config);

    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            // SAFETY: NSURLSession passes valid objects or null, alive for this call.
            let (data, response, error) =
                unsafe { (data.as_ref(), response.as_ref(), error.as_ref()) };
            let result = match (error, response) {
                (Some(e), _) => Err(format!("network error: {}", e.localizedDescription())),
                (None, Some(r)) => match r.downcast_ref::<NSHTTPURLResponse>() {
                    Some(http) if http.statusCode() == 200 => {
                        Ok(data.map(|d| d.to_vec()).unwrap_or_default())
                    }
                    Some(http) => Err(format!("HTTP {}", http.statusCode())),
                    None => Err("network error: not an HTTP response".into()),
                },
                (None, None) => Err("network error: no response".into()),
            };
            tx.send(result).ok();
        },
    );
    // SAFETY: the handler only sends owned values over a channel, so it may
    // run on the session's own queue.
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &handler) };
    task.resume();
    // The task holds its own reference to the handler.
    drop(handler);
    // The session keeps its handler until invalidated; this lets it go once done.
    session.finishTasksAndInvalidate();
    // A backstop in case the handler never runs.
    let backstop = Duration::from_secs_f64(timeout_s.max(0.) + 5.);
    rx.recv_timeout(backstop)
        .unwrap_or_else(|_| Err("network error: timed out".into()))
}
