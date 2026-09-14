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

use crate::value::hash::{hash_combine, string_hash_code, string_opt_hash_code};

/// A [qualified name].
///
/// [qualified name]: https://www.w3.org/TR/css3-namespace/#css-qnames
#[derive(Clone, Debug)]
pub struct QualifiedName {
    /// The identifier name.
    pub name: String,
    /// The namespace name.
    ///
    /// `None` means `name` belongs to the default namespace; `Some("")`
    /// means it belongs to no namespace; `Some("*")` means it belongs to any
    /// namespace; otherwise it belongs to the given namespace.
    pub namespace: Option<String>,
}

impl QualifiedName {
    /// Creates a qualified name in the default namespace.
    pub fn new(name: String) -> Self {
        QualifiedName {
            name,
            namespace: None,
        }
    }

    /// Creates a qualified name with an explicit namespace (`None` for the
    /// default namespace, `Some("*")` for any).
    pub fn new_with_namespace(name: String, namespace: Option<String>) -> Self {
        QualifiedName { name, namespace }
    }

    // Matches Dart: combines the name and namespace hashes.
    pub fn hash_code(&self) -> i32 {
        hash_combine(
            string_hash_code(&self.name),
            string_opt_hash_code(&self.namespace),
        )
    }
}

// Renders `name`, or `namespace|name` when a namespace is present,
// matching Dart's `toString()`.
impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.namespace {
            None => write!(f, "{}", self.name),
            Some(ns) => write!(f, "{ns}|{}", self.name),
        }
    }
}

// Matches Dart: structural equality over name and namespace.
impl PartialEq for QualifiedName {
    fn eq(&self, other: &Self) -> bool {
        if self.name != other.name {
            return false;
        }
        match (&self.namespace, &other.namespace) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for QualifiedName {}

// Hashes via `hash_code`, matching Dart's `hashCode`.
impl Hash for QualifiedName {
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
        let q = QualifiedName::new("div".into());
        assert_eq!(q.name, "div");
        assert_eq!(q.namespace, None);
    }

    #[test]
    fn test_new_with_namespace() {
        let ns = Some("svg".to_string());
        let q = QualifiedName::new_with_namespace("circle".into(), ns.clone());
        assert_eq!(q.name, "circle");
        assert_eq!(q.namespace, ns);
    }

    #[test]
    fn test_new_with_empty_namespace() {
        let ns = Some(String::new());
        let q = QualifiedName::new_with_namespace("test".into(), ns);
        assert_eq!(q.namespace, Some(String::new()));
    }

    #[test]
    fn test_eq_same_name_no_namespace() {
        let q1 = QualifiedName::new("div".into());
        let q2 = QualifiedName::new("div".into());
        assert_eq!(q1, q2);
    }

    #[test]
    fn test_eq_different_name() {
        let q1 = QualifiedName::new("div".into());
        let q2 = QualifiedName::new("span".into());
        assert_ne!(q1, q2);
    }

    #[test]
    fn test_eq_one_nil_one_non_nil_namespace() {
        let q1 = QualifiedName::new("div".into());
        let q2 = QualifiedName::new_with_namespace("div".into(), Some("svg".into()));
        assert_ne!(q1, q2);
    }

    #[test]
    fn test_eq_same_namespace() {
        let q1 = QualifiedName::new_with_namespace("div".into(), Some("svg".into()));
        let q2 = QualifiedName::new_with_namespace("div".into(), Some("svg".into()));
        assert_eq!(q1, q2);
    }

    #[test]
    fn test_eq_different_namespace() {
        let q1 = QualifiedName::new_with_namespace("div".into(), Some("svg".into()));
        let q2 = QualifiedName::new_with_namespace("div".into(), Some("html".into()));
        assert_ne!(q1, q2);
    }

    #[test]
    fn test_eq_both_empty_namespace() {
        let q1 = QualifiedName::new_with_namespace("div".into(), Some(String::new()));
        let q2 = QualifiedName::new_with_namespace("div".into(), Some(String::new()));
        assert_eq!(q1, q2);
    }

    #[test]
    fn test_display_no_namespace() {
        let q = QualifiedName::new("div".into());
        assert_eq!(q.to_string(), "div");
    }

    #[test]
    fn test_display_with_namespace() {
        let q = QualifiedName::new_with_namespace("circle".into(), Some("svg".into()));
        assert_eq!(q.to_string(), "svg|circle");
    }

    #[test]
    fn test_display_any_namespace() {
        let q = QualifiedName::new_with_namespace("div".into(), Some("*".into()));
        assert_eq!(q.to_string(), "*|div");
    }

    #[test]
    fn test_display_empty_namespace() {
        let q = QualifiedName::new_with_namespace("div".into(), Some(String::new()));
        assert_eq!(q.to_string(), "|div");
    }

    #[test]
    fn test_hash_code_deterministic() {
        let q1 = QualifiedName::new("div".into());
        let q2 = QualifiedName::new("div".into());
        assert_eq!(q1.hash_code(), q2.hash_code());
    }

    #[test]
    fn test_hash_code_different_name() {
        let q1 = QualifiedName::new("div".into());
        let q2 = QualifiedName::new("span".into());
        assert_ne!(q1.hash_code(), q2.hash_code());
    }

    #[test]
    fn test_hash_code_with_namespace() {
        let q1 = QualifiedName::new_with_namespace("div".into(), Some("svg".into()));
        let q2 = QualifiedName::new_with_namespace("div".into(), Some("svg".into()));
        assert_eq!(q1.hash_code(), q2.hash_code());
    }

    #[test]
    fn test_hash_trait_delegates_to_hash_code() {
        let q1 = QualifiedName::new("div".into());
        let q2 = QualifiedName::new("div".into());
        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        Hash::hash(&q1, &mut h1);
        Hash::hash(&q2, &mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }
}
