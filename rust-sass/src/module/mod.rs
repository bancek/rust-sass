// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/module.dart + lib/src/module/built_in.dart
// go-source: go/sassmodule/module.go

//! Sass modules: the [`Module`] interface and its built-in implementation.
//!
//! Matches Dart: `Module` (`module.dart`) + `BuiltInModule`
//! (`module/built_in.dart`). The remaining `Module` implementations live in
//! [`environment_module`], [`forwarded`], and [`shadowed`].

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use bumpalo::Bump;

use crate::url::SassUrl;
use indexmap::IndexMap;

use crate::ast::css::comment::CssComment;
use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::stylesheet::ModifiableCssStylesheet;
use crate::callable::Callable;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::{FileSpan, BOGUS_SPAN};
use crate::extend::store::ExtensionStore;
use crate::member_map::{self, MemberMap};
use crate::value::Value;

/// A module provided by Sass, available under the special `sass:` URL space.
///
/// Dart: `BuiltInModule` (`module/built_in.dart`). Functions and mixins are
/// stored keyed by their own names; variables are immutable once constructed.
#[derive(Clone)]
pub struct BuiltInModule<'compile, 'parse> {
    pub url: String,
    pub variables: IndexMap<String, Value<'parse>>,
    pub functions: IndexMap<String, Callable<'compile, 'parse>>,
    pub mixins: IndexMap<String, Callable<'compile, 'parse>>,
    pub css: ModifiableCssNode<'parse>,
    variable_nodes: IndexMap<String, FileSpan<'parse>>,
    pre_module_comments: IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>>,
}

impl<'compile: 'parse, 'parse> BuiltInModule<'compile, 'parse> {
    /// Creates a module named `name` (exposed as `sass:<name>`), keying each
    /// callable in `functions`/`mixins` under its own name.
    pub fn new(
        arena: &'compile Bump,
        name: String,
        functions: &[Callable<'compile, 'parse>],
        mixins: &[Callable<'compile, 'parse>],
        variables: IndexMap<String, Value<'parse>>,
    ) -> Self {
        let fn_map: IndexMap<String, Callable<'compile, 'parse>> = functions
            .iter()
            .map(|f| (f.name().to_string(), *f))
            .collect();
        let mx_map: IndexMap<String, Callable<'compile, 'parse>> =
            mixins.iter().map(|m| (m.name().to_string(), *m)).collect();
        BuiltInModule {
            url: format!("sass:{name}"),
            variables,
            functions: fn_map,
            mixins: mx_map,
            css: ModifiableCssNode::new(
                arena,
                ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(BOGUS_SPAN)),
            ),
            variable_nodes: IndexMap::new(),
            pre_module_comments: IndexMap::new(),
        }
    }

    /// The canonical URL for this module's source file (`sass:<name>`).
    pub fn url(&self) -> SassResult<Option<SassUrl>> {
        Ok(Some(SassUrl::parse(&self.url).map_err(|_| {
            SassError::Script {
                message: format!("Invalid module URL: {}", self.url),
                argument_name: None,
            }
        })?))
    }

    /// Matches Go: BuiltInModule.AddFunction.
    pub fn add_function(&mut self, fn_: Callable<'compile, 'parse>) {
        self.functions.insert(fn_.name().to_string(), fn_);
    }

    /// Matches Go: BuiltInModule.AddMixin.
    pub fn add_mixin(&mut self, mx: Callable<'compile, 'parse>) {
        self.mixins.insert(mx.name().to_string(), mx);
    }

    /// Modules that this module uses. Built-in modules use no others.
    pub fn upstream(&self) -> Vec<Module<'compile, 'parse>> {
        vec![]
    }

    /// The nodes where each variable was defined. Built-in modules carry no
    /// spans, so this is always empty.
    pub fn variable_nodes(&self) -> &IndexMap<String, FileSpan<'parse>> {
        &self.variable_nodes
    }

    /// The extensions defined in this module. Built-in modules define none.
    pub fn extension_store(&self) -> ExtensionStore<'compile> {
        ExtensionStore::Empty
    }

    /// A map from modules in [`upstream`](BuiltInModule::upstream) to loud
    /// comments written in this module that should be emitted before them.
    /// Built-in modules carry none.
    pub fn pre_module_comments(
        &self,
    ) -> &IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>> {
        &self.pre_module_comments
    }

    /// Whether this module *or* any modules in
    /// [`upstream`](BuiltInModule::upstream) contain any CSS. Always false for
    /// built-in modules.
    pub fn transitively_contains_css(&self) -> bool {
        false
    }

    /// Whether this module *or* any modules in
    /// [`upstream`](BuiltInModule::upstream) contain `@extend` rules. Always
    /// false for built-in modules.
    pub fn transitively_contains_extensions(&self) -> bool {
        false
    }

    /// Sets the variable named `name` to `val`. Always fails: throws when the
    /// module defines no such variable ("Undefined variable."), and when it
    /// does ("Cannot modify built-in variable.").
    pub fn set_variable(
        &self,
        name: &str,
        _val: Value<'parse>,
        _node_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        if !self.variables.contains_key(name) {
            return Err(Box::new(SassError::Script {
                message: "Undefined variable.".into(),
                argument_name: None,
            }));
        }
        Err(Box::new(SassError::Script {
            message: "Cannot modify built-in variable.".into(),
            argument_name: None,
        }))
    }

    /// Returns an opaque value identifying the definition of `name`: equal
    /// across modules if and only if both expose identical definitions (per
    /// the Sass spec). For built-ins this is the module itself; use
    /// [`Module::variable_identity`] instead (this stub only asserts the
    /// variable exists, panicking otherwise like Dart's `assert`).
    pub fn variable_identity(&self, name: &str) -> SassResult<Module<'compile, 'parse>> {
        if !self.variables.contains_key(name) {
            panic!("assertion failed: variable not found");
        }
        // Returns self via the outer Module wrapper — the caller wraps this
        // BuiltInModule in a Module and passes self-reference back.
        // We can't construct Module here without a circular reference, so we
        // return a placeholder that the dispatcher handles.
        Err(Box::new(SassError::Script {
            message: "variable_identity: use Module::variable_identity() instead".into(),
            argument_name: None,
        }))
    }

    /// Whether this module exposes any of `variables` that could have been
    /// configured when the module was loaded. Always false for built-ins.
    pub fn could_have_been_configured(&self, _variables: &HashSet<String>) -> bool {
        false
    }

    /// Creates a copy of this module with a fresh CSS tree. Built-in modules
    /// carry no CSS, so this is the module itself (identity-preserving).
    pub fn clone_css(&self, arena: &'compile Bump) -> SassResult<Module<'compile, 'parse>> {
        // Dart: BuiltInModule.cloneCss => this (lib/src/module/built_in.dart:67).
        // (Module::clone_css handles the BuiltIn variant by returning self
        // before dispatching here, so this method is only called through the
        // ModuleKind::BuiltIn arm of the dispatch — at which point identity
        // is the correct answer.)
        Ok(Module::new(arena, ModuleKind::BuiltIn(self.clone())))
    }
}

pub mod environment_module;
pub mod forwarded;
pub mod shadowed;

use environment_module::EnvironmentModule;
use forwarded::ForwardedModuleView;
use shadowed::ShadowedModuleView;

// === ModuleKind enum ===
//
// The four Sass module implementations behind the [`Module`] handle. Dart
// expresses these as classes implementing the `Module` interface
// (`module/built_in.dart`, `module/forwarded_view.dart`,
// `module/shadowed_view.dart`, `_EnvironmentModule`); Rust makes the closed
// set an enum so dispatch is an exhaustive `match`.

#[derive(Clone)]
pub enum ModuleKind<'compile, 'parse> {
    BuiltIn(BuiltInModule<'compile, 'parse>),
    Forwarded(ForwardedModuleView<'compile, 'parse>),
    Shadowed(ShadowedModuleView<'compile, 'parse>),
    Environment(Box<EnvironmentModule<'compile, 'parse>>),
}

// === Module wrapper with arena identity ===
//
// The interface for a Sass module (Dart: `Module`, `module.dart`).
// A `Copy` handle to an arena-allocated [`ModuleKind`]: `==`/`Hash` are
// address identity (the allocation itself), matching Dart where module
// identity is the object, not a counter.

#[derive(Clone, Copy)]
pub struct Module<'compile, 'parse>(&'parse ModuleKind<'compile, 'parse>);

impl<'compile: 'parse, 'parse> Module<'compile, 'parse> {
    pub fn new(arena: &'compile Bump, kind: ModuleKind<'compile, 'parse>) -> Self {
        Module(arena.alloc(kind))
    }

    pub fn kind(&self) -> &ModuleKind<'compile, 'parse> {
        self.0
    }

    // --- Dispatch methods ---
    //
    // Each mirrors the same-named getter/method on Dart's `Module` interface
    // (`module.dart`); see `BuiltInModule` docs for per-method semantics.

    /// The canonical URL for this module's source file, or `None` when loaded
    /// from a string without a URL.
    pub fn url(&self) -> SassResult<Option<SassUrl>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => b.url(),
            ModuleKind::Forwarded(f) => f.inner.url(),
            ModuleKind::Shadowed(s) => s.inner.url(),
            ModuleKind::Environment(e) => Ok(e.css.span()?.source_url().cloned()),
        }
    }

    /// Modules that this module uses.
    pub fn upstream(&self) -> Vec<Module<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(_) => vec![],
            ModuleKind::Forwarded(f) => f.inner.upstream(),
            ModuleKind::Shadowed(s) => s.inner.upstream(),
            ModuleKind::Environment(e) => e.upstream.clone(),
        }
    }

    /// The module's variables.
    pub fn variables(&self) -> ModuleVariables<'_, Value<'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => ModuleVariables::Owned(&b.variables),
            ModuleKind::Forwarded(f) => ModuleVariables::View(f.variables),
            ModuleKind::Shadowed(s) => ModuleVariables::View(s.variables),
            ModuleKind::Environment(e) => ModuleVariables::View(e.variables),
        }
    }

    pub fn variables_view(&self, arena: &'compile Bump) -> &'parse dyn MemberMap<Value<'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => arena.alloc(member_map::IndexMapView {
                map: b.variables.clone(),
            }),
            ModuleKind::Forwarded(f) => f.variables,
            ModuleKind::Shadowed(s) => s.variables,
            ModuleKind::Environment(e) => e.variables,
        }
    }

    /// The nodes where each variable in [`variables`](Module::variables) was
    /// defined. Kept parallel to `variables`: implementations must expose the
    /// same keys.
    pub fn variable_nodes(&self) -> ModuleVariables<'_, FileSpan<'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => ModuleVariables::Owned(b.variable_nodes()),
            ModuleKind::Forwarded(f) => ModuleVariables::View(f.variable_nodes),
            ModuleKind::Shadowed(s) => ModuleVariables::View(s.variable_nodes),
            ModuleKind::Environment(e) => ModuleVariables::View(e.variable_nodes),
        }
    }

    pub fn variable_nodes_view(
        &self,
        arena: &'compile Bump,
    ) -> &'parse dyn MemberMap<FileSpan<'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => arena.alloc(member_map::IndexMapView {
                map: b.variable_nodes().clone(),
            }),
            ModuleKind::Forwarded(f) => f.variable_nodes,
            ModuleKind::Shadowed(s) => s.variable_nodes,
            ModuleKind::Environment(e) => e.variable_nodes,
        }
    }

    /// The module's functions. Each callable is stored under its own name.
    pub fn functions(&self) -> ModuleVariables<'_, Callable<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => ModuleVariables::Owned(&b.functions),
            ModuleKind::Forwarded(f) => ModuleVariables::View(f.functions),
            ModuleKind::Shadowed(s) => ModuleVariables::View(s.functions),
            ModuleKind::Environment(e) => ModuleVariables::View(e.functions),
        }
    }

    pub fn functions_view(
        &self,
        arena: &'compile Bump,
    ) -> &'parse dyn MemberMap<Callable<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => arena.alloc(member_map::IndexMapView {
                map: b.functions.clone(),
            }),
            ModuleKind::Forwarded(f) => f.functions,
            ModuleKind::Shadowed(s) => s.functions,
            ModuleKind::Environment(e) => e.functions,
        }
    }

    /// The module's mixins. Each callable is stored under its own name.
    pub fn mixins(&self) -> ModuleVariables<'_, Callable<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => ModuleVariables::Owned(&b.mixins),
            ModuleKind::Forwarded(f) => ModuleVariables::View(f.mixins),
            ModuleKind::Shadowed(s) => ModuleVariables::View(s.mixins),
            ModuleKind::Environment(e) => ModuleVariables::View(e.mixins),
        }
    }

    pub fn mixins_view(
        &self,
        arena: &'compile Bump,
    ) -> &'parse dyn MemberMap<Callable<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => arena.alloc(member_map::IndexMapView {
                map: b.mixins.clone(),
            }),
            ModuleKind::Forwarded(f) => f.mixins,
            ModuleKind::Shadowed(s) => s.mixins,
            ModuleKind::Environment(e) => e.mixins,
        }
    }

    /// The extensions defined in this module, also able to update the CSS
    /// tree's style rules in place based on downstream extensions.
    pub fn extension_store(&self) -> ExtensionStore<'parse> {
        match self.0 {
            ModuleKind::BuiltIn(_) => ExtensionStore::Empty,
            ModuleKind::Forwarded(_) => ExtensionStore::Empty,
            ModuleKind::Shadowed(_) => ExtensionStore::Empty,
            ModuleKind::Environment(e) => e.extension_store.clone(),
        }
    }

    // --- CSS access is per-variant via eval::statement::css_of_module().
    //     No shared Module::css() — BuiltIn needs arena for CssStylesheet::empty
    //     ('compile lifetime bound), Environment just clones (no arena). Keeping
    //     them separate avoids leaking the arena constraint through dispatch.

    /// A map from modules in [`upstream`](Module::upstream) to loud comments
    /// written in this module that should be emitted before the given module.
    pub fn pre_module_comments(
        &self,
    ) -> &IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => b.pre_module_comments(),
            ModuleKind::Forwarded(f) => f.inner.pre_module_comments(),
            ModuleKind::Shadowed(s) => s.inner.pre_module_comments(),
            ModuleKind::Environment(e) => &e.pre_module_comments,
        }
    }

    /// Whether this module *or* any modules in [`upstream`](Module::upstream)
    /// contain any CSS.
    pub fn transitively_contains_css(&self) -> bool {
        match self.0 {
            ModuleKind::BuiltIn(b) => b.transitively_contains_css(),
            ModuleKind::Forwarded(f) => f.inner.transitively_contains_css(),
            ModuleKind::Shadowed(s) => s.inner.transitively_contains_css(),
            ModuleKind::Environment(e) => e.transitively_contains_css,
        }
    }

    /// Whether this module *or* any modules in [`upstream`](Module::upstream)
    /// contain `@extend` rules.
    pub fn transitively_contains_extensions(&self) -> bool {
        match self.0 {
            ModuleKind::BuiltIn(b) => b.transitively_contains_extensions(),
            ModuleKind::Forwarded(f) => f.inner.transitively_contains_extensions(),
            ModuleKind::Shadowed(s) => s.inner.transitively_contains_extensions(),
            ModuleKind::Environment(e) => e.transitively_contains_extensions,
        }
    }

    /// Sets the variable named `name` to `val`, associated with `node_span`'s
    /// source span. Throws if this module defines no variable named `name`.
    pub fn set_variable(
        &self,
        name: &str,
        val: Value<'parse>,
        node_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        match self.0 {
            ModuleKind::BuiltIn(b) => b.set_variable(name, val, node_span),
            ModuleKind::Forwarded(f) => f.set_variable(name, val, node_span),
            ModuleKind::Shadowed(s) => s.set_variable(name, val, node_span),
            ModuleKind::Environment(e) => e.set_variable(name, val, node_span),
        }
    }

    /// Returns an opaque value identifying the definition of `name`: equal
    /// across modules if and only if both expose identical definitions (per
    /// the Sass spec).
    pub fn variable_identity(&self, name: &str) -> SassResult<Module<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(b) => {
                if !b.variables.contains_key(name) {
                    panic!("assertion failed: variable not found");
                }
                Ok(*self)
            }
            ModuleKind::Forwarded(f) => f.variable_identity(name),
            ModuleKind::Shadowed(s) => s.variable_identity(name),
            ModuleKind::Environment(e) => {
                if !e.variables.has(name) {
                    panic!("assertion failed: variable not found");
                }
                if let Some(module) = e.modules_by_variable.get(name) {
                    return module.variable_identity(name);
                }
                Ok(*self)
            }
        }
    }

    /// Whether this module exposes any of `variables` that could have been
    /// configured when the module was loaded.
    pub fn could_have_been_configured(&self, variables: &HashSet<String>) -> bool {
        match self.0 {
            ModuleKind::BuiltIn(b) => b.could_have_been_configured(variables),
            ModuleKind::Forwarded(f) => f.could_have_been_configured(variables),
            ModuleKind::Shadowed(s) => s.could_have_been_configured(variables),
            ModuleKind::Environment(e) => e.could_have_been_configured(variables),
        }
    }

    /// Creates a copy of this module with a fresh CSS tree and extension
    /// store. Modules without CSS keep their identity (return `self`).
    pub fn clone_css(&self, arena: &'compile Bump) -> SassResult<Module<'compile, 'parse>> {
        match self.0 {
            ModuleKind::BuiltIn(_) => Ok(*self),
            ModuleKind::Forwarded(f) => f.clone_css(arena),
            ModuleKind::Shadowed(s) => s.clone_css(arena),
            // Dart: `_EnvironmentModule.cloneCss` returns `this` when the module
            // has no CSS (same identity). Forwarded/Shadowed views still build a
            // fresh view wrapper (matching Dart), but the underlying no-CSS
            // environment module keeps its identity.
            ModuleKind::Environment(e) if !e.transitively_contains_css => Ok(*self),
            ModuleKind::Environment(e) => e.clone_css(arena),
        }
    }
}

impl PartialEq for Module<'_, '_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Module<'_, '_> {}

impl Hash for Module<'_, '_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0 as *const ModuleKind<'_, '_>).hash(state)
    }
}

// ===========================================================================
// ModuleVariables — unified return type for Module::variables() etc.
// Owned variant wraps &IndexMap (BuiltIn). View variant wraps &dyn MemberMap
// (Forwarded, Shadowed, Environment). All access goes through a uniform API.
//
// Rust-only: Dart returns `Map` views directly; the enum bridges the two
// storage shapes without copying.
// ===========================================================================

use std::fmt::Debug;

#[derive(Clone, Copy)]
pub enum ModuleVariables<'a, V: Clone + Debug> {
    Owned(&'a IndexMap<String, V>),
    View(&'a dyn MemberMap<V>),
}

impl<'a, V: Clone + Debug> ModuleVariables<'a, V> {
    pub fn get(&self, key: &str) -> Option<V> {
        match self {
            ModuleVariables::Owned(m) => m.get(key).cloned(),
            ModuleVariables::View(v) => v.get(key),
        }
    }

    pub fn has(&self, key: &str) -> bool {
        match self {
            ModuleVariables::Owned(m) => m.contains_key(key),
            ModuleVariables::View(v) => v.has(key),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ModuleVariables::Owned(m) => m.len(),
            ModuleVariables::View(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn keys(&self) -> Vec<String> {
        match self {
            ModuleVariables::Owned(m) => m.keys().cloned().collect(),
            ModuleVariables::View(v) => v.keys(),
        }
    }

    pub fn entries(&self) -> Vec<(String, V)> {
        match self {
            ModuleVariables::Owned(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ModuleVariables::View(v) => v.entries(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::ast::sass::statement::ForwardRule;
    use crate::callable::BuiltInCallable;
    use crate::callable::CallableKind;
    use crate::callable::SyncBuiltInCallback;
    use crate::environment::Environment;
    use crate::value::SassNumber;
    use std::rc::Rc;

    use crate::ast::css::comment::ModifiableCssComment;
    use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
    use crate::ast::css::stylesheet::ModifiableCssStylesheet;
    use crate::url::SassUrl;
    use crate::value::{Value, ValueKind};

    use super::*;

    fn test_span() -> FileSpan<'static> {
        FileSpan::new(None, 0, 0)
    }

    fn test_callable<'compile, 'parse>(
        arena: &'compile Bump,
        name: &str,
    ) -> Callable<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let pl = ParameterList::empty(test_span());
        let cb: SyncBuiltInCallback<'compile, 'parse> =
            Rc::new(|_config, _state, _args, arena: &'compile Bump| {
                Ok(Value::new_with_arena(arena, ValueKind::Null))
            });
        let bic = BuiltInCallable::new(name.to_string(), pl, cb);
        Callable::new(arena, CallableKind::BuiltIn(bic))
    }

    // --- BuiltInModule ---

    #[test]
    fn test_builtin_url() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        let u = m.url().unwrap().unwrap();
        assert_eq!(u.as_str(), "sass:math");
    }

    #[test]
    fn test_builtin_upstream() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        assert!(m.upstream().is_empty());
    }

    #[test]
    fn test_builtin_variables() {
        let arena = Bump::new();
        let mut vars = IndexMap::new();
        vars.insert(
            "pi".into(),
            Value::new_with_arena(
                &arena,
                // Arbitrary value: this test only asserts key presence.
                ValueKind::Number(SassNumber::new(1.0, None)),
            ),
        );
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], vars);
        assert!(m.variables.contains_key("pi"));
    }

    #[test]
    fn test_builtin_functions() {
        let arena = Bump::new();
        let fn_ = test_callable(&arena, "abs");
        let m = BuiltInModule::new(&arena, "math".into(), &[fn_], &[], IndexMap::new());
        let f = m.functions.get("abs").unwrap();
        assert_eq!(f.name(), "abs");
    }

    #[test]
    fn test_builtin_mixins() {
        let arena = Bump::new();
        let mx = test_callable(&arena, "apply");
        let m = BuiltInModule::new(&arena, "meta".into(), &[], &[mx], IndexMap::new());
        let mix = m.mixins.get("apply").unwrap();
        assert_eq!(mix.name(), "apply");
    }

    #[test]
    fn test_builtin_transitively_contains_css_false() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        assert!(!m.transitively_contains_css());
    }

    #[test]
    fn test_builtin_extension_store_empty() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        assert!(matches!(m.extension_store(), ExtensionStore::Empty));
    }

    #[test]
    fn test_builtin_transitively_contains() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        assert!(!m.transitively_contains_css());
        assert!(!m.transitively_contains_extensions());
    }

    #[test]
    fn test_builtin_set_variable_undefined() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        let err = m
            .set_variable(
                "x",
                Value::new_with_arena(&arena, ValueKind::Null),
                test_span(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Undefined variable.");
    }

    #[test]
    fn test_builtin_set_variable_cannot_modify() {
        let arena = Bump::new();
        let mut vars = IndexMap::new();
        vars.insert("x".into(), Value::new_with_arena(&arena, ValueKind::Null));
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], vars);
        let err = m
            .set_variable(
                "x",
                Value::new_with_arena(&arena, ValueKind::Null),
                test_span(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Cannot modify built-in variable.");
    }

    #[test]
    fn test_builtin_could_have_been_configured() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        assert!(!m.could_have_been_configured(&HashSet::new()));
    }

    #[test]
    fn test_builtin_clone_css() {
        let arena = Bump::new();
        let m = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        let module = Module::new(&arena, ModuleKind::BuiltIn(m));
        let cloned = module.clone_css(&arena).unwrap();
        assert!(module == cloned);
    }

    // --- ForwardedModuleView ---

    #[test]
    fn test_forwarded_prefix() {
        let arena = Bump::new();
        let mut vars = IndexMap::new();
        vars.insert("a".into(), Value::new_with_arena(&arena, ValueKind::Null));
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], vars);
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));

        let rule = ForwardRule::new(
            SassUrl::parse("file:///test").unwrap(),
            test_span(),
            Some("ns-".into()),
            vec![],
        );
        let fv = ForwardedModuleView::new(&arena, inner_mod, &rule);
        // Go PrefixedMapView ADDS prefix: inner "a" → visible as "ns-a"
        assert!(fv.variables.has("ns-a"));
        assert!(!fv.variables.has("a"));
    }

    #[test]
    fn test_forwarded_if_necessary_returns_inner() {
        let arena = Bump::new();
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], IndexMap::new());
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let rule = ForwardRule::new(
            SassUrl::parse("file:///test").unwrap(),
            test_span(),
            None,
            vec![],
        );
        let result = ForwardedModuleView::if_necessary(&arena, inner_mod, &rule);
        assert!(result == inner_mod);
    }

    #[test]
    fn test_forwarded_if_necessary_wraps() {
        let arena = Bump::new();
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], IndexMap::new());
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let rule = ForwardRule::new(
            SassUrl::parse("file:///test").unwrap(),
            test_span(),
            Some("ns-".into()),
            vec![],
        );
        let result = ForwardedModuleView::if_necessary(&arena, inner_mod, &rule);
        assert!(result != inner_mod);
    }

    #[test]
    fn test_forwarded_url_delegates() {
        let arena = Bump::new();
        let inner = BuiltInModule::new(&arena, "math".into(), &[], &[], IndexMap::new());
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let rule = ForwardRule::new(
            SassUrl::parse("file:///test").unwrap(),
            test_span(),
            None,
            vec![],
        );
        let fv = ForwardedModuleView::new(&arena, inner_mod, &rule);
        let fv_mod = Module::new(&arena, ModuleKind::Forwarded(fv));
        let u = fv_mod.url().unwrap().unwrap();
        assert_eq!(u.as_str(), "sass:math");
    }

    // --- ShadowedModuleView ---

    #[test]
    fn test_shadowed_block_variable() {
        let arena = Bump::new();
        let mut vars = IndexMap::new();
        vars.insert("a".into(), Value::new_with_arena(&arena, ValueKind::Null));
        vars.insert("b".into(), Value::new_with_arena(&arena, ValueKind::Null));
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], vars);
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));

        let blocked: HashSet<String> = ["b".into()].into();
        let sv = ShadowedModuleView::new(
            &arena,
            inner_mod,
            &blocked,
            &HashSet::new(),
            &HashSet::new(),
        );

        assert!(sv.variables.has("a"));
        assert!(!sv.variables.has("b"));
    }

    #[test]
    fn test_shadowed_if_necessary_none() {
        let arena = Bump::new();
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], IndexMap::new());
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let result = ShadowedModuleView::if_necessary(
            &arena,
            &inner_mod,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_shadowed_if_necessary_overlap() {
        let arena = Bump::new();
        let mut vars = IndexMap::new();
        vars.insert("a".into(), Value::new_with_arena(&arena, ValueKind::Null));
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], vars);
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let blocked: HashSet<String> = ["a".into()].into();
        let result = ShadowedModuleView::if_necessary(
            &arena,
            &inner_mod,
            &blocked,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert!(result.is_some());
    }

    #[test]
    fn test_shadowed_is_empty() {
        let arena = Bump::new();
        let mut vars = IndexMap::new();
        vars.insert("a".into(), Value::new_with_arena(&arena, ValueKind::Null));
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], vars);
        let inner_mod = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let blocked: HashSet<String> = ["a".into()].into();
        let sv = ShadowedModuleView::new(
            &arena,
            inner_mod,
            &blocked,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert!(sv.is_empty().unwrap());
    }

    // --- Module identity ---

    #[test]
    fn test_module_identity_same_rc() {
        let arena = Bump::new();
        let inner = BuiltInModule::new(&arena, "test".into(), &[], &[], IndexMap::new());
        let m1 = Module::new(&arena, ModuleKind::BuiltIn(inner));
        let m2 = m1;
        assert!(m1 == m2);
    }

    #[test]
    fn test_module_identity_different_rc() {
        let arena = Bump::new();
        let inner1 = BuiltInModule::new(&arena, "a".into(), &[], &[], IndexMap::new());
        let inner2 = BuiltInModule::new(&arena, "a".into(), &[], &[], IndexMap::new());
        let m1 = Module::new(&arena, ModuleKind::BuiltIn(inner1));
        let m2 = Module::new(&arena, ModuleKind::BuiltIn(inner2));
        assert!(m1 != m2);
    }

    // --- EnvironmentModule clone_css ---

    fn env_module_with_css<'compile, 'parse>(
        arena: &'compile Bump,
        has_content: bool,
    ) -> Module<'compile, 'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let env = Environment::new(arena);
        let mut children: Vec<ModifiableCssNode<'parse>> = Vec::new();
        if has_content {
            let comment = ModifiableCssComment::new("/* test */".into(), test_span());
            children.push(ModifiableCssNode::new(
                arena,
                ModifiableCssNodeKind::Comment(comment),
            ));
        }
        let css = ModifiableCssNode::new(
            arena,
            ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(test_span())),
        );
        for child in &children {
            css.add_child(child).unwrap();
        }
        let env_mod = EnvironmentModule::new(
            arena,
            env,
            css,
            IndexMap::new(),
            ExtensionStore::Empty,
            &[],
            &[],
        );
        let ModuleKind::Environment(em) = env_mod.kind().clone() else {
            panic!("expected Environment variant");
        };
        // Reconstruct via Module so clone_css is reachable through the dispatch.
        Module::new(arena, ModuleKind::Environment(em))
    }

    #[test]
    fn test_clone_css_identity_no_css() {
        let arena = Bump::new();
        let m = env_module_with_css(&arena, false);
        let cloned = m.clone_css(&arena).unwrap();
        // Dart: `_EnvironmentModule.cloneCss` returns `this` for a module with
        // no CSS, so the clone shares identity with the original.
        assert!(std::ptr::eq(m.0, cloned.0));
        // The cloned module also has no CSS.
        let ModuleKind::Environment(ref em) = *cloned.kind() else {
            panic!("expected Environment")
        };
        assert!(em.css.children().unwrap_or_default().is_empty());
        assert!(!em.transitively_contains_css);
    }

    #[test]
    fn test_clone_css_with_css() {
        let arena = Bump::new();
        let m = env_module_with_css(&arena, true);
        let cloned = m.clone_css(&arena).unwrap();
        // Different Rc allocations (cloned CSS).
        assert!(!std::ptr::eq(m.0, cloned.0));
        let ModuleKind::Environment(ref orig_em) = *m.kind() else {
            panic!("expected Environment")
        };
        let ModuleKind::Environment(ref cloned_em) = *cloned.kind() else {
            panic!("expected Environment")
        };
        assert_eq!(
            orig_em.css.children().unwrap_or_default().len(),
            cloned_em.css.children().unwrap_or_default().len(),
        );
        assert_eq!(orig_em.css.span().unwrap(), cloned_em.css.span().unwrap(),);
        // CSS children are deep-copied — lengths match, distinct CssStylesheet.
        // Environment is the same object (shared via clone).
        assert!(std::ptr::eq(orig_em.environment.0, cloned_em.environment.0));
        assert_eq!(
            orig_em.transitively_contains_css,
            cloned_em.transitively_contains_css
        );
        assert_eq!(
            orig_em.transitively_contains_extensions,
            cloned_em.transitively_contains_extensions
        );
    }

    #[test]
    fn test_clone_css_dispatch_through_module() {
        let arena = Bump::new();
        // Verify that Module::clone_css dispatches correctly to
        // EnvironmentModule::clone_css for the Environment variant.
        let m = env_module_with_css(&arena, true);
        let cloned = m.clone_css(&arena).unwrap();
        assert!(
            matches!(*cloned.kind(), ModuleKind::Environment(_)),
            "cloned module should retain the Environment variant"
        );
        assert!(!std::ptr::eq(m.0, cloned.0));
    }
}
