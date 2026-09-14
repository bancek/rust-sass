//! Target-portable wall-clock time.
//!
//! `std::time::SystemTime::now()` panics on `wasm32-unknown-unknown` ("time
//! not implemented on this platform"), so all of rust-sass goes through
//! [`SassTime`] instead: an `i64` millisecond count since the Unix epoch.
//!
//! - **Native**: `now()` wraps `SystemTime::now()`.
//! - **wasm32**: `now()` reads a host-set atomic; the host (JS) primes it via
//!   [`set_wasm_now`] — the `rust-sass-wasm` crate exposes a binding for it.

#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicI64, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;
#[cfg(not(target_arch = "wasm32"))]
use std::time::UNIX_EPOCH;

#[cfg(target_arch = "wasm32")]
static WASM_NOW_MILLIS: AtomicI64 = AtomicI64::new(0);

/// Milliseconds since the Unix epoch.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SassTime(i64);

impl SassTime {
    /// The Unix epoch (used as a placeholder "zero" time by virtual filesystems).
    pub const UNIX_EPOCH: SassTime = SassTime(0);

    /// Current wall-clock time.
    pub fn now() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let d = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            SassTime(d.as_millis() as i64)
        }
        #[cfg(target_arch = "wasm32")]
        {
            SassTime(WASM_NOW_MILLIS.load(Ordering::Relaxed))
        }
    }

    /// wasm32 only: prime the clock from the host (e.g. `Date.now()`).
    #[cfg(target_arch = "wasm32")]
    pub fn set_wasm_now(millis: i64) {
        WASM_NOW_MILLIS.store(millis, Ordering::Relaxed);
    }

    /// Milliseconds since the Unix epoch.
    pub fn as_millis(self) -> i64 {
        self.0
    }

    /// From host-provided milliseconds (native: converts from `SystemTime`).
    pub fn from_millis(millis: i64) -> Self {
        SassTime(millis)
    }
}
