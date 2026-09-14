// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/use_rule.dart
// go-source: go/value/sass_statement_use_rule.go

use std::fmt;
use std::fmt::Write;

use crate::url::SassUrl;

use crate::common::ast_node::AstNode;
use crate::common::core_errors::ArgumentError;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;

use crate::ast::sass::configured_variable::ConfiguredVariable;

#[derive(Clone, Debug)]
/// A `@use` rule.
pub struct UseRule<'parse> {
    /// The URI of the module to load, relative to the containing file.
    pub url: SassUrl,
    /// The namespace for the loaded module's members, or `None` for
    /// namespace-less access.
    pub namespace: Option<String>,
    /// Variable assignments configuring the loaded module.
    pub configuration: Vec<ConfiguredVariable<'parse>>,
    pub span: FileSpan<'parse>,
}

impl<'parse> UseRule<'parse> {
    pub fn new(
        url: SassUrl,
        namespace: Option<String>,
        span: FileSpan<'parse>,
        configuration: Vec<ConfiguredVariable<'parse>>,
    ) -> SassResult<Self> {
        for var in &configuration {
            if var.is_guarded {
                return Err(Box::new(SassError::from(ArgumentError {
                    name: Some("configured variable".into()),
                    message: format!(
                        "Configured variable {:?} can't be guarded in a @use rule.",
                        var.name
                    ),
                })));
            }
        }
        Ok(UseRule {
            url,
            namespace,
            configuration,
            span,
        })
    }

    pub fn url_span(&self) -> SassResult<FileSpan<'parse>> {
        self.span
            .without_initial_at_rule()
            .and_then(|s| s.initial_quoted())
            .map_err(Into::into)
    }
}

impl<'parse> AstNode<'parse> for UseRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> UseRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@use \"{}\"", self.url).unwrap();
        let basename = url_basename(&self.url);
        let effective_ns = self.namespace.as_deref();
        if effective_ns.is_none() || effective_ns != Some(basename) {
            write!(buf, " as ").unwrap();
            if effective_ns.is_none() {
                write!(buf, "*").unwrap();
            } else if let Some(ns) = effective_ns {
                write!(buf, "{ns}").unwrap();
            }
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

impl<'parse> fmt::Display for UseRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

fn url_basename(url: &SassUrl) -> &str {
    let path = url.path();
    let path = match path.rfind('/') {
        Some(idx) => &path[idx + 1..],
        None => path,
    };
    match path.find('.') {
        Some(idx) => &path[..idx],
        None => path,
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
        let span = make_span(&arena, "@use 'mod';");
        let url = SassUrl::parse("https://example.com/module").unwrap();
        let ur = UseRule::new(url.clone(), None, span, vec![]).unwrap();
        assert_eq!(ur.url, url);
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "@use 'mod';");
        let url = SassUrl::parse("https://example.com/module").unwrap();
        let ur = UseRule::new(url, None, span, vec![]).unwrap();
        let s = format!("{ur}");
        assert!(s.starts_with("@use "), "got {s:?}");
    }
}
