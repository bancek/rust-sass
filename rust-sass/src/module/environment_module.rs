// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/environment.dart (_EnvironmentModule class, L961-1152)
// go-source: go/sassenv/environment.go (environmentModule)

//! The module view over an [`Environment`]: the top-level members a
//! stylesheet defines, plus everything it forwards.
//!
//! Matches Dart: `_EnvironmentModule` (`environment.dart:961-1152`).

use crate::ast::css::clone_css::clone_css_stylesheet;
use std::collections::HashSet;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::ast::css::comment::CssComment;
use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::stylesheet::ModifiableCssStylesheet;
use crate::callable::Callable;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;
use crate::environment::Environment;
use crate::extend::store::ExtensionStore;
use crate::member_map::{self, MemberMap, RefCellMap};
use crate::module::Module;
use crate::module::ModuleKind;
use crate::value::Value;

/// A module that represents the top-level members defined in an
/// [`Environment`].
///
/// Member views merge the environment's root scope with the forwarded
/// modules' members; `modules_by_variable` maps each forwarded variable back
/// to its originating module so [`set_variable`](Self::set_variable) and
/// [`variable_identity`](Self::variable_identity) delegate correctly.
#[derive(Clone)]
pub struct EnvironmentModule<'compile, 'parse> {
    pub environment: Environment<'compile, 'parse>,
    pub upstream: Vec<Module<'compile, 'parse>>,
    pub variables: &'parse dyn MemberMap<Value<'parse>>,
    pub variable_nodes: &'parse dyn MemberMap<FileSpan<'parse>>,
    pub functions: &'parse dyn MemberMap<Callable<'compile, 'parse>>,
    pub mixins: &'parse dyn MemberMap<Callable<'compile, 'parse>>,
    pub extension_store: ExtensionStore<'parse>,
    pub pre_module_comments: IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>>,
    pub css: ModifiableCssNode<'parse>,
    pub transitively_contains_css: bool,
    pub transitively_contains_extensions: bool,
    pub(crate) modules_by_variable: IndexMap<String, Module<'compile, 'parse>>,
}

impl<'compile: 'parse, 'parse> EnvironmentModule<'compile, 'parse> {
    /// Creates the module for `environment`'s top-level members, holding
    /// `css`/`pre_module_comments`/`extension_store` and exposing `forwarded`
    /// members downstream. `upstream` is every module `@use`d so far, used to
    /// derive the transitive CSS/extension flags.
    ///
    /// Named `new` but returns the unified [`Module`] handle by design (the
    /// environment module is wrapped in `ModuleKind::Environment`).
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        arena: &'compile Bump,
        environment: Environment<'compile, 'parse>,
        css: ModifiableCssNode<'parse>,
        pre_module_comments: IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>>,
        extension_store: ExtensionStore<'parse>,
        forwarded: &[Module<'compile, 'parse>],
        all_modules: &[Module<'compile, 'parse>],
    ) -> Module<'compile, 'parse> {
        let modules_by_variable = make_modules_by_variable(forwarded);
        let (vars, var_nodes, fns, mixs) = build_member_views(arena, &environment, forwarded);

        let transitively_contains_css = !css.children().unwrap_or_default().is_empty()
            || !pre_module_comments.is_empty()
            || all_modules.iter().any(|m| m.transitively_contains_css());

        let transitively_contains_extensions = !extension_store.is_empty()
            || all_modules
                .iter()
                .any(|m| m.transitively_contains_extensions());

        let env_module = EnvironmentModule {
            environment,
            upstream: all_modules.to_vec(),
            variables: vars,
            variable_nodes: var_nodes,
            functions: fns,
            mixins: mixs,
            extension_store,
            pre_module_comments,
            css,
            transitively_contains_css,
            transitively_contains_extensions,
            modules_by_variable,
        };

        Module::new(arena, ModuleKind::Environment(Box::new(env_module)))
    }

    /// Returns a module with the same members and upstream modules as
    /// `environment`, but an empty stylesheet and extension store.
    ///
    /// Used when resolving imports, which need to inject forwarded members
    /// into the current scope. The only case where a nested environment
    /// becomes a module.
    pub fn new_dummy(
        environment: Environment<'compile, 'parse>,
        forwarded: &[Module<'compile, 'parse>],
        all_modules: &[Module<'compile, 'parse>],
        arena: &'compile Bump,
    ) -> Module<'compile, 'parse> {
        let fs = FileSource::new_in(arena, "", None);
        let span = FileSpan::new(Some(fs), 0, 0);
        let css = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(span)),
        );
        Self::new(
            arena,
            environment,
            css,
            IndexMap::new(),
            ExtensionStore::Empty,
            forwarded,
            all_modules,
        )
    }

    /// Sets the variable named `name` to `val`. Variables originating from a
    /// forwarded module delegate to that module; otherwise the variable must
    /// exist in the environment's root scope ("Undefined variable.").
    pub fn set_variable(
        &self,
        name: &str,
        val: Value<'parse>,
        node_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        if let Some(module) = self.modules_by_variable.get(name) {
            return module.set_variable(name, val, node_span);
        }

        let inner = self.environment.0.borrow();
        if !inner.variables[0].borrow().contains_key(name) {
            return Err(Box::new(SassError::Script {
                message: "Undefined variable.".into(),
                argument_name: None,
            }));
        }
        drop(inner);

        self.environment
            .set_variable(name, val, node_span, None, true)
    }

    /// Returns the opaque identity of `name`'s definition: the originating
    /// forwarded module's identity when the variable was forwarded, else this
    /// module itself. Callers must use [`Module::variable_identity`] (this
    /// stub panics for unknown names like Dart's `assert`).
    pub fn variable_identity(&self, name: &str) -> SassResult<Module<'compile, 'parse>> {
        if !self.variables.has(name) {
            panic!("assertion failed: variable {name} not found in module");
        }
        if let Some(module) = self.modules_by_variable.get(name) {
            return module.variable_identity(name);
        }
        Err(Box::new(SassError::Script {
            message: "variable_identity: use Module::variable_identity() instead".into(),
            argument_name: None,
        }))
    }

    /// Whether this module exposes any of `variables` that could have been
    /// configured when the module was loaded: either a directly configurable
    /// variable matching by name, or a forwarded module that could have been.
    // `Module` keys carry `RefCell` environment state excluded from
    // identity `Eq`/`Hash`; the lint is a false positive by design.
    #[allow(clippy::mutable_key_type)]
    pub fn could_have_been_configured(&self, variables: &HashSet<String>) -> bool {
        let inner = self.environment.0.borrow();
        let configurable = inner.configurable_variables.borrow();

        if variables.len() < configurable.len() {
            if variables.iter().any(|v| configurable.contains(v)) {
                return true;
            }
        } else if configurable.iter().any(|v| variables.contains(v)) {
            return true;
        }

        let mut checked: HashSet<Module<'compile, 'parse>> = HashSet::new();
        if variables.len() < self.modules_by_variable.len() {
            for v in variables {
                if let Some(module) = self.modules_by_variable.get(v) {
                    if checked.insert(*module) && module.could_have_been_configured(variables) {
                        return true;
                    }
                }
            }
        } else {
            for (v, module) in &self.modules_by_variable {
                if variables.contains(v)
                    && checked.insert(*module)
                    && module.could_have_been_configured(variables)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Creates a copy of this module with a fresh CSS tree and extension
    /// store. Returns an identical handle when the module contains no CSS.
    pub fn clone_css(&self, arena: &'compile Bump) -> SassResult<Module<'compile, 'parse>> {
        if !self.transitively_contains_css {
            return Ok(Module::new(
                arena,
                ModuleKind::Environment(Box::new(self.clone())),
            ));
        }
        let (cloned_css, cloned_store) =
            clone_css_stylesheet(arena, &self.css, &self.extension_store)?;
        let cloned_comments = self.pre_module_comments.clone();
        let cloned = EnvironmentModule {
            environment: self.environment,
            upstream: self.upstream.clone(),
            variables: self.variables,
            variable_nodes: self.variable_nodes,
            functions: self.functions,
            mixins: self.mixins,
            extension_store: cloned_store,
            pre_module_comments: cloned_comments,
            css: cloned_css,
            transitively_contains_css: self.transitively_contains_css,
            transitively_contains_extensions: self.transitively_contains_extensions,
            modules_by_variable: self.modules_by_variable.clone(),
        };
        Ok(Module::new(
            arena,
            ModuleKind::Environment(Box::new(cloned)),
        ))
    }
}

/// Builds the variable-name → originating-module map for `forwarded`.
///
/// Dart: `_EnvironmentModule._makeModulesByVariable` (`environment.dart`).
/// Nested environment modules are flattened (each forwarded variable maps to
/// its ultimate origin) to avoid linear-depth delegation chains.
fn make_modules_by_variable<'compile: 'parse, 'parse>(
    forwarded: &[Module<'compile, 'parse>],
) -> IndexMap<String, Module<'compile, 'parse>> {
    if forwarded.is_empty() {
        return IndexMap::new();
    }
    let mut result: IndexMap<String, Module<'compile, 'parse>> = IndexMap::new();
    for module in forwarded {
        for key in module.variables().keys() {
            result.insert(key.clone(), *module);
        }
    }
    result
}

/// Builds the four member views: each merges the environment's root scope
/// (public members only) with the corresponding forwarded members, so locals
/// shadow forwarded ones.
///
/// Dart: `_EnvironmentModule._memberMap` (`environment.dart:1055-1072`); the
/// merge itself lives in [`member_map::member_map`].
// Single producer/consumer for the four-view tuple; a named alias would be
// single-use indirection.
#[allow(clippy::type_complexity)]
fn build_member_views<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    env: &Environment<'compile, 'parse>,
    forwarded: &[Module<'compile, 'parse>],
) -> (
    &'parse dyn MemberMap<Value<'parse>>,
    &'parse dyn MemberMap<FileSpan<'parse>>,
    &'parse dyn MemberMap<Callable<'compile, 'parse>>,
    &'parse dyn MemberMap<Callable<'compile, 'parse>>,
) {
    let inner = env.0.borrow();

    let local_vars: &'parse dyn MemberMap<Value<'parse>> =
        arena.alloc(RefCellMap::new(inner.variables[0]));
    let forwarded_vars: Vec<&'parse dyn MemberMap<Value<'parse>>> =
        forwarded.iter().map(|m| m.variables_view(arena)).collect();
    let vars = member_map::member_map(arena, local_vars, forwarded_vars);

    let local_nodes: &'parse dyn MemberMap<FileSpan<'parse>> =
        arena.alloc(RefCellMap::new(inner.variable_nodes[0]));
    let forwarded_nodes: Vec<&'parse dyn MemberMap<FileSpan<'parse>>> = forwarded
        .iter()
        .map(|m| m.variable_nodes_view(arena))
        .collect();
    let var_nodes = member_map::member_map(arena, local_nodes, forwarded_nodes);

    let local_fns: &'parse dyn MemberMap<Callable<'compile, 'parse>> =
        arena.alloc(RefCellMap::new(inner.functions[0]));
    let forwarded_fns: Vec<&'parse dyn MemberMap<Callable<'compile, 'parse>>> =
        forwarded.iter().map(|m| m.functions_view(arena)).collect();
    let fns = member_map::member_map(arena, local_fns, forwarded_fns);

    let local_mixs: &'parse dyn MemberMap<Callable<'compile, 'parse>> =
        arena.alloc(RefCellMap::new(inner.mixins[0]));
    let forwarded_mixs: Vec<&'parse dyn MemberMap<Callable<'compile, 'parse>>> =
        forwarded.iter().map(|m| m.mixins_view(arena)).collect();
    let mixs = member_map::member_map(arena, local_mixs, forwarded_mixs);

    (vars, var_nodes, fns, mixs)
}
