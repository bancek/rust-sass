// Copyright 2019 Google LLC. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/configuration.dart + lib/src/configured_value.dart
// go-source: go/configuration/configuration.go + go/configuration/configured_value.go

//! Module configuration: the `@use ... with (...)` variable overrides.
//!
//! A [`Configuration`] is a set of variables meant to configure a module by
//! overriding its `!default` declarations. It is either *implicit* (empty, or
//! created by importing a file containing a `@forward` rule) or *explicit*
//! (created by passing a `with` clause to a `@use` rule, carrying the node's
//! span). Both kinds pass through `@forward` rules, but an explicit
//! configuration errors when applied to an already-loaded module while an
//! implicit one is silently ignored.

use std::cell::{Ref, RefCell};
use std::collections::HashSet;

use indexmap::IndexMap;

use bumpalo::Bump;

use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::value::Value;

// === ConfiguredValue ===

/// A variable value configured for a [`Configuration`].
///
/// Pairs the value with the span where the configuration was written (or
/// `None` when configured implicitly) and the span where the variable's
/// value originated (Dart keeps the full `AstNode` as `assignmentNode`;
/// Rust keeps its span).
#[derive(Clone)]
pub struct ConfiguredValue<'parse> {
    /// The value of the variable.
    pub value: Value<'parse>,
    /// The span where the variable's configuration was written, or `None` if
    /// this value was configured implicitly.
    pub configuration_span: Option<FileSpan<'parse>>,
    /// The span where the variable's value originated.
    pub assignment_span: FileSpan<'parse>,
}

impl<'parse> ConfiguredValue<'parse> {
    /// Creates a variable value explicitly configured with a `with` clause.
    pub fn explicit(
        value: Value<'parse>,
        configuration_span: FileSpan<'parse>,
        assignment_span: FileSpan<'parse>,
    ) -> Self {
        ConfiguredValue {
            value,
            configuration_span: Some(configuration_span),
            assignment_span,
        }
    }

    /// Creates a variable value implicitly configured by setting a variable
    /// prior to an `@import` of a file that contains a `@forward`.
    pub fn implicit(value: Value<'parse>, assignment_span: FileSpan<'parse>) -> Self {
        ConfiguredValue {
            value,
            configuration_span: None,
            assignment_span,
        }
    }

    fn to_string(&self) -> SassResult<String> {
        self.value.to_display_string()
    }
}

// === Filter ===

/// One `@forward` visibility layer: a name prefix, a `show` safelist, or a
/// `hide` blocklist.
///
/// Dart stacks `UnprefixedMapView`/`LimitedMapView` wrappers around the values
/// map; Rust records the layers as data on the [`Configuration`] variant so
/// the backing map stays shared and unmodified.
#[derive(Clone, Debug)]
pub enum Filter {
    Prefix(String),
    Safelist(HashSet<String>),
    Blocklist(HashSet<String>),
}

// === ConfigurationInner ===

/// The shared backing store for a [`Configuration`]: variable names (without
/// `$`) mapped to values. Shared by reference across `@forward` derivations;
/// `remove` mutates it, which is how a used configuration variable is marked
/// even when viewed through filters.
pub struct ConfigurationInner<'parse> {
    values: IndexMap<String, ConfiguredValue<'parse>>,
}

// === Configuration enum ===

/// A set of variables meant to configure a module by overriding its
/// `!default` declarations.
///
/// May be either *implicit*, meaning that it's either empty or created by
/// importing a file containing a `@forward` rule; or *explicit*, meaning that
/// it's created by passing a `with` clause to a `@use` rule. Explicit
/// configurations carry the node's span. Dart models this as a
/// `Configuration`/`ExplicitConfiguration` class pair; Rust uses one enum
/// with shared backing storage plus accumulated [`Filter`] layers.
#[derive(Clone)]
pub enum Configuration<'parse> {
    Implicit {
        inner: &'parse RefCell<ConfigurationInner<'parse>>,
        filters: Vec<Filter>,
    },
    Explicit {
        inner: &'parse RefCell<ConfigurationInner<'parse>>,
        filters: Vec<Filter>,
        node_span: FileSpan<'parse>,
    },
}

impl<'parse> Configuration<'parse> {
    // --- private helpers ---

    // The shared backing map, regardless of variant.
    fn inner(&self) -> &'parse RefCell<ConfigurationInner<'parse>> {
        match self {
            Implicit { inner, .. } => inner,
            Explicit { inner, .. } => inner,
        }
    }

    // The `@forward` visibility layers accumulated by `through_forward`.
    fn filters_slice(&self) -> &[Filter] {
        match self {
            Implicit { filters, .. } => filters,
            Explicit { filters, .. } => filters,
        }
    }

    /// Apply filters in forward order (0..n). Returns transformed key or None if blocked.
    fn apply_forward(name: &str, filters: &[Filter]) -> Option<String> {
        let mut key = name.to_string();
        for f in filters {
            match f {
                Filter::Prefix(prefix) => key = format!("{prefix}{key}"),
                Filter::Safelist(allowed) => {
                    if !allowed.contains(&key) {
                        return None;
                    }
                }
                Filter::Blocklist(blocked) => {
                    if blocked.contains(&key) {
                        return None;
                    }
                }
            }
        }
        Some(key)
    }

    /// Apply filters in backward order (n..0). Returns visible key or None if blocked.
    fn apply_backward(inner_key: &str, filters: &[Filter]) -> Option<String> {
        let mut key = inner_key.to_string();
        for f in filters.iter().rev() {
            match f {
                Filter::Safelist(allowed) => {
                    if !allowed.contains(&key) {
                        return None;
                    }
                }
                Filter::Blocklist(blocked) => {
                    if blocked.contains(&key) {
                        return None;
                    }
                }
                Filter::Prefix(prefix) => {
                    if let Some(stripped) = key.strip_prefix(prefix.as_str()) {
                        key = stripped.to_string();
                    } else {
                        return None;
                    }
                }
            }
        }
        Some(key)
    }

    // --- constructors ---

    /// The empty configuration, which indicates that the module has not been
    /// configured. Empty configurations are always implicit, since they are
    /// ignored if the module has already been loaded.
    pub fn empty<'compile: 'parse>(arena: &'compile Bump) -> Self {
        Implicit {
            inner: arena.alloc(RefCell::new(ConfigurationInner {
                values: IndexMap::new(),
            })),
            filters: Vec::new(),
        }
    }

    /// Creates an implicit configuration with the given values.
    pub fn new_implicit<'compile: 'parse>(
        arena: &'compile Bump,
        values: IndexMap<String, ConfiguredValue<'parse>>,
    ) -> Self {
        Implicit {
            inner: arena.alloc(RefCell::new(ConfigurationInner { values })),
            filters: Vec::new(),
        }
    }

    /// Creates an explicit configuration with a values map and the node's
    /// span (Dart's `ExplicitConfiguration(values, nodeWithSpan)`).
    pub fn new_explicit<'compile: 'parse>(
        arena: &'compile Bump,
        values: IndexMap<String, ConfiguredValue<'parse>>,
        node_span: FileSpan<'parse>,
    ) -> Self {
        Explicit {
            inner: arena.alloc(RefCell::new(ConfigurationInner { values })),
            filters: Vec::new(),
            node_span,
        }
    }

    // --- query methods ---

    /// Returns whether this is an explicit (`@use ... with`) configuration.
    /// Only explicit configurations error when applied to an already-loaded
    /// module; implicit ones are silently ignored.
    pub fn is_explicit(&self) -> bool {
        matches!(self, Explicit { .. })
    }

    // The node span for explicit configurations (Dart's
    // `ExplicitConfiguration.nodeWithSpan`); `None` for implicit ones.
    pub fn node_span(&self) -> Option<FileSpan<'parse>> {
        match self {
            Implicit { .. } => None,
            Explicit { node_span, .. } => Some(*node_span),
        }
    }

    /// Returns whether no variables are visible through this configuration
    /// (accounting for `@forward` filters, not just the backing map).
    pub fn is_empty(&self) -> bool {
        let inner = self.inner().borrow();
        let filters = self.filters_slice();
        if filters.is_empty() {
            return inner.values.is_empty();
        }
        // O(n) — iterate inner keys through backward filters to check for any visible key
        for key in inner.values.keys() {
            if Self::apply_backward(key, filters).is_some() {
                return false;
            }
        }
        true
    }

    /// Looks up a variable by name through the `@forward` filters, returning
    /// a borrow into the shared backing map.
    pub fn get(&self, name: &str) -> Option<Ref<'_, ConfiguredValue<'parse>>> {
        let transformed = Self::apply_forward(name, self.filters_slice())?;
        let inner = self.inner().borrow();
        // Safe: we return a Ref that borrows the RefCell guard
        // But we can't return Ref with a borrowed key... need a different approach.
        // Let's use index lookup.
        if inner.values.contains_key(&transformed) {
            Some(Ref::map(inner, |i| &i.values[&transformed]))
        } else {
            None
        }
    }

    /// Removes a variable with `name` from this configuration, returning it.
    /// Returns `None` when the configuration is empty or holds no such
    /// variable.
    pub fn remove(&self, name: &str) -> Option<ConfiguredValue<'parse>> {
        if self.is_empty() {
            return None;
        }
        let transformed = Self::apply_forward(name, self.filters_slice())?;
        self.inner().borrow_mut().values.swap_remove(&transformed)
    }

    /// A map from variable names (without `$`) to values, filtered to what
    /// is visible through any `@forward` layers. The map may not be modified
    /// directly; use [`remove`](Self::remove) to take a value out.
    pub fn values(&self) -> IndexMap<String, ConfiguredValue<'parse>> {
        let inner = self.inner().borrow();
        let filters = self.filters_slice();
        if filters.is_empty() {
            return inner.values.clone();
        }
        let mut result = IndexMap::new();
        for (key, val) in &inner.values {
            if let Some(visible_key) = Self::apply_backward(key, filters) {
                result.insert(
                    visible_key,
                    ConfiguredValue {
                        value: val.value,
                        configuration_span: val.configuration_span,
                        assignment_span: val.assignment_span,
                    },
                );
            }
        }
        result
    }

    /// The names of all variables visible through the `@forward` filters.
    pub fn keys(&self) -> Vec<String> {
        let inner = self.inner().borrow();
        let filters = self.filters_slice();
        if filters.is_empty() {
            return inner.values.keys().cloned().collect();
        }
        let mut result = Vec::new();
        for key in inner.values.keys() {
            if let Some(visible_key) = Self::apply_backward(key, filters) {
                result.push(visible_key);
            }
        }
        result
    }

    /// Returns whether `self` and `other` derive from the same original
    /// configuration (Dart's `sameOriginal`, compared by identity). An
    /// implicit configuration never matches, because it was not created
    /// through another configuration.
    pub fn same_original(&self, other: &Self) -> bool {
        std::ptr::eq(self.inner(), other.inner())
    }

    /// Creates a new configuration from this one based on a `@forward` rule.
    /// Only variables visible through the rule's prefix/`show`/`hide` remain
    /// configurable; the copy shares the same original configuration.
    pub fn through_forward(&self, rule: &ForwardRule) -> Self {
        if self.is_empty() {
            return self.clone();
        }

        let mut new_filters = self.filters_slice().to_vec();

        if let Some(ref prefix) = rule.prefix {
            new_filters.insert(0, Filter::Prefix(prefix.clone()));
        }

        if let Some(ref shown) = rule.shown_variables {
            new_filters.insert(0, Filter::Safelist(shown.clone()));
        } else if let Some(ref hidden) = rule.hidden_variables {
            if !hidden.is_empty() {
                new_filters.insert(0, Filter::Blocklist(hidden.clone()));
            }
        }

        let shared_inner = self.inner();
        match self {
            Implicit { .. } => Implicit {
                inner: shared_inner,
                filters: new_filters,
            },
            Explicit { node_span, .. } => Explicit {
                inner: shared_inner,
                filters: new_filters,
                node_span: *node_span,
            },
        }
    }

    /// Renders the visible configuration as `($name: value, ...)` (Dart's
    /// `toString`: `$`-prefixed names joined by `,`).
    pub fn to_string(&self) -> SassResult<String> {
        let inner = self.inner().borrow();
        let filters = self.filters_slice();
        let mut parts: Vec<String> = Vec::new();
        for (key, val) in &inner.values {
            let display_key = if filters.is_empty() {
                key.clone()
            } else if let Some(k) = Self::apply_backward(key, filters) {
                k
            } else {
                continue;
            };
            let s = val.to_string()?;
            parts.push(format!("${display_key}: {s}"));
        }
        Ok(format!("({})", parts.join(",")))
    }
}

use Configuration::{Explicit, Implicit};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
    use crate::value::SassNumber;
    use crate::value::SASS_FALSE;
    use crate::value::SASS_TRUE;
    use crate::value::{Value, ValueKind};

    fn make_span<'compile, 'parse>(arena: &'compile bumpalo::Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn bogus_span() -> FileSpan<'static> {
        FileSpan::new(None, 0, 0)
    }

    fn make_bool<'compile, 'parse>(arena: &'compile bumpalo::Bump, v: bool) -> Value<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        if v {
            Value::new_with_arena(arena, ValueKind::Boolean(SASS_TRUE))
        } else {
            Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE))
        }
    }

    fn make_num<'compile, 'parse>(arena: &'compile bumpalo::Bump, n: f64) -> Value<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(n, None)))
    }

    fn make_cv<'compile, 'parse>(
        _arena: &'compile bumpalo::Bump,
        value: Value<'parse>,
    ) -> ConfiguredValue<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        ConfiguredValue::implicit(value, bogus_span())
    }

    fn empty_forward_rule() -> ForwardRule<'static> {
        ForwardRule::new(
            SassUrl::parse("file:///test").unwrap(),
            bogus_span(),
            None,
            vec![],
        )
    }

    fn forward_with_prefix(prefix: &str) -> ForwardRule<'static> {
        let mut rule = empty_forward_rule();
        rule.prefix = Some(prefix.to_string());
        rule
    }

    fn forward_with_shown(names: &[&str]) -> ForwardRule<'static> {
        let mut rule = empty_forward_rule();
        rule.shown_variables = Some(names.iter().map(|s| s.to_string()).collect());
        rule
    }

    fn forward_with_hidden(names: &[&str]) -> ForwardRule<'static> {
        let mut rule = empty_forward_rule();
        rule.hidden_variables = Some(names.iter().map(|s| s.to_string()).collect());
        rule
    }

    // --- ConfiguredValue ---

    #[test]
    fn test_configured_value_explicit() {
        let arena = bumpalo::Bump::new();
        let val = make_bool(&arena, true);
        let config_span = make_span(&arena, "$a: 1");
        let cv = ConfiguredValue::explicit(val, config_span, bogus_span());

        assert!(cv.configuration_span.is_some());
        assert_eq!(cv.value, val);
    }

    #[test]
    fn test_configured_value_implicit() {
        let arena = bumpalo::Bump::new();
        let val = make_bool(&arena, true);
        let cv = ConfiguredValue::implicit(val, bogus_span());

        assert!(cv.configuration_span.is_none());
        assert_eq!(cv.value, val);
    }

    #[test]
    fn test_configured_value_to_string() {
        let arena = bumpalo::Bump::new();
        let val = make_bool(&arena, true);
        let cv = make_cv(&arena, val);
        assert_eq!(cv.to_string().unwrap(), "true");
    }

    // --- Configuration construction ---

    #[test]
    fn test_configuration_empty() {
        let arena = bumpalo::Bump::new();
        let cfg = Configuration::empty(&arena);
        assert!(cfg.is_empty());
        assert!(!cfg.is_explicit());
    }

    #[test]
    fn test_configuration_implicit() {
        let arena = bumpalo::Bump::new();
        let val = make_bool(&arena, true);
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, val));
        let cfg = Configuration::new_implicit(&arena, values);

        assert!(!cfg.is_explicit());
        assert!(!cfg.is_empty());
        assert!(cfg.get("a").is_some());
        assert!(cfg.get("missing").is_none());
    }

    #[test]
    fn test_configuration_explicit() {
        let arena = bumpalo::Bump::new();
        let val = make_bool(&arena, true);
        let span = make_span(&arena, "node");
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, val));
        let cfg = Configuration::new_explicit(&arena, values, span);

        assert!(cfg.is_explicit());
        assert!(!cfg.is_empty());
        assert_eq!(cfg.node_span(), Some(span));
    }

    #[test]
    fn test_configuration_implicit_node_span_none() {
        let arena = bumpalo::Bump::new();
        let _arena = bumpalo::Bump::new();
        let cfg = Configuration::new_implicit(&arena, IndexMap::new());
        assert!(cfg.node_span().is_none());
    }

    // --- is_empty ---

    #[test]
    fn test_configuration_is_empty_true() {
        let arena = bumpalo::Bump::new();
        assert!(Configuration::empty(&arena).is_empty());
    }

    #[test]
    fn test_configuration_is_empty_false() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg = Configuration::new_implicit(&arena, values);
        assert!(!cfg.is_empty());
    }

    // --- get ---

    #[test]
    fn test_configuration_get_existing() {
        let arena = bumpalo::Bump::new();
        let val_a = make_num(&arena, 1.0);
        let val_b = make_num(&arena, 2.0);
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, val_a));
        values.insert("b".into(), make_cv(&arena, val_b));
        let cfg = Configuration::new_implicit(&arena, values);

        assert!(cfg.get("a").is_some());
        assert!(cfg.get("b").is_some());
    }

    #[test]
    fn test_configuration_get_missing() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg = Configuration::new_implicit(&arena, values);

        assert!(cfg.get("missing").is_none());
    }

    // --- remove ---

    #[test]
    fn test_configuration_remove_existing() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg = Configuration::new_implicit(&arena, values);

        let removed = cfg.remove("a");
        assert!(removed.is_some());
        assert!(cfg.is_empty());
    }

    #[test]
    fn test_configuration_remove_missing() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg = Configuration::new_implicit(&arena, values);

        assert!(cfg.remove("missing").is_none());
    }

    #[test]
    fn test_configuration_remove_from_empty() {
        let arena = bumpalo::Bump::new();
        let cfg = Configuration::empty(&arena);
        assert!(cfg.remove("a").is_none());
    }

    // --- values ---

    #[test]
    fn test_configuration_values() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        values.insert("b".into(), make_cv(&arena, make_num(&arena, 2.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let result = cfg.values();
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("a"));
        assert!(result.contains_key("b"));
    }

    #[test]
    fn test_configuration_values_empty() {
        let arena = bumpalo::Bump::new();
        let cfg = Configuration::empty(&arena);
        assert_eq!(cfg.values().len(), 0);
    }

    // --- keys ---

    #[test]
    fn test_configuration_keys() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        values.insert("b".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg = Configuration::new_implicit(&arena, values);

        let keys = cfg.keys();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"a".to_string()));
        assert!(keys.contains(&"b".to_string()));
    }

    // --- same_original ---

    #[test]
    fn test_configuration_same_original_same_inner() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg = Configuration::new_implicit(&arena, values);
        let derived = cfg.through_forward(&empty_forward_rule());

        assert!(cfg.same_original(&derived));
    }

    #[test]
    fn test_configuration_same_original_different() {
        let arena = bumpalo::Bump::new();
        let mut v1 = IndexMap::new();
        v1.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg1 = Configuration::new_implicit(&arena, v1);

        let mut v2 = IndexMap::new();
        v2.insert("a".into(), make_cv(&arena, make_bool(&arena, true)));
        let cfg2 = Configuration::new_implicit(&arena, v2);

        assert!(!cfg1.same_original(&cfg2));
    }

    // --- through_forward ---

    #[test]
    fn test_configuration_through_forward_empty() {
        let arena = bumpalo::Bump::new();
        let cfg = Configuration::empty(&arena);
        let derived = cfg.through_forward(&empty_forward_rule());
        let same = std::ptr::eq(cfg.inner(), derived.inner());
        assert!(same);
    }

    #[test]
    fn test_configuration_through_forward_prefix() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("ns-a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let derived = cfg.through_forward(&forward_with_prefix("ns-"));

        assert!(derived.get("a").is_some());
        assert!(derived.get("ns-a").is_none());
    }

    #[test]
    fn test_configuration_through_forward_safelist() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        values.insert("b".into(), make_cv(&arena, make_num(&arena, 2.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let derived = cfg.through_forward(&forward_with_shown(&["a"]));

        assert!(derived.get("a").is_some());
        assert!(derived.get("b").is_none());
    }

    #[test]
    fn test_configuration_through_forward_blocklist() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        values.insert("b".into(), make_cv(&arena, make_num(&arena, 2.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let derived = cfg.through_forward(&forward_with_hidden(&["b"]));

        assert!(derived.get("a").is_some());
        assert!(derived.get("b").is_none());
    }

    #[test]
    fn test_configuration_through_forward_prefix_and_safelist() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("ns-a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        values.insert("ns-b".into(), make_cv(&arena, make_num(&arena, 2.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let mut rule = empty_forward_rule();
        rule.prefix = Some("ns-".into());
        rule.shown_variables = Some(HashSet::from_iter(["a".into()]));
        let derived = cfg.through_forward(&rule);

        assert!(derived.get("a").is_some());
        assert!(derived.get("b").is_none());
    }

    #[test]
    fn test_configuration_through_forward_preserves_explicit() {
        let arena = bumpalo::Bump::new();
        let span = make_span(&arena, "node");
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        let cfg = Configuration::new_explicit(&arena, values, span);

        let derived = cfg.through_forward(&forward_with_prefix("ns-"));

        assert!(derived.is_explicit());
        assert_eq!(derived.node_span(), Some(span));
    }

    #[test]
    fn test_configuration_through_forward_preserves_implicit() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let derived = cfg.through_forward(&forward_with_prefix("ns-"));

        assert!(!derived.is_explicit());
    }

    // --- to_string ---

    #[test]
    fn test_configuration_to_string() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("a".into(), make_cv(&arena, make_num(&arena, 42.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        assert_eq!(cfg.to_string().unwrap(), "($a: 42)");
    }

    #[test]
    fn test_configuration_to_string_empty() {
        let arena = bumpalo::Bump::new();
        assert_eq!(Configuration::empty(&arena).to_string().unwrap(), "()");
    }

    // --- Remove through filter chain ---

    #[test]
    fn test_configuration_remove_through_filter_chain() {
        let arena = bumpalo::Bump::new();
        let mut values = IndexMap::new();
        values.insert("ns-a".into(), make_cv(&arena, make_num(&arena, 1.0)));
        let cfg = Configuration::new_implicit(&arena, values);

        let derived = cfg.through_forward(&forward_with_prefix("ns-"));
        let removed = derived.remove("a");
        assert!(removed.is_some());
        assert!(derived.is_empty());
        assert!(cfg.is_empty());
    }
}
