// dart-source: N/A (test-harness plumbing — no Dart counterpart; see
//   docs/plans/libsass.md §8/D11-D13)

//! Links the test binaries against `libsass` (`-lsass`), flipping the search
//! path by feature:
//!
//! - default (`rust-impl`): the workspace adapter's freshly built shared
//!   library. The optional `rust-sass-libsass` build-dependency guarantees the
//!   cdylib exists; its output dir is derived from `OUT_DIR`
//!   (`target/<profile>/build/<pkg>-<hash>/out` → up three levels), plus
//!   `target/<profile>/deps/` (Windows stages the cdylib there).
//! - `upstream` (with `--no-default-features`): upstream libsass, from
//!   `LIBSASS_LIB_DIR` or `<workspace>/../libsass/lib` by default.
//!
//! Panics with an actionable message when the library is missing.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBSASS_LIB_DIR");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = if std::env::var("CARGO_FEATURE_UPSTREAM").is_ok() {
        match std::env::var("LIBSASS_LIB_DIR") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => manifest.join("../libsass/lib"),
        }
    } else {
        // OUT_DIR = <target>/<profile>/build/<pkg>-<hash>/out.
        let mut dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        dir.pop();
        dir.pop();
        dir.pop();
        dir
    };
    // Prefer the shared library when both exist: cargo places ALL crate-type
    // outputs (cdylib + staticlib) directly in target/<profile>/, so a
    // libsass.a sits next to libsass.dylib and the linker may pick the stale
    // archive. Force shared linkage explicitly. On Windows cargo instead
    // stages the cdylib (sass.dll plus the sass.lib import library) under
    // target/<profile>/deps/ — next to the test executables that load it —
    // so probe both dirs.
    let shared_dir = [dir.clone(), dir.join("deps")].into_iter().find(|d| {
        d.join("libsass.dylib").exists()
            || d.join("libsass.so").exists()
            || d.join("sass.dll").exists()
    });
    if let Some(shared_dir) = shared_dir {
        println!("cargo:rustc-link-search=native={}", shared_dir.display());
        println!("cargo:rustc-link-lib=dylib=sass");
    } else if dir.join("libsass.a").exists() {
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static=sass");
    } else {
        panic!(
            "no libsass library in {}; build it first (rust-impl: `cargo build -p rust-sass-libsass`; upstream: `make -C <libsass> static` or set LIBSASS_LIB_DIR)",
            dir.display()
        );
    }
    if std::env::var("CARGO_FEATURE_UPSTREAM").is_ok() {
        // Upstream ships as a static C++ archive; rustc does not link the
        // C++ standard library automatically (same reason sassc's Makefile
        // adds it): libc++ on macOS, libstdc++ elsewhere.
        if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "macos" {
            println!("cargo:rustc-link-lib=c++");
        } else {
            println!("cargo:rustc-link-lib=stdc++");
        }
    }
}
