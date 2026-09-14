// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/include_rule.dart
// go-source: go/value/sass_statement_include_rule.go

use crate::common::span_error::SpanError;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::sass::argument_list::ArgumentList;
use crate::ast::sass::statement::content_block::ContentBlock;

/// A mixin invocation.
#[derive(Clone, Debug)]
pub struct IncludeRule<'parse> {
    /// The mixin name as written, without underscore-to-hyphen conversion.
    pub original_name: String,
    /// The name of the mixin being invoked, with underscores converted to
    /// hyphens.
    pub name: String,
    /// The arguments to pass to the mixin.
    pub arguments: ArgumentList<'parse>,
    /// The namespace of the mixin being invoked, or `None` when it's
    /// invoked without a namespace.
    pub namespace: Option<String>,
    /// The block invoked for `@content` rules in the mixin being invoked,
    /// or `None` when this passes no content block.
    pub content: Option<Box<ContentBlock<'parse>>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> IncludeRule<'parse> {
    pub fn new(
        original_name: String,
        arguments: ArgumentList<'parse>,
        span: FileSpan<'parse>,
        namespace: Option<String>,
        content: Option<ContentBlock<'parse>>,
    ) -> Self {
        let name = original_name.replace('_', "-");
        IncludeRule {
            original_name,
            name,
            arguments,
            namespace,
            content: content.map(Box::new),
            span,
        }
    }

    /// The span covering the mixin name within the `@include` rule.
    ///
    /// Strips the `+`/at-rule prefix and any namespace, so for
    /// `@include ns.foo` this covers `foo`.
    pub fn name_span(&self) -> SassResult<FileSpan<'parse>> {
        // Dart `IncludeRule.nameSpan`: strip the `+`/at-rule prefix, then
        // strip the namespace (`ns.`), then take the identifier — so for
        // `@include ns.foo` the span covers `foo`, not `ns.foo`.
        let text = self.span.text();
        let start_span = if text.starts_with('+') {
            let start = 1;
            let end = text.len();
            let result = self.span.subspan(start, end).map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "subspan failed".into(),
                    argument_name: None,
                }),
            })?;
            result.trim_left().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "trim_left failed".into(),
                    argument_name: None,
                }),
            })?
        } else {
            self.span.without_initial_at_rule().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "without_initial_at_rule failed".into(),
                    argument_name: None,
                }),
            })?
        };
        let name_start = if self.namespace.is_some() {
            start_span.without_namespace().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "without_namespace failed".into(),
                    argument_name: None,
                }),
            })?
        } else {
            start_span
        };
        name_start.initial_identifier(0).map_err(Into::into)
    }

    /// Returns this include's span, without its content block (if it has one).
    /// Matches Dart: IncludeRule.spanWithoutContent.
    pub fn span_without_content(&self) -> SassResult<FileSpan<'parse>> {
        if self.content.is_none() {
            return Ok(self.span);
        }
        let end = self.arguments.span.end_location().offset;
        let len = end - self.span.start_location().offset;
        self.span
            .subspan(0, len)
            .map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "subspan failed".into(),
                    argument_name: None,
                }),
            })?
            .trim_right()
            .map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "trim_right failed".into(),
                    argument_name: None,
                }),
            })
    }

    /// The span covering the namespace within the `@include` rule, or
    /// `None` when the mixin is invoked without a namespace.
    pub fn namespace_span(&self) -> SassResult<Option<FileSpan<'parse>>> {
        if self.namespace.is_none() {
            return Ok(None);
        }
        // Dart `IncludeRule.namespaceSpan`: same prefix-stripping as
        // `nameSpan` (`+`-prefix with trimLeft, else withoutInitialAtRule),
        // then the identifier — so for `@include ns.foo` the span covers
        // `ns`, and for indented-Sass `+ns.foo` the `+` is excluded.
        let text = self.span.text();
        let start_span = if text.starts_with('+') {
            let result = self.span.subspan(1, text.len()).map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "subspan failed".into(),
                    argument_name: None,
                }),
            })?;
            result.trim_left().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "trim_left failed".into(),
                    argument_name: None,
                }),
            })?
        } else {
            self.span.without_initial_at_rule().map_err(|e| match e {
                SpanError::Sass(e) => e,
                _ => Box::new(SassError::Script {
                    message: "without_initial_at_rule failed".into(),
                    argument_name: None,
                }),
            })?
        };
        let ns = start_span
            .initial_identifier(0)
            .map_err(Into::<SassError>::into)?;
        Ok(Some(ns))
    }
}

impl<'parse> AstNode<'parse> for IncludeRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> IncludeRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@include ").unwrap();
        if let Some(ref ns) = self.namespace {
            write!(buf, "{ns}.").unwrap();
        }
        write!(buf, "{}", self.name).unwrap();
        if !self.arguments.is_empty() || self.arguments.rest.is_some() {
            write!(buf, "({})", self.arguments).unwrap();
        }
        match self.content {
            Some(ref content) => write!(buf, " {content}").unwrap(),
            None => write!(buf, ";").unwrap(),
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for IncludeRule<'parse> {
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
        let span = make_span(&arena, "@include foo;");
        let args = ArgumentList::empty(span);
        let ir = IncludeRule::new("foo".into(), args, span, None, None);
        assert_eq!(ir.name, "foo");
        assert_eq!(ir.original_name, "foo");
    }

    #[test]
    fn test_underscore_to_hyphen() {
        let arena = Bump::new();
        let span = make_span(&arena, "@include my_mixin;");
        let args = ArgumentList::empty(span);
        let ir = IncludeRule::new("my_mixin".into(), args, span, None, None);
        assert_eq!(ir.name, "my-mixin");
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@include foo;");
        let args = ArgumentList::empty(span);
        let ir = IncludeRule::new("foo".into(), args, span, None, None);
        let s = format!("{ir}");
        assert!(s.starts_with("@include foo"), "got {s:?}");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_include_name_span_strips_namespace() {
        // `@include ns.undefined-mixin` name_span must
        // cover `foo` (after `withoutNamespace`), not `ns.foo`.
        let arena = Bump::new();
        let span = make_span(&arena, "@include ns.foo;");
        let args = ArgumentList::empty(span);
        let ir = IncludeRule::new("foo".into(), args, span, Some("ns".into()), None);
        assert_eq!(ir.name_span().unwrap().text(), "foo");
        assert_eq!(
            ir.namespace_span().unwrap().unwrap().text(),
            "ns",
            "namespace_span must cover `ns`"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_include_namespace_span_sass_plus_prefix() {
        // Indented-Sass `+ns.foo` namespace_span must
        // exclude the `+` prefix (subspan(1) + trimLeft, like Dart).
        let arena = Bump::new();
        let span = make_span(&arena, "+ns.foo");
        let args = ArgumentList::empty(span);
        let ir = IncludeRule::new("foo".into(), args, span, Some("ns".into()), None);
        assert_eq!(ir.namespace_span().unwrap().unwrap().text(), "ns");
        assert_eq!(ir.name_span().unwrap().text(), "foo");
    }
}
