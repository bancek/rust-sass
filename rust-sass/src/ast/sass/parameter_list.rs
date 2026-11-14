// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/parameter_list.dart
// go-source: go/value/sass_parameter_list.go

use crate::util::css_identifier::to_css_identifier;
use crate::util::utils::to_sentence;
use indexmap::IndexMap;
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
    /// The parameters that are taken. Allocated in the compile arena.
    pub parameters: &'parse [Parameter<'parse>],
    /// The name of the rest parameter (as in `$args...`), or `None` if none
    /// was declared.
    pub rest_parameter: Option<&'parse str>,
    pub span: FileSpan<'parse>,
}

impl<'parse> ParameterList<'parse> {
    /// Creates a declaration with the given parameters and optional rest
    /// parameter.
    pub fn new(
        parameters: &'parse [Parameter<'parse>],
        span: FileSpan<'parse>,
        rest_parameter: Option<&'parse str>,
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
            parameters: &[],
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
    /// `names` is the evaluated named-argument map, used as an ordered set —
    /// Dart builds `MapKeySet(evaluated.named)` and passes it to `matches`,
    /// `callbackFor`, and `verify`. The unknown-name list therefore follows
    /// the order the arguments were passed.
    ///
    /// Both messages name parameters by
    /// [`original_parameter_name`](Self::original_parameter_name), as Dart does
    /// (`parameter_list.dart:90, :98`).
    ///
    /// Matches Dart: ParameterList.verify
    pub fn verify<V>(&self, positional: usize, names: &IndexMap<String, V>) -> SassResult<()> {
        let mut named_used = 0;
        for (i, param) in self.parameters.iter().enumerate() {
            // Names are matched in canonical form (sass/sass#4268); Dart calls
            // `parameter.name.toCssIdentifier()` unconditionally at :88, :94
            // and :122, so a name that can't be canonicalized propagates the
            // error rather than falling back to the raw text.
            let name_ident = to_css_identifier(param.name)?;
            if i < positional {
                if names.contains_key(&name_ident) {
                    return Err(Box::new(SassError::Script {
                        message: format!(
                            "Argument {} was passed both by position and by name.",
                            self.original_parameter_name(param.name)?
                        ),
                        argument_name: None,
                    }));
                }
            } else if names.contains_key(&name_ident) {
                named_used += 1;
            } else if param.default_value.is_none() {
                // Script-level: the caller (`add_exception_span`) attaches the
                // invocation span + trace, matching Dart's `_verifyArguments`
                // throwing a `MultiSpanSassScriptException`.
                return Err(Box::new(SassError::MultiSpanScript {
                    message: format!(
                        "Missing argument {}.",
                        self.original_parameter_name(param.name)?
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
            // Dart: `Set.of(names)..removeAll(parameters.map((p) =>
            // p.name.toCssIdentifier()))` — insertion order is preserved.
            let param_idents: Vec<String> = self
                .parameters
                .iter()
                .map(|p| to_css_identifier(p.name))
                .collect::<SassResult<Vec<String>>>()?;
            let unknown_names: Vec<String> = names
                .keys()
                .filter(|n| !param_idents.iter().any(|p| p == *n))
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

    /// Returns the parameter named `name` with a leading `$` and its original
    /// underscores (which are otherwise converted to hyphens).
    ///
    /// Matches Dart: ParameterList._originalParameterName
    fn original_parameter_name(&self, name: &str) -> SassResult<String> {
        if self.rest_parameter == Some(name) {
            let text = self.span.text();
            // Dart: `text.substring(text.lastIndexOf("\$"))` then
            // `fromDollar.substring(0, text.indexOf("."))`. Unreachable from
            // `verify` (the rest parameter is not in `parameters`); Dart would
            // throw on a span without `$`/`.`, which the crate forbids, so the
            // slicing is clamped here instead.
            let from_dollar = &text[text.rfind('$').unwrap_or(0)..];
            let end = text.find('.').unwrap_or(from_dollar.len());
            return Ok(from_dollar[..end].to_string());
        }

        for parameter in self.parameters {
            if parameter.name == name {
                return Ok(parameter.original_name());
            }
        }

        Err(Box::new(SassError::Script {
            message: format!("This declaration has no parameter named \"${name}\"."),
            argument_name: None,
        }))
    }

    /// Returns whether `positional` and `names` are valid for this parameter
    /// declaration (the non-throwing twin of [`verify`](Self::verify)).
    ///
    /// Matches Dart: ParameterList.matches
    pub fn matches<V>(&self, positional: usize, names: &IndexMap<String, V>) -> bool {
        let mut named_used = 0;
        for (i, param) in self.parameters.iter().enumerate() {
            // Dart compares the raw name here (`matches`, :155,:157), unlike
            // `verify`'s canonical `toCssIdentifier` comparison.
            if i < positional {
                if names.contains_key(param.name) {
                    return false;
                }
            } else if names.contains_key(param.name) {
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
        for arg in self.parameters {
            if first {
                first = false;
            } else {
                write!(buf, ", ").unwrap();
            }
            write!(buf, "${}", arg).unwrap();
        }
        if let Some(rest) = self.rest_parameter {
            if !first {
                write!(buf, ", ").unwrap();
            }
            // Dart: `'\$${restParameter.toCssIdentifier()}...'`.
            write!(buf, "${}...", to_css_identifier(rest)?).unwrap();
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
        Parameter::new(arena.alloc_str(name), span, None)
    }

    fn params_list<'compile: 'parse, 'parse>(
        arena: &'compile Bump,
        parameters: Vec<Parameter<'parse>>,
        span: FileSpan<'parse>,
        rest_parameter: Option<&'parse str>,
    ) -> ParameterList<'parse> {
        ParameterList::new(
            arena.alloc_slice_fill_iter(parameters),
            span,
            rest_parameter,
        )
    }

    fn named(items: &[&str]) -> IndexMap<String, ()> {
        items.iter().map(|item| ((*item).to_string(), ())).collect()
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
        let p = Parameter::new(arena.alloc_str("name"), FileSpan::new(Some(fs), 1, 6), None);
        let pl = params_list(&arena, vec![p], span, None);
        assert!(!pl.is_empty());
        assert_eq!(pl.parameters.len(), 1);
    }

    #[test]
    fn test_rest_param() {
        let arena = Bump::new();
        let span = make_span(&arena, "($args...)");
        let pl = params_list(&arena, vec![], span, Some(arena.alloc_str("args")));
        assert_eq!(pl.rest_parameter, Some("args"));
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name, $other, $rest...)", None);
        let span = FileSpan::new(Some(fs), 0, 27);
        let pl = params_list(
            &arena,
            vec![
                Parameter::new(arena.alloc_str("name"), FileSpan::new(Some(fs), 1, 6), None),
                Parameter::new(
                    arena.alloc_str("other"),
                    FileSpan::new(Some(fs), 8, 14),
                    None,
                ),
            ],
            span,
            Some(arena.alloc_str("rest")),
        );
        assert_eq!(format!("{pl}"), "$name, $other, $rest...");
    }

    #[test]
    fn test_display_escapes_rest_parameter() {
        // Dart's `toString` uses `restParameter.toCssIdentifier()`.
        let arena = Bump::new();
        let span = make_span(&arena, "($$)");
        let pl = params_list(&arena, vec![], span, Some(arena.alloc_str("$")));
        assert_eq!(format!("{pl}"), "$\\$...");
    }

    #[test]
    fn test_verify_exact_match() {
        let arena = Bump::new();
        let span = make_span(&arena, "($name)");
        let p = make_param(&arena, "name", "$name");
        let pl = params_list(&arena, vec![p], span, None);
        assert!(pl.verify(1, &named(&[])).is_ok());
    }

    #[test]
    fn test_verify_extra_positional() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let p = Parameter::new(arena.alloc_str("name"), FileSpan::new(Some(fs), 1, 6), None);
        let pl = params_list(&arena, vec![p], span, None);
        assert!(pl.verify(2, &named(&[])).is_err());
    }

    #[test]
    fn test_verify_missing_required() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let p = Parameter::new(arena.alloc_str("name"), FileSpan::new(Some(fs), 1, 6), None);
        let pl = params_list(&arena, vec![p], span, None);
        assert!(pl.verify(0, &named(&[])).is_err());
    }

    #[test]
    fn test_verify_named() {
        let arena = Bump::new();
        let fs = FileSource::new_in(&arena, "($name)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let p = Parameter::new(arena.alloc_str("name"), FileSpan::new(Some(fs), 1, 6), None);
        let pl = params_list(&arena, vec![p], span, None);
        assert!(pl.verify(0, &named(&["name"])).is_ok());
    }

    // Dart `ParameterList.verify` reports `_originalParameterName` in both
    // messages (parameter_list.dart:90, :98), so a parameter written with an
    // underscore keeps its source spelling even though binding uses the
    // normalized name.
    #[test]
    fn test_verify_missing_argument_uses_original_name() {
        let arena = Bump::new();
        let p = make_param(&arena, "foo-bar", "$foo_bar");
        let span = make_span(&arena, "($foo_bar)");
        let pl = params_list(&arena, vec![p], span, None);
        let err = pl.verify(0, &named(&[])).unwrap_err();
        assert_eq!(err.message(), "Missing argument $foo_bar.");
    }

    #[test]
    fn test_verify_both_by_position_uses_original_name() {
        let arena = Bump::new();
        let p = make_param(&arena, "foo-bar", "$foo_bar");
        let span = make_span(&arena, "($foo_bar)");
        let pl = params_list(&arena, vec![p], span, None);
        let err = pl.verify(1, &named(&["foo-bar"])).unwrap_err();
        assert_eq!(
            err.message(),
            "Argument $foo_bar was passed both by position and by name."
        );
    }

    // Named arguments are stored canonically (sass/sass#4268), so a parameter
    // named `$` is matched through its canonical spelling `\$`.
    #[test]
    fn test_verify_matches_canonical_escaped_name() {
        let arena = Bump::new();
        let p = make_param(&arena, "$", "$\\$");
        let span = make_span(&arena, "($\\$)");
        let pl = params_list(&arena, vec![p], span, None);
        assert!(pl.verify(0, &named(&["\\$"])).is_ok());
    }

    #[test]
    fn test_verify_missing_argument_uses_escaped_source_spelling() {
        let arena = Bump::new();
        let p = make_param(&arena, "$", "$\\$");
        let span = make_span(&arena, "($\\$)");
        let pl = params_list(&arena, vec![p], span, None);
        let err = pl.verify(0, &named(&[])).unwrap_err();
        assert_eq!(err.message(), "Missing argument $\\$.");
    }

    // Dart calls `parameter.name.toCssIdentifier()` unconditionally
    // (parameter_list.dart:88, :94, :122); `toCssIdentifier("")` throws rather
    // than falling back to the raw name, so a bogus parameter can never match.
    #[test]
    fn test_verify_does_not_fall_back_to_raw_name() {
        let arena = Bump::new();
        let p = make_param(&arena, "", "$");
        let span = make_span(&arena, "($)");
        let pl = params_list(&arena, vec![p], span, None);
        let err = pl.verify(0, &named(&[""])).unwrap_err();
        assert_eq!(
            err.message(),
            "The empty string can't be represented as a CSS identifier."
        );
    }

    #[test]
    fn test_verify_unknown_named() {
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let pl = params_list(&arena, vec![], span, None);
        let err = pl.verify(0, &named(&["z"])).unwrap_err();
        assert_eq!(err.message(), "No parameter named $z.");
    }

    #[test]
    fn test_verify_lists_unknown_named_in_insertion_order() {
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let pl = params_list(&arena, vec![], span, None);
        let err = pl.verify(0, &named(&["z", "y"])).unwrap_err();
        assert_eq!(err.message(), "No parameters named $z or $y.");
    }

    #[test]
    fn test_verify_lists_three_unknown_named_without_oxford_comma() {
        // Dart's `toSentence` is `a, b or c` (no comma before the conjunction).
        let arena = Bump::new();
        let span = make_span(&arena, "()");
        let pl = params_list(&arena, vec![], span, None);
        let err = pl.verify(0, &named(&["z", "y", "x"])).unwrap_err();
        assert_eq!(err.message(), "No parameters named $z, $y or $x.");
    }

    #[test]
    fn test_verify_rest_accepts_all() {
        let arena = Bump::new();
        let span = make_span(&arena, "($x...)");
        let p = make_param(&arena, "x", "$x");
        let pl = params_list(&arena, vec![p], span, Some(arena.alloc_str("rest")));
        assert!(pl.verify(10, &named(&[])).is_ok());
    }

    #[test]
    fn test_matches_valid() {
        let arena = Bump::new();
        let span = make_span(&arena, "($name)");
        let p = make_param(&arena, "name", "$name");
        let pl = params_list(&arena, vec![p], span, None);
        assert!(pl.matches(1, &named(&[])));
        assert!(pl.matches(0, &named(&["name"])));
    }

    #[test]
    fn test_matches_invalid() {
        let arena = Bump::new();
        let span = make_span(&arena, "($name)");
        let p = make_param(&arena, "name", "$name");
        let pl = params_list(&arena, vec![p], span, None);
        assert!(!pl.matches(2, &named(&[])));
        assert!(!pl.matches(0, &named(&["unknown"])));
    }

    #[test]
    fn test_span_with_name() {
        let arena = Bump::new();
        let source = "@mixin foo($name)";
        let fs = FileSource::new_in(&arena, source, None);
        let pl_span = FileSpan::new(Some(fs), 10, source.len());
        let p = Parameter::new("name", FileSpan::new(Some(fs), 11, source.len() - 1), None);
        let pl = params_list(&arena, vec![p], pl_span, None);
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
