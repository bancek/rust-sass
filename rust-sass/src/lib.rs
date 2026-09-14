// dart-source: N/A (true-original crate root — module declarations only, no Dart counterpart)
/// `rust-sass`: a Rust port of Dart Sass (see `docs/architecture.md`).
pub mod ast;
pub mod callable;
pub mod common;
pub mod compile;
pub mod compile_context;
pub mod configuration;
pub mod deprecation;
pub mod environment;
pub mod eval;

pub mod extend;
pub mod functions;
pub mod io;
pub mod logger;
pub mod math;
#[cfg(feature = "glibc-math")]
pub mod math_glibc_pow;
#[cfg(feature = "glibc-math")]
pub mod math_glibc_tables;
pub mod member_map;
pub mod module;
pub mod parse;
pub mod selector;
pub mod serialize;
pub mod source_map_buffer;
pub mod sourcemap;
pub mod termglyph;
pub mod unvendor;
pub mod url;
pub mod util;
pub mod value;

pub use bumpalo::Bump;
