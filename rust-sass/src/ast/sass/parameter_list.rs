// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/parameter_list.dart
// go-source: go/value/sass_parameter_list.go

use crate::util::utils::to_sentence;
use std::collections::HashSet;
use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::{is_name, is_whitespace_byte, FileSpan};
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::util::utils::pluralize;

use crate::ast::sass::parameter::Parameter;

/// A parameter declaration, as for a function or mixin definition.
#[derive(Clone, Debug)]
pub struct ParameterList<'parse> {
    /// The parameters that are taken.
    pub parameters: Vec<Parameter<'parse>>,
    /// The name of the rest parameter (as in `$args...`), or `None` if none
    /// was declared.
    pub rest_parameter: Option<String>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ParameterList<'parse> {
    /// Creates a declaration with the given parameters and optional rest
    /// parameter.
    pub fn new(
        parameters: Vec<Parameter<'parse>>,
        span: FileSpan<'parse>,
        rest_parameter: Option<String>,
    ) -> Self {
        ParameterList {
            parameters,
            rest_parameter,
            span,
        }
    }

    /// Creates a declaration that declares no parameters.
    pub fn empty(span: FileSpan<'parse>) -> Self {
        ParameterList {
            parameters: Vec::new(),
            rest_parameter: None,
            span,
        }
    }

    /// Returns whether this declaration takes no parameters.
    ///
    /// Matches Dart: ParameterList.isEmpty
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty() && self.rest_parameter.is_none()
    }

    /// Returns `span` expanded to include an identifier immediately before
    /// the declaration, if possible.
    ///
    /// Falls back to `self.span` when the name is missing or invalid, or when
    /// the source file is unavailable. Trailing whitespace is trimmed (the
    /// span may be empty, e.g. a mixin declared without a parameter list).
    ///
    /// Matches Dart: ParameterList.spanWithName
    pub fn span_with_name(&self) -> FileSpan<'parse> {
        let file = match self.span.file() {
            Some(f) => f,
            None => return self.span,
        };
        let text = file.text();
        if text.is_empty() {
            return self.span;
        }
        let start_offset = self.span.start_location().offset;
        let end_offset = self.span.end_location().offset;
        let mut i = start_offset as isize - 1;
        while i >= 0 && is_whitespace_byte(text.as_bytes()[i as usize]) {
            i -= 1;
        }
        if i < 0 || !is_name(text.as_bytes()[i as usize]) {
            return self.span;
        }
        i -= 1;
        while i >= 0 && is_name(text.as_bytes()[i as usize]) {
            i -= 1;
        }
        let name_start = (i + 1) as usize;
        let ch = text.as_bytes()[name_start];
        if !(ch == b'_' || ch.is_ascii_alphabetic() || ch >= 0x80) {
            return self.span;
        }
        let mut end_offset = end_offset;
        while end_offset > name_start && is_whitespace_byte(text.as_bytes()[end_offset - 1]) {
            end_offset -= 1;
        }
        FileSpan::new(Some(file), name_start, end_offset)
    }

    /// Throws an error if `positional` and `names` aren't valid for this
    /// parameter declaration.
    ///
    /// Names in the messages are the normalized parameter names (Dart reports
    /// `_originalParameterName` source spellings here; the evaluator's inline
    /// copy in `eval/helpers.rs` does use `original_name` — split rule). The
    /// declaration side of each multi-span error is
    /// [`span_with_name`](Self::span_with_name) labeled `"declaration"`, with
    /// the invocation labeled `"invocation"`.
    ///
    /// Matches Dart: ParameterList.verify
    pub fn verify(
        &self,
        positional: usize,
        names: &HashSet<&str>,
        _invocation_span: &FileSpan<'parse>,
    ) -> SassResult<()> {
        let mut named_used = 0;
        for (i, param) in self.parameters.iter().enumerate() {
            if i < positional {
                if names.contains(param.name.as_str()) {
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "Argument ${} was passed both by position and by name.",
                            param.name
                        ),
                        argument_name: None,
                    }));
                }
            } else if names.contains(param.name.as_str()) {
                named_used += 1;
            } else if param.default_value.is_none() {
                // Script-level: the caller (`add_exception_span`) attaches the
                // invocation span + trace, matching Dart's `_verifyArguments`
                // throwing a `MultiSpanSassScriptException`.
                return Err(Box::new(SassError::MultiSpanScript {
                    message: format!("Missing argument ${}.", param.name),
                    primary_label: Some("invocation".into()),
                    secondary: vec![(
                        SourceSpanWithContext::from_file_span(&self.span_with_name())?,
                        "declaration".into(),
                    )],
                    cause: None,
                    loaded_urls: vec![],
                }));
            }
        }

        if self.rest_parameter.is_some() {
            return Ok(());
        }

        if positional > self.parameters.len() {
            let pos_word = if names.is_empty() { "" } else { "positional " };
            return Err(Box::new(SassError::MultiSpanScript {
                message: format!(
                    "Only {} {}{} allowed, but {} {} passed.",
                    self.parameters.len(),
                    pos_word,
                    pluralize("argument", self.parameters.len() as i32, None),
                    positional,
                    pluralize("was", positional as i32, Some("were")),
                ),
                primary_label: Some("invocation".into()),
                secondary: vec![(
                    SourceSpanWithContext::from_file_span(&self.span_with_name())?,
                    "declaration".into(),
                )],
                cause: None,
                loaded_urls: vec![],
            }));
        }

        if named_used < names.len() {
            let unknown_names: Vec<String> = names
                .iter()
                .filter(|n| !self.parameters.iter().any(|p| &p.name == *n))
                .map(|n| format!("${n}"))
                .collect();
            return Err(Box::new(SassError::MultiSpanScript {
                message: format!(
                    "No {} named {}.",
                    pluralize("parameter", unknown_names.len() as i32, None),
                    to_sentence(&unknown_names, "or")
                ),
                primary_label: Some("invocation".into()),
                secondary: vec![(
                    SourceSpanWithContext::from_file_span(&self.span_with_name())?,
                    "declaration".into(),
                )],
                cause: None,
                loaded_urls: vec![],
            }));
        }

        Ok(())
    }

    /// Returns whether `positional` and `names` are valid for this parameter
    /// declaration (the non-throwing twin of [`verify`](Self::verify)).
    ///
    /// Matches Dart: ParameterList.matches
    pub fn matches(&self, positional: usize, names: &HashSet<&str>) -> bool {
        let mut named_used = 0;
        for (i, param) in self.parameters.iter().enumerate() {
            if i < positional {
                if names.contains(param.name.as_str()) {
                    return false;
                }
            } else if names.contains(param.name.as_str()) {
                named_used += 1;
            } else if param.default_value.is_none() {
                return false;
            }
        }

        if self.rest_parameter.is_some() {
            return true;
        }
        if positional > self.parameters.len() {
            return false;
        }
        named_used >= names.len()
    }
}

impl<'parse> AstNode<'parse> for ParameterList<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> ParameterList<'parse> {
    /// Renders `$param, ..., $rest...`, without the surrounding parentheses.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        let mut first = true;
        for arg in &self.parameters {
            if first {
                first = false;
            } else {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "${}", arg).unwrap();
        }
        if let Some(ref rest) = self.rest_parameter {
            if !first {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "${rest}...").unwrap();
        }
        Ok(buf)
    }
}

impl<'parse> fmt::Display for ParameterList<'parse> {
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

    fn make_param<'compile, 'parse>(
        arena: &'compile Bump,
        name: &str,
        span_text: &str,
    ) -> Parameter<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let span = make_span(arena, span_text);
        Parameter::new(name.into(), span, None)
    }

    #[test]
    fn test_empty() {
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let pl = ParameterList::empty(span);
        assert!(pl.is_empty());
        assert!(pl.rest_parameter.is_none());
    }

    #[test]
    fn test_with_params() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let p = Parameter::new("name".into(), FileSpan::new(Some(fs), 1, 6), None);
        let pl = ParameterList::new(vec![p], span, None);
        assert!(!pl.is_empty());
        assert_eq!(pl.parameters.len(), 1);
    }

    #[test]
    fn test_rest_param() {
        let arena = Bump::new();
        let span = make_span(&arena, "($args...)");
        let pl = ParameterList::new(vec![], span, Some("args".into()));
        assert_eq!(pl.rest_parameter.as_deref(), Some("args"));
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name, $other, $rest...)", None);
        let span = FileSpan::new(Some(fs), 0, 27);
        let pl = ParameterList::new(
            vec![
                Parameter::new("name".into(), FileSpan::new(Some(fs), 1, 6), None),
                Parameter::new("other".into(), FileSpan::new(Some(fs), 8, 14), None),
            ],
            span,
            Some("rest".into()),
        );
        assert_eq!(format!("{pl}"), "$name, $other, $rest...");
    }

    #[test]
    fn test_verify_exact_match() {
        let arena = Bump::new();
        let span = make_span(&arena, "($name)");
        let p = make_param(&arena, "name", "$name");
        let pl = ParameterList::new(vec![p], span, None);
        assert!(pl.verify(1, &HashSet::new(), &span).is_ok());
    }

    #[test]
    fn test_verify_extra_positional() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let p = Parameter::new("name".into(), FileSpan::new(Some(fs), 1, 6), None);
        let pl = ParameterList::new(vec![p], span, None);
        assert!(pl.verify(2, &HashSet::new(), &span).is_err());
    }

    #[test]
    fn test_verify_missing_required() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let p = Parameter::new("name".into(), FileSpan::new(Some(fs), 1, 6), None);
        let pl = ParameterList::new(vec![p], span, None);
        assert!(pl.verify(0, &HashSet::new(), &span).is_err());
    }

    #[test]
    fn test_verify_named() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let p = Parameter::new("name".into(), FileSpan::new(Some(fs), 1, 6), None);
        let pl = ParameterList::new(vec![p], span, None);
        let mut names = HashSet::new();
        names.insert("name");
        assert!(pl.verify(0, &names, &span).is_ok());
    }

    #[test]
    fn test_matches_valid() {
        let arena = Bump::new();
        let span = make_span(&arena, "($name)");
        let p = make_param(&arena, "name", "$name");
        let pl = ParameterList::new(vec![p], span, None);
        assert!(pl.matches(1, &HashSet::new()));
        let mut names = HashSet::new();
        names.insert("name");
        assert!(pl.matches(0, &names));
    }

    #[test]
    fn test_matches_invalid() {
        let arena = Bump::new();
        let span = make_span(&arena, "($name)");
        let p = make_param(&arena, "name", "$name");
        let pl = ParameterList::new(vec![p], span, None);
        assert!(!pl.matches(2, &HashSet::new()));
        let mut names = HashSet::new();
        names.insert("unknown");
        assert!(!pl.matches(0, &names));
    }

    #[test]
    fn test_span_with_name() {
        let arena = Bump::new();
        let source = "@mixin foo($name)";
        let fs = FileSource::new_in(&arena, source, None);
        let pl_span = FileSpan::new(Some(fs), 10, source.len());
        let p = Parameter::new(
            "name".into(),
            FileSpan::new(Some(fs), 11, source.len() - 1),
            None,
        );
        let pl = ParameterList::new(vec![p], pl_span, None);
        let got = pl.span_with_name();
        assert!(!got.is_empty(), "span_with_name should not be empty");
        let got_text = got.text();
        assert!(
            got_text.contains("foo"),
            "span_with_name({got_text:?}) should contain 'foo'"
        );
    }

    #[test]
    fn test_span() {
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let pl = ParameterList::empty(span);
        assert_eq!(pl.span().unwrap(), span);
    }
}
