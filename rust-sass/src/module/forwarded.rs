// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/module/forwarded_view.dart
// go-source: go/sassmodule/module_forwarded_view.go

//! A [`Module`] view exposing an inner module's members through a
//! [`ForwardRule`]'s prefix/show/hide filters.
//!
//! Matches Dart: `ForwardedModuleView` (`module/forwarded_view.dart`).

use crate::module::ModuleKind;
use std::collections::HashSet;

use bumpalo::Bump;

use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::callable::Callable;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::member_map::{self, MemberMap};
use crate::module::Module;
use crate::value::Value;

/// A [`Module`] that exposes members according to a [`ForwardRule`].
///
/// The `variables`/`variable_nodes` maps share the rule's variable filters;
/// `functions`/`mixins` share the mixin-and-function filters. All four are
/// precomputed filtered views over the inner module.
#[derive(Clone)]
pub struct ForwardedModuleView<'compile, 'parse> {
    pub inner: Module<'compile, 'parse>,
    pub rule: ForwardRule<'parse>,
    pub variables: &'parse dyn MemberMap<Value<'parse>>,
    pub variable_nodes: &'parse dyn MemberMap<FileSpan<'parse>>,
    pub functions: &'parse dyn MemberMap<Callable<'compile, 'parse>>,
    pub mixins: &'parse dyn MemberMap<Callable<'compile, 'parse>>,
}

impl<'compile: 'parse, 'parse> ForwardedModuleView<'compile, 'parse> {
    /// Like [`new`](Self::new), but returns `inner` as-is when the `rule`
    /// needs no modification (no prefix, no show lists, empty hide lists).
    pub fn if_necessary(
        arena: &'compile Bump,
        inner: Module<'compile, 'parse>,
        rule: &ForwardRule<'parse>,
    ) -> Module<'compile, 'parse> {
        if rule.prefix.is_none()
            && rule.shown_mixins_and_functions.is_none()
            && rule.shown_variables.is_none()
            && (rule.hidden_mixins_and_functions.is_none()
                || rule
                    .hidden_mixins_and_functions
                    .as_ref()
                    .is_some_and(|h| h.is_empty()))
            && (rule.hidden_variables.is_none()
                || rule.hidden_variables.as_ref().is_some_and(|h| h.is_empty()))
        {
            return inner;
        }
        Module::new(arena, ModuleKind::Forwarded(Self::new(arena, inner, rule)))
    }

    /// Wraps `inner` so it only shows members allowed by the `rule`'s
    /// prefix/show/hide filters.
    pub fn new(
        arena: &'compile Bump,
        inner: Module<'compile, 'parse>,
        rule: &ForwardRule<'parse>,
    ) -> Self {
        ForwardedModuleView {
            variables: member_map::forwarded_map_ordered(
                arena,
                inner.variables_view(arena),
                rule.prefix.as_deref(),
                rule.shown_order_vars.as_deref(),
                rule.shown_variables.as_ref(),
                rule.hidden_variables.as_ref(),
            ),
            variable_nodes: member_map::forwarded_map_ordered(
                arena,
                inner.variable_nodes_view(arena),
                rule.prefix.as_deref(),
                rule.shown_order_vars.as_deref(),
                rule.shown_variables.as_ref(),
                rule.hidden_variables.as_ref(),
            ),
            functions: member_map::forwarded_map_ordered(
                arena,
                inner.functions_view(arena),
                rule.prefix.as_deref(),
                rule.shown_order_mf.as_deref(),
                rule.shown_mixins_and_functions.as_ref(),
                rule.hidden_mixins_and_functions.as_ref(),
            ),
            mixins: member_map::forwarded_map_ordered(
                arena,
                inner.mixins_view(arena),
                rule.prefix.as_deref(),
                rule.shown_order_mf.as_deref(),
                rule.shown_mixins_and_functions.as_ref(),
                rule.hidden_mixins_and_functions.as_ref(),
            ),
            inner,
            rule: rule.clone(),
        }
    }

    /// Sets the variable named `name` to `val`, translating through the
    /// rule's prefix and show/hide lists. Throws "Undefined variable." when
    /// the name is hidden or lacks the prefix, then delegates to the inner
    /// module with the prefix stripped.
    pub fn set_variable(
        &self,
        name: &str,
        val: Value<'parse>,
        node_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        if let Some(ref shown) = self.rule.shown_variables {
            if !shown.contains(name) {
                return Err(Box::new(SassError::Script {
                    message: "Undefined variable.".into(),
                    argument_name: None,
                }));
            }
        } else if let Some(ref hidden) = self.rule.hidden_variables {
            if hidden.contains(name) {
                return Err(Box::new(SassError::Script {
                    message: "Undefined variable.".into(),
                    argument_name: None,
                }));
            }
        }

        let inner_name = if let Some(ref prefix) = self.rule.prefix {
            if let Some(stripped) = name.strip_prefix(prefix.as_str()) {
                stripped
            } else {
                return Err(Box::new(SassError::Script {
                    message: "Undefined variable.".into(),
                    argument_name: None,
                }));
            }
        } else {
            name
        };

        self.inner.set_variable(inner_name, val, node_span)
    }

    /// Returns the opaque identity of `name`'s definition: the inner
    /// module's identity for the prefix-stripped name.
    pub fn variable_identity(&self, name: &str) -> SassResult<Module<'compile, 'parse>> {
        if !self.variables.has(name) {
            panic!("assertion failed: variable not found");
        }

        let inner_name = if let Some(ref prefix) = self.rule.prefix {
            if !name.starts_with(prefix.as_str()) {
                panic!("assertion failed: variable name does not have prefix");
            }
            &name[prefix.len()..]
        } else {
            name
        };

        self.inner.variable_identity(inner_name)
    }

    /// Whether this module exposes any of `variables` that could have been
    /// configured when the module was loaded. Translates the names back
    /// through the prefix/show/hide filters before delegating to the inner
    /// module; delegates unchanged when the rule filters nothing.
    pub fn could_have_been_configured(&self, variables: &HashSet<String>) -> bool {
        if self.rule.prefix.is_none()
            && self.rule.shown_variables.is_none()
            && (self.rule.hidden_variables.is_none()
                || self
                    .rule
                    .hidden_variables
                    .as_ref()
                    .is_some_and(|h| h.is_empty()))
        {
            return self.inner.could_have_been_configured(variables);
        }

        let mut filtered = variables.clone();

        if let Some(ref prefix) = self.rule.prefix {
            filtered = filtered
                .iter()
                .filter(|name| name.starts_with(prefix.as_str()))
                .map(|name| name[prefix.len()..].to_string())
                .collect();
        }

        if let Some(ref safelist) = self.rule.shown_variables {
            filtered = filtered.intersection(safelist).cloned().collect();
        } else if let Some(ref blocklist) = self.rule.hidden_variables {
            if !blocklist.is_empty() {
                filtered = filtered.difference(blocklist).cloned().collect();
            }
        }

        self.inner.could_have_been_configured(&filtered)
    }

    /// Creates a copy of this module with a fresh CSS tree: clones the inner
    /// module and re-applies the same rule.
    pub fn clone_css(&self, arena: &'compile Bump) -> SassResult<Module<'compile, 'parse>> {
        let cloned_inner = self.inner.clone_css(arena)?;
        Ok(Module::new(
            arena,
            ModuleKind::Forwarded(Self::new(arena, cloned_inner, &self.rule)),
        ))
    }
}
