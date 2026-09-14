// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/forward_rule.dart
// go-source: go/value/sass_statement_forward_rule.go

use std::collections::HashSet;
use std::fmt;
use std::fmt::Write;

use crate::url::SassUrl;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::configured_variable::ConfiguredVariable;

/// A `@forward` rule.
#[derive(Clone, Debug)]
pub struct ForwardRule<'parse> {
    /// The URL of the module to forward.
    ///
    /// Relative URLs resolve against the containing file.
    pub url: SassUrl,
    /// The mixin and function names accessible from the forwarded module.
    ///
    /// Empty means no mixins or functions may be accessed; `None` imposes
    /// no restriction. When non-`None`, the hidden sets are both `None`
    /// and [`ForwardRule::shown_variables`] is non-`None`.
    pub shown_mixins_and_functions: Option<HashSet<String>>,
    /// The variable names (without `$`) accessible from the forwarded
    /// module.
    ///
    /// Empty means no variables may be accessed; `None` imposes no
    /// restriction. When non-`None`, the hidden sets are both `None` and
    /// [`ForwardRule::shown_mixins_and_functions`] is non-`None`.
    pub shown_variables: Option<HashSet<String>>,
    /// Source-order `@forward show` lists (Dart `LinkedHashSet` iteration
    /// drives `LimitedMapView` key order, observed via
    /// `meta.module-variables`). `None` unless this is a show-rule.
    pub shown_order_mf: Option<Vec<String>>,
    pub shown_order_vars: Option<Vec<String>>,
    /// The mixin and function names that may not be accessed from the
    /// forwarded module.
    ///
    /// Empty means any mixins or functions may be accessed; `None` imposes
    /// no restriction. When non-`None`, the shown sets are both `None` and
    /// [`ForwardRule::hidden_variables`] is non-`None`.
    pub hidden_mixins_and_functions: Option<HashSet<String>>,
    /// The variable names (without `$`) that may not be accessed from the
    /// forwarded module.
    ///
    /// Empty means any variables may be accessed; `None` imposes no
    /// restriction. When non-`None`, the shown sets are both `None` and
    /// [`ForwardRule::hidden_mixins_and_functions`] is non-`None`.
    pub hidden_variables: Option<HashSet<String>>,
    /// The prefix added to the front of member names from the forwarded
    /// module, or `None` when member names are used as-is.
    pub prefix: Option<String>,
    /// The variable assignments used to configure the forwarded module.
    pub configuration: Vec<ConfiguredVariable<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ForwardRule<'parse> {
    /// Creates a `@forward` rule that allows all members to be accessed.
    pub fn new(
        url: SassUrl,
        span: FileSpan<'parse>,
        prefix: Option<String>,
        configuration: Vec<ConfiguredVariable<'parse>>,
    ) -> Self {
        ForwardRule {
            url,
            shown_mixins_and_functions: None,
            shown_variables: None,
            shown_order_mf: None,
            shown_order_vars: None,
            hidden_mixins_and_functions: None,
            hidden_variables: None,
            prefix,
            configuration,
            span,
        }
    }

    /// Creates a `@forward` rule that allows only the listed members to be
    /// accessed.
    pub fn show(
        url: SassUrl,
        shown_mixins_and_functions: HashSet<String>,
        shown_variables: HashSet<String>,
        span: FileSpan<'parse>,
        prefix: Option<String>,
        configuration: Vec<ConfiguredVariable<'parse>>,
    ) -> Self {
        Self::show_ordered(
            url,
            shown_mixins_and_functions,
            shown_variables,
            Vec::new(),
            Vec::new(),
            span,
            prefix,
            configuration,
        )
    }

    /// Ordered variant: `shown_order_*` carry the source-order member lists
    /// for `LimitedMapView` key order (Dart `LinkedHashSet` iteration).
    // Arity mirrors the ordered-show parameters; packing into a struct would
    // diverge from the sibling `show` constructor shape.
    #[allow(clippy::too_many_arguments)]
    pub fn show_ordered(
        url: SassUrl,
        shown_mixins_and_functions: HashSet<String>,
        shown_variables: HashSet<String>,
        shown_order_mf: Vec<String>,
        shown_order_vars: Vec<String>,
        span: FileSpan<'parse>,
        prefix: Option<String>,
        configuration: Vec<ConfiguredVariable<'parse>>,
    ) -> Self {
        ForwardRule {
            url,
            shown_mixins_and_functions: Some(shown_mixins_and_functions),
            shown_variables: Some(shown_variables),
            shown_order_mf: Some(shown_order_mf),
            shown_order_vars: Some(shown_order_vars),
            hidden_mixins_and_functions: None,
            hidden_variables: None,
            prefix,
            configuration,
            span,
        }
    }

    /// Creates a `@forward` rule that allows only members *not* in the
    /// given lists to be accessed.
    pub fn hide(
        url: SassUrl,
        hidden_mixins_and_functions: HashSet<String>,
        hidden_variables: HashSet<String>,
        span: FileSpan<'parse>,
        prefix: Option<String>,
        configuration: Vec<ConfiguredVariable<'parse>>,
    ) -> Self {
        ForwardRule {
            url,
            shown_mixins_and_functions: None,
            shown_variables: None,
            shown_order_mf: None,
            shown_order_vars: None,
            hidden_mixins_and_functions: Some(hidden_mixins_and_functions),
            hidden_variables: Some(hidden_variables),
            prefix,
            configuration,
            span,
        }
    }

    /// The span covering the quoted URL within the `@forward` rule.
    pub fn url_span(&self) -> SassResult<FileSpan<'parse>> {
        self.span
            .without_initial_at_rule()
            .and_then(|s| s.initial_quoted())
            .map_err(Into::into)
    }
}

impl<'parse> AstNode<'parse> for ForwardRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ForwardRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@forward \"{}\"", self.url).unwrap();
        if let Some(ref shown) = self.shown_mixins_and_functions {
            write!(
                buf,
                " show {}",
                member_list(shown, self.shown_variables.as_ref())
            )
            .unwrap();
        } else if let Some(ref hidden) = self.hidden_mixins_and_functions {
            if !hidden.is_empty() {
                write!(
                    buf,
                    " hide {}",
                    member_list(hidden, self.hidden_variables.as_ref())
                )
                .unwrap();
            }
        }
        if let Some(ref prefix) = self.prefix {
            write!(buf, " as {prefix}*").unwrap();
        }
        if !self.configuration.is_empty() {
            write!(buf, " with (").unwrap();
            let parts: Vec<String> = self.configuration.iter().map(|v| format!("{v}")).collect();
            write!(buf, "{}", parts.join(", ")).unwrap();
            write!(buf, ")").unwrap();
        }
        write!(buf, ";").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ForwardRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

fn member_list(
    mixins_and_functions: &HashSet<String>,
    variables: Option<&HashSet<String>>,
) -> String {
    let mut names: Vec<String> = mixins_and_functions.iter().cloned().collect();
    if let Some(vars) = variables {
        for name in vars {
            names.push(format!("${name}"));
        }
    }
    names.join(", ")
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
        let span = make_span(&arena, "@forward 'mod';");
        let url = SassUrl::parse("https://example.com/module").unwrap();
        let fr = ForwardRule::new(url.clone(), span, None, vec![]);
        assert_eq!(fr.url, url);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@forward 'mod';");
        let url = SassUrl::parse("https://example.com/module").unwrap();
        let fr = ForwardRule::new(url, span, None, vec![]);
        let s = format!("{fr}");
        assert!(s.starts_with("@forward "), "got {s:?}");
    }
}
