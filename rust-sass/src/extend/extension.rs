// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/extension.dart
// go-source: go/extend/extension.go

use crate::common::pretty_uri::pretty_uri;
use crate::common::source_span_highlighter::HighlightOptions;
use crate::common::source_span_highlighter::Highlighter;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::termglyph::GlyphSet;
use bumpalo::Bump;
use std::fmt;
use std::fmt::Write;
use std::hash::{Hash, Hasher};

use crate::ast::css::media_query::CssMediaQuery;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::io::Io;
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
    pub extension_media_context: Option<Vec<CssMediaQuery>>,
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
    /// If this extender has no backing extension, no check is performed.
    /// Dart: `Extender.assertCompatibleMediaContext`
    pub fn assert_compatible_media_context(
        &self,
        media_context: &Option<Vec<CssMediaQuery>>,
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
        if let Some(span) = self.extension_span {
            return Err(Box::new(SassError::Sass {
                message: "You may not @extend selectors across media queries.".into(),
                span: SourceSpanWithContext::from_file_span(&span)?,
                cause: None,
                loaded_urls: vec![],
            }));
        }
        Err(Box::new(SassError::Script {
            message: "You may not @extend selectors across media queries.".into(),
            argument_name: None,
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
    pub media_context: Option<Vec<CssMediaQuery>>,
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
        media_context: Option<Vec<CssMediaQuery>>,
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
    pub fn media_context(&self) -> Option<&Vec<CssMediaQuery>> {
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
        Option<Vec<CssMediaQuery>>,
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

/// Builds the `"From <location>: <highlight>"` prefix for extend errors.
///
/// Mirrors Dart's `span.message('')`: the `"line L, column C of URL: "` header
/// plus the span's source highlight when non-empty.
pub fn span_message(span: FileSpan<'_>, io: &dyn Io, unicode: bool) -> String {
    let url = match span.source_url() {
        Some(u) => u.clone(),
        None => return String::new(),
    };
    let loc = span.start_location();
    let mut out = format!(
        "line {}, column {} of {}: ",
        loc.line + 1,
        loc.column + 1,
        pretty_uri(&url, io)
    );
    // Dart's `FileSpan.message('')` (source_span span_mixin.dart:53-66) appends
    // the span's highlight when non-empty, so a "From ..." prefix embeds the
    // original selector's highlight before the rest of the error.
    if let Ok(ctx) = SourceSpanWithContext::from_file_span(&span) {
        let opts = HighlightOptions {
            glyphs: if unicode {
                GlyphSet::default()
            } else {
                GlyphSet::Ascii
            },
            ..Default::default()
        };
        let mut hl = Highlighter::new(&ctx, &opts, io).ok();
        if let Some(h) = hl.as_mut() {
            if let Ok(s) = h.highlight() {
                if !s.is_empty() {
                    out.push('\n');
                    out.push_str(&s);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::common::source_span_file_source::FileSource;
    use crate::io::VirtualIo;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::url::SassUrl;
    use bumpalo::Bump;
    use std::collections::hash_map::DefaultHasher;

    fn class(name: &str) -> SimpleSelector<'static> {
        SimpleSelector::Class(ClassSelector::new(name.into(), BOGUS_SPAN))
    }

    fn make_complex(name: &str) -> ComplexSelector<'static> {
        let s = class(name);
        let compound = CompoundSelector::new(vec![s], BOGUS_SPAN).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), vec![], BOGUS_SPAN);
        ComplexSelector::new(vec![], vec![comp.clone()], BOGUS_SPAN, false).unwrap()
    }

    #[test]
    fn test_new_extension() {
        let arena = Bump::new();
        let extender = make_complex("a");
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
        let extender = make_complex("a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, true);
        assert!(ext.is_optional());
    }

    #[test]
    fn test_extension_media_context() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let mc = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, Some(mc), false);
        assert!(ext.media_context().is_some());
    }

    #[test]
    fn test_extension_with_extender() {
        let arena = Bump::new();
        let extender1 = make_complex("a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender1, target.clone(), BOGUS_SPAN, None, false);

        let new_extender = make_complex("c");
        let new_ext = ext.with_extender(&arena, new_extender.clone());
        assert_eq!(new_ext.target(), &target);
    }

    #[test]
    fn test_extension_string() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, false);
        let s = ext.to_display_string().unwrap();
        assert!(!s.is_empty(), "extension string should not be empty");
    }

    #[test]
    fn test_extension_string_optional() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, true);
        let s = ext.to_display_string().unwrap();
        assert!(s.contains("!optional"));
        assert!(s.contains("@extend"));
    }

    #[test]
    fn test_extension_is_merged() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target, BOGUS_SPAN, None, false);
        assert!(!ext.is_merged());
    }

    #[test]
    fn test_extension_clone_is_identity_equal() {
        let arena = Bump::new();
        let extender = make_complex("a");
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
            make_complex("a"),
            class("b"),
            BOGUS_SPAN,
            None,
            false,
        );
        let ext2 = BaseExtension::new(
            &arena,
            make_complex("a"),
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
            make_complex("a"),
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
        let extender = make_complex("a");
        let expected_spec = extender.specificity();
        let ex = Extender::new(extender, None, false);
        assert_eq!(ex.specificity, expected_spec);
    }

    #[test]
    fn test_extender_specificity_explicit() {
        let extender = make_complex("a");
        let ex = Extender::new(extender, Some(500), false);
        assert_eq!(ex.specificity, 500);
    }

    #[test]
    fn test_extender_is_original() {
        let extender = make_complex("a");
        let ex = Extender::new(extender, None, true);
        assert!(ex.is_original);
    }

    #[test]
    fn test_assert_compatible_media_context_both_nil() {
        let extender = make_complex("a");
        let ex = Extender::new(extender, None, false);
        let result = ex.assert_compatible_media_context(&None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_compatible_media_context_extender_has_no_media_context() {
        let extender = make_complex("a");
        let ex = Extender::new(extender, None, false);
        let mc = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let result = ex.assert_compatible_media_context(&Some(mc));
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_compatible_media_context_matching() {
        let extender = make_complex("a");
        let mc = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let mut ex = Extender::new(extender, None, false);
        ex.extension_media_context = Some(mc.clone());
        let result = ex.assert_compatible_media_context(&Some(mc));
        assert!(result.is_ok());
    }

    #[test]
    fn test_assert_compatible_media_context_conflicting() {
        let arena = Bump::new();
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let src = FileSource::new_in(&arena, ".a", Some(url));
        let real_span = FileSpan::new(Some(src), 0, 2);

        let extender = make_complex("a");
        let mc1 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let mc2 = vec![CssMediaQuery::new_type(Some("print".into()), None, vec![])];
        let mut ex = Extender::new(extender, None, false);
        ex.extension_media_context = Some(mc1);
        ex.extension_span = Some(real_span);
        let result = ex.assert_compatible_media_context(&Some(mc2));
        assert!(result.is_err());
    }

    #[test]
    fn test_media_queries_equal_both_nil() {
        // tested via Option comparison
    }

    #[test]
    fn test_media_queries_equal_same() {
        let mc1 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let mc2 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        assert!(media_queries_equal(&mc1, &mc2));
    }

    #[test]
    fn test_media_queries_equal_different() {
        let mc1 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let mc2 = vec![CssMediaQuery::new_type(Some("print".into()), None, vec![])];
        assert!(!media_queries_equal(&mc1, &mc2));
    }

    #[test]
    fn test_media_queries_equal_different_length() {
        let mc1 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let mc2 = vec![
            CssMediaQuery::new_type(Some("screen".into()), None, vec![]),
            CssMediaQuery::new_type(Some("print".into()), None, vec![]),
        ];
        assert!(!media_queries_equal(&mc1, &mc2));
    }

    #[test]
    fn test_extender_string() {
        let extender = make_complex("a");
        let ex = Extender::new(extender, None, false);
        let s = ex.to_display_string().unwrap();
        assert!(!s.is_empty(), "extender string should not be empty");
    }

    #[test]
    fn test_span_message() {
        let arena = Bump::new();
        let url = SassUrl::parse("file:///test.scss").unwrap();
        let src = FileSource::new_in(&arena, ".a { color: red; }", Some(url.clone()));
        let span = FileSpan::new(Some(src), 0, 2);
        let msg = span_message(span, &VirtualIo::new(), true);
        assert!(!msg.is_empty());
    }
}
