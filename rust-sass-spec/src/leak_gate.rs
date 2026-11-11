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
use std::cell::Cell;
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

thread_local! {
    /// Set while this thread runs harness-only work; see [`without_tracking`].
    /// `const`-initialized so reading it never allocates (which would re-enter
    /// the allocator).
    static BYPASS: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread's allocations are currently counted.
#[inline]
fn counting() -> bool {
    TRACKING.load(Ordering::Relaxed) && BYPASS.try_with(|b| !b.get()).unwrap_or(true)
}

/// Whether this thread is inside [`without_tracking`].
///
/// Lets measured code skip the wrapper entirely: calling it unconditionally
/// allocates a [`std::panic::Location`] for every `?`/panic site and for every
/// `#[track_caller]` call made inside the closure, and those bytes survive the
/// bypass because they are allocated (uncounted) *and freed* (uncounted) inside
/// it — harmless for the total, but the per-test deltas cannot see them.
pub fn paused() -> bool {
    BYPASS.try_with(|b| b.get()).unwrap_or(false)
}

/// Runs `f` without counting its allocations.
///
/// The harness allocates for its own purposes inside a measured region —
/// normalizing expected output, regex engines growing their lazy caches — and
/// the allocator cannot distinguish those from the compiler's. Wrapping such
/// work in this closure excludes it by construction instead of by guessing a
/// threshold.
///
/// The bypass is thread-local and restored on panic. It is intended for short,
/// synchronous harness code only: a closure that spawns work or holds its guard
/// across an `await` stops counting for that span.
pub fn without_tracking<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = BYPASS.try_with(|b| b.set(self.0));
        }
    }

    let previous = BYPASS.try_with(|b| b.replace(true)).unwrap_or(false);
    let _restore = Restore(previous);
    f()
}

struct CountingAlloc;

// --- block tags ---------------------------------------------------------------
//
// The counter must mean "bytes allocated while counting that are still alive",
// but `alloc` and `dealloc` for one block can happen in different regions (the
// measured pass allocates, a `without_tracking` region frees). Asking "am I
// counting?" at dealloc time would subtract blocks that were never added, and
// the total can go negative.
//
// So each block records the decision made when it was allocated: a word is
// stored just before the user's pointer, and `dealloc`/`realloc` act on that
// word rather than on the current region.

/// Bytes reserved before every allocation, so a tag fits and the user pointer
/// keeps the alignment `System` gave us.
fn padding(layout: Layout) -> usize {
    std::mem::size_of::<usize>().max(layout.align())
}

/// The layout actually requested from `System` for a user `layout`.
fn padded(layout: Layout) -> Layout {
    let align = std::mem::align_of::<usize>().max(layout.align());
    match Layout::from_size_align(layout.size() + padding(layout), align) {
        Ok(padded) => padded,
        // A `Layout` this allocator receives is already valid, so adding a
        // power-of-two padding to it cannot overflow the address space.
        Err(_) => unreachable!("leak_gate: padded layout overflows"),
    }
}

/// Tag for a counted block (high bit set; low bits are free for future use).
const COUNTED: usize = 1 << (usize::BITS - 1);

/// Tag for a bypassed block.
const UNCOUNTED: usize = 0;

/// Writes a block's tag and returns the user pointer.
///
/// SAFETY: `base` is a live block from `System` allocated with `padded(layout)`
/// for the same `layout`.
unsafe fn tag_block(base: *mut u8, layout: Layout, counted: bool) -> *mut u8 {
    let user = unsafe { base.add(padding(layout)) };
    let tag = if counted { COUNTED } else { UNCOUNTED };
    unsafe { (user.sub(std::mem::size_of::<usize>()) as *mut usize).write(tag) };
    user
}

/// Reads a block's tag.
///
/// SAFETY: `ptr` is a live user pointer returned by [`tag_block`].
unsafe fn is_counted(ptr: *mut u8) -> bool {
    unsafe { (ptr.sub(std::mem::size_of::<usize>()) as *const usize).read() == COUNTED }
}

// SAFETY: every method forwards to `System`, adding only a tag word before the
// user pointer. The tag is written through a properly aligned, in-bounds
// pointer of the block this allocator owns, and `base + padding(layout)` keeps
// the alignment `padded()` promised.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = unsafe { System.alloc(padded(layout)) };
        if base.is_null() {
            return base;
        }
        let counted = counting();
        if counted {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        unsafe { tag_block(base, layout, counted) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let base = unsafe { System.alloc_zeroed(padded(layout)) };
        if base.is_null() {
            return base;
        }
        let counted = counting();
        if counted {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        unsafe { tag_block(base, layout, counted) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if unsafe { is_counted(ptr) } {
            LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        }
        let base = unsafe { ptr.sub(padding(layout)) };
        unsafe { System.dealloc(base, padded(layout)) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Read the tag before the block moves: `realloc` keeps a block's tag.
        let counted = unsafe { is_counted(ptr) };
        let base = unsafe { ptr.sub(padding(layout)) };
        let new_base = unsafe { System.realloc(base, padded(layout), new_size + padding(layout)) };
        if new_base.is_null() {
            return new_base;
        }
        if counted {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        unsafe { new_base.add(padding(layout)) }
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
