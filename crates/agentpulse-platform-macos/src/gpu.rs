//! Whole-Mac GPU utilisation from the IORegistry, through public IOKit calls.
//!
//! `PerformanceStatistics` / `"Device Utilization %"` is not a documented
//! contract: it is read defensively and any surprise yields None.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

type CFTypeRef = *const c_void;
type CFIndex = isize;
type CFTypeID = usize;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    /// Consumes one reference to `matching`, whatever it returns.
    fn IOServiceGetMatchingServices(main_port: u32, matching: CFTypeRef, existing: *mut u32)
    -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFDictionaryGetTypeID() -> CFTypeID;
    fn CFNumberGetTypeID() -> CFTypeID;
    fn CFNumberGetValue(number: CFTypeRef, kind: CFIndex, out: *mut c_void) -> u8;
    fn CFRelease(cf: CFTypeRef);
}

const UTF8: u32 = 0x0800_0100;
const NUMBER_DOUBLE: CFIndex = 13;
/// `kIOMainPortDefault`.
const MAIN_PORT: u32 = 0;

/// GPU busy percent of the whole Mac: the busiest `IOAccelerator` that reports
/// one (a Mac with two GPUs lists both). None on any failure. Never
/// attributable to a process.
pub fn gpu_utilization() -> Option<u8> {
    // SAFETY: plain IOKit/CoreFoundation calls; every object created or
    // copied here is released exactly once below, and nothing escapes.
    unsafe {
        let matching = IOServiceMatching(c"IOAccelerator".as_ptr());
        if matching.is_null() {
            return None;
        }
        let mut iter = 0;
        if IOServiceGetMatchingServices(MAIN_PORT, matching, &mut iter) != 0 {
            return None;
        }
        let stats_key = cf_string(c"PerformanceStatistics");
        let util_key = cf_string(c"Device Utilization %");
        let mut found = None;
        while !stats_key.is_null() && !util_key.is_null() {
            let service = IOIteratorNext(iter);
            if service == 0 {
                break;
            }
            let stats = IORegistryEntryCreateCFProperty(service, stats_key, ptr::null(), 0);
            IOObjectRelease(service);
            if stats.is_null() {
                continue;
            }
            if CFGetTypeID(stats) == CFDictionaryGetTypeID() {
                let v = CFDictionaryGetValue(stats, util_key);
                let mut n = f64::NAN;
                if !v.is_null()
                    && CFGetTypeID(v) == CFNumberGetTypeID()
                    && CFNumberGetValue(v, NUMBER_DOUBLE, (&raw mut n).cast()) != 0
                {
                    found = found.max(percent(n));
                }
            }
            CFRelease(stats);
        }
        for key in [stats_key, util_key] {
            if !key.is_null() {
                CFRelease(key);
            }
        }
        IOObjectRelease(iter);
        found
    }
}

/// # Safety
/// Returns an owned CFString or null; the caller releases it.
unsafe fn cf_string(s: &CStr) -> CFTypeRef {
    unsafe { CFStringCreateWithCString(ptr::null(), s.as_ptr(), UTF8) }
}

/// Rounded and clamped to 0..=100; None for NaN.
fn percent(v: f64) -> Option<u8> {
    (!v.is_nan()).then(|| v.round().clamp(0.0, 100.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_clamped_and_rounded() {
        assert_eq!(percent(f64::NAN), None);
        assert_eq!(percent(-3.0), Some(0));
        assert_eq!(percent(37.6), Some(38));
        assert_eq!(percent(100.0), Some(100));
        assert_eq!(percent(250.0), Some(100));
        assert_eq!(percent(f64::INFINITY), Some(100));
    }

    #[test]
    fn reading_is_a_percent_when_present() {
        // The key is undocumented: absence is allowed, an out-of-range value is not.
        for _ in 0..3 {
            if let Some(v) = gpu_utilization() {
                assert!(v <= 100);
            }
        }
    }
}
