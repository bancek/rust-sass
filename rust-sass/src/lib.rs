// dart-source: N/A (true-original crate root — module declarations only, no Dart counterpart)
#![forbid(unsafe_code)]

/// `rust-sass`: a Rust port of Dart Sass (see `docs/architecture.md`).
pub mod arena_map;
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

/// Forces every process-lifetime cache to be allocated.
///
/// The compiler lazily builds a handful of static tables (named colors, unit
/// conversion factors, the D50 white point, the VLQ decode table) on first
/// use, and keeps them for the life of the process. A harness that measures
/// allocations across compilations — the spec runner's counting allocator —
/// calls this before the measured region so that building them is not
/// attributed to that region.
///
/// Not needed by normal callers: the tables are built on demand.
pub fn warm_up() {
    use std::sync::LazyLock;

    LazyLock::force(&value::color_names::NAME_BY_RGB);
    LazyLock::force(&value::color_names::NAME_BY_NAME);
    LazyLock::force(&value::number_util::CONVERSION_FACTORS);
    LazyLock::force(&value::number_util::KNOWN_COMPATIBILITY_SETS);
    LazyLock::force(&value::color_conversions::D50);
    LazyLock::force(&sourcemap::vlq::BASE64_DECODE);
}
