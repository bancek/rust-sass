// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector.dart
// go-source: go/value/sass_interpolated_selector_interpolated_selector.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolated_selector::complex::InterpolatedComplexSelector;
use crate::ast::sass::interpolated_selector::compound::InterpolatedCompoundSelector;
use crate::ast::sass::interpolated_selector::list::InterpolatedSelectorList;
use crate::ast::sass::interpolated_selector::simple::InterpolatedSimpleSelector;
use crate::ast::sass::interpolated_selector::visitor::InterpolatedSelectorVisitor;

// Note: Dart keeps this a concrete class so the JS parser can expose its
// accept() function; Rust models the hierarchy as an enum instead.

/// A selector still containing `#{}` interpolation at parse time.
///
/// Unlike the resolved selector types, this is produced during the initial
/// stylesheet parse. The `Simple` case delegates [`accept`](Self::accept) to
/// the inner simple selector's own dispatch.
// Variant sizes mirror Dart's hierarchy; boxing the hot `Simple` case would
// add indirection to selector dispatch for no observable change.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum InterpolatedSelector<'parse> {
    /// A comma-separated selector list.
    List(InterpolatedSelectorList<'parse>),
    /// A complex selector (compounds joined by combinators).
    Complex(InterpolatedComplexSelector<'parse>),
    /// A compound selector (adjacent simple selectors).
    Compound(InterpolatedCompoundSelector<'parse>),
    /// A single simple selector.
    Simple(InterpolatedSimpleSelector<'parse>),
}

impl<'parse> InterpolatedSelector<'parse> {
    /// Calls the appropriate visit method on `visitor`.
    ///
    /// The `Simple` case delegates to the inner simple selector's dispatch.
    pub fn accept<V: InterpolatedSelectorVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            InterpolatedSelector::List(node) => visitor.visit_selector_list(node),
            InterpolatedSelector::Complex(node) => visitor.visit_complex_selector(node),
            InterpolatedSelector::Compound(node) => visitor.visit_compound_selector(node),
            InterpolatedSelector::Simple(node) => node.accept(visitor),
        }
    }

    /// Renders this selector as source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        match self {
            InterpolatedSelector::List(node) => node.to_display_string(),
            InterpolatedSelector::Complex(node) => node.to_display_string(),
            InterpolatedSelector::Compound(node) => node.to_display_string(),
            InterpolatedSelector::Simple(node) => node.to_display_string(),
        }
    }
}

impl<'parse> AstNode<'parse> for InterpolatedSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            InterpolatedSelector::List(node) => node.span(),
            InterpolatedSelector::Complex(node) => node.span(),
            InterpolatedSelector::Compound(node) => node.span(),
            InterpolatedSelector::Simple(node) => node.span(),
        }
    }
}

impl<'parse> fmt::Display for InterpolatedSelector<'parse> {
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
    use crate::ast::sass::interpolated_selector::attribute::InterpolatedAttributeSelector;
    use crate::ast::sass::interpolated_selector::class::InterpolatedClassSelector;
    use crate::ast::sass::interpolated_selector::complex::InterpolatedComplexSelector;
    use crate::ast::sass::interpolated_selector::id::InterpolatedIDSelector;
    use crate::ast::sass::interpolated_selector::list::InterpolatedSelectorList;
    use crate::ast::sass::interpolated_selector::parent::InterpolatedParentSelector;
    use crate::ast::sass::interpolated_selector::placeholder::InterpolatedPlaceholderSelector;
    use crate::ast::sass::interpolated_selector::pseudo::InterpolatedPseudoSelector;
    use crate::ast::sass::interpolated_selector::ty::InterpolatedTypeSelector;
    use crate::ast::sass::interpolated_selector::universal::InterpolatedUniversalSelector;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    fn make_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 1, text.len())
    }

    struct Mock {
        pub visited: String,
    }

    impl<'parse> InterpolatedSelectorVisitor<'parse> for Mock {
        type Output = String;

        fn visit_attribute_selector(
            &mut self,
            _: &InterpolatedAttributeSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Attribute".into();
            Ok("Attribute".into())
        }
        fn visit_class_selector(
            &mut self,
            _: &InterpolatedClassSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Class".into();
            Ok("Class".into())
        }
        fn visit_complex_selector(
            &mut self,
            _: &InterpolatedComplexSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Complex".into();
            Ok("Complex".into())
        }
        fn visit_compound_selector(
            &mut self,
            _: &InterpolatedCompoundSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Compound".into();
            Ok("Compound".into())
        }
        fn visit_id_selector(&mut self, _: &InterpolatedIDSelector<'parse>) -> SassResult<String> {
            self.visited = "ID".into();
            Ok("ID".into())
        }
        fn visit_parent_selector(
            &mut self,
            _: &InterpolatedParentSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Parent".into();
            Ok("Parent".into())
        }
        fn visit_placeholder_selector(
            &mut self,
            _: &InterpolatedPlaceholderSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Placeholder".into();
            Ok("Placeholder".into())
        }
        fn visit_pseudo_selector(
            &mut self,
            _: &InterpolatedPseudoSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Pseudo".into();
            Ok("Pseudo".into())
        }
        fn visit_selector_list(
            &mut self,
            _: &InterpolatedSelectorList<'parse>,
        ) -> SassResult<String> {
            self.visited = "SelectorList".into();
            Ok("SelectorList".into())
        }
        fn visit_type_selector(
            &mut self,
            _: &InterpolatedTypeSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Type".into();
            Ok("Type".into())
        }
        fn visit_universal_selector(
            &mut self,
            _: &InterpolatedUniversalSelector<'parse>,
        ) -> SassResult<String> {
            self.visited = "Universal".into();
            Ok("Universal".into())
        }
    }

    #[test]
    fn test_accept_dispatch_compound() {
        let arena = Bump::new();
        let span = make_span(&arena, "foo");
        let name = Interpolation::plain("foo".into(), span);
        let class = InterpolatedClassSelector::new(name);
        let simp = InterpolatedSimpleSelector::Class(class);
        let compound = InterpolatedCompoundSelector::new(vec![simp]).unwrap();
        let sel = InterpolatedSelector::Compound(compound);
        let mut vis = Mock {
            visited: String::new(),
        };
        let result = sel.accept(&mut vis).unwrap();
        assert_eq!(result, "Compound");
    }
}
