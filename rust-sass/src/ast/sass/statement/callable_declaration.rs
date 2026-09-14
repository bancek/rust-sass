// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/callable_declaration.dart
// go-source: go/value/sass_statement_callable_declaration.go

use crate::common::file_span::FileSpan;

use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::content_block::ContentBlock;
use crate::ast::sass::statement::function_rule::FunctionRule;
use crate::ast::sass::statement::mixin_rule::MixinRule;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::Statement;

/// A callable (function or mixin) declared in user code.
///
/// Dart models this as an abstract base class implemented by `MixinRule`,
/// `FunctionRule`, and `ContentBlock`; Go embeds a shared
/// `*CallableDeclaration` base in the three rule structs. The Rust AST keeps
/// the flattened per-type structs (port convention: abstract class → enum),
/// so this enum wraps them and provides the shared accessors.
#[derive(Clone, Debug)]
pub enum CallableDeclaration<'parse> {
    Mixin(MixinRule<'parse>),
    Function(FunctionRule<'parse>),
    ContentBlock(ContentBlock<'parse>),
}

impl<'parse> CallableDeclaration<'parse> {
    /// The name of this callable, with underscores converted to hyphens.
    /// A content block's name is "@content" (Dart: `super("@content", ...)`).
    pub fn name(&self) -> &str {
        match self {
            CallableDeclaration::Mixin(m) => &m.name,
            CallableDeclaration::Function(f) => &f.name,
            CallableDeclaration::ContentBlock(_) => "@content",
        }
    }

    /// The callable's original name, without underscores converted to hyphens.
    pub fn original_name(&self) -> &str {
        match self {
            CallableDeclaration::Mixin(m) => &m.original_name,
            CallableDeclaration::Function(f) => &f.original_name,
            CallableDeclaration::ContentBlock(_) => "@content",
        }
    }

    /// The declared parameters this callable accepts.
    pub fn parameters(&self) -> &ParameterList<'parse> {
        match self {
            CallableDeclaration::Mixin(m) => &m.parameters,
            CallableDeclaration::Function(f) => &f.parameters,
            CallableDeclaration::ContentBlock(c) => &c.parameters,
        }
    }

    pub fn children(&self) -> &[Statement<'parse>] {
        match self {
            CallableDeclaration::Mixin(m) => &m.children,
            CallableDeclaration::Function(f) => &f.children,
            CallableDeclaration::ContentBlock(c) => &c.children,
        }
    }

    pub fn span(&self) -> FileSpan<'parse> {
        match self {
            CallableDeclaration::Mixin(m) => m.span,
            CallableDeclaration::Function(f) => f.span,
            CallableDeclaration::ContentBlock(c) => c.span,
        }
    }

    /// The comment immediately preceding this declaration.
    /// Content blocks never have one (Dart passes no comment to super).
    pub fn comment(&self) -> Option<&SilentComment<'parse>> {
        match self {
            CallableDeclaration::Mixin(m) => m.comment.as_deref(),
            CallableDeclaration::Function(f) => f.comment.as_deref(),
            CallableDeclaration::ContentBlock(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::parameter_list::ParameterList;
    use crate::common::file_span::BOGUS_SPAN;

    fn params() -> ParameterList<'static> {
        ParameterList::empty(BOGUS_SPAN)
    }

    #[test]
    fn test_mixin_declaration_name_normalization() {
        // Mirrors Go: TestMixinRuleNameNormalization.
        let mr = MixinRule::new("foo_bar".into(), params(), vec![], BOGUS_SPAN, None);
        let decl = CallableDeclaration::Mixin(mr);
        assert_eq!(decl.name(), "foo-bar");
        assert_eq!(decl.original_name(), "foo_bar");
    }

    #[test]
    fn test_function_declaration_name() {
        let fr = FunctionRule::new("f_n".into(), params(), vec![], BOGUS_SPAN, None);
        let decl = CallableDeclaration::Function(fr);
        assert_eq!(decl.name(), "f-n");
        assert_eq!(decl.original_name(), "f_n");
    }

    #[test]
    fn test_content_block_declaration_name() {
        // Mirrors Go: TestContentBlockName — Dart passes "@content" as the name.
        let cb = ContentBlock::new(params(), vec![], BOGUS_SPAN);
        let decl = CallableDeclaration::ContentBlock(cb);
        assert_eq!(decl.name(), "@content");
        assert_eq!(decl.original_name(), "@content");
    }

    #[test]
    fn test_declaration_accessors() {
        let mr = MixinRule::new("m".into(), params(), vec![], BOGUS_SPAN, None);
        let decl = CallableDeclaration::Mixin(mr);
        assert!(decl.children().is_empty());
        assert!(decl.parameters().is_empty());
        assert!(decl.comment().is_none());
        assert_eq!(decl.span(), BOGUS_SPAN);
    }
}
