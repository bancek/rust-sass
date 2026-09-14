// dart-source: N/A (test-harness plumbing — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11-D13)

//! C-ABI contract tests for `rust-sass-libsass`.
//!
//! Integration tests in `tests/` link the real shared library (ours or
//! upstream, per feature — see `build.rs`) and assert the ABI contract
//! identically on both legs: lifecycle/ownership, getter/setter round-trips,
//! error paths, value identities. Per plan D11, CSS bytes are never compared
//! across legs; goldens gate our implementation via the sass-spec harness.

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
pub mod bindings;
