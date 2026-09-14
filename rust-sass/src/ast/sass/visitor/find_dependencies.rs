// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/find_dependencies.dart
// go-source: go/value/sass_find_dependencies.go

use std::collections::HashSet;
use std::marker::PhantomData;

use crate::url::SassUrl;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::import::Import;
use crate::ast::sass::statement::{
    AtRootRule, AtRule, ContentBlock, ContentRule, DebugRule, Declaration, EachRule, ErrorRule,
    ExtendRule, ForRule, ForwardRule, FunctionRule, IfRule, ImportRule, IncludeRule, LoudComment,
    MediaRule, MixinRule, ReturnRule, SilentComment, Statement, StatementVisitor, StyleRule,
    Stylesheet, SupportsRule, UseRule, VariableDeclaration, WarnRule, WhileRule,
};
use crate::common::exception::SassResult;

/// A struct of different types of dependencies a Sass stylesheet can contain.
#[derive(Debug, Default)]
pub struct DependencyReport {
    /// All `@use`d URLs in the stylesheet, excluding built-in modules.
    pub uses: HashSet<SassUrl>,
    /// All `@forward`ed URLs in the stylesheet, excluding built-in modules.
    pub forwards: HashSet<SassUrl>,
    /// All URLs loaded by `meta.load-css()` calls with static string
    /// arguments outside of mixins.
    pub meta_load_css: HashSet<SassUrl>,
    /// All dynamically `@import`ed URLs in the stylesheet.
    pub imports: HashSet<SassUrl>,
}

impl DependencyReport {
    /// Returns all URLs from uses, forwards, and meta_load_css.
    ///
    /// These are the module-system dependencies, as opposed to legacy
    /// `@import`s.
    pub fn modules(&self) -> HashSet<SassUrl> {
        let mut result = HashSet::new();
        for u in &self.uses {
            result.insert(u.clone());
        }
        for u in &self.forwards {
            result.insert(u.clone());
        }
        for u in &self.meta_load_css {
            result.insert(u.clone());
        }
        result
    }

    /// Returns all URLs from uses, forwards, meta_load_css, and imports.
    ///
    /// The union of [`DependencyReport::modules`] with the legacy
    /// `@import` dependencies.
    pub fn all(&self) -> HashSet<SassUrl> {
        let mut result = self.modules();
        for u in &self.imports {
            result.insert(u.clone());
        }
        result
    }
}

/// Returns stylesheet's statically-declared dependencies.
///
/// Only top-level static declarations count: control flow, callable
/// declarations, interpolations, and supports conditions can never contain
/// imports, so those subtrees are skipped rather than traversed.
pub fn find_dependencies(stylesheet: &Stylesheet<'_>) -> SassResult<DependencyReport> {
    let mut visitor = FindDependenciesVisitor::new();
    Statement::Stylesheet(stylesheet.clone()).accept(&mut visitor)?;
    Ok(visitor.into_report())
}

// A visitor that traverses a stylesheet and records all its dependencies on
// other stylesheets. Extends the statement-only recursion: expression-bearing
// rules are leaves (their URLs, if any, are dynamic and not statically
// analyzable). `sass:meta` namespaces are tracked so that only `load-css`
// calls through a known `sass:meta` namespace count.
struct FindDependenciesVisitor<'parse> {
    uses: HashSet<SassUrl>,
    forwards: HashSet<SassUrl>,
    meta_load_css: HashSet<SassUrl>,
    imports: HashSet<SassUrl>,
    /// The namespaces under which `sass:meta` has been `@use`d in this
    /// stylesheet. An empty string means `sass:meta` was loaded without a
    /// namespace.
    meta_namespaces: HashSet<String>,
    _pd: PhantomData<&'parse ()>,
}

impl<'parse> FindDependenciesVisitor<'parse> {
    fn new() -> Self {
        FindDependenciesVisitor {
            uses: HashSet::new(),
            forwards: HashSet::new(),
            meta_load_css: HashSet::new(),
            imports: HashSet::new(),
            meta_namespaces: HashSet::new(),
            _pd: PhantomData,
        }
    }

    fn into_report(self) -> DependencyReport {
        DependencyReport {
            uses: self.uses,
            forwards: self.forwards,
            meta_load_css: self.meta_load_css,
            imports: self.imports,
        }
    }

    fn visit_children(&mut self, children: &[Statement<'parse>]) -> SassResult<()> {
        for child in children {
            child.accept(self)?;
        }
        Ok(())
    }
}

impl<'parse> StatementVisitor<'parse> for FindDependenciesVisitor<'parse> {
    type Output = ();

    fn visit_at_root_rule(&mut self, node: &AtRootRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_at_rule(&mut self, node: &AtRule<'parse>) -> SassResult<()> {
        if let Some(ref children) = node.children {
            self.visit_children(children)?;
        }
        Ok(())
    }

    fn visit_content_block(&mut self, _node: &ContentBlock<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_content_rule(&mut self, _node: &ContentRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_debug_rule(&mut self, _node: &DebugRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_declaration(&mut self, node: &Declaration<'parse>) -> SassResult<()> {
        if let Some(ref children) = node.children {
            self.visit_children(children)?;
        }
        Ok(())
    }

    fn visit_each_rule(&mut self, _node: &EachRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_error_rule(&mut self, _node: &ErrorRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_extend_rule(&mut self, _node: &ExtendRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_for_rule(&mut self, _node: &ForRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_forward_rule(&mut self, node: &ForwardRule<'parse>) -> SassResult<()> {
        if node.url.scheme() != "sass" {
            self.forwards.insert(node.url.clone());
        }
        Ok(())
    }

    fn visit_function_rule(&mut self, _node: &FunctionRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_if_rule(&mut self, _node: &IfRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_import_rule(&mut self, node: &ImportRule<'parse>) -> SassResult<()> {
        for imp in &node.imports {
            if let Import::Dynamic(d) = imp {
                self.imports.insert(d.url());
            }
        }
        Ok(())
    }

    fn visit_include_rule(&mut self, node: &IncludeRule<'parse>) -> SassResult<()> {
        if node.name != "load-css" {
            return Ok(());
        }
        let ns = node.namespace.as_deref().unwrap_or("");
        if !self.meta_namespaces.contains(ns) {
            return Ok(());
        }
        // Dart matches exactly one positional argument
        // (`case [StringExpression(...)]`); extra args mean "not statically
        // analyzable" and are ignored.
        if node.arguments.positional.len() != 1 {
            return Ok(());
        }
        // Edition 2021 has no let-chains, so the nested `if let`s stay;
        // the nesting mirrors Dart's `case [StringExpression(...)]` cascade.
        #[allow(clippy::collapsible_match)]
        if let Some(first) = node.arguments.positional.first() {
            if let Expression::String(se) = first {
                if let Some(plain) = se.text.as_plain() {
                    if let Ok(u) = SassUrl::parse(plain) {
                        self.meta_load_css.insert(u);
                    }
                }
            }
        }
        Ok(())
    }

    fn visit_loud_comment(&mut self, _node: &LoudComment<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_media_rule(&mut self, node: &MediaRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_mixin_rule(&mut self, _node: &MixinRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_return_rule(&mut self, _node: &ReturnRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_silent_comment(&mut self, _node: &SilentComment<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_stylesheet(&mut self, node: &Stylesheet<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_style_rule(&mut self, node: &StyleRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_supports_rule(&mut self, node: &SupportsRule<'parse>) -> SassResult<()> {
        self.visit_children(&node.children)
    }

    fn visit_use_rule(&mut self, node: &UseRule<'parse>) -> SassResult<()> {
        if node.url.scheme() != "sass" {
            self.uses.insert(node.url.clone());
        } else if node.url.as_str() == "sass:meta" {
            let ns = node.namespace.clone().unwrap_or_default();
            self.meta_namespaces.insert(ns);
        }
        Ok(())
    }

    fn visit_variable_declaration(
        &mut self,
        _node: &VariableDeclaration<'parse>,
    ) -> SassResult<()> {
        Ok(())
    }

    fn visit_warn_rule(&mut self, _node: &WarnRule<'parse>) -> SassResult<()> {
        Ok(())
    }

    fn visit_while_rule(&mut self, _node: &WhileRule<'parse>) -> SassResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::argument_list::ArgumentList;
    use crate::ast::sass::dynamic_import::DynamicImport;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_string::StringExpression;
    use crate::ast::sass::import::Import;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
    use bumpalo::Bump;

    fn test_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_find_dependencies_use_rule() {
        let arena = Bump::new();
        let span = test_span(&arena, "@use 'foo';");
        let u = SassUrl::parse("file:///foo").unwrap();
        let ur = UseRule::new(u.clone(), None, span, vec![]).unwrap();
        let ss = Stylesheet::new(vec![Statement::UseRule(ur)], span);
        let report = find_dependencies(&ss).unwrap();

        assert!(report.uses.contains(&u));
    }

    #[test]
    fn test_find_dependencies_forward_rule() {
        let arena = Bump::new();
        let span = test_span(&arena, "@forward 'bar';");
        let u = SassUrl::parse("file:///bar").unwrap();
        let fr = ForwardRule::new(u.clone(), span, None, vec![]);
        let ss = Stylesheet::new(vec![Statement::ForwardRule(fr)], span);
        let report = find_dependencies(&ss).unwrap();

        assert!(report.forwards.contains(&u));
    }

    #[test]
    fn test_find_dependencies_built_in_excluded() {
        let arena = Bump::new();
        let span = test_span(&arena, "@use 'sass:color';");
        let u = SassUrl::parse("sass:color").unwrap();
        let ur = UseRule::new(u, None, span, vec![]).unwrap();
        let ss = Stylesheet::new(vec![Statement::UseRule(ur)], span);
        let report = find_dependencies(&ss).unwrap();

        assert!(report.uses.is_empty());
    }

    #[test]
    fn test_find_dependencies_meta_load_css() {
        let arena = Bump::new();
        let span = test_span(&arena, "@use 'sass:meta';");
        let meta_url = SassUrl::parse("sass:meta").unwrap();
        let meta_use = UseRule::new(meta_url, Some("meta".into()), span, vec![]).unwrap();

        let include_span = test_span(&arena, "@include meta.load-css('baz');");
        let arg_text = Interpolation::plain("file:///baz".into(), test_span(&arena, "file:///baz"));
        let str_expr = Expression::String(StringExpression::new(arg_text, true));
        let args = ArgumentList::new(
            vec![str_expr],
            indexmap::IndexMap::new(),
            indexmap::IndexMap::new(),
            include_span,
            None,
            None,
        );
        let inc = IncludeRule::new(
            "load-css".into(),
            args,
            include_span,
            Some("meta".into()),
            None,
        );

        let ss = Stylesheet::new(
            vec![Statement::UseRule(meta_use), Statement::IncludeRule(inc)],
            span,
        );
        let report = find_dependencies(&ss).unwrap();

        let expected = SassUrl::parse("file:///baz").unwrap();
        assert!(report.meta_load_css.contains(&expected));
    }

    #[test]
    fn test_find_dependencies_import_rule() {
        let arena = Bump::new();
        let span = test_span(&arena, "@import 'file';");
        let u = SassUrl::parse("file:///file").unwrap();
        let di = Import::Dynamic(DynamicImport::new(
            u.to_string(),
            test_span(&arena, "'file'"),
        ));
        let ir = ImportRule::new(vec![di], span);
        let ss = Stylesheet::new(vec![Statement::ImportRule(ir)], span);
        let report = find_dependencies(&ss).unwrap();

        assert!(report.imports.contains(&u));
    }

    #[test]
    fn test_find_dependencies_modules_method() {
        let arena = Bump::new();
        let span = test_span(&arena, "@use 'mod';");
        let u = SassUrl::parse("file:///mod").unwrap();
        let ur = UseRule::new(u.clone(), None, span, vec![]).unwrap();
        let ss = Stylesheet::new(vec![Statement::UseRule(ur)], span);
        let report = find_dependencies(&ss).unwrap();

        let modules = report.modules();
        assert!(modules.contains(&u));
    }

    #[test]
    fn test_find_dependencies_all_method() {
        let arena = Bump::new();
        let span = test_span(&arena, "@use 'mod'; @import 'imp';");
        let u = SassUrl::parse("file:///mod").unwrap();
        let ur = UseRule::new(u.clone(), None, span, vec![]).unwrap();
        let di = Import::Dynamic(DynamicImport::new(
            "file:///imp".into(),
            test_span(&arena, "'imp'"),
        ));
        let ir = ImportRule::new(vec![di], span);
        let ss = Stylesheet::new(
            vec![Statement::UseRule(ur), Statement::ImportRule(ir)],
            span,
        );
        let report = find_dependencies(&ss).unwrap();

        let all = report.all();
        assert!(all.contains(&u));
        assert!(all.contains(&SassUrl::parse("file:///imp").unwrap()));
    }

    #[test]
    fn test_find_dependencies_skip_bodies() {
        let arena = Bump::new();
        let span = test_span(&arena, "@function f() { @use 'inner'; @return 1; }");
        let params = ParameterList::empty(test_span(&arena, ""));
        let fr = FunctionRule::new("f".into(), params, vec![], span, None);
        let ss = Stylesheet::new(vec![Statement::FunctionRule(fr)], span);
        let report = find_dependencies(&ss).unwrap();

        assert!(report.uses.is_empty());
        assert!(report.forwards.is_empty());
        assert!(report.imports.is_empty());
    }
}
