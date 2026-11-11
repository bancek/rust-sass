// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/selector/qualified_name.dart
// go-source: go/value/selector_qualified_name.go

use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;

use crate::common::exception::SassResult;
use crate::util::css_identifier::to_css_identifier;
use crate::value::hash::{hash_combine, string_hash_code};

/// A namespace for a [`QualifiedName`] or a [`UniversalSelector`].
///
/// Matches Dart: `SelectorNamespace` (`qualified_name.dart`), a sealed class
/// introduced in dart-sass 1.106.0. Rust models the variants as an enum.
///
/// [`UniversalSelector`]: crate::selector::universal::UniversalSelector
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectorNamespace<'parse> {
    /// The default namespace for the document (`div`).
    Default,
    /// Matches any possible namespace (`*|div`).
    Any,
    /// The absence of a namespace (`|div`).
    None,
    /// An explicitly named namespace (`foo|div`).
    Named(&'parse str),
}

impl<'parse> SelectorNamespace<'parse> {
    /// Creates an explicitly-named namespace.
    pub fn named(name: &'parse str) -> Self {
        SelectorNamespace::Named(name)
    }

    /// Returns whether this is the default namespace.
    pub fn is_default(&self) -> bool {
        matches!(self, SelectorNamespace::Default)
    }

    /// Returns whether this matches any namespace.
    pub fn is_any(&self) -> bool {
        matches!(self, SelectorNamespace::Any)
    }

    pub(crate) fn hash_code(&self) -> i32 {
        match self {
            SelectorNamespace::Default => 0,
            SelectorNamespace::Any => 1,
            SelectorNamespace::None => 2,
            SelectorNamespace::Named(name) => string_hash_code(name),
        }
    }
}

/// A [qualified name].
///
/// [qualified name]: https://www.w3.org/TR/css3-namespace/#css-qnames
#[derive(Clone, Copy, Debug)]
pub struct QualifiedName<'parse> {
    /// The identifier name, with escapes resolved.
    pub name: &'parse str,
    /// The namespace.
    pub namespace: SelectorNamespace<'parse>,
}

impl<'parse> QualifiedName<'parse> {
    /// Creates a qualified name in the default namespace.
    pub fn new(name: &'parse str) -> Self {
        QualifiedName {
            name,
            namespace: SelectorNamespace::Default,
        }
    }

    /// Creates a qualified name with an explicit namespace.
    pub fn new_with_namespace(name: &'parse str, namespace: SelectorNamespace<'parse>) -> Self {
        QualifiedName { name, namespace }
    }

    /// Renders this name, matching Dart's `QualifiedName.toString`.
    pub fn to_css_string(&self) -> SassResult<String> {
        let name = to_css_identifier(self.name)?;
        match &self.namespace {
            SelectorNamespace::Default => Ok(name),
            SelectorNamespace::Any => Ok(format!("*|{name}")),
            SelectorNamespace::None => Ok(format!("|{name}")),
            SelectorNamespace::Named(ns) => Ok(format!("{}|{name}", to_css_identifier(ns)?)),
        }
    }

    // Matches Dart: combines the name and namespace hashes.
    pub fn hash_code(&self) -> i32 {
        hash_combine(string_hash_code(self.name), self.namespace.hash_code())
    }
}

// Renders `name`, or `namespace|name` when a namespace is present,
// matching Dart's `toString()`.
impl fmt::Display for QualifiedName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_css_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => f.write_str(self.name),
        }
    }
}

// Matches Dart: structural equality over name and namespace.
impl PartialEq for QualifiedName<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.namespace == other.namespace
    }
}

impl Eq for QualifiedName<'_> {}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl Hash for QualifiedName<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    #[test]
    fn test_new_qualified_name() {
        let q = QualifiedName::new("div");
        assert_eq!(q.name, "div");
        assert_eq!(q.namespace, SelectorNamespace::Default);
    }

    #[test]
    fn test_new_with_namespace() {
        let ns = SelectorNamespace::named("svg");
        let q = QualifiedName::new_with_namespace("circle", ns);
        assert_eq!(q.name, "circle");
        assert_eq!(q.namespace, ns);
    }

    #[test]
    fn test_new_with_empty_namespace() {
        let q = QualifiedName::new_with_namespace("test", SelectorNamespace::None);
        assert_eq!(q.namespace, SelectorNamespace::None);
    }

    #[test]
    fn test_eq_same_name_no_namespace() {
        let q1 = QualifiedName::new("div");
        let q2 = QualifiedName::new("div");
        assert_eq!(q1, q2);
    }

    #[test]
    fn test_eq_different_name() {
        let q1 = QualifiedName::new("div");
        let q2 = QualifiedName::new("span");
        assert_ne!(q1, q2);
    }

    #[test]
    fn test_eq_same_namespace() {
        let ns = SelectorNamespace::named("svg");
        let q1 = QualifiedName::new_with_namespace("div", ns);
        let q2 = QualifiedName::new_with_namespace("div", ns);
        assert_eq!(q1, q2);
    }

    #[test]
    fn test_eq_different_namespace() {
        let q1 = QualifiedName::new_with_namespace("div", SelectorNamespace::named("svg"));
        let q2 = QualifiedName::new_with_namespace("div", SelectorNamespace::named("html"));
        assert_ne!(q1, q2);
    }

    #[test]
    fn test_eq_none_namespaces() {
        let q1 = QualifiedName::new_with_namespace("div", SelectorNamespace::None);
        let q2 = QualifiedName::new_with_namespace("div", SelectorNamespace::None);
        assert_eq!(q1, q2);
    }

    #[test]
    fn test_hash_same() {
        let q1 = QualifiedName::new("div");
        let q2 = QualifiedName::new("div");
        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        q1.hash(&mut h1);
        q2.hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn test_display_default() {
        let q = QualifiedName::new("div");
        assert_eq!(q.to_string(), "div");
    }

    #[test]
    fn test_display_named() {
        let q = QualifiedName::new_with_namespace("div", SelectorNamespace::named("svg"));
        assert_eq!(q.to_string(), "svg|div");
    }

    #[test]
    fn test_display_any() {
        let q = QualifiedName::new_with_namespace("div", SelectorNamespace::Any);
        assert_eq!(q.to_string(), "*|div");
    }

    #[test]
    fn test_display_none() {
        let q = QualifiedName::new_with_namespace("div", SelectorNamespace::None);
        assert_eq!(q.to_string(), "|div");
    }

    #[test]
    fn test_display_escaped_name() {
        let q = QualifiedName::new("foo.bar");
        assert_eq!(q.to_string(), "foo\\.bar");
    }
}
