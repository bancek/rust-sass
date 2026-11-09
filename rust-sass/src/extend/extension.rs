// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/extension.dart
// go-source: go/extend/extension.go

use crate::common::source_span_span_with_context::SourceSpanWithContext;
use bumpalo::Bump;
use std::fmt;
use std::fmt::Write;
use std::hash::{Hash, Hasher};

use crate::ast::css::media_query::CssMediaQuery;
use crate::common::exception::{SassError, SassResult, Trace};
use crate::common::file_span::FileSpan;
use crate::selector::complex::ComplexSelector;
use crate::selector::SimpleSelector;

/// A selector that's extending another selector, such as `A` in `A {@extend B}`.
///
/// An extender either represents a selector that was originally in the
/// document, or one created by an `@extend` rule (tracked through
/// [`Extender::extension_media_context`] and [`Extender::extension_span`]).
#[derive(Clone, Debug)]
pub struct Extender<'parse> {
    /// The selector in which the `@extend` appeared.
    pub selector: ComplexSelector<'parse>,
    /// The minimum specificity required for any selector generated from this
    /// extender.
    pub specificity: usize,
    /// Whether this extender represents a selector that was originally in the
    /// document, rather than one defined with `@extend`.
    pub is_original: bool,
    /// The media context of the extension that created this extender.
    /// `None` for original extenders (those not created by an @extend rule).
    /// Dart: `_extension?.mediaContext`
    pub extension_media_context: Option<Vec<CssMediaQuery<'parse>>>,
    /// The span of the extension that created this extender.
    /// `None` for original extenders.
    /// Dart: `_extension?.span`
    pub extension_span: Option<FileSpan<'parse>>,
}

impl<'parse> Extender<'parse> {
    /// Creates a new extender.
    ///
    /// If `specificity` isn't passed, it defaults to the selector's own
    /// specificity.
    pub fn new(
        selector: ComplexSelector<'parse>,
        specificity: Option<usize>,
        is_original: bool,
    ) -> Self {
        let specificity = specificity.unwrap_or_else(|| selector.specificity());
        Extender {
            selector,
            specificity,
            is_original,
            extension_media_context: None,
            extension_span: None,
        }
    }

    /// Asserts that the `media_context` for a selector is compatible with the
    /// query context for this extender.
    ///
    /// `selector_span` is used for error reporting as the location of the
    /// selector being extended.
    ///
    /// If this extender has no backing extension, no check is performed.
    /// Dart: `Extender.assertCompatibleMediaContext`
    pub fn assert_compatible_media_context(
        &self,
        media_context: &Option<Vec<CssMediaQuery<'parse>>>,
        selector_span: FileSpan<'parse>,
    ) -> SassResult<()> {
        let expected = match &self.extension_media_context {
            Some(ctx) => ctx,
            None => return Ok(()),
        };
        if let Some(ref ctx) = media_context {
            if media_queries_equal(expected, ctx) {
                return Ok(());
            }
        }
        let extension_span = match self.extension_span {
            Some(span) => span,
            None => {
                return Err(Box::new(SassError::Script {
                    message: "You may not @extend selectors across media queries.".into(),
                    argument_name: None,
                }))
            }
        };

        let mut secondary = Vec::new();
        if let Some(last) = expected.last() {
            secondary.push((
                SourceSpanWithContext::from_file_span(&last.span)?,
                "extension @media".into(),
            ));
        }
        if let Some(last) = media_context.as_ref().and_then(|ctx| ctx.last()) {
            secondary.push((
                SourceSpanWithContext::from_file_span(&last.span)?,
                "extended selector @media".into(),
            ));
        }
        secondary.push((
            SourceSpanWithContext::from_file_span(&selector_span)?,
            "extended selector".into(),
        ));

        Err(Box::new(SassError::MultiSpan {
            message: "You may not @extend selectors across media queries.".into(),
            span: SourceSpanWithContext::from_file_span(&extension_span)?,
            primary_label: Some("extension".into()),
            secondary,
            original_source: None,
            cause: None,
            loaded_urls: vec![],
            trace: Trace::default(),
        }))
    }
}

impl<'parse> Extender<'parse> {
    /// Returns the CSS string of the wrapped selector (mirrors the Dart
    /// `toString` which returns the selector string).
    pub fn to_display_string(&self) -> SassResult<String> {
        self.selector.to_css_string(false)
    }
}

impl<'parse> fmt::Display for Extender<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// A single non-merged `@extend` rule.
///
/// The extension target is also represented externally, as the key of the
/// store map holding this extension. If several mandatory extensions share
/// one extender and target they are folded into a merged extension (see
/// [`crate::extend::merged`]) so that each is marked resolved.
#[derive(Clone, Debug)]
pub struct BaseExtension<'parse> {
    /// The extender (such as `A` in `A {@extend B}`).
    pub extender: Extender<'parse>,
    /// The selector that's being extended.
    pub target: SimpleSelector<'parse>,
    /// The media query context this extension is restricted to, or [`None`]
    /// if it can apply within any context.
    pub media_context: Option<Vec<CssMediaQuery<'parse>>>,
    /// Whether this extension is optional.
    pub is_optional: bool,
    /// The span of the `@extend` rule that defined this extension.
    ///
    /// If any extend rule for this extension is mandatory, this is guaranteed
    /// to be the span of a mandatory rule.
    pub span: FileSpan<'parse>,
}

impl<'parse> BaseExtension<'parse> {
    /// Creates a new extension, linking the fresh extender back to it.
    ///
    /// Named `new` but returns the linked [`Extension`] handle rather than
    /// `Self` by design (it allocates the extension in `arena`).
    #[allow(clippy::new_ret_no_self)]
    pub fn new<'compile: 'parse>(
        arena: &'compile Bump,
        extender: ComplexSelector<'parse>,
        target: SimpleSelector<'parse>,
        span: FileSpan<'parse>,
        media_context: Option<Vec<CssMediaQuery<'parse>>>,
        is_optional: bool,
    ) -> Extension<'parse> {
        let specificity = extender.specificity();
        let ext = Extender {
            selector: extender,
            specificity,
            is_original: false,
            extension_media_context: media_context.clone(),
            extension_span: Some(span),
        };
        let b = BaseExtension {
            extender: ext,
            target,
            media_context,
            is_optional,
            span,
        };
        Extension::new_base(arena, b)
    }
}

/// The internal variant of an Extension.
/// Arena-allocated (Dart's reference identity).
#[derive(Clone, Debug)]
pub(crate) enum ExtensionKind<'parse> {
    Base(BaseExtension<'parse>),
    Merged {
        base: BaseExtension<'parse>,
        left: Extension<'parse>,
        right: Extension<'parse>,
    },
}

/// An `@extend` rule's extension state.
///
/// Wraps `&'parse ExtensionKind` for cheap copying and identity-based equality.
/// Dart uses default reference identity for Extension. Rust uses pointer
/// equality on the arena allocation.
#[derive(Debug, Clone, Copy)]
pub struct Extension<'parse> {
    pub(crate) inner: &'parse ExtensionKind<'parse>,
}

impl<'parse> Extension<'parse> {
    /// Creates a base (non-merged) extension handle in the arena.
    pub fn new_base<'compile: 'parse>(arena: &'compile Bump, b: BaseExtension<'parse>) -> Self {
        Self {
            inner: arena.alloc(ExtensionKind::Base(b)),
        }
    }

    /// Creates a merged extension handle in the arena.
    pub fn new_merged<'compile: 'parse>(
        arena: &'compile Bump,
        base: BaseExtension<'parse>,
        left: Extension<'parse>,
        right: Extension<'parse>,
    ) -> Self {
        Self {
            inner: arena.alloc(ExtensionKind::Merged { base, left, right }),
        }
    }

    pub fn extender(&self) -> &Extender<'parse> {
        match self.inner {
            ExtensionKind::Base(b) => &b.extender,
            ExtensionKind::Merged { base, .. } => &base.extender,
        }
    }

    /// The selector that's being extended.
    pub fn target(&self) -> &SimpleSelector<'parse> {
        match self.inner {
            ExtensionKind::Base(b) => &b.target,
            ExtensionKind::Merged { base, .. } => &base.target,
        }
    }

    /// The media query context to which this extension is restricted, or
    /// [`None`] if it can apply within any context.
    pub fn media_context(&self) -> Option<&Vec<CssMediaQuery<'parse>>> {
        match self.inner {
            ExtensionKind::Base(b) => b.media_context.as_ref(),
            ExtensionKind::Merged { base, .. } => base.media_context.as_ref(),
        }
    }

    /// Whether this extension is optional.
    pub fn is_optional(&self) -> bool {
        match self.inner {
            ExtensionKind::Base(b) => b.is_optional,
            ExtensionKind::Merged { base, .. } => base.is_optional,
        }
    }

    /// The span of the `@extend` rule that defined this extension.
    pub fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self.inner {
            ExtensionKind::Base(b) => Ok(b.span),
            ExtensionKind::Merged { base, .. } => Ok(base.span),
        }
    }

    pub fn is_merged(&self) -> bool {
        matches!(self.inner, ExtensionKind::Merged { .. })
    }

    /// Returns the same extension with the extender selector replaced.
    pub fn with_extender<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        new_extender: ComplexSelector<'parse>,
    ) -> Extension<'parse> {
        let (target, span, media_context, is_optional) = self.base_fields();
        BaseExtension::new(
            arena,
            new_extender,
            target,
            span,
            media_context,
            is_optional,
        )
    }

    fn base_fields(
        &self,
    ) -> (
        SimpleSelector<'parse>,
        FileSpan<'parse>,
        Option<Vec<CssMediaQuery<'parse>>>,
        bool,
    ) {
        match self.inner {
            ExtensionKind::Base(b) => (
                b.target.clone(),
                b.span,
                b.media_context.clone(),
                b.is_optional,
            ),
            ExtensionKind::Merged { base, .. } => (
                base.target.clone(),
                base.span,
                base.media_context.clone(),
                base.is_optional,
            ),
        }
    }

    pub fn extender_selector(&self) -> &ComplexSelector<'parse> {
        &self.extender().selector
    }
}

// Identity-based equality, matching Dart's Set<Extension>.identity().
impl<'parse> PartialEq for Extension<'parse> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.inner, other.inner)
    }
}

impl<'parse> Eq for Extension<'parse> {}

impl<'parse> Hash for Extension<'parse> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.inner as *const ExtensionKind<'parse>).hash(state);
    }
}

impl<'parse> Extension<'parse> {
    /// Returns `"<extender> {@extend <target>[ !optional]}"`, mirroring the
    /// Dart `toString`.
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "{}", self.extender_selector().to_css_string(false)?).unwrap();
        write!(buf, " {{@extend {}", self.target()).unwrap();
        if self.is_optional() {
            write!(buf, " !optional").unwrap();
        }
        write!(buf, "}}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for Extension<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// Compares two media query lists element-wise (Dart `listEquals` on the
/// media contexts).
pub fn media_queries_equal(a: &Vec<CssMediaQuery>, b: &Vec<CssMediaQuery>) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::common::source_span_file_source::FileSource;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::url::OwnedUrl;
    use bumpalo::Bump;
    use std::collections::hash_map::DefaultHasher;

    fn class(name: &'static str) -> SimpleSelector<'static> {
        SimpleSelector::Class(ClassSelector::new(name, BOGUS_SPAN))
    }

    fn make_complex<'p>(arena: &'p Bump, name: &'static str) -> ComplexSelector<'p> {
        let s = class(name);
        let compound = CompoundSelector::new(arena, vec![s], BOGUS_SPAN).unwrap();
        let comp = ComplexSelectorComponent::new(arena, compound, vec![], BOGUS_SPAN);
        ComplexSelector::new(arena, vec![], vec![comp.clone()], BOGUS_SPAN, false).unwrap()
    }

    #[test]
    fn test_new_extension() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender.clone(), target, BOGUS_SPAN, None, false);
        assert_eq!(
            *ext.extender().selector.components[0]
                .selector
                .components
                .first()
                .unwrap(),
            class("a")
        );
        assert!(!ext.is_optional());
        ext.span().unwrap();
    }

    #[test]
    fn test_new_extension_optional() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, true);
        assert!(ext.is_optional());
    }

    #[test]
    fn test_extension_media_context() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let mc = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, Some(mc), false);
        assert!(ext.media_context().is_some());
    }

    #[test]
    fn test_extension_with_extender() {
        let arena = Bump::new();
        let extender1 = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender1, target.clone(), BOGUS_SPAN, None, false);

        let new_extender = make_complex(&arena, "c");
        let new_ext = ext.with_extender(&arena, new_extender.clone());
        assert_eq!(new_ext.target(), &target);
    }

    #[test]
    fn test_extension_string() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, false);
        let s = ext.to_display_string().unwrap();
        assert!(!s.is_empty(), "extension string should not be empty");
    }

    #[test]
    fn test_extension_string_optional() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, true);
        let s = ext.to_display_string().unwrap();
        assert!(s.contains("!optional"));
        assert!(s.contains("@extend"));
    }

    #[test]
    fn test_extension_is_merged() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, false);
        assert!(!ext.is_merged());
    }

    #[test]
    fn test_extension_clone_is_identity_equal() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, false);
        let cloned = ext;
        assert_eq!(ext, cloned);
    }

    #[test]
    fn test_extension_different_not_equal() {
        let arena = Bump::new();
        let ext1 = BaseExtension::new(
            &arena,
            make_complex(&arena, "a"),
            class("b"),
            BOGUS_SPAN,
            None,
            false,
        );
        let ext2 = BaseExtension::new(
            &arena,
            make_complex(&arena, "a"),
            class("b"),
            BOGUS_SPAN,
            None,
            false,
        );
        assert_ne!(ext1, ext2);
    }

    #[test]
    fn test_extension_hash_consistent() {
        let arena = Bump::new();
        let ext = BaseExtension::new(
            &arena,
            make_complex(&arena, "a"),
            class("b"),
            BOGUS_SPAN,
            None,
            false,
        );
        let h1 = {
            let mut h = DefaultHasher::new();
            ext.hash(&mut h);
            h.finish()
        };
        let h2 = {
            let mut h = DefaultHasher::new();
            ext.hash(&mut h);
            h.finish()
        };
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_extender_specificity_default() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let expected_spec = extender.specificity();
        let ex = Extender::new(extender, None, false);
        assert_eq!(ex.specificity, expected_spec);
    }

    #[test]
    fn test_extender_specificity_explicit() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let ex = Extender::new(extender, Some(500), false);
        assert_eq!(ex.specificity, 500);
    }

    #[test]
    fn test_extender_is_original() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let ex = Extender::new(extender, None, true);
        assert!(ex.is_original);
    }

    #[test]
    fn test_assert_compatible_media_context_both_nil() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let ex = Extender::new(extender, None, false);
        let result = ex.assert_compatible_media_context(&None, BOGUS_SPAN);
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_compatible_media_context_extender_has_no_media_context() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let ex = Extender::new(extender, None, false);
        let mc = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        let result = ex.assert_compatible_media_context(&Some(mc), BOGUS_SPAN);
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_compatible_media_context_matching() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let mc = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        let mut ex = Extender::new(extender, None, false);
        ex.extension_media_context = Some(mc.clone());
        let result = ex.assert_compatible_media_context(&Some(mc), BOGUS_SPAN);
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_compatible_media_context_conflicting() {
        let arena = Bump::new();
        let url = OwnedUrl::parse("file:///test.scss").unwrap();
        let src = FileSource::new_in_owned(&arena, "@media screen {} @media print {}", Some(&url));
        let ext_src = FileSource::new_in_owned(&arena, "@extend .a;", Some(&url));
        let sel_src = FileSource::new_in_owned(&arena, ".a", Some(&url));
        let extension_media_span = FileSpan::new(Some(src), 7, 13); // screen
        let extended_span = FileSpan::new(Some(src), 24, 29); // print
        let extension_span = FileSpan::new(Some(ext_src), 0, 10); // @extend .a
        let selector_span = FileSpan::new(Some(sel_src), 0, 2); // .a

        let extender = make_complex(&arena, "a");
        let mc1 = vec![CssMediaQuery::new_type(
            &arena,
            Some("screen".into()),
            extension_media_span,
            None,
            vec![],
        )];
        let mc2 = vec![CssMediaQuery::new_type(
            &arena,
            Some("print".into()),
            extended_span,
            None,
            vec![],
        )];
        let mut ex = Extender::new(extender, None, false);
        ex.extension_media_context = Some(mc1);
        ex.extension_span = Some(extension_span);
        let err = ex
            .assert_compatible_media_context(&Some(mc2), selector_span)
            .unwrap_err();
        match *err {
            SassError::MultiSpan {
                message,
                primary_label,
                secondary,
                ..
            } => {
                assert_eq!(
                    message,
                    "You may not @extend selectors across media queries."
                );
                assert_eq!(primary_label.as_deref(), Some("extension"));
                let labels: Vec<&str> = secondary.iter().map(|(_, label)| label.as_str()).collect();
                assert_eq!(
                    labels,
                    vec![
                        "extension @media",
                        "extended selector @media",
                        "extended selector"
                    ]
                );
            }
            other => panic!("expected MultiSpan error, got {other:?}"),
        }
    }

    #[test]
    fn test_media_queries_equal_both_nil() {
        // tested via Option comparison
    }

    #[test]
    fn test_media_queries_equal_same() {
        let mc1 = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        let mc2 = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        assert!(media_queries_equal(&mc1, &mc2));
    }

    #[test]
    fn test_media_queries_equal_different() {
        let mc1 = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        let mc2 = vec![CssMediaQuery::test_type(Some("print".into()), None, vec![])];
        assert!(!media_queries_equal(&mc1, &mc2));
    }

    #[test]
    fn test_media_queries_equal_different_length() {
        let mc1 = vec![CssMediaQuery::test_type(
            Some("screen".into()),
            None,
            vec![],
        )];
        let mc2 = vec![
            CssMediaQuery::test_type(Some("screen".into()), None, vec![]),
            CssMediaQuery::test_type(Some("print".into()), None, vec![]),
        ];
        assert!(!media_queries_equal(&mc1, &mc2));
    }

    #[test]
    fn test_extender_string() {
        let arena = Bump::new();
        let extender = make_complex(&arena, "a");
        let ex = Extender::new(extender, None, false);
        let s = ex.to_display_string().unwrap();
        assert!(!s.is_empty(), "extender string should not be empty");
    }
}
