// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/mixin_rule.dart
// go-source: go/value/sass_statement_mixin_rule.go

use crate::ast::sass::visitor::statement_search::StatementSearchVisitor;
use crate::common::span_error::SpanError;
use std::cell::Cell;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::sass::parameter_list::ParameterList;
use crate::ast::sass::statement::silent_comment::SilentComment;
use crate::ast::sass::statement::Statement;

#[derive(Clone, Debug)]
/// A mixin declaration.
///
/// This declares a mixin that call sites invoke with `@include`.
pub struct MixinRule<'parse> {
    /// The mixin name, with underscores converted to hyphens.
    pub name: String,
    /// The mixin name as written, without underscore-to-hyphen conversion.
    pub original_name: String,
    /// The parameters the mixin accepts.
    pub parameters: ParameterList<'parse>,
    pub children: Vec<Statement<'parse>>,
    pub span: FileSpan<'parse>,
    /// The comment immediately preceding this declaration, if any.
    pub comment: Option<Box<SilentComment<'parse>>>,
    pub namespace: Option<String>,
    /// Lazily computed cache for `has_content()`.
    /// Matches Dart: `late final bool hasContent` / Go: `hasContent` +
    /// `hasContentComputed` on CallableDeclaration.
    has_content: Cell<Option<bool>>,
}

impl<'parse> MixinRule<'parse> {
    pub fn new(
        original_name: String,
        parameters: ParameterList<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
        comment: Option<SilentComment<'parse>>,
    ) -> Self {
        let name = original_name.replace('_', "-");
        MixinRule {
            name,
            original_name,
            parameters,
            children,
            span,
            comment: comment.map(Box::new),
            namespace: None,
            has_content: Cell::new(None),
        }
    }

    /// Whether the mixin contains a `@content` rule.
    ///
    /// Matches Dart: `late final bool hasContent =
    /// const _HasContentVisitor().visitMixinRule(this) == true` — the search
    /// recurses through child statements via the StatementSearchVisitor.
    /// (Go: `CallableDeclaration.HasContent` via `hasContentRule`.)
    pub fn has_content(&self) -> bool {
        if let Some(cached) = self.has_content.get() {
            return cached;
        }
        let mut visitor = StatementSearchVisitor::new();
        visitor.content_rule_func = Some(Box::new(|_| Ok(true)));
        let mut result = false;
        for child in &self.children {
            if child.accept(&mut visitor).unwrap_or(false) {
                result = true;
                break;
            }
        }
        self.has_content.set(Some(result));
        result
    }

    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        // Dart `MixinRule.nameSpan`: `=`-prefix is subspanned then
        // **trimLeft** before the identifier — so indented `= foo` names
        // start at `foo`, not at the whitespace.
        let text = self.span.text();
        if text.starts_with('=') {
            let start = 1;
            let end = text.len();
            let result = self.span.subspan(start, end).map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "subspan failed".into(),
                    argument_name: None,
                }),
            })?;
            let trimmed = result.trim_left().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "trim_left failed".into(),
                    argument_name: None,
                }),
            })?;
            return trimmed.initial_identifier(0).map_err(Into::into);
        }
        self.span
            .without_initial_at_rule()
            .and_then(|s| s.initial_identifier(0))
            .map_err(Into::into)
    }
}

impl<'parse> AstNode<'parse> for MixinRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> MixinRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@mixin {}", self.name).unwrap();
        if !self.parameters.is_empty() {
            write!(buf, "({})", self.parameters).unwrap();
        }
        write!(buf, " {{").unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for MixinRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::parameter::Parameter;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_new() {
        let arena = Bump::new();
        let span = make_span(&arena, "@mixin foo { }");
        let params = ParameterList::empty(span);
        let mr = MixinRule::new("foo".into(), params, vec![], span, None);
        assert_eq!(mr.name, "foo");
    }

    #[test]
    fn test_display_without_params() {
        let arena = Bump::new();
        let span = make_span(&arena, "@mixin foo { }");
        let params = ParameterList::empty(span);
        let mr = MixinRule::new("foo".into(), params, vec![], span, None);
        let s = format!("{mr}");
        assert!(s.contains("@mixin"), "got {s:?}");
    }

    #[test]
    fn test_display_with_params() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "@mixin foo($x) { }", None);
        let span = FileSpan::new(Some(fs), 0, 18);
        let p = Parameter::new("x".into(), FileSpan::new(Some(fs), 12, 14), None);
        let params = ParameterList::new(vec![p], span, None);
        let mr = MixinRule::new("foo".into(), params, vec![], span, None);
        let s = format!("{mr}");
        assert!(s.contains("($x)"), "got {s:?}");
    }

    // --- has_content (mirrors Go: TestHasContent*) ---

    use crate::ast::sass::argument_list::ArgumentList;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::statement::if_rule::{IfClause, IfRule};
    use crate::ast::sass::statement::ContentRule;

    fn content_rule<'compile, 'parse>(arena: &'compile Bump) -> Statement<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, "@content;");
        Statement::ContentRule(ContentRule::new(ArgumentList::empty(span), span))
    }

    #[test]
    fn test_has_content_false() {
        let arena = Bump::new();
        let span = make_span(&arena, "@mixin m { }");
        let mr = MixinRule::new("m".into(), ParameterList::empty(span), vec![], span, None);
        assert!(!mr.has_content());
    }

    #[test]
    fn test_has_content_direct_child() {
        let arena = Bump::new();
        let span = make_span(&arena, "@mixin m { @content; }");
        let mr = MixinRule::new(
            "m".into(),
            ParameterList::empty(span),
            vec![content_rule(&arena)],
            span,
            None,
        );
        assert!(mr.has_content());
    }

    #[test]
    fn test_has_content_nested() {
        // The @content search recurses through child statements
        // (Dart: _HasContentVisitor via StatementSearchVisitor).
        let arena = Bump::new();
        let span = make_span(&arena, "@mixin m { @if true { @content; } }");
        let cond = Expression::Boolean(BooleanExpression::new(true, make_span(&arena, "true")));
        let clause = IfClause::new(cond, vec![content_rule(&arena)]);
        let if_rule = Statement::IfRule(IfRule::new(
            vec![clause],
            make_span(&arena, "@if true { @content; }"),
            None,
        ));
        let mr = MixinRule::new(
            "m".into(),
            ParameterList::empty(span),
            vec![if_rule],
            span,
            None,
        );
        assert!(mr.has_content());
    }

    #[test]
    fn test_has_content_lazy_cached() {
        let arena = Bump::new();
        let span = make_span(&arena, "@mixin m { @content; }");
        let mr = MixinRule::new(
            "m".into(),
            ParameterList::empty(span),
            vec![content_rule(&arena)],
            span,
            None,
        );
        assert!(mr.has_content());
        assert!(
            mr.has_content(),
            "second call should return the cached value"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_mixin_name_span_trims_after_equals() {
        // Dart `MixinRule.nameSpan` applies `trimLeft`
        // after the `=` subspan — indented `= foo` names start at `foo`.
        let arena = Bump::new();
        let span = make_span(&arena, "= foo");
        let mr = MixinRule::new("foo".into(), ParameterList::empty(span), vec![], span, None);
        assert_eq!(mr.name_span().unwrap().text(), "foo");
    }
}
