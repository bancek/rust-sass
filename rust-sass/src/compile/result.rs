// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/compile_result.dart
// go-source: go/compile/compile_result.go + go/compile/compile_stylesheet.go

use std::error::Error;
use std::fmt;

use crate::url::SassUrl;

use crate::eval::result::EvaluateResult;
use crate::serialize::SerializeResult;
use crate::sourcemap::SingleMapping;

/// The result of compiling a Sass document to CSS, along with metadata
/// about the compilation process.
///
/// Bundles the evaluation result (loaded URLs) with the serialization result
/// (CSS text and optional source map). Constructed by the compile pipeline;
/// use the accessors rather than the fields.
/// Matches Go: compile.CompileResult
pub struct CompileResult<'compile, 'parse> {
    evaluate_result: EvaluateResult<'compile, 'parse>,
    serialize_result: SerializeResult,
}

impl<'compile, 'parse> CompileResult<'compile, 'parse> {
    /// Creates a result from its two halves. Called by the compile pipeline
    /// once evaluation and serialization both succeed (or when error CSS is
    /// synthesized); not part of the public compile API surface.
    pub fn new(
        evaluate_result: EvaluateResult<'compile, 'parse>,
        serialize_result: SerializeResult,
    ) -> Self {
        CompileResult {
            evaluate_result,
            serialize_result,
        }
    }

    /// The compiled CSS.
    /// Matches Go: CompileResult.CSS()
    pub fn css(&self) -> &str {
        &self.serialize_result.css
    }

    /// The source map indicating how the source files map to [`css`](Self::css).
    ///
    /// This is `None` if source mapping was disabled for this compilation.
    /// Matches Go: CompileResult.SourceMap()
    pub fn source_map(&self) -> Option<&SingleMapping> {
        self.serialize_result.source_map.as_ref()
    }

    /// The canonical URLs of all stylesheets loaded during compilation.
    /// Matches Go: CompileResult.LoadedUrls()
    pub fn loaded_urls(&self) -> &[SassUrl] {
        &self.evaluate_result.loaded_urls
    }
}

impl fmt::Debug for CompileResult<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompileResult")
            .field("css", &self.css())
            .field("loaded_urls", &self.loaded_urls())
            .finish()
    }
}

/// An error with an associated process exit code, returned by the executable
/// layer ([`compile_stylesheet`](super::compile_stylesheet)).
///
/// Exit code 65 marks invalid Sass input (a `SassException`); 66 marks a
/// filesystem failure such as a missing input file. Dart returns the code as
/// part of a record tuple; Rust surfaces it as this error type.
/// Matches Go: compile.StylesheetError
#[derive(Debug)]
pub struct StylesheetError {
    pub exit_code: i32,
    pub message: String,
}

impl fmt::Display for StylesheetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for StylesheetError {}
