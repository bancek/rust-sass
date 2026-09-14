//! `rust-sass-libsass`: the libsass C API implemented on `rust-sass`.
//!
//! Produces `libsass.{so,dylib,a}` with the upstream `sass.h` ABI so existing
//! native consumers link unmodified — and get the modern Sass language underneath
//! ("links like libsass, behaves like Dart Sass").
//!
//! # Async
//!
//! Sync-only. The `async` cargo feature is a workspace-graph consistency
//! stub: it forwards to `rust-sass/async` so the whole workspace resolves as
//! async under `--features async`, but the crate-level `cfg` below removes
//! every module, leaving an empty crate. The supported artifact is always the
//! sync build (see `docs/ref/libsass.md`).
//!
//! # Safety boundary
//!
//! This is the only workspace crate allowed `unsafe` code and a `libc`
//! dependency: raw C pointers, NUL-terminated strings, malloc/free ownership,
//! and `catch_unwind` at every `extern "C"` boundary live here so the core
//! `rust-sass` crates stay `unsafe`-free. Each public function documents its
//! pointer contract under `# Safety`.

// In async builds the adapter is deliberately empty: none of it may touch the
// async core. See the crate docs above and `docs/ref/libsass.md`.
#![cfg(not(feature = "async"))]

pub mod base;
pub mod context;
pub mod env;
pub mod functions;
pub mod importers;
pub mod options;
pub mod sass2scss;
pub mod values;
