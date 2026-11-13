//! HTTP(S) requests through NSURLSession: the update check and the phone relay.

use std::sync::mpsc;
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread as _, define_class, msg_send};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSObject, NSObjectProtocol, NSString,
    NSURL, NSURLRequest, NSURLResponse, NSURLSession, NSURLSessionConfiguration,
    NSURLSessionDelegate, NSURLSessionTask, NSURLSessionTaskDelegate,
};

define_class!(
    /// Refuses every redirect, so a request's headers (a bearer token) never reach
    /// another URL; the 3xx comes back as the response.
    #[unsafe(super(NSObject))]
    #[name = "AgentPulseNoRedirect"]
    struct NoRedirect;

    unsafe impl NSObjectProtocol for NoRedirect {}
    unsafe impl NSURLSessionDelegate for NoRedirect {}
    unsafe impl NSURLSessionTaskDelegate for NoRedirect {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        fn will_redirect(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            _response: &NSHTTPURLResponse,
            _request: &NSURLRequest,
            completion: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            completion.call((std::ptr::null_mut(),));
        }
    }
);

// SAFETY: no state at all, so any thread may use it.
unsafe impl Send for NoRedirect {}
unsafe impl Sync for NoRedirect {}

impl NoRedirect {
    fn new() -> Retained<Self> {
        // SAFETY: NSObject's plain `init`.
        unsafe { msg_send![Self::alloc(), init] }
    }
}

/// GETs `url` and returns the body of an HTTP 200 response, or a short message.
/// Blocks until the response or `timeout_s` seconds; call it off the main thread.
/// An ephemeral session: no cookies, credentials or cache are stored on disk.
/// Sends `User-Agent: <user_agent>`, the GitHub `Accept` type and
/// `Accept-Language: en`, so the user's locale is not sent; macOS adds
/// `Accept-Encoding`.
pub fn fetch(url: &str, user_agent: &str, timeout_s: f64) -> Result<Vec<u8>, String> {
    let headers = [
        ("User-Agent", user_agent),
        ("Accept", "application/vnd.github+json"),
        ("Accept-Language", "en"),
    ];
    match request("GET", url, &headers, None, timeout_s)? {
        Response {
            status: 200, body, ..
        } => Ok(body),
        r => Err(format!("HTTP {}", r.status)),
    }
}

/// An HTTP response of any status.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    /// The `Retry-After` header, when there is one.
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

/// One request with exactly `headers` (macOS adds `Accept-Encoding`, and
/// `Accept-Language` unless given) and `body`. Any HTTP status is a response;
/// network errors are `Err` with a short message. Redirects are not followed: a 3xx is
/// the response. Blocks until the response or
/// `timeout_s` seconds; call it off the main thread. An ephemeral session: no
/// cookies, credentials or cache are stored on disk.
pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout_s: f64,
) -> Result<Response, String> {
    // A background thread has no pool of its own to drain autoreleased objects.
    autoreleasepool(|_| request_in_pool(method, url, headers, body, timeout_s))
}

fn request_in_pool(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout_s: f64,
) -> Result<Response, String> {
    let ns_url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("bad URL")?;
    let request = NSMutableURLRequest::requestWithURL(&ns_url);
    request.setHTTPMethod(&NSString::from_str(method));
    for (name, value) in headers {
        request.setValue_forHTTPHeaderField(
            Some(&NSString::from_str(value)),
            &NSString::from_str(name),
        );
    }
    if let Some(body) = body {
        request.setHTTPBody(Some(&NSData::with_bytes(body)));
    }
    request.setTimeoutInterval(timeout_s);
    let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    // The whole transfer, not just the gap between packets.
    config.setTimeoutIntervalForResource(timeout_s);
    let delegate = NoRedirect::new();
    // SAFETY: the delegate is Send + Sync; a nil queue lets the session make its own.
    // The session keeps the delegate until it is invalidated below.
    let session = unsafe {
        NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
            &config,
            Some(ProtocolObject::from_ref(&*delegate)),
            None,
        )
    };

    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            // SAFETY: NSURLSession passes valid objects or null, alive for this call.
            let (data, response, error) =
                unsafe { (data.as_ref(), response.as_ref(), error.as_ref()) };
            let result = match (error, response) {
                (Some(e), _) => Err(format!("network error: {}", e.localizedDescription())),
                (None, Some(r)) => match r.downcast_ref::<NSHTTPURLResponse>() {
                    Some(http) => Ok(Response {
                        status: u16::try_from(http.statusCode()).unwrap_or(0),
                        retry_after: http
                            .valueForHTTPHeaderField(&NSString::from_str("Retry-After"))
                            .map(|v| v.to_string()),
                        body: data.map(|d| d.to_vec()).unwrap_or_default(),
                    }),
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn a_redirect_comes_back_unfollowed() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        // One connection only: a followed redirect would wait for a second and time out.
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            let reply = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/elsewhere\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            reader.get_mut().write_all(reply.as_bytes()).unwrap();
        });
        let url = format!("http://127.0.0.1:{port}/start");
        let r = super::request("GET", &url, &[("Authorization", "Bearer x")], None, 5.).unwrap();
        assert_eq!(r.status, 302);
    }
}
