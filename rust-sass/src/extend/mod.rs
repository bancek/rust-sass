// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/extension_store.dart + lib/src/extend/extension.dart
//   + lib/src/extend/merged_extension.dart + lib/src/extend/mode.dart
//   + lib/src/extend/empty_extension_store.dart + lib/src/extend/functions.dart
//   + lib/src/util/box.dart
// go-source: go/extend/default_extension_store.go + go/extend/extension.go
//   + go/extend/extension_merged.go + go/extend/extend_mode.go
//   + go/extend/empty_extension_store.go + go/extend/extension_store.go
//   + go/box/box.go

//! The `@extend` store: registers extensions and retroactively applies them
//! to selectors.
//!
//! Matches Dart `lib/src/extend/` (`extension_store.dart` with its
//! `empty_extension_store.dart` const-empty variant, `extension.dart`,
//! `merged_extension.dart`, `mode.dart`, plus the `functions.dart`
//! extend-algorithm helpers that live in [`store`] here).

pub mod extension;
pub mod merged;
pub mod mode;
pub mod store;

pub use extension::{media_queries_equal, span_message, BaseExtension, Extender, Extension};
pub use merged::{merge_extensions, unmerge_extensions};
pub use mode::ExtendMode;
pub use store::{extend_static, replace_static, ExtensionStore, StoreBox};
