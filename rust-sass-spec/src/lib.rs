// dart-source: N/A (Go spec runner only)
// go-source: go/spec/runner_test.go

pub mod hrx;
#[cfg(feature = "leak-gate")]
pub mod leak_gate;
pub mod options;
pub mod runner;

pub use runner::{run_specs, RunnerStats};

/// Forces the runner's process-lifetime caches to be allocated.
///
/// The normalizers build two `LazyLock<Regex>`es on first use, and the regex
/// engines cache their automata behind them. A harness that measures
/// allocations — the `leak-gate` counting allocator — calls this, together
/// with [`rust_sass::warm_up`], before the measured region.
pub fn warm_up() {
    runner::warm_up();
}
