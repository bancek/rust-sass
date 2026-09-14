// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: bin/sass.dart
// go-source: go/gosass/gosass.go

use crate::options::CliOptions;
use std::env;
use std::rc::Rc;

use bumpalo::Bump;

use rust_sass::io::DefaultIo;

mod compile;
mod options;

/// Optional mimalloc global allocator (perf.md B0 / `--features mimalloc`).
/// Compile-time choice: zero cost when the feature is off.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Matches Go: gosass.go lines 32-34
    // if (args case ['--embedded', ...var rest]) { embedded.Run(rest); return; }
    #[cfg(feature = "embedded")]
    if args.len() >= 2 && args[1] == "--embedded" {
        rust_sass_embedded::run(&args[2..]);
    }

    let io: Rc<dyn rust_sass::io::IoExt> = Rc::new(DefaultIo::new());
    let command = options::build_command();

    // clap parses flags. Dart Sass exits 0 for `--version` (bin/sass.dart:36-40)
    // but 64 for `--help` (options.dart:540 throws `UsageException`); other
    // parse errors also exit 64. clap reports all of these via `ErrorKind`, so
    // classify explicitly rather than trusting its default exit code.
    let matches = match command.clone().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            std::process::exit(display_error_exit_code(&e));
        }
    };

    // Post-parse validation (positionals, source maps, deprecations). Mirror
    // Dart's `UsageException` handler (bin/sass.dart:69-75): print the message,
    // a blank line, then the full usage, and exit 64.
    let opts = match CliOptions::from_matches(io.clone(), &matches) {
        Ok(o) => o,
        Err(usage) => {
            println!("{}\n", usage.0);
            let mut cmd = command;
            let _ = cmd.print_long_help();
            std::process::exit(64);
        }
    };

    let arena = Bump::new();
    #[cfg(feature = "async")]
    let exit_code = futures::executor::block_on(compile::compile_all(io, &arena, &opts));
    #[cfg(not(feature = "async"))]
    let exit_code = compile::compile_all(io, &arena, &opts);
    std::process::exit(exit_code);
}

/// The process exit code for a clap parsing error.
///
/// `--version` exits 0; `--help` and every genuine parse error exit 64 (Dart
/// Sass's usage-error code). clap's own `Error::exit_code()` returns 0 for
/// `--help`, which is why it isn't used here.
fn display_error_exit_code(e: &clap::Error) -> i32 {
    match e.kind() {
        clap::error::ErrorKind::DisplayVersion => 0,
        _ => 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(args: &[&str]) -> clap::Error {
        options::build_command()
            .try_get_matches_from(args.iter())
            .unwrap_err()
    }

    #[test]
    fn help_exits_64() {
        assert_eq!(display_error_exit_code(&err(&["sass", "--help"])), 64);
        assert_eq!(display_error_exit_code(&err(&["sass", "-h"])), 64);
    }

    #[test]
    fn version_exits_0() {
        assert_eq!(display_error_exit_code(&err(&["sass", "--version"])), 0);
        assert_eq!(display_error_exit_code(&err(&["sass", "-V"])), 0);
    }

    #[test]
    fn parse_error_exits_64() {
        assert_eq!(
            display_error_exit_code(&err(&["sass", "--style", "bogus"])),
            64
        );
    }
}
