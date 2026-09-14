// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/functions.dart (unifyUniversalAndElement, _namespaceAndName)
// go-source: go/value/selector_extend.go (unifyUniversalAndElement, namespaceAndName; verified)

use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::qualified_name::QualifiedName;
use crate::selector::ty::TypeSelector;
use crate::selector::universal::UniversalSelector;

use crate::selector::SimpleSelector;

/// Returns the namespace and name for `s`, which must be a universal or a
/// type selector.
///
/// Returns a script error otherwise. Matches Dart: `_namespaceAndName`
/// (extend/functions.dart) — private there, hence a plain `//`-style note
/// would suffice, but this is `pub` here so it keeps rustdoc; the `name`
/// parameter Dart uses for error reporting is dropped.
pub fn namespace_and_name(s: &SimpleSelector<'_>) -> SassResult<(Option<String>, Option<String>)> {
    match s {
        SimpleSelector::Universal(ref u) => Ok((u.namespace.clone(), None)),
        SimpleSelector::Type(ref t) => Ok((t.name.namespace.clone(), Some(t.name.name.clone()))),
        _ => Err(Box::new(SassError::Script {
            message: "must be UniversalSelector or TypeSelector".into(),
            argument_name: None,
        })),
    }
}

/// Returns a simple selector matching only elements matched by both `s1` and
/// `s2`, which must both be universal or type selectors.
///
/// `span` is used for the new selector. Returns `None` when no such selector
/// exists (conflicting namespaces or names); a wildcard (`*`/`None`)
/// namespace or name defers to the other side. Matches Dart:
/// `unifyUniversalAndElement` (extend/functions.dart) — Dart takes no `span`
/// and reuses `selector1.span`; this port threads it explicitly. The Go
/// `go-source:` path above is verified to exist; no Go text was copied.
pub fn unify_universal_and_element<'parse>(
    s1: &SimpleSelector<'parse>,
    s2: &SimpleSelector<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<Option<SimpleSelector<'parse>>> {
    let (ns1, n1) = namespace_and_name(s1)?;
    let (ns2, n2) = namespace_and_name(s2)?;

    let ns = if ns1 == ns2 || ns2.as_deref() == Some("*") {
        ns1
    } else if ns1.as_deref() == Some("*") {
        ns2
    } else {
        return Ok(None);
    };

    let n = if n1 == n2 || n2.is_none() {
        n1
    } else if n1.is_none() || n1.as_deref() == Some("*") {
        n2
    } else {
        return Ok(None);
    };

    Ok(Some(match n {
        None => SimpleSelector::Universal(UniversalSelector::new(span, ns)),
        Some(name) => SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new_with_namespace(name, ns),
            span,
        )),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;

    fn ty(name: &str) -> SimpleSelector<'static> {
        SimpleSelector::Type(TypeSelector::new(
            QualifiedName::new(name.into()),
            BOGUS_SPAN,
        ))
    }

    fn univ(ns: Option<&str>) -> SimpleSelector<'static> {
        SimpleSelector::Universal(UniversalSelector::new(BOGUS_SPAN, ns.map(|s| s.into())))
    }

    #[test]
    fn test_both_type() {
        let a = ty("div");
        let b = ty("div");
        let r = unify_universal_and_element(&a, &b, BOGUS_SPAN).unwrap();
        assert!(r.is_some());
        if let SimpleSelector::Type(ref ts) = r.unwrap() {
            assert_eq!(ts.name.name, "div");
        } else {
            panic!("expected TypeSelector");
        }
    }

    #[test]
    fn test_universal_wildcard_ns_type() {
        let u = univ(Some("*"));
        let t = ty("div");
        let r = unify_universal_and_element(&u, &t, BOGUS_SPAN).unwrap();
        assert!(r.is_some());
        if let SimpleSelector::Type(ref ts) = r.unwrap() {
            assert_eq!(ts.name.name, "div");
        } else {
            panic!("expected TypeSelector");
        }
    }

    #[test]
    fn test_type_universal_wildcard_ns() {
        let t = ty("span");
        let u = univ(Some("*"));
        let r = unify_universal_and_element(&t, &u, BOGUS_SPAN).unwrap();
        assert!(r.is_some());
        if let SimpleSelector::Type(ref ts) = r.unwrap() {
            assert_eq!(ts.name.name, "span");
        } else {
            panic!("expected TypeSelector");
        }
    }

    #[test]
    fn test_conflicting_ns() {
        let a = univ(Some("svg"));
        let b = univ(Some("html"));
        let r = unify_universal_and_element(&a, &b, BOGUS_SPAN).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_universal_nil_name_type() {
        let u = univ(None);
        let t = ty("div");
        let r = unify_universal_and_element(&u, &t, BOGUS_SPAN).unwrap();
        assert!(r.is_some());
        if let SimpleSelector::Type(ref ts) = r.unwrap() {
            assert_eq!(ts.name.name, "div");
        } else {
            panic!("expected TypeSelector");
        }
    }

    #[test]
    fn test_both_universal() {
        let a = univ(Some("svg"));
        let b = univ(Some("svg"));
        let r = unify_universal_and_element(&a, &b, BOGUS_SPAN).unwrap();
        assert!(r.is_some());
        if let SimpleSelector::Universal(ref us) = r.unwrap() {
            assert_eq!(us.namespace.as_deref(), Some("svg"));
        } else {
            panic!("expected UniversalSelector");
        }
    }

    #[test]
    fn test_name_conflict() {
        let a = ty("div");
        let b = ty("span");
        let r = unify_universal_and_element(&a, &b, BOGUS_SPAN).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_universal_rhs() {
        let t = ty("span");
        let u = univ(None);
        let r = unify_universal_and_element(&t, &u, BOGUS_SPAN).unwrap();
        assert!(r.is_some());
        if let SimpleSelector::Type(ref ts) = r.unwrap() {
            assert_eq!(ts.name.name, "span");
        } else {
            panic!("expected TypeSelector");
        }
    }
}
