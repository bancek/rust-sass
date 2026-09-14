// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/module/shadowed_view.dart
// go-source: go/sassmodule/module_shadowed_view.go

//! A [`Module`] view exposing only the inner module's members not shadowed
//! by a blocklist of member names.
//!
//! Matches Dart: `ShadowedModuleView` (`module/shadowed_view.dart`).

use crate::module::ModuleKind;
use std::collections::HashSet;
use std::fmt::Debug;

use bumpalo::Bump;

use crate::callable::Callable;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::member_map::{self, LimitedMapView, MemberMap};
use crate::module::Module;
use crate::value::Value;

/// A [`Module`] that only exposes members not shadowed by the given
/// blocklists of member names.
///
/// The `variable_nodes` map is filtered with the variable blocklist too, so
/// namespaced node lookups agree with variable lookups.
#[derive(Clone)]
pub struct ShadowedModuleView<'compile, 'parse> {
    pub inner: Module<'compile, 'parse>,
    pub variables: &'parse dyn MemberMap<Value<'parse>>,
    pub functions: &'parse dyn MemberMap<Callable<'compile, 'parse>>,
    pub mixins: &'parse dyn MemberMap<Callable<'compile, 'parse>>,
    pub variable_nodes: &'parse dyn MemberMap<FileSpan<'parse>>,
}

impl<'compile: 'parse, 'parse> ShadowedModuleView<'compile, 'parse> {
    /// Returns a view of `inner` omitting the blocked `variables`,
    /// `functions`, and `mixins`.
    pub fn new(
        arena: &'compile Bump,
        inner: Module<'compile, 'parse>,
        variables: &HashSet<String>,
        functions: &HashSet<String>,
        mixins: &HashSet<String>,
    ) -> Self {
        ShadowedModuleView {
            variables: Self::maybe_blocklist(arena, inner.variables_view(arena), variables),
            functions: Self::maybe_blocklist(arena, inner.functions_view(arena), functions),
            mixins: Self::maybe_blocklist(arena, inner.mixins_view(arena), mixins),
            // Dart `_shadowedMap(_inner.variableNodes, variables)` — the
            // variable-nodes map is filtered too (namespaced node lookup).
            variable_nodes: Self::maybe_blocklist(
                arena,
                inner.variable_nodes_view(arena),
                variables,
            ),
            inner,
        }
    }

    fn maybe_blocklist<V: Clone + Debug + 'parse>(
        arena: &'compile Bump,
        inner_view: &'parse dyn MemberMap<V>,
        blocklist: &HashSet<String>,
    ) -> &'parse dyn MemberMap<V> {
        if blocklist.is_empty() || !member_map::needs_blocklist(inner_view, blocklist) {
            return inner_view;
        }
        arena.alloc(LimitedMapView::new_blocklist(inner_view, blocklist))
    }

    /// Sets the variable named `name` to `val`. Throws "Undefined variable."
    /// when the name is shadowed away, otherwise delegates to the inner
    /// module.
    pub fn set_variable(
        &self,
        name: &str,
        val: Value<'parse>,
        node_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        if !self.variables.has(name) {
            return Err(Box::new(SassError::Script {
                message: "Undefined variable.".into(),
                argument_name: None,
            }));
        }
        self.inner.set_variable(name, val, node_span)
    }

    /// Returns the opaque identity of `name`'s definition: the inner
    /// module's identity, since shadowing only hides members.
    pub fn variable_identity(&self, name: &str) -> SassResult<Module<'compile, 'parse>> {
        if !self.variables.has(name) {
            panic!("assertion failed: variable not found");
        }
        self.inner.variable_identity(name)
    }

    /// Whether this module exposes any of `variables` that could have been
    /// configured when the module was loaded. Delegates unchanged when no
    /// blocklist applied; otherwise restricts the query to this view's
    /// (filtered) keys before delegating.
    pub fn could_have_been_configured(&self, variables: &HashSet<String>) -> bool {
        // Dart compares map IDENTITY (`this.variables == _inner.variables`,
        // true when no blocklist was needed); the length+containment check
        // below is the value-level equivalent. Query THIS view's keys (the
        // filtered set), not the inner module's.
        let inner_vars = self.inner.variables();
        if self.variables.len() == inner_vars.len()
            && inner_vars.keys().iter().all(|k| self.variables.has(k))
        {
            return self.inner.could_have_been_configured(variables);
        }

        let filtered: HashSet<String> = self
            .variables
            .keys()
            .into_iter()
            .filter(|name| variables.contains(name))
            .collect();
        self.inner.could_have_been_configured(&filtered)
    }

    /// Like [`new`](Self::new), but returns `None` when the blocklists shadow
    /// nothing in `module`, so the caller can keep the inner module as-is.
    pub fn if_necessary(
        arena: &'compile Bump,
        module: &Module<'compile, 'parse>,
        variables: &HashSet<String>,
        functions: &HashSet<String>,
        mixins: &HashSet<String>,
    ) -> Option<Module<'compile, 'parse>> {
        let module_vars = module.variables();
        let module_funcs = module.functions();
        let module_mixins = module.mixins();
        let has_overlap = module_vars.keys().iter().any(|k| variables.contains(k))
            || module_funcs.keys().iter().any(|k| functions.contains(k))
            || module_mixins.keys().iter().any(|k| mixins.contains(k));
        if has_overlap {
            Some(Module::new(
                arena,
                ModuleKind::Shadowed(Self::new(arena, *module, variables, functions, mixins)),
            ))
        } else {
            None
        }
    }

    /// Returns whether this module exposes no members and no CSS. A
    /// memberless-but-CSS-carrying view is kept, never dropped: the
    /// environment shadowing path would otherwise lose CSS.
    pub fn is_empty(&self) -> SassResult<bool> {
        // Dart `ShadowedModuleView.isEmpty` also requires `css.children.isEmpty`
        // — without it a memberless-but-CSS-carrying view is wrongly dropped
        // by the environment shadowing path (CSS loss).
        Ok(self.variables.len() == 0
            && self.functions.len() == 0
            && self.mixins.len() == 0
            && !self.inner.transitively_contains_css())
    }

    /// Creates a copy of this module with a fresh CSS tree: clones the inner
    /// module, then re-derives the blocklists from the key difference between
    /// the cloned maps and this view's filtered maps.
    pub fn clone_css(&self, arena: &'compile Bump) -> SassResult<Module<'compile, 'parse>> {
        let cloned_inner = self.inner.clone_css(arena)?;
        let shadowed_vars =
            member_map::map_key_diff(cloned_inner.variables_view(arena), self.variables);
        let shadowed_funcs =
            member_map::map_key_diff(cloned_inner.functions_view(arena), self.functions);
        let shadowed_mixins =
            member_map::map_key_diff(cloned_inner.mixins_view(arena), self.mixins);
        Ok(Module::new(
            arena,
            ModuleKind::Shadowed(Self::new(
                arena,
                cloned_inner,
                &shadowed_vars,
                &shadowed_funcs,
                &shadowed_mixins,
            )),
        ))
    }
}
