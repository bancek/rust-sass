// dart-source: N/A (spec-harness instrumentation, no Dart counterpart)
// go-source: N/A (spec-harness instrumentation, no Go counterpart)

//! Heap-leak instrumentation for the spec runner (feature `leak-gate`).
//!
//! The spec runner is driven from a single sequential `#[test]`, so a
//! process-global counter is sufficient: no libtest thread allocates
//! concurrently while a run is being measured.
//!
//! Protocol (see `tests/runner_test.rs`): one untracked warm-up pass fills the
//! process-lifetime caches (`LazyLock` regexes, color-name tables, matrices),
//! then [`start`] resets the counter and enables tracking for the measured
//! pass. After the measured pass's own results have been dropped, [`stop`]
//! disables tracking and [`live`] is the net number of bytes allocated during
//! the pass. With no leaks it is zero; anything else is per-compilation
//! retention.
//!
//! Only compiled with `--features leak-gate`, so the normal spec run pays
//! nothing for it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

/// Whether allocations are currently counted.
static TRACKING: AtomicBool = AtomicBool::new(false);

/// Net bytes allocated while tracking.
static LIVE: AtomicIsize = AtomicIsize::new(0);

/// Per-test deltas that were non-zero, in completion order.
static REPORT: Mutex<Vec<(String, isize)>> = Mutex::new(Vec::new());

/// Cap on reported tests, so a leaking run cannot grow the report without
/// bound (or distort the measurement with its own bookkeeping).
const REPORT_LIMIT: usize = 256;

struct CountingAlloc;

// SAFETY: every method forwards to `System` unchanged. The only addition is
// the atomic byte accounting around the call, which does not touch the
// allocation's memory or its layout.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACKING.load(Ordering::Relaxed) {
            LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// Enables tracking, resets the counter, and clears the per-test report. Call
/// after the warm-up pass and after dropping its results.
pub fn start() {
    LIVE.store(0, Ordering::Relaxed);
    TRACKING.store(true, Ordering::Relaxed);
    report_lock().clear();
}

/// Disables tracking. [`live`] keeps the final value.
pub fn stop() {
    TRACKING.store(false, Ordering::Relaxed);
}

/// Net bytes allocated since [`start`].
pub fn live() -> isize {
    LIVE.load(Ordering::Relaxed)
}

/// Takes the per-test report collected so far.
pub fn report() -> Vec<(String, isize)> {
    std::mem::take(&mut *report_lock())
}

fn report_lock() -> std::sync::MutexGuard<'static, Vec<(String, isize)>> {
    // A panic while holding the lock would poison it; the data is still valid.
    REPORT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Measures one spec test's live-byte delta and records it when non-zero.
///
/// The guard stores only a borrowed path, so creating it does not perturb the
/// measurement it is about to take.
pub struct TestGuard<'a> {
    path: &'a str,
    before: isize,
}

impl<'a> TestGuard<'a> {
    pub fn new(path: &'a str) -> Self {
        Self {
            path,
            before: live(),
        }
    }
}

impl Drop for TestGuard<'_> {
    fn drop(&mut self) {
        if !TRACKING.load(Ordering::Relaxed) {
            return;
        }
        let delta = live() - self.before;
        if delta == 0 {
            return;
        }
        let mut report = report_lock();
        if report.len() < REPORT_LIMIT {
            report.push((self.path.to_string(), delta));
        }
    }
}
