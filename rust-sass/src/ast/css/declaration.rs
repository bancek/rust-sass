// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/declaration.dart + lib/src/ast/css/modifiable/declaration.dart
// go-source: go/value/css_declaration.go + go/value/css_modifiable_declaration.go

use std::fmt;

use crate::common::ast_css_value::CssValue;
use crate::common::ast_node::AstNode;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::value::{Value, ValueKind};

/// A plain CSS declaration (that is, a `name: value` pair).
#[derive(Clone, Debug)]
pub struct CssDeclaration<'parse> {
    /// The name of this declaration.
    pub name: CssValue<'parse, String>,
    /// The value of this declaration.
    pub value: CssValue<'parse, Value<'parse>>,
    /// Whether this property's value was originally parsed as SassScript, as
    /// opposed to a custom property which is parsed as an interpolated
    /// sequence of tokens.
    ///
    /// If this is `false`, [`value`](CssDeclaration::value) holds an unquoted
    /// [`SassString`](crate::value::string::SassString).
    /// [`is_custom_property`](CssDeclaration::is_custom_property) is *usually*
    /// true in that case, but other properties may also skip SassScript
    /// parsing, like `return` in a plain CSS `@function`.
    pub parsed_as_sass_script: bool,
    /// The span for [`value`](CssDeclaration::value) to emit to the source map.
    ///
    /// When the declaration's expression is just a variable, this is the span
    /// where that variable was declared whereas the value span is where the
    /// variable was used. Otherwise it is identical to the value span.
    pub value_span_for_map: FileSpan<'parse>,
    /// The source span for this declaration.
    pub span: FileSpan<'parse>,
    /// Whether this node was the last in a nested Sass tree flattened during
    /// evaluation. See [`CssNode::is_group_end`](super::node::CssNode::is_group_end).
    pub is_group_end: bool,
}

impl<'parse> CssDeclaration<'parse> {
    /// Creates a declaration.
    ///
    /// When `value_span_for_map` is `None`, it defaults to the value's own
    /// span. Fails when `parsed_as_sass_script` is `false` but `value` does
    /// not hold a SassString.
    pub fn new(
        name: CssValue<'parse, String>,
        value: CssValue<'parse, Value<'parse>>,
        span: FileSpan<'parse>,
        parsed_as_sass_script: bool,
        value_span_for_map: Option<FileSpan<'parse>>,
    ) -> SassResult<Self> {
        if !parsed_as_sass_script && !matches!(&*value.value, ValueKind::String(_)) {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "If parsedAsSassScript is false, value must contain a SassString \
                         (was `{}` of type {:?}).",
                    &*value.value, value.value
                ),
                argument_name: None,
            }));
        }
        let vs = match value_span_for_map {
            Some(s) => s,
            // `CssValue::span` is infallible (stored, never computed) — the
            // `unwrap` here only bridges the `AstNode::span` Result shape.
            None => value.span().unwrap(),
        };
        Ok(CssDeclaration {
            name,
            value,
            parsed_as_sass_script,
            value_span_for_map: vs,
            span,
            is_group_end: false,
        })
    }

    /// Whether this is a CSS custom-property declaration (name starts with `--`).
    pub fn is_custom_property(&self) -> bool {
        self.name.value.starts_with("--")
    }
}

impl<'parse> AstNode<'parse> for CssDeclaration<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for CssDeclaration<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {};", self.name, &*self.value.value)
    }
}

// Frozen-class docs ported from `CssDeclaration` (declaration.dart). The
// modifiable counterpart (`ModifiableCssDeclaration` in
// modifiable/declaration.dart) implements the frozen interface for use during
// evaluation; see `docs/ref/ast.md` (CSS AST).
#[derive(Clone, Debug)]
pub struct ModifiableCssDeclaration<'parse> {
    /// The name of this declaration.
    pub name: CssValue<'parse, String>,
    /// The value of this declaration.
    pub value: CssValue<'parse, Value<'parse>>,
    /// Whether this property's value was originally parsed as SassScript.
    /// If `false`, [`value`](ModifiableCssDeclaration::value) holds an
    /// unquoted SassString.
    pub parsed_as_sass_script: bool,
    /// The span for [`value`](ModifiableCssDeclaration::value) to emit to the
    /// source map; defaults to the value's own span.
    pub value_span_for_map: FileSpan<'parse>,
    /// The source span for this declaration.
    pub span: FileSpan<'parse>,
}

impl<'parse> ModifiableCssDeclaration<'parse> {
    /// Creates a modifiable declaration, with the same SassString requirement
    /// and span fallback as [`CssDeclaration::new`].
    pub fn new(
        name: CssValue<'parse, String>,
        value: CssValue<'parse, Value<'parse>>,
        span: FileSpan<'parse>,
        parsed_as_sass_script: bool,
        value_span_for_map: Option<FileSpan<'parse>>,
    ) -> SassResult<Self> {
        if !parsed_as_sass_script && !matches!(&*value.value, ValueKind::String(_)) {
            return Err(Box::new(SassError::Script {
                message: format!(
                    "If parsedAsSassScript is false, value must contain a SassString \
                         (was `{}` of type {:?}).",
                    &*value.value, value.value
                ),
                argument_name: None,
            }));
        }
        let vs = match value_span_for_map {
            Some(s) => s,
            // `CssValue::span` is infallible (stored, never computed) — the
            // `unwrap` here only bridges the `AstNode::span` Result shape.
            None => value.span().unwrap(),
        };
        Ok(ModifiableCssDeclaration {
            name,
            value,
            parsed_as_sass_script,
            value_span_for_map: vs,
            span,
        })
    }

    /// Whether this is a CSS custom-property declaration (name starts with `--`).
    pub fn is_custom_property(&self) -> bool {
        self.name.value.starts_with("--")
    }
}

impl<'parse> AstNode<'parse> for ModifiableCssDeclaration<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> fmt::Display for ModifiableCssDeclaration<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {};", self.name, &*self.value.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::source_span_file_source::FileSource;
    use crate::value::number::SassNumber;
    use crate::value::string::SassString;
    use crate::value::Value;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    fn make_val<'compile, 'parse>(arena: &'compile Bump, s: &str) -> CssValue<'parse, String>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        CssValue::new(s.into(), make_span(arena, s))
    }

    #[test]
    fn test_declaration_construction() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_val(&arena, "color");
        let s = SassString::new("red", false);
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::String(s)),
            make_span(&arena, "red"),
        );
        let d = ModifiableCssDeclaration::new(name, val, span, true, None).unwrap();
        assert_eq!(d.name.value, "color");
        assert!(d.parsed_as_sass_script);
        assert!(!d.is_custom_property());
    }

    #[test]
    fn test_declaration_custom_property() {
        let arena = Bump::new();
        let span = make_span(&arena, "--x: 1;");
        let name = make_val(&arena, "--custom");
        let s = SassString::new("red", false);
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::String(s)),
            make_span(&arena, "red"),
        );
        let d = ModifiableCssDeclaration::new(name, val, span, true, None).unwrap();
        assert!(d.is_custom_property());
    }

    #[test]
    fn test_declaration_not_parsed_error() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: 42;");
        let name = make_val(&arena, "color");
        let num = SassNumber::new(42.0, None);
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::Number(num)),
            make_span(&arena, "42"),
        );
        let result = ModifiableCssDeclaration::new(name, val, span, false, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_declaration_display() {
        let arena = Bump::new();
        let span = make_span(&arena, "color: red;");
        let name = make_val(&arena, "color");
        let s = SassString::new("red", false);
        let val = CssValue::new(
            Value::new_with_arena(&arena, ValueKind::String(s)),
            make_span(&arena, "red"),
        );
        let d = CssDeclaration::new(name, val, span, true, None).unwrap();
        assert_eq!(format!("{d}"), "color: red;");
    }
}
