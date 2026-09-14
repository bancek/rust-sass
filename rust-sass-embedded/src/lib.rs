// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/executable.dart
// go-source: go/embedded/executable.go

pub mod compilation;
pub mod dispatcher;
pub use rust_sass_embedded_pb::embedded_sass;
pub mod error;
pub mod function;
pub mod importer_file;
pub mod importer_host;
pub mod logger;
pub mod opaque_registry;
pub mod packet;
pub mod protofier;

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use rust_sass::common::exception::{SassError, SassResult};
use rust_sass::parse::stylesheet::{CssState, SassIndentState, Syntax};

pub use dispatcher::Dispatcher;

/// The embedded compiler protocol version (sass.embedded_protocol).
pub const PROTOCOL_VERSION: &str = "3.2.0";
/// The compiler version reported to the host.
pub const COMPILER_VERSION: &str = "1.104.0";

/// Runs the embedded compiler over stdin/stdout.
///
/// Matches Go: embedded.Run (executable.go).
pub fn run(args: &[String]) -> ! {
    if args.contains(&"--version".to_string()) {
        print!("{}", version_response_json());
        std::process::exit(0);
    } else if !args.is_empty() {
        eprintln!(
            "sass --embedded is not intended to be executed with additional arguments.\n\
             See https://github.com/sass/dart-sass#embedded-dart-sass for details."
        );
        std::process::exit(64);
    } else {
        let reader: Box<dyn Read + Send> = Box::new(io::stdin());
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(io::stdout())));
        let stderr: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(io::stderr())));
        std::process::exit(Dispatcher::new(reader, writer, stderr).listen());
    }
}

/// Maps a proto `Syntax` value to the compiler's `Syntax`. Returns a `Script`
/// error for unknown values (Go: `syntaxToSyntax`).
pub(crate) fn syntax_from_proto(syntax: i32) -> SassResult<Syntax> {
    match syntax {
        x if x == embedded_sass::Syntax::Scss as i32 => Ok(Syntax::Scss),
        x if x == embedded_sass::Syntax::Indented as i32 => Ok(Syntax::Sass(SassIndentState {
            current_indentation: 0,
            next_indentation: None,
            next_indentation_end: None,
            indent_spaces: None,
        })),
        x if x == embedded_sass::Syntax::Css as i32 => Ok(Syntax::Css(CssState {
            disallowed_function_names: HashSet::new(),
        })),
        _ => Err(Box::new(SassError::Script {
            message: format!("Unknown syntax {syntax}"),
            argument_name: None,
        })),
    }
}

/// The `--embedded --version` output: protojson (camelCase, 2-space indent) of
/// the version response. Matches Dart's `--embedded --version` byte-for-byte
/// (including the `id: 0` field that Go's protojson omits). Built from the
/// version consts so a bump can't leave a stale copy behind.
pub fn version_response_json() -> String {
    format!(
        "{{\n  \"protocolVersion\": \"{PROTOCOL_VERSION}\",\n  \"compilerVersion\": \"{COMPILER_VERSION}\",\n  \"implementationVersion\": \"{COMPILER_VERSION}\",\n  \"implementationName\": \"dart-sass\",\n  \"id\": 0\n}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_response_json_matches_dart_bytes() {
        assert_eq!(
            version_response_json(),
            "{\n  \"protocolVersion\": \"3.2.0\",\n  \"compilerVersion\": \"1.104.0\",\n  \"implementationVersion\": \"1.104.0\",\n  \"implementationName\": \"dart-sass\",\n  \"id\": 0\n}"
        );
    }
}
