// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/async_evaluate.dart (EvaluateResult,
//   async_evaluate.dart:4893-4901; re-exported from the generated
//   evaluate.dart)
// go-source: go/eval/evaluate_result.go

use crate::url::SassUrl;

use crate::ast::css::stylesheet::CssStylesheet;
use crate::eval::import_cache::ImportCache;

/// The result of compiling a Sass document to a CSS tree, along with metadata
/// about the compilation process.
///
/// Matches Dart: `EvaluateResult = ({CssStylesheet stylesheet, Set<Uri>
/// loadedUrls})` (`async_evaluate.dart:4893-4901`). The `stylesheet` is the
/// frozen CSS tree (`combine_css` output); `loaded_urls` collects every
/// canonical URL seen during evaluation. The `import_cache` has no Dart
/// counterpart on the record — Rust threads the cache by ownership (created
/// in `compile_string`, taken back out of [`EvalState`] here) so the
/// post-evaluation source-map rewrite can reuse it; it is `None` when
/// evaluation failed before the take.
pub struct EvaluateResult<'compile, 'parse> {
    /// The CSS syntax tree.
    pub stylesheet: CssStylesheet<'parse>,

    /// The canonical URLs of all stylesheets loaded during compilation.
    pub loaded_urls: Vec<SassUrl>,

    /// The import cache used during evaluation, returned for post-evaluation
    /// processing (e.g., source map URL rewriting). None if no cache was used.
    pub import_cache: Option<ImportCache<'compile, 'parse>>,
}
