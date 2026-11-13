//! The Swift link bridge (`macos/LinkBridge/`, `libAgentPulseLinkBridge.dylib`): keys in
//! the Keychain, sealing, the QR image and the direct listener, behind one JSON call.

use std::cell::OnceCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use serde_json::Value;

use super::Msg;

/// Relative to `Contents`.
pub const PATH: &str = "Frameworks/libAgentPulseLinkBridge.dylib";

/// One request `{"op": ..., ...}` to its `ok` object, or the bridge's error.
/// Listener events reach the worker as [`Msg::Event`].
pub trait Bridge: Send {
    fn call(&self, request: &Value) -> Result<Value, String>;
}

type CallFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type FreeFn = unsafe extern "C" fn(*mut c_char);
type Handler = extern "C" fn(*mut c_void, *const c_char);
type SetHandlerFn = unsafe extern "C" fn(Option<Handler>, *mut c_void);

/// The dylib, opened on first use so that with no pairings nothing is loaded.
pub struct Dylib {
    path: PathBuf,
    events: Sender<Msg>,
    fns: OnceCell<Option<(CallFn, FreeFn)>>,
}

impl Dylib {
    pub fn new(path: PathBuf, events: Sender<Msg>) -> Self {
        Dylib {
            path,
            events,
            fns: OnceCell::new(),
        }
    }
}

impl Bridge for Dylib {
    fn call(&self, request: &Value) -> Result<Value, String> {
        let (call, free) = self
            .fns
            .get_or_init(|| load(&self.path, self.events.clone()))
            .ok_or("the phone bridge did not load")?;
        let request = CString::new(request.to_string()).map_err(|_| "NUL in request")?;
        // SAFETY: a NUL-terminated request; the response is the bridge's malloc'd,
        // NUL-terminated string, copied before it is freed with the bridge's own free.
        let text = unsafe {
            let response = call(request.as_ptr());
            if response.is_null() {
                return Err("the phone bridge returned nothing".into());
            }
            let text = CStr::from_ptr(response).to_string_lossy().into_owned();
            free(response);
            text
        };
        let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        match v.get_mut("ok") {
            Some(ok) => Ok(ok.take()),
            None => Err(v["error"].as_str().unwrap_or("bad bridge response").into()),
        }
    }
}

fn load(path: &Path, events: Sender<Msg>) -> Option<(CallFn, FreeFn)> {
    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: NUL-terminated strings. The library is never closed, so its symbols and
    // the leaked handler context stay valid for the life of the process. The bridge
    // defines the three symbols with `@_cdecl` and these signatures.
    unsafe {
        let handle = libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            let err = libc::dlerror();
            let err = (!err.is_null()).then(|| CStr::from_ptr(err).to_string_lossy());
            tracing::warn!(error = ?err, "cannot load the phone bridge");
            return None;
        }
        let sym = |name: &CStr| libc::dlsym(handle, name.as_ptr());
        let (call, free, set) = (
            sym(c"aplink_call"),
            sym(c"aplink_free"),
            sym(c"aplink_set_event_handler"),
        );
        if call.is_null() || free.is_null() || set.is_null() {
            tracing::warn!("the phone bridge lacks its symbols");
            return None;
        }
        let set = std::mem::transmute::<*mut c_void, SetHandlerFn>(set);
        set(Some(on_event), Box::into_raw(Box::new(events)).cast());
        Some((
            std::mem::transmute::<*mut c_void, CallFn>(call),
            std::mem::transmute::<*mut c_void, FreeFn>(free),
        ))
    }
}

/// Any thread; `event` is valid only during the call.
extern "C" fn on_event(ctx: *mut c_void, event: *const c_char) {
    if ctx.is_null() || event.is_null() {
        return;
    }
    // SAFETY: `ctx` is the leaked `Sender<Msg>` from `load` (Sync, so any thread may
    // send), and `event` a NUL-terminated string alive for this call.
    let (events, text) = unsafe {
        (
            &*ctx.cast::<Sender<Msg>>(),
            CStr::from_ptr(event).to_string_lossy(),
        )
    };
    if let Ok(v) = serde_json::from_str(&text) {
        events.send(Msg::Event(v)).ok();
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url, the bridge's encoding for bytes.
pub fn b64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Unpadded base64url to bytes; None for anything else.
pub fn from_b64url(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 2);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= (B64.iter().position(|&b| b == c)? as u32) << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}
