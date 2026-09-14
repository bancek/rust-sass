// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (re-exports `isJS` from package:cli_pkg/js.dart; used at evaluate.dart:2040)
// go-source: go/eval/compat.go

/// Whether the code is running in a JavaScript/Node.js context.
/// Always `false` in this crate (native). Node.js hosts needing the
/// platform-specific Dart behavior (e.g. the `"package:"` URL error message
/// in the importer) must branch on their own binding flag.
///
/// Matches Dart's `isJS` compile-time constant (from `package:cli_pkg/js.dart`,
/// referenced in `lib/src/visitor/evaluate.dart`). Unlike Go's mutable `IsJS`
/// variable, this is a `const` — the Rust port has no JS build.
pub const IS_JS: bool = false;
