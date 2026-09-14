// Copyright 2025 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/interpolated_selector/simple.dart
// go-source: go/value/sass_interpolated_selector_simple.go

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::interpolated_selector::attribute::InterpolatedAttributeSelector;
use crate::ast::sass::interpolated_selector::class::InterpolatedClassSelector;
use crate::ast::sass::interpolated_selector::id::InterpolatedIDSelector;
use crate::ast::sass::interpolated_selector::parent::InterpolatedParentSelector;
use crate::ast::sass::interpolated_selector::placeholder::InterpolatedPlaceholderSelector;
use crate::ast::sass::interpolated_selector::pseudo::InterpolatedPseudoSelector;
use crate::ast::sass::interpolated_selector::ty::InterpolatedTypeSelector;
use crate::ast::sass::interpolated_selector::universal::InterpolatedUniversalSelector;
use crate::ast::sass::interpolated_selector::visitor::InterpolatedSelectorVisitor;

/// A simple selector still containing `#{}` interpolation at parse time.
///
/// Unlike the resolved simple selector, this is produced during the initial
/// stylesheet parse. Dispatches through the simple-selector visit methods.
// Variant sizes mirror the 8 Dart-mandated cases; boxing one case would
// distort AST fidelity for no observable change.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum InterpolatedSimpleSelector<'parse> {
    /// An attribute selector such as `[href]`.
    Attribute(InterpolatedAttributeSelector<'parse>),
    /// A class selector such as `.foo`.
    Class(InterpolatedClassSelector<'parse>),
    /// An ID selector such as `#bar`.
    ID(InterpolatedIDSelector<'parse>),
    /// A type selector such as `div`.
    Type(InterpolatedTypeSelector<'parse>),
    /// A universal selector (`*`, possibly namespaced).
    Universal(InterpolatedUniversalSelector<'parse>),
    /// A placeholder selector such as `%baz`.
    Placeholder(InterpolatedPlaceholderSelector<'parse>),
    /// A pseudo-class or pseudo-element selector.
    Pseudo(InterpolatedPseudoSelector<'parse>),
    /// A parent selector (`&`, possibly with suffix).
    Parent(InterpolatedParentSelector<'parse>),
}

impl<'parse> InterpolatedSimpleSelector<'parse> {
    /// Calls the appropriate visit method on `visitor`.
    pub fn accept<V: InterpolatedSelectorVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            InterpolatedSimpleSelector::Attribute(node) => visitor.visit_attribute_selector(node),
            InterpolatedSimpleSelector::Class(node) => visitor.visit_class_selector(node),
            InterpolatedSimpleSelector::ID(node) => visitor.visit_id_selector(node),
            InterpolatedSimpleSelector::Type(node) => visitor.visit_type_selector(node),
            InterpolatedSimpleSelector::Universal(node) => visitor.visit_universal_selector(node),
            InterpolatedSimpleSelector::Placeholder(node) => {
                visitor.visit_placeholder_selector(node)
            }
            InterpolatedSimpleSelector::Pseudo(node) => visitor.visit_pseudo_selector(node),
            InterpolatedSimpleSelector::Parent(node) => visitor.visit_parent_selector(node),
        }
    }

    /// Renders this selector as source text.
    pub fn to_display_string(&self) -> SassResult<String> {
        match self {
            InterpolatedSimpleSelector::Attribute(node) => node.to_display_string(),
            InterpolatedSimpleSelector::Class(node) => node.to_display_string(),
            InterpolatedSimpleSelector::ID(node) => node.to_display_string(),
            InterpolatedSimpleSelector::Type(node) => node.to_display_string(),
            InterpolatedSimpleSelector::Universal(node) => node.to_display_string(),
            InterpolatedSimpleSelector::Placeholder(node) => node.to_display_string(),
            InterpolatedSimpleSelector::Pseudo(node) => node.to_display_string(),
            InterpolatedSimpleSelector::Parent(node) => node.to_display_string(),
        }
    }
}

impl<'parse> AstNode<'parse> for InterpolatedSimpleSelector<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            InterpolatedSimpleSelector::Attribute(node) => node.span(),
            InterpolatedSimpleSelector::Class(node) => node.span(),
            InterpolatedSimpleSelector::ID(node) => node.span(),
            InterpolatedSimpleSelector::Type(node) => node.span(),
            InterpolatedSimpleSelector::Universal(node) => node.span(),
            InterpolatedSimpleSelector::Placeholder(node) => node.span(),
            InterpolatedSimpleSelector::Pseudo(node) => node.span(),
            InterpolatedSimpleSelector::Parent(node) => node.span(),
        }
    }
}

impl<'parse> fmt::Display for InterpolatedSimpleSelector<'parse> {
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
    use crate::ast::sass::interpolated_selector::compound::InterpolatedCompoundSelector;
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

    struct MockVisitor {
        visited: String,
    }

    impl<'parse> InterpolatedSelectorVisitor<'parse> for MockVisitor {
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
    fn test_accept_dispatch_class() {
        let arena = Bump::new();
        let span = make_span(&arena, "foo");
        let name = Interpolation::plain("foo".into(), span);
        let class = InterpolatedClassSelector::new(name);
        let sel = InterpolatedSimpleSelector::Class(class);
        let mut vis = MockVisitor {
            visited: String::new(),
        };
        let result = sel.accept(&mut vis).unwrap();
        assert_eq!(result, "Class");
    }

    #[test]
    fn test_accept_dispatch_id() {
        let arena = Bump::new();
        let span = make_span(&arena, "bar");
        let name = Interpolation::plain("bar".into(), span);
        let id = InterpolatedIDSelector::new(name);
        let sel = InterpolatedSimpleSelector::ID(id);
        let mut vis = MockVisitor {
            visited: String::new(),
        };
        let result = sel.accept(&mut vis).unwrap();
        assert_eq!(result, "ID");
    }
}
