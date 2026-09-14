// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/environment.dart
// go-source: go/sassenv/environment.go

//! The lexical environment in which Sass executes: lexically-scoped
//! variables, functions, mixins, and visible modules.
//!
//! Matches Dart: `Environment` (`environment.dart:44-958`). The module view
//! over an environment (`_EnvironmentModule`) lives in
//! [`crate::module::environment_module`].

use crate::ast::css::comment::CssComment;
use crate::ast::css::modifiable_node::ModifiableCssNode;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::extend::ExtensionStore;
use crate::module::environment_module::EnvironmentModule;
use crate::module::forwarded::ForwardedModuleView;
use crate::module::ModuleKind;
use crate::module::ModuleVariables;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Debug;
#[cfg(feature = "async")]
use std::ops::AsyncFnOnce;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::ast::sass::statement::forward_rule::ForwardRule;
use crate::callable::Callable;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::configuration::{Configuration, ConfiguredValue};
use crate::module::shadowed::ShadowedModuleView;
use crate::module::Module;
use crate::value::Value;

// === EnvironmentInner ===
//
// The environment's storage. Each scope-frame list's first element is the
// global scope; each successive element is deeper in the tree. (Dart:
// `Environment`'s `_variables`/`_functions`/`_mixins` fields,
// `environment.dart:80-124`.)

pub(crate) struct EnvironmentInner<'compile, 'parse> {
    pub(crate) variables: Vec<&'parse RefCell<IndexMap<String, Value<'parse>>>>,
    pub(crate) variable_nodes: Vec<&'parse RefCell<IndexMap<String, FileSpan<'parse>>>>,
    pub(crate) functions: Vec<&'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>>,
    pub(crate) mixins: Vec<&'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>>,
    /// The modules used in the current scope, indexed by their namespaces.
    pub(crate) modules: &'parse RefCell<IndexMap<String, Module<'compile, 'parse>>>,
    /// A map from module namespaces to the nodes whose spans indicate where
    /// those modules were originally loaded.
    pub(crate) namespace_nodes: &'parse RefCell<IndexMap<String, FileSpan<'parse>>>,
    /// A map from namespaceless modules to the `@use` rules whose spans
    /// indicate where those modules were originally loaded. Excludes modules
    /// imported into the current scope.
    pub(crate) global_modules:
        &'parse RefCell<IndexMap<Module<'compile, 'parse>, FileSpan<'parse>>>,
    /// A map from modules imported into the current scope to the nodes whose
    /// spans indicate where those modules were originally loaded.
    pub(crate) imported_modules:
        &'parse RefCell<IndexMap<Module<'compile, 'parse>, FileSpan<'parse>>>,
    /// A map from modules forwarded by this module to the nodes whose spans
    /// indicate where those modules were originally forwarded. `None` when
    /// there are no forwarded modules.
    pub(crate) forwarded_modules:
        Option<&'parse RefCell<IndexMap<Module<'compile, 'parse>, FileSpan<'parse>>>>,
    /// Modules forwarded by nested imports at each lexical scope level
    /// *beneath the global scope*. `None` until needed, since most
    /// environments never use this.
    pub(crate) nested_forwarded_modules:
        &'parse RefCell<Option<Vec<Vec<Module<'compile, 'parse>>>>>,
    /// Modules from `modules`, `global_modules`, and `forwarded_modules`, in
    /// the order in which they were `@use`d.
    pub(crate) all_modules: &'parse RefCell<Vec<Module<'compile, 'parse>>>,
    /// A map of variable/function/mixin names to their indices in the scope
    /// lists. Filled in as-needed; may not be complete. (Dart:
    /// `_variableIndices` etc., `environment.dart:95-124`.)
    pub(crate) variable_indices: HashMap<String, usize>,
    pub(crate) function_indices: HashMap<String, usize>,
    pub(crate) mixin_indices: HashMap<String, usize>,
    /// The name/index of the last variable accessed. Cached to speed up
    /// repeated references to the same variable and its span.
    pub(crate) last_variable_name: String,
    pub(crate) last_variable_index: usize,
    pub(crate) has_last_variable_index: bool,
    /// The content block passed to the lexically-enclosing mixin, or `None`
    /// when not in a mixin or when no content block was passed.
    pub(crate) content: Option<Callable<'compile, 'parse>>,
    /// The set of variable names that could be configured when loading the
    /// module. Used to detect passing a new configuration through `@forward`
    /// to an already-loaded module.
    pub(crate) configurable_variables: &'parse RefCell<HashSet<String>>,
    /// Whether the environment is lexically within a mixin.
    pub(crate) in_mixin: bool,
    /// Whether the environment is currently in a global or semi-global scope.
    /// A semi-global scope can assign to global variables, but doesn't
    /// declare them by default.
    pub(crate) in_semi_global_scope: bool,
}

impl<'compile: 'parse, 'parse> EnvironmentInner<'compile, 'parse> {
    fn new(arena: &'compile Bump) -> Self {
        EnvironmentInner {
            variables: vec![arena.alloc(RefCell::new(IndexMap::new()))],
            variable_nodes: vec![arena.alloc(RefCell::new(IndexMap::new()))],
            functions: vec![arena.alloc(RefCell::new(IndexMap::new()))],
            mixins: vec![arena.alloc(RefCell::new(IndexMap::new()))],
            modules: arena.alloc(RefCell::new(IndexMap::new())),
            namespace_nodes: arena.alloc(RefCell::new(IndexMap::new())),
            global_modules: arena.alloc(RefCell::new(IndexMap::new())),
            imported_modules: arena.alloc(RefCell::new(IndexMap::new())),
            forwarded_modules: None,
            nested_forwarded_modules: arena.alloc(RefCell::new(None)),
            all_modules: arena.alloc(RefCell::new(Vec::new())),
            variable_indices: HashMap::new(),
            function_indices: HashMap::new(),
            mixin_indices: HashMap::new(),
            last_variable_name: String::new(),
            last_variable_index: 0,
            has_last_variable_index: false,
            content: None,
            configurable_variables: arena.alloc(RefCell::new(HashSet::new())),
            in_mixin: false,
            in_semi_global_scope: true,
        }
    }

    fn at_root(&self) -> bool {
        self.variables.len() == 1
    }

    fn push_frame(&mut self, arena: &'compile Bump) {
        self.variables
            .push(arena.alloc(RefCell::new(IndexMap::new())));
        self.variable_nodes
            .push(arena.alloc(RefCell::new(IndexMap::new())));
        self.functions
            .push(arena.alloc(RefCell::new(IndexMap::new())));
        self.mixins.push(arena.alloc(RefCell::new(IndexMap::new())));
        if let Some(ref mut nested) = *self.nested_forwarded_modules.borrow_mut() {
            nested.push(Vec::new());
        }
    }

    fn pop_frame(&mut self) {
        if let Some(last_vars) = self.variables.last() {
            for name in last_vars.borrow().keys() {
                self.variable_indices.remove(name);
            }
        }
        self.variables.pop();
        self.variable_nodes.pop();
        if let Some(last_fns) = self.functions.last() {
            for name in last_fns.borrow().keys() {
                self.function_indices.remove(name);
            }
        }
        self.functions.pop();
        if let Some(last_mix) = self.mixins.last() {
            for name in last_mix.borrow().keys() {
                self.mixin_indices.remove(name);
            }
        }
        self.mixins.pop();
        if let Some(ref mut nested) = *self.nested_forwarded_modules.borrow_mut() {
            nested.pop();
        }
        self.last_variable_name.clear();
        self.has_last_variable_index = false;
    }

    fn variable_index(&self, name: &str) -> Option<usize> {
        (0..self.variables.len())
            .rev()
            .find(|&i| self.variables[i].borrow().contains_key(name))
    }

    fn function_index(&self, name: &str) -> Option<usize> {
        (0..self.functions.len())
            .rev()
            .find(|&i| self.functions[i].borrow().contains_key(name))
    }

    fn mixin_index(&self, name: &str) -> Option<usize> {
        (0..self.mixins.len())
            .rev()
            .find(|&i| self.mixins[i].borrow().contains_key(name))
    }
}

// === Environment ===
//
// The lexical environment in which Sass is executed (Dart: `Environment`,
// `environment.dart:40-43`). A `Copy` handle to arena-allocated inner state:
// closures share frame 0 by copying the frame `Vec`s (the arena refs are
// `Copy`), so `!global` writes stay visible to the caller.

#[derive(Clone, Copy)]
pub struct Environment<'compile, 'parse>(
    pub(crate) &'parse RefCell<EnvironmentInner<'compile, 'parse>>,
);

impl<'compile: 'parse, 'parse> fmt::Debug for Environment<'compile, 'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment").finish_non_exhaustive()
    }
}

impl<'compile: 'parse, 'parse> Environment<'compile, 'parse> {
    pub fn new(arena: &'compile Bump) -> Self {
        Environment(arena.alloc(RefCell::new(EnvironmentInner::new(arena))))
    }

    /// Whether the environment is lexically at the root of the document
    /// (a single scope frame).
    pub fn at_root(&self) -> bool {
        self.0.borrow().at_root()
    }

    /// Whether the environment is lexically within a mixin.
    pub fn in_mixin(&self) -> bool {
        self.0.borrow().in_mixin
    }

    pub fn set_in_mixin(&self, v: bool) {
        self.0.borrow_mut().in_mixin = v;
    }

    /// The content block passed to the lexically-enclosing mixin, or `None`
    /// when not in a mixin or when no content block was passed.
    pub fn content(&self) -> Option<Callable<'compile, 'parse>> {
        self.0.borrow().content
    }

    pub fn set_content(&self, content: Option<Callable<'compile, 'parse>>) {
        self.0.borrow_mut().content = content;
    }

    /// Sets `content` as [`content`](Self::content) for the duration of
    /// `callback`, restoring the previous block afterwards.
    #[rust_sass_macros::async_impl]
    pub async fn with_content<T>(
        &self,
        content: Option<Callable<'compile, 'parse>>,
        callback: impl AsyncFnOnce() -> T,
    ) -> T {
        let old = {
            let mut inner = self.0.borrow_mut();
            std::mem::replace(&mut inner.content, content)
        };
        let result = callback().await;
        self.0.borrow_mut().content = old;
        result
    }

    #[rust_sass_macros::sync_impl]
    #[rust_sass_macros::must_be_sync]
    pub async fn with_content<T>(
        &self,
        content: Option<Callable<'compile, 'parse>>,
        callback: impl FnOnce() -> T,
    ) -> T {
        let old = {
            let mut inner = self.0.borrow_mut();
            std::mem::replace(&mut inner.content, content)
        };
        let result = callback().await;
        self.0.borrow_mut().content = old;
        result
    }

    /// Sets [`in_mixin`](Self::in_mixin) for the duration of `callback`,
    /// restoring the previous value afterwards.
    #[rust_sass_macros::async_impl]
    pub async fn as_mixin<T>(&self, callback: impl AsyncFnOnce() -> T) -> T {
        let old = {
            let mut inner = self.0.borrow_mut();
            std::mem::replace(&mut inner.in_mixin, true)
        };
        let result = callback().await;
        self.0.borrow_mut().in_mixin = old;
        result
    }

    #[rust_sass_macros::sync_impl]
    #[rust_sass_macros::must_be_sync]
    pub async fn as_mixin<T>(&self, callback: impl FnOnce() -> T) -> T {
        let old = {
            let mut inner = self.0.borrow_mut();
            std::mem::replace(&mut inner.in_mixin, true)
        };
        let result = callback().await;
        self.0.borrow_mut().in_mixin = old;
        result
    }

    /// Creates a closure based on this environment.
    ///
    /// Scope changes in this environment will not affect the closure, but new
    /// declarations or assignments in scopes visible at creation are
    /// reflected (the frame lists are copied while the module maps stay
    /// shared by reference).
    pub fn closure(&self, arena: &'compile Bump) -> Self {
        let inner = self.0.borrow();
        // Dart `Environment.closure`: the module maps are shared by reference
        // (only the scope-frame lists are copied); configurable variables get
        // a fresh detached set; `_inMixin` resets to false (a function defined
        // in a mixin body sees `content-exists()` fail once called outside).
        let new_inner = EnvironmentInner {
            variables: inner.variables.clone(),
            variable_nodes: inner.variable_nodes.clone(),
            functions: inner.functions.clone(),
            mixins: inner.mixins.clone(),
            modules: inner.modules,
            namespace_nodes: inner.namespace_nodes,
            global_modules: inner.global_modules,
            imported_modules: inner.imported_modules,
            forwarded_modules: inner.forwarded_modules,
            nested_forwarded_modules: inner.nested_forwarded_modules,
            all_modules: inner.all_modules,
            variable_indices: HashMap::new(),
            function_indices: HashMap::new(),
            mixin_indices: HashMap::new(),
            last_variable_name: String::new(),
            last_variable_index: 0,
            has_last_variable_index: false,
            content: inner.content,
            configurable_variables: arena.alloc(RefCell::new(HashSet::new())),
            in_mixin: false,
            in_semi_global_scope: true,
        };
        Environment(arena.alloc(RefCell::new(new_inner)))
    }

    /// Returns a new environment to use for an imported file. Shares this
    /// environment's variables, functions, and mixins, but excludes most
    /// modules (except global modules resulting from importing a file with
    /// forwards).
    pub fn for_import(&self, arena: &'compile Bump) -> Self {
        let inner = self.0.borrow();
        // Dart `Environment.forImport`: fresh module maps except the shared
        // `_importedModules`/`_nestedForwardedModules`; the configurable set
        // is shared (marks made by the imported file stay visible);
        // `_inMixin` resets to false.
        let new_inner = EnvironmentInner {
            variables: inner.variables.clone(),
            variable_nodes: inner.variable_nodes.clone(),
            functions: inner.functions.clone(),
            mixins: inner.mixins.clone(),
            modules: arena.alloc(RefCell::new(IndexMap::new())),
            namespace_nodes: arena.alloc(RefCell::new(IndexMap::new())),
            global_modules: arena.alloc(RefCell::new(IndexMap::new())),
            imported_modules: inner.imported_modules,
            forwarded_modules: None,
            nested_forwarded_modules: inner.nested_forwarded_modules,
            all_modules: arena.alloc(RefCell::new(Vec::new())),
            variable_indices: HashMap::new(),
            function_indices: HashMap::new(),
            mixin_indices: HashMap::new(),
            last_variable_name: String::new(),
            last_variable_index: 0,
            has_last_variable_index: false,
            content: inner.content,
            configurable_variables: inner.configurable_variables,
            in_mixin: false,
            in_semi_global_scope: true,
        };
        Environment(arena.alloc(RefCell::new(new_inner)))
    }

    // --- Module operations ---

    pub fn modules(&self) -> IndexMap<String, Module<'compile, 'parse>> {
        self.0.borrow().modules.borrow().clone()
    }

    /// Adds `module` to the set of modules visible in this environment.
    /// `node_span` reports errors with the module. When `namespace` is given
    /// the module is made available under it; otherwise it becomes a
    /// namespaceless (global) module.
    ///
    /// Throws when `namespace` is taken, or when a namespaceless `module`
    /// defines a variable also defined in this environment.
    pub fn add_module(
        &self,
        module: Module<'compile, 'parse>,
        node_span: FileSpan<'parse>,
        namespace: Option<&str>,
    ) -> SassResult<()> {
        let inner = self.0.borrow();
        if let Some(ns) = namespace {
            if inner.modules.borrow().contains_key(ns) {
                let secondary_span = inner.namespace_nodes.borrow().get(ns).copied();
                let mut secondary = Vec::new();
                if let Some(sp) = secondary_span {
                    secondary.push((
                        SourceSpanWithContext::from_file_span(&sp)?,
                        "original @use".into(),
                    ));
                }
                return Err(Box::new(SassError::MultiSpan {
                    message: format!("There's already a module with namespace \"{}\".", ns),
                    span: SourceSpanWithContext::from_file_span(&node_span)?,
                    primary_label: Some("new @use".into()),
                    secondary,
                    original_source: None,
                    cause: None,
                    loaded_urls: vec![],
                    trace: Default::default(),
                }));
            }
            inner.modules.borrow_mut().insert(ns.to_string(), module);
            inner
                .namespace_nodes
                .borrow_mut()
                .insert(ns.to_string(), node_span);
        } else {
            // Check for variable name conflicts with global module
            let global_vars: HashSet<_> = inner.variables[0].borrow().keys().cloned().collect();
            for name in module.variables().keys() {
                if global_vars.contains(&name) {
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "This module and the new module both define a variable named \"${}\".",
                            name
                        ),
                        argument_name: None,
                    }));
                }
            }
            inner.global_modules.borrow_mut().insert(module, node_span);
        }
        inner.all_modules.borrow_mut().push(module);
        Ok(())
    }

    /// Exposes the members in [module] to downstream modules as though they were
    /// defined in this module, according to the modifications defined by [rule].
    ///
    /// Matches Go: Environment.ForwardModule
    pub fn forward_module(
        &self,
        arena: &'compile Bump,
        module: Module<'compile, 'parse>,
        rule: &ForwardRule<'parse>,
        node_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        let mut inner = self.0.borrow_mut();

        if inner.forwarded_modules.is_none() {
            inner.forwarded_modules = Some(arena.alloc(RefCell::new(IndexMap::new())));
        }

        // Go: view = ForwardedModuleViewIfNecessary(module, rule)
        // ForwardedModuleView::if_necessary already handles prefix/show/hide
        let view: Module<'compile, 'parse> = ForwardedModuleView::if_necessary(arena, module, rule);

        // Go: for other := range e.forwardedModules { assertNoConflicts(...) }
        // Hold the shared-map guard across the conflict checks (read-only).
        let forwarded_guard = inner.forwarded_modules.as_ref().unwrap().borrow();
        let forwarded: &IndexMap<Module<'compile, 'parse>, FileSpan<'parse>> = &forwarded_guard;
        for (other_view, _) in forwarded.iter() {
            assert_no_conflicts(
                &view.variables(),
                &other_view.variables(),
                &view,
                other_view,
                "variable",
                forwarded,
                node_span,
            )?;
            assert_no_conflicts(
                &view.functions(),
                &other_view.functions(),
                &view,
                other_view,
                "function",
                forwarded,
                node_span,
            )?;
            assert_no_conflicts(
                &view.mixins(),
                &other_view.mixins(),
                &view,
                other_view,
                "mixin",
                forwarded,
                node_span,
            )?;
        }

        // Go: e.allModules = append(e.allModules, module)
        inner.all_modules.borrow_mut().push(module);

        // Go: e.forwardedModules[view] = nodeWithSpan
        drop(forwarded_guard);
        inner
            .forwarded_modules
            .as_ref()
            .unwrap()
            .borrow_mut()
            .insert(view, node_span);

        Ok(())
    }

    fn get_module(&self, namespace: &str) -> SassResult<Module<'compile, 'parse>> {
        let inner = self.0.borrow();
        inner
            .modules
            .borrow()
            .get(namespace)
            .cloned()
            .ok_or_else(|| {
                Box::new(SassError::Script {
                    message: format!("There is no module with the namespace \"{}\".", namespace),
                    argument_name: None,
                })
            })
    }

    // --- Variable access ---

    /// Returns the value of the variable named `name`, optionally with the
    /// given `namespace`, or `None` when undeclared. Throws when no module
    /// has `namespace`, or when multiple global modules expose `name`.
    pub fn get_variable(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> SassResult<Option<Value<'parse>>> {
        if let Some(ns) = namespace {
            let module = self.get_module(ns)?;
            return Ok(module.variables().get(name));
        }

        // Fast path: last variable cache
        let fast_hit: Option<Value<'parse>> = {
            let inner = self.0.borrow();
            if inner.has_last_variable_index && inner.last_variable_name == name {
                let map = inner.variables[inner.last_variable_index].borrow();
                map.get(name).cloned()
            } else {
                None
            }
        };
        if fast_hit.is_some() {
            return Ok(fast_hit);
        }
        let cache_hit: bool = {
            let inner = self.0.borrow();
            inner.has_last_variable_index && inner.last_variable_name == name
        };
        if cache_hit {
            return self.get_variable_from_global_module(name);
        }

        // Index cache
        let index_cache: Option<usize> = self.0.borrow().variable_indices.get(name).copied();
        if let Some(index) = index_cache {
            let found: Option<Value<'parse>> = {
                let mut inner = self.0.borrow_mut();
                inner.last_variable_name = name.to_string();
                inner.last_variable_index = index;
                inner.has_last_variable_index = true;
                let map = inner.variables[index].borrow();
                map.get(name).cloned()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_variable_from_global_module(name);
        }

        // Scan
        let scan_index: Option<usize> = self.0.borrow().variable_index(name);
        if let Some(index) = scan_index {
            let found: Option<Value<'parse>> = {
                let mut inner = self.0.borrow_mut();
                inner.last_variable_name = name.to_string();
                inner.last_variable_index = index;
                inner.has_last_variable_index = true;
                inner.variable_indices.insert(name.to_string(), index);
                let map = inner.variables[index].borrow();
                map.get(name).cloned()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_variable_from_global_module(name);
        }

        self.get_variable_from_global_module(name)
    }

    /// Returns the value of the variable named `name` from a namespaceless
    /// module, or `None` when no namespaceless module declares it.
    fn get_variable_from_global_module(&self, name: &str) -> SassResult<Option<Value<'parse>>> {
        from_one_module(self, name, "variable", |m: &Module<'_, '_>| {
            m.variables().get(name)
        })
    }

    /// Returns the node for the variable named `name`, or `None` when
    /// undeclared. The node proxies the span where the value originated
    /// (kept as a [`FileSpan`] here; Dart keeps the `AstNode` to defer span
    /// manufacture — every stored node here was only ever `.span()`-ed).
    pub fn get_variable_node(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> SassResult<Option<FileSpan<'parse>>> {
        if let Some(ns) = namespace {
            let module = self.get_module(ns)?;
            return Ok(module.variable_nodes().get(name));
        }

        // Fast path
        let fast_hit: Option<FileSpan<'parse>> = {
            let inner = self.0.borrow();
            if inner.has_last_variable_index && inner.last_variable_name == name {
                let map = inner.variable_nodes[inner.last_variable_index].borrow();
                map.get(name).copied()
            } else {
                None
            }
        };
        if fast_hit.is_some() {
            return Ok(fast_hit);
        }
        let cache_hit: bool = {
            let inner = self.0.borrow();
            inner.has_last_variable_index && inner.last_variable_name == name
        };
        if cache_hit {
            return self.get_variable_node_from_global_module(name);
        }

        // Index cache
        let index_cache: Option<usize> = self.0.borrow().variable_indices.get(name).copied();
        if let Some(index) = index_cache {
            let found: Option<FileSpan<'parse>> = {
                let mut inner = self.0.borrow_mut();
                inner.last_variable_name = name.to_string();
                inner.last_variable_index = index;
                inner.has_last_variable_index = true;
                let map = inner.variable_nodes[index].borrow();
                map.get(name).copied()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_variable_node_from_global_module(name);
        }

        // Scan
        let scan_index: Option<usize> = self.0.borrow().variable_index(name);
        if let Some(index) = scan_index {
            let found: Option<FileSpan<'parse>> = {
                let mut inner = self.0.borrow_mut();
                inner.last_variable_name = name.to_string();
                inner.last_variable_index = index;
                inner.has_last_variable_index = true;
                inner.variable_indices.insert(name.to_string(), index);
                let map = inner.variable_nodes[index].borrow();
                map.get(name).copied()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_variable_node_from_global_module(name);
        }

        self.get_variable_node_from_global_module(name)
    }

    /// Returns the node for the variable named `name` from a namespaceless
    /// module, or `None` when undeclared. Conflict checks are already done by
    /// [`get_variable`](Self::get_variable), so the first match wins.
    fn get_variable_node_from_global_module(
        &self,
        name: &str,
    ) -> SassResult<Option<FileSpan<'parse>>> {
        let inner = self.0.borrow();
        for module in inner
            .imported_modules
            .borrow()
            .keys()
            .chain(inner.global_modules.borrow().keys())
        {
            if let Some(node) = module.variable_nodes().get(name) {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    /// Returns whether a variable named `name` exists.
    pub fn variable_exists(&self, name: &str) -> SassResult<bool> {
        self.get_variable(name, None).map(|v| v.is_some())
    }

    /// Returns whether a global variable named `name` exists. Throws when no
    /// module has `namespace`, or when multiple global modules expose `name`.
    pub fn global_variable_exists(&self, name: &str, namespace: Option<&str>) -> SassResult<bool> {
        if let Some(ns) = namespace {
            let module = self.get_module(ns)?;
            return Ok(module.variables().has(name));
        }
        {
            let inner = self.0.borrow();
            if inner.variables[0].borrow().contains_key(name) {
                return Ok(true);
            }
        }
        self.get_variable_from_global_module(name)
            .map(|v| v.is_some())
    }

    /// Sets the variable named `name` to `val`, associated with `node_span`'s
    /// source span. With `namespace`, sets it in that module; with `global`,
    /// at the top-level scope. Otherwise sets it in the previous scope when
    /// already defined, or the current scope when undefined.
    ///
    /// Throws when `namespace` has no module, when that module defines no
    /// such variable, or when multiple global modules define `name`.
    pub fn set_variable(
        &self,
        name: &str,
        val: Value<'parse>,
        node_span: FileSpan<'parse>,
        namespace: Option<&str>,
        global: bool,
    ) -> SassResult<()> {
        if let Some(ns) = namespace {
            let module = self.get_module(ns)?;
            return module.set_variable(name, val, node_span);
        }

        let is_global = {
            let inner = self.0.borrow();
            global || inner.at_root()
        };

        if is_global {
            // Update cache
            {
                let mut inner = self.0.borrow_mut();
                if !inner.variable_indices.contains_key(name) {
                    inner.last_variable_name = name.to_string();
                    inner.last_variable_index = 0;
                    inner.has_last_variable_index = true;
                    inner.variable_indices.insert(name.to_string(), 0);
                }
            }

            // Check if variable exists in a global module
            let needs_global = {
                let inner = self.0.borrow();
                let map = inner.variables[0].borrow();
                !map.contains_key(name)
            };

            if needs_global {
                let module_with_name: Option<Module<'compile, 'parse>> =
                    from_one_module(self, name, "variable", |m: &Module<'compile, 'parse>| {
                        if m.variables().has(name) {
                            Some(*m)
                        } else {
                            None
                        }
                    })?;
                if let Some(module) = module_with_name {
                    return module.set_variable(name, val, node_span);
                }
            }

            // Write to frame 0
            {
                let inner = self.0.borrow();
                inner.variables[0]
                    .borrow_mut()
                    .insert(name.to_string(), val);
                inner.variable_nodes[0]
                    .borrow_mut()
                    .insert(name.to_string(), node_span);
            }
            return Ok(());
        }

        // Not global, not at root — check nested forwarded modules
        let nested_module: Option<Module<'compile, 'parse>> = {
            let inner = self.0.borrow();
            let mut result = None;
            if let Some(ref nested) = *inner.nested_forwarded_modules.borrow() {
                if !inner.variable_indices.contains_key(name)
                    && inner.variable_index(name).is_none()
                {
                    'outer: for modules in nested.iter().rev() {
                        for module in modules.iter().rev() {
                            if module.variables().has(name) {
                                result = Some(*module);
                                break 'outer;
                            }
                        }
                    }
                }
            }
            result
        };
        if let Some(module) = nested_module {
            return module.set_variable(name, val, node_span);
        }

        // Resolve index
        let index = {
            let mut inner = self.0.borrow_mut();
            let idx = if inner.has_last_variable_index && inner.last_variable_name == name {
                inner.last_variable_index
            } else if let Some(&idx) = inner.variable_indices.get(name) {
                idx
            } else {
                let idx = inner
                    .variable_index(name)
                    .unwrap_or(inner.variables.len() - 1);
                inner.variable_indices.insert(name.to_string(), idx);
                idx
            };

            let idx = if !inner.in_semi_global_scope && idx == 0 {
                let new_idx = inner.variables.len() - 1;
                inner.variable_indices.insert(name.to_string(), new_idx);
                new_idx
            } else {
                idx
            };

            inner.last_variable_name = name.to_string();
            inner.last_variable_index = idx;
            inner.has_last_variable_index = true;
            idx
        };

        // Write at resolved index
        {
            let inner = self.0.borrow();
            inner.variables[index]
                .borrow_mut()
                .insert(name.to_string(), val);
            inner.variable_nodes[index]
                .borrow_mut()
                .insert(name.to_string(), node_span);
        }
        Ok(())
    }

    /// Sets the variable named `name` to `val` in the current scope, even
    /// when an outer scope already declares it (unlike
    /// [`set_variable`](Self::set_variable)).
    pub fn set_local_variable(&self, name: &str, val: Value<'parse>, node_span: FileSpan<'parse>) {
        let mut inner = self.0.borrow_mut();
        let index = inner.variables.len() - 1;
        inner.last_variable_name = name.to_string();
        inner.last_variable_index = index;
        inner.has_last_variable_index = true;
        inner.variable_indices.insert(name.to_string(), index);
        inner.variables[index]
            .borrow_mut()
            .insert(name.to_string(), val);
        inner.variable_nodes[index]
            .borrow_mut()
            .insert(name.to_string(), node_span);
    }

    /// Returns the value of the variable named `name` in the current scope
    /// frame only, or `None` when the current frame declares nothing by that
    /// name. Unlike [`get_variable`](Self::get_variable), outer scopes and
    /// modules are never consulted — and unlike libsass `Env::get_local`,
    /// a miss inserts nothing (reads never mutate the environment here).
    ///
    /// Seeded by the libsass C-API `sass_env_get_local` seam
    /// (`rust-sass-libsass`); semantics-preserving.
    pub fn get_local_variable(&self, name: &str) -> Option<Value<'parse>> {
        let inner = self.0.borrow();
        inner
            .variables
            .last()
            .and_then(|frame| frame.borrow().get(name).cloned())
    }

    /// Returns the value of the global variable named `name`, or `None` when
    /// neither the global frame nor any namespaceless module declares it.
    /// The lookup mirrors [`global_variable_exists`](Self::global_variable_exists)
    /// (frame 0, then the global-module fallback), returning the value.
    ///
    /// Seeded by the libsass C-API `sass_env_get_global` seam
    /// (`rust-sass-libsass`); semantics-preserving.
    pub fn get_global_variable(&self, name: &str) -> SassResult<Option<Value<'parse>>> {
        if let Some(val) = self.0.borrow().variables[0].borrow().get(name).cloned() {
            return Ok(Some(val));
        }
        self.get_variable_from_global_module(name)
    }

    // --- Function access ---

    /// Returns the value of the function named `name`, optionally with the
    /// given `namespace`, or `None` when undeclared. Throws when no module
    /// has `namespace`, or when multiple global modules expose `name`.
    pub fn get_function(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> SassResult<Option<Callable<'compile, 'parse>>> {
        if let Some(ns) = namespace {
            let module = self.get_module(ns)?;
            return Ok(module.functions().get(name));
        }

        // Index cache
        let index_cache: Option<usize> = self.0.borrow().function_indices.get(name).copied();
        if let Some(index) = index_cache {
            let found: Option<Callable<'compile, 'parse>> = {
                let inner = self.0.borrow();
                let map = inner.functions[index].borrow();
                map.get(name).cloned()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_function_from_global_module(name);
        }

        // Scan
        let scan_index: Option<usize> = self.0.borrow().function_index(name);
        if let Some(index) = scan_index {
            let found: Option<Callable<'compile, 'parse>> = {
                let mut inner = self.0.borrow_mut();
                inner.function_indices.insert(name.to_string(), index);
                let map = inner.functions[index].borrow();
                map.get(name).cloned()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_function_from_global_module(name);
        }

        self.get_function_from_global_module(name)
    }

    /// Returns the value of the function named `name` from a namespaceless
    /// module, or `None` when no namespaceless module declares it.
    fn get_function_from_global_module(
        &self,
        name: &str,
    ) -> SassResult<Option<Callable<'compile, 'parse>>> {
        from_one_module_callable(self, name, "function", |m: &Module<'compile, 'parse>| {
            m.functions().get(name)
        })
    }

    /// Returns whether a function named `name` exists. Throws when no module
    /// has `namespace`, or when multiple global modules expose `name`.
    pub fn function_exists(&self, name: &str, namespace: Option<&str>) -> SassResult<bool> {
        self.get_function(name, namespace).map(|f| f.is_some())
    }

    /// Sets `callable` in the current scope, keyed under its own name.
    pub fn set_function(&self, callable: Callable<'compile, 'parse>) {
        let mut inner = self.0.borrow_mut();
        let index = inner.functions.len() - 1;
        let name = callable.name().to_string();
        inner.function_indices.insert(name.clone(), index);
        inner.functions[index].borrow_mut().insert(name, callable);
    }

    // --- Mixin access ---

    /// Returns the value of the mixin named `name`, optionally with the
    /// given `namespace`, or `None` when undeclared. Throws when no module
    /// has `namespace`, or when multiple global modules expose `name`.
    pub fn get_mixin(
        &self,
        name: &str,
        namespace: Option<&str>,
    ) -> SassResult<Option<Callable<'compile, 'parse>>> {
        if let Some(ns) = namespace {
            let module = self.get_module(ns)?;
            return Ok(module.mixins().get(name));
        }

        // Index cache
        let index_cache: Option<usize> = self.0.borrow().mixin_indices.get(name).copied();
        if let Some(index) = index_cache {
            let found: Option<Callable<'compile, 'parse>> = {
                let inner = self.0.borrow();
                let map = inner.mixins[index].borrow();
                map.get(name).cloned()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_mixin_from_global_module(name);
        }

        // Scan
        let scan_index: Option<usize> = self.0.borrow().mixin_index(name);
        if let Some(index) = scan_index {
            let found: Option<Callable<'compile, 'parse>> = {
                let mut inner = self.0.borrow_mut();
                inner.mixin_indices.insert(name.to_string(), index);
                let map = inner.mixins[index].borrow();
                map.get(name).cloned()
            };
            if found.is_some() {
                return Ok(found);
            }
            return self.get_mixin_from_global_module(name);
        }

        self.get_mixin_from_global_module(name)
    }

    /// Returns the value of the mixin named `name` from a namespaceless
    /// module, or `None` when no namespaceless module declares it.
    fn get_mixin_from_global_module(
        &self,
        name: &str,
    ) -> SassResult<Option<Callable<'compile, 'parse>>> {
        from_one_module_callable(self, name, "mixin", |m: &Module<'compile, 'parse>| {
            m.mixins().get(name)
        })
    }

    /// Returns whether a mixin named `name` exists. Throws when no module
    /// has `namespace`, or when multiple global modules expose `name`.
    pub fn mixin_exists(&self, name: &str, namespace: Option<&str>) -> SassResult<bool> {
        self.get_mixin(name, namespace).map(|m| m.is_some())
    }

    /// Sets `callable` in the current scope, keyed under its own name.
    pub fn set_mixin(&self, callable: Callable<'compile, 'parse>) {
        let mut inner = self.0.borrow_mut();
        let index = inner.mixins.len() - 1;
        let name = callable.name().to_string();
        inner.mixin_indices.insert(name.clone(), index);
        inner.mixins[index].borrow_mut().insert(name, callable);
    }

    // --- Configuration ---

    /// Records that `name` is a variable that could have been configured for
    /// this module, whether or not it actually was. Used to detect passing a
    /// new configuration through `@forward` to an already-loaded module.
    pub fn mark_variable_configurable(&self, name: &str) {
        self.0
            .borrow()
            .configurable_variables
            .borrow_mut()
            .insert(name.to_string());
    }

    /// Creates an implicit configuration from the variables declared in this
    /// environment. Imported and nested-forwarded members precede locals at
    /// each scope level, so locals win on conflict.
    pub fn to_implicit_configuration(
        &self,
        arena: &'compile Bump,
    ) -> SassResult<Configuration<'parse>> {
        let inner = self.0.borrow();
        let mut config = IndexMap::new();

        for i in 0..inner.variables.len() {
            let modules: Vec<Module<'compile, 'parse>> = if i == 0 {
                inner.imported_modules.borrow().keys().cloned().collect()
            } else if let Some(ref nested) = *inner.nested_forwarded_modules.borrow() {
                if i < nested.len() {
                    nested[i].clone()
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            for module in &modules {
                for (name, val) in module.variables().entries() {
                    let node = module.variable_nodes().get(&name);
                    let node_span = node.unwrap_or(FileSpan::new(None, 0, 0));
                    config.insert(name.clone(), ConfiguredValue::implicit(val, node_span));
                }
            }

            let values = inner.variables[i].borrow();
            let nodes = inner.variable_nodes[i].borrow();
            for (name, val) in values.iter() {
                let node_span = nodes
                    .get(name)
                    .copied()
                    .unwrap_or(FileSpan::new(None, 0, 0));
                config.insert(name.clone(), ConfiguredValue::implicit(*val, node_span));
            }
        }

        Ok(Configuration::new_implicit(arena, config))
    }

    // --- Scope ---

    /// Runs `callback` in a new scope. Variables, functions, and mixins
    /// declared in the scope are inaccessible outside it. When `semi_global`,
    /// the scope can assign to global variables without a `!global`
    /// declaration. When `when` is false, no scope is created — but
    /// semi-globalness is still tracked, so a conditional assignment inside a
    /// style rule doesn't leak to the global scope.
    #[rust_sass_macros::async_impl]
    pub async fn scope<T>(
        &self,
        arena: &'compile Bump,
        callback: impl AsyncFnOnce() -> SassResult<T>,
        semi_global: bool,
        when: bool,
    ) -> SassResult<T> {
        let semi_global = semi_global && self.0.borrow().in_semi_global_scope;
        let was_in_semi_global_scope = {
            let mut inner = self.0.borrow_mut();
            let old = inner.in_semi_global_scope;
            inner.in_semi_global_scope = semi_global;
            old
        };

        if !when {
            let result = callback().await;
            self.0.borrow_mut().in_semi_global_scope = was_in_semi_global_scope;
            return result;
        }

        // Push frames
        {
            let mut inner = self.0.borrow_mut();
            inner.push_frame(arena);
        }

        let result = callback().await;

        // Pop frames
        {
            let mut inner = self.0.borrow_mut();
            inner.in_semi_global_scope = was_in_semi_global_scope;
            inner.pop_frame();
        }

        result
    }

    #[rust_sass_macros::sync_impl]
    #[rust_sass_macros::must_be_sync]
    pub async fn scope<T>(
        &self,
        arena: &'compile Bump,
        callback: impl FnOnce() -> SassResult<T>,
        semi_global: bool,
        when: bool,
    ) -> SassResult<T> {
        let semi_global = semi_global && self.0.borrow().in_semi_global_scope;
        let was_in_semi_global_scope = {
            let mut inner = self.0.borrow_mut();
            let old = inner.in_semi_global_scope;
            inner.in_semi_global_scope = semi_global;
            old
        };

        if !when {
            let result = callback().await;
            self.0.borrow_mut().in_semi_global_scope = was_in_semi_global_scope;
            return result;
        }

        // Push frames
        {
            let mut inner = self.0.borrow_mut();
            inner.push_frame(arena);
        }

        let result = callback().await;

        // Pop frames
        {
            let mut inner = self.0.borrow_mut();
            inner.in_semi_global_scope = was_in_semi_global_scope;
            inner.pop_frame();
        }

        result
    }

    // --- toModule / toDummyModule ---

    /// Returns a module representing the top-level members defined here,
    /// holding `css`/`pre_module_comments` as its CSS, extendable via
    /// `extension_store`.
    pub fn to_module(
        &self,
        arena: &'compile Bump,
        css: &ModifiableCssNode<'parse>,
        pre_module_comments: &IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>>,
        extension_store: ExtensionStore<'parse>,
    ) -> Module<'compile, 'parse> {
        let inner = self.0.borrow();
        let forwarded_guard = inner.forwarded_modules.as_ref().map(|fm| fm.borrow());
        // Dart iterates `forwarded` (a LinkedHashSet) in insertion order;
        // `IndexMap` preserves it — keep a Vec (not a HashSet) so member
        // order (e.g. `meta.module-variables`) matches Dart.
        let forwarded: Vec<Module<'compile, 'parse>> = forwarded_guard
            .as_ref()
            .map(|fm| fm.keys().cloned().collect())
            .unwrap_or_default();

        EnvironmentModule::new(
            arena,
            *self,
            css.clone(),
            pre_module_comments.clone(),
            extension_store,
            &forwarded,
            &inner.all_modules.borrow(),
        )
    }

    /// Returns a module with the same members and upstream modules, but an
    /// empty stylesheet and extension store. Used when resolving imports,
    /// which need to inject forwarded members into the current scope — the
    /// only case where a nested environment becomes a module.
    pub fn to_dummy_module(&self, arena: &'compile Bump) -> Module<'compile, 'parse> {
        let inner = self.0.borrow();
        let forwarded_guard = inner.forwarded_modules.as_ref().map(|fm| fm.borrow());
        // Dart iterates `forwarded` (a LinkedHashSet) in insertion order;
        // `IndexMap` preserves it — keep a Vec (not a HashSet) so member
        // order (e.g. `meta.module-variables`) matches Dart.
        let forwarded: Vec<Module<'compile, 'parse>> = forwarded_guard
            .as_ref()
            .map(|fm| fm.keys().cloned().collect())
            .unwrap_or_default();

        EnvironmentModule::new_dummy(*self, &forwarded, &inner.all_modules.borrow(), arena)
    }

    /// Makes members forwarded by module available in the current environment.
    /// Called when module is @imported.
    ///
    /// At the root, conflicting members are shadowed and the forwarded
    /// modules join the imported/forwarded maps; when nested, they append to
    /// the nested-forwarded list. Either way, now-shadowed locals are
    /// removed.
    ///
    /// Go: environment.go:409
    /// Dart: environment.dart:358
    pub fn import_forwards(
        &self,
        arena: &'compile Bump,
        module: &Module<'compile, 'parse>,
    ) -> SassResult<()> {
        let env_mod = match module.kind() {
            ModuleKind::Environment(e) => e,
            _ => return Ok(()),
        };

        let module_inner = env_mod.environment.0.borrow();
        if module_inner.forwarded_modules.is_none() {
            return Ok(());
        }
        let mut forwarded = module_inner
            .forwarded_modules
            .as_ref()
            .unwrap()
            .borrow()
            .clone();
        drop(module_inner);

        let mut inner = self.0.borrow_mut();

        // Omit modules from forwarded that are already globally available and
        // forwarded in this module.
        if let Some(existing) = inner.forwarded_modules.as_ref() {
            let existing_guard = existing.borrow();
            let global_guard = inner.global_modules.borrow();
            forwarded.retain(|mod_, _| {
                !(existing_guard.contains_key(mod_) && global_guard.contains_key(mod_))
            });
        } else {
            inner.forwarded_modules = Some(arena.alloc(RefCell::new(IndexMap::new())));
        }

        let mut forwarded_vars = HashSet::new();
        let mut forwarded_funcs = HashSet::new();
        let mut forwarded_mixins = HashSet::new();
        for mod_ in forwarded.keys() {
            for name in mod_.variables().keys() {
                forwarded_vars.insert(name.clone());
            }
            for name in mod_.functions().keys() {
                forwarded_funcs.insert(name.clone());
            }
            for name in mod_.mixins().keys() {
                forwarded_mixins.insert(name.clone());
            }
        }

        if inner.at_root() {
            // Hide members from modules that have already been imported or
            // forwarded that would otherwise conflict with the @imported members.
            let imported_keys: Vec<_> = inner.imported_modules.borrow().keys().cloned().collect();
            for mod_ in imported_keys {
                if let Some(shadowed) = ShadowedModuleView::if_necessary(
                    arena,
                    &mod_,
                    &forwarded_vars,
                    &forwarded_funcs,
                    &forwarded_mixins,
                ) {
                    let node = inner
                        .imported_modules
                        .borrow_mut()
                        .shift_remove(&mod_)
                        .unwrap();
                    let is_empty = match shadowed.kind() {
                        ModuleKind::Shadowed(s) => s.is_empty()?,
                        _ => false,
                    };
                    if !is_empty {
                        inner.imported_modules.borrow_mut().insert(shadowed, node);
                    }
                }
            }

            if let Some(fwd_cell) = inner.forwarded_modules.as_ref() {
                let mut fwd = fwd_cell.borrow_mut();
                let forwarded_keys: Vec<_> = fwd.keys().cloned().collect();
                for mod_ in forwarded_keys {
                    if let Some(shadowed) = ShadowedModuleView::if_necessary(
                        arena,
                        &mod_,
                        &forwarded_vars,
                        &forwarded_funcs,
                        &forwarded_mixins,
                    ) {
                        let node = fwd.shift_remove(&mod_).unwrap();
                        let is_empty = match shadowed.kind() {
                            ModuleKind::Shadowed(s) => s.is_empty()?,
                            _ => false,
                        };
                        if !is_empty {
                            fwd.insert(shadowed, node);
                        }
                    }
                }
            }

            // Copy forwarded modules into imported and forwarded maps
            for (mod_, node) in &forwarded {
                inner.imported_modules.borrow_mut().insert(*mod_, *node);
            }
            if let Some(fwd_cell) = inner.forwarded_modules.as_ref() {
                let mut fwd = fwd_cell.borrow_mut();
                for (mod_, node) in &forwarded {
                    fwd.insert(*mod_, *node);
                }
            }
        } else {
            if inner.nested_forwarded_modules.borrow().is_none() {
                let len = inner.variables.len();
                let mut nested: Vec<Vec<Module<'compile, 'parse>>> = Vec::with_capacity(len);
                for _ in 0..len {
                    nested.push(Vec::new());
                }
                *inner.nested_forwarded_modules.borrow_mut() = Some(nested);
            }
            let mut guard = inner.nested_forwarded_modules.borrow_mut();
            let nested = guard.as_mut().unwrap();
            let last = nested.len() - 1;
            for mod_ in forwarded.keys() {
                nested[last].push(*mod_);
            }
        }

        // Remove existing member definitions that are now shadowed by the
        // forwarded modules.
        for name in &forwarded_vars {
            inner.variable_indices.remove(name);
            if let Some(last_vars) = inner.variables.last() {
                last_vars.borrow_mut().shift_remove(name);
            }
            if let Some(last_nodes) = inner.variable_nodes.last() {
                last_nodes.borrow_mut().shift_remove(name);
            }
        }
        for name in &forwarded_funcs {
            inner.function_indices.remove(name);
            if let Some(last_funcs) = inner.functions.last() {
                last_funcs.borrow_mut().shift_remove(name);
            }
        }
        for name in &forwarded_mixins {
            inner.mixin_indices.remove(name);
            if let Some(last_mixins) = inner.mixins.last() {
                last_mixins.borrow_mut().shift_remove(name);
            }
        }

        Ok(())
    }
}

// === assert_no_conflicts ===
//
// Throws when `new_members` from `new_module` overlaps `old_members` from
// `old_module` with different origins (Dart: `Environment._assertNoConflicts`,
// `environment.dart:318-352`). Variables compare by definition identity
// (`variable_identity`); functions/mixins compare by value (callable address
// identity). Same-origin overlaps are allowed.

/// Returns an error if newMembers has any keys that overlap with oldMembers
/// from different origins.
///
/// Matches Go: assertNoConflicts
fn assert_no_conflicts<'compile: 'parse, 'parse, V: Clone + Debug + PartialEq>(
    new_members: &ModuleVariables<'_, V>,
    old_members: &ModuleVariables<'_, V>,
    new_module: &Module<'compile, 'parse>,
    old_module: &Module<'compile, 'parse>,
    member_type: &str,
    forwarded_modules: &IndexMap<Module<'compile, 'parse>, FileSpan<'parse>>,
    new_span: FileSpan<'parse>,
) -> SassResult<()> {
    let (smaller, larger, smaller_mod, larger_mod) = if new_members.len() < old_members.len() {
        (new_members, old_members, new_module, old_module)
    } else {
        (old_members, new_members, old_module, new_module)
    };

    for name in smaller.keys() {
        if !larger.has(&name) {
            continue;
        }

        let same_identity = if member_type == "variable" {
            let smaller_id = smaller_mod.variable_identity(&name).ok();
            let larger_id = larger_mod.variable_identity(&name).ok();
            match (smaller_id, larger_id) {
                (Some(ref a), Some(ref b)) => a == b,
                _ => false,
            }
        } else {
            let small_val = smaller.get(&name);
            let large_val = larger.get(&name);
            match (small_val, large_val) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        };

        if same_identity {
            continue;
        }

        let display_name = if member_type == "variable" {
            format!("${name}")
        } else {
            name.clone()
        };

        let mut secondary = Vec::new();
        if let Some(old_span) = forwarded_modules.get(old_module) {
            secondary.push((
                SourceSpanWithContext::from_file_span(old_span)?,
                "original @forward".into(),
            ));
        }

        // Go: returns MultiSpanSassScriptException (primary span from caller)
        return Err(Box::new(SassError::MultiSpan {
            message: format!(
                "Two forwarded modules both define a {} named {}.",
                member_type, display_name
            ),
            span: SourceSpanWithContext::from_file_span(&new_span)?,
            primary_label: Some("new @forward".to_string()),
            secondary,
            original_source: None,
            cause: None,
            loaded_urls: vec![],
            trace: Default::default(),
        }));
    }

    Ok(())
}

// === from_one_module ===
//
// Resolves `name` from exactly one namespaceless module (Dart:
// `Environment._fromOneModule`, `environment.dart:918-957`): nested
// forwarded modules first, then imported, then global — with
// identity-based conflict detection only in the global phase (a differing
// identity produces a "multiple global modules" error). Split into two
// functions because the identity type differs: variables use
// `variable_identity` (a `Module`), callables use the `Callable` itself.

// Dart: `Environment._fromOneModule` (environment.dart:918-962). The
// variable path: identity is `module.variableIdentity(name)` — a Module
// is never a Callable, so Dart takes the `variableIdentity` branch.
pub(crate) fn from_one_module<'compile: 'parse, 'parse, T>(
    env: &Environment<'compile, 'parse>,
    name: &str,
    type_name: &str,
    callback: impl Fn(&Module<'compile, 'parse>) -> Option<T>,
) -> SassResult<Option<T>>
where
    T: Clone,
{
    let inner = env.0.borrow();

    if let Some(ref nested) = *inner.nested_forwarded_modules.borrow() {
        for modules in nested.iter().rev() {
            for module in modules.iter().rev() {
                if let Some(val) = callback(module) {
                    return Ok(Some(val));
                }
            }
        }
    }

    for module in inner.imported_modules.borrow().keys() {
        if let Some(val) = callback(module) {
            return Ok(Some(val));
        }
    }

    let mut value: Option<T> = None;
    let mut identity: Option<Module<'compile, 'parse>> = None;

    let global_guard = inner.global_modules.borrow();
    for (module, _) in global_guard.iter() {
        if let Some(val_in_module) = callback(module) {
            let identity_in_module: Option<Module<'compile, 'parse>> =
                module.variable_identity(name).ok();

            if let (Some(ref a), Some(ref b)) = (&identity, &identity_in_module) {
                if a == b {
                    continue;
                }
            }

            if value.is_some() {
                // Dart's `_fromOneModule` lists EVERY matching global module
                // as a secondary span (labeled "includes $type"), including
                // same-identity entries skipped above — so rebuild the list
                // from scratch instead of reusing an incremental collection.
                let mut secondary: Vec<(SourceSpanWithContext, String)> = Vec::new();
                for (other, other_span) in global_guard.iter() {
                    if callback(other).is_some() {
                        let span_ctx = SourceSpanWithContext::from_file_span(other_span)?;
                        secondary.push((span_ctx, format!("includes {type_name}")));
                    }
                }
                // Mirrors Dart's `MultiSpanSassScriptException`: no primary
                // span yet. The eval call site attaches the member-use span +
                // trace via `SassError::with_member_use_span`.
                return Err(Box::new(SassError::MultiSpanScript {
                    message: format!("This {type_name} is available from multiple global modules."),
                    primary_label: Some(format!("{type_name} use")),
                    secondary,
                    cause: None,
                    loaded_urls: vec![],
                }));
            }
            value = Some(val_in_module);
            identity = identity_in_module;
        }
    }

    Ok(value)
}

// Dart: `Environment._fromOneModule` (environment.dart:918-962). The
// callable path: when the resolved value is a `Callable`, Dart uses the
// callable itself as the dedup identity (`valueInModule is Callable ?
// valueInModule : module.variableIdentity(name)`), so two global modules
// forwarding the same upstream function/mixin resolve instead of
// conflicting. `Callable: PartialEq` is address identity.
pub(crate) fn from_one_module_callable<'compile: 'parse, 'parse>(
    env: &Environment<'compile, 'parse>,
    _name: &str,
    type_name: &str,
    callback: impl Fn(&Module<'compile, 'parse>) -> Option<Callable<'compile, 'parse>>,
) -> SassResult<Option<Callable<'compile, 'parse>>> {
    let inner = env.0.borrow();

    if let Some(ref nested) = *inner.nested_forwarded_modules.borrow() {
        for modules in nested.iter().rev() {
            for module in modules.iter().rev() {
                if let Some(val) = callback(module) {
                    return Ok(Some(val));
                }
            }
        }
    }

    for module in inner.imported_modules.borrow().keys() {
        if let Some(val) = callback(module) {
            return Ok(Some(val));
        }
    }

    let mut value: Option<Callable<'compile, 'parse>> = None;
    let mut identity: Option<Callable<'compile, 'parse>> = None;

    let global_guard = inner.global_modules.borrow();
    for (module, _) in global_guard.iter() {
        if let Some(val_in_module) = callback(module) {
            if let Some(ref id) = identity {
                if *id == val_in_module {
                    continue;
                }
            }

            if value.is_some() {
                // Same all-matching-modules secondary-span rule as above.
                let mut secondary: Vec<(SourceSpanWithContext, String)> = Vec::new();
                for (other, other_span) in global_guard.iter() {
                    if callback(other).is_some() {
                        let span_ctx = SourceSpanWithContext::from_file_span(other_span)?;
                        secondary.push((span_ctx, format!("includes {type_name}")));
                    }
                }
                return Err(Box::new(SassError::MultiSpanScript {
                    message: format!("This {type_name} is available from multiple global modules."),
                    primary_label: Some(format!("{type_name} use")),
                    secondary,
                    cause: None,
                    loaded_urls: vec![],
                }));
            }
            value = Some(val_in_module);
            identity = Some(val_in_module);
        }
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::compile::compile;
    use crate::compile::compile_string;
    use crate::compile::CompileOptions;
    use crate::io::Io;
    use crate::io::VirtualIo;
    use crate::value::SassNumber;
    use crate::value::ValueKind;
    use std::collections::HashMap;
    use std::rc::Rc;

    // Dart `Environment._fromOneModule` (environment.dart:937-939): when the
    // resolved value is a `Callable`, identity is the callable itself — so
    // two namespaceless `@use`d modules forwarding the same upstream `f()`
    // resolve instead of raising "multiple global modules".
    #[rust_sass_macros::maybe_test]
    async fn test_forwarded_function_shared_identity() {
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/main.scss".to_string(),
            "@use \"m1\" as *;\n@use \"m2\" as *;\na { b: f(); }".to_string(),
        );
        files.insert(
            "/_up.scss".to_string(),
            "@function f() { @return 1; }".to_string(),
        );
        files.insert("/_m1.scss".to_string(), "@forward \"up\";".to_string());
        files.insert("/_m2.scss".to_string(), "@forward \"up\";".to_string());
        let io: Rc<dyn Io> = Rc::new(VirtualIo::with_files(files));
        let result = compile("/main.scss", io, CompileOptions::new(&arena), &arena)
            .await
            .unwrap();
        assert_eq!(result.css(), "a {\n  b: 1;\n}");
    }

    // Same-identity entries are skipped for the conflict decision, but Dart
    // lists EVERY matching global module as a secondary span — so with two
    // same-identity forwarders plus one distinct definer, the error carries
    // all three "includes function" spans.
    #[rust_sass_macros::maybe_test]
    async fn test_conflict_lists_all_matching_modules() {
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/main.scss".to_string(),
            "@use \"m1\" as *;\n@use \"m2\" as *;\n@use \"m3\" as *;\na { b: f(); }".to_string(),
        );
        files.insert(
            "/_up.scss".to_string(),
            "@function f() { @return 1; }".to_string(),
        );
        files.insert(
            "/_other.scss".to_string(),
            "@function f() { @return 2; }".to_string(),
        );
        files.insert("/_m1.scss".to_string(), "@forward \"up\";".to_string());
        files.insert("/_m2.scss".to_string(), "@forward \"up\";".to_string());
        files.insert("/_m3.scss".to_string(), "@forward \"other\";".to_string());
        let io: Rc<dyn Io> = Rc::new(VirtualIo::with_files(files));
        let err = compile("/main.scss", io, CompileOptions::new(&arena), &arena)
            .await
            .unwrap_err();
        match *err {
            SassError::MultiSpan {
                message, secondary, ..
            }
            | SassError::MultiSpanScript {
                message, secondary, ..
            } => {
                assert_eq!(
                    message,
                    "This function is available from multiple global modules."
                );
                assert_eq!(secondary.len(), 3);
                for (_, label) in &secondary {
                    assert_eq!(label, "includes function");
                }
            }
            other => panic!("expected MultiSpan error, got {other:?}"),
        }
    }

    // Dart `Environment.closure` resets `_inMixin` to false and shares the
    // module maps: a `@content` block created by an `@include` inside a mixin
    // body must not carry `in_mixin=true` — `content-exists()` called from it
    // errors instead of returning `false`.
    #[rust_sass_macros::maybe_test]
    async fn test_closure_resets_in_mixin() {
        let arena = Bump::new();
        let io: Rc<dyn Io> = Rc::new(VirtualIo::new());
        let err = compile_string(
            "@use \"sass:meta\";\n@mixin inner { @content; }\n@mixin outer {\n  @include inner {\n    a { b: meta.content-exists(); }\n  }\n}\n@include outer;\n",
            io,
            CompileOptions::new(&arena),
            &arena,
        )
        .await
        .unwrap_err();
        match *err {
            SassError::Runtime { message, .. } => assert_eq!(
                message,
                "content-exists() may only be called within a mixin."
            ),
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    // Frame-scoped readers seeded by the libsass C-API seam
    // (`sass_env_get_local` / `sass_env_get_global`): `get_local_variable`
    // sees the current frame only, `get_global_variable` the global frame
    // (plus the global-module fallback, like `global_variable_exists`).
    // Frames are pushed/popped directly: the tests live in the same file,
    // so the private frame primitives are in reach.
    #[test]
    fn test_frame_scoped_variable_readers() {
        fn num<'a>(arena: &'a Bump, v: f64) -> Value<'a> {
            Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
        }

        fn number_of(value: Value<'_>) -> Option<f64> {
            match *value {
                ValueKind::Number(ref n) => Some(n.value),
                _ => None,
            }
        }

        let arena = Bump::new();
        let env = Environment::new(&arena);
        env.set_variable("g", num(&arena, 1.0), BOGUS_SPAN, None, false)
            .unwrap();

        // Root: current frame and global frame coincide.
        assert_eq!(number_of(env.get_local_variable("g").unwrap()), Some(1.0));
        assert_eq!(
            number_of(env.get_global_variable("g").unwrap().unwrap()),
            Some(1.0)
        );
        // Misses read None — and insert nothing (no default-insert on read).
        assert!(env.get_local_variable("missing").is_none());
        assert!(env.get_global_variable("missing").unwrap().is_none());
        assert!(env.get_variable("missing", None).unwrap().is_none());

        // A pushed frame shadows: local sees only it, global still sees
        // frame 0, lexical sees both.
        env.0.borrow_mut().push_frame(&arena);
        env.set_local_variable("l", num(&arena, 2.0), BOGUS_SPAN);
        assert_eq!(number_of(env.get_local_variable("l").unwrap()), Some(2.0));
        assert!(env.get_local_variable("g").is_none());
        assert!(env.get_global_variable("l").unwrap().is_none());
        assert_eq!(
            number_of(env.get_global_variable("g").unwrap().unwrap()),
            Some(1.0)
        );
        assert!(env.get_variable("l", None).unwrap().is_some());
        assert!(env.get_variable("g", None).unwrap().is_some());

        // Popping drops the frame's bindings from every reader.
        env.0.borrow_mut().pop_frame();
        assert!(env.get_local_variable("l").is_none());
        assert!(env.get_variable("l", None).unwrap().is_none());
        assert_eq!(number_of(env.get_local_variable("g").unwrap()), Some(1.0));
    }

    // Dart `closure()`/`forImport()` share the module maps by reference and
    // reset `in_mixin`; `forImport` additionally shares the configurable set
    // while `closure` gets a detached empty one.
    #[rust_sass_macros::maybe_test]
    async fn test_scope_map_sharing() {
        let arena = Bump::new();
        let env = Environment::new(&arena);
        env.set_in_mixin(true);

        let closed = env.closure(&arena);
        assert!(!closed.in_mixin());
        let imported = env.for_import(&arena);
        assert!(!imported.in_mixin());

        // Module maps shared with the closure (same allocations).
        {
            let outer = env.0.borrow();
            let inner = closed.0.borrow();
            assert!(std::ptr::eq(outer.modules, inner.modules));
            assert!(std::ptr::eq(outer.namespace_nodes, inner.namespace_nodes));
            assert!(std::ptr::eq(outer.global_modules, inner.global_modules));
            assert!(std::ptr::eq(outer.imported_modules, inner.imported_modules));
            assert!(std::ptr::eq(outer.all_modules, inner.all_modules));
            assert_eq!(
                outer.forwarded_modules.is_none(),
                inner.forwarded_modules.is_none()
            );
        }

        // Configurable set: shared with for_import, detached for closure.
        imported.mark_variable_configurable("shared-var");
        assert!(env
            .0
            .borrow()
            .configurable_variables
            .borrow()
            .contains("shared-var"));
        closed.mark_variable_configurable("closure-var");
        assert!(!env
            .0
            .borrow()
            .configurable_variables
            .borrow()
            .contains("closure-var"));
    }
}
