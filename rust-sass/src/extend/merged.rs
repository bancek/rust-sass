// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/merged_extension.dart
// go-source: go/extend/extension_merged.go

use crate::common::exception::{SassError, SassResult};
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::io::Io;
use bumpalo::Bump;

use crate::extend::extension::{
    media_queries_equal, span_message, BaseExtension, Extension, ExtensionKind,
};

/// An [`Extension`] created by merging two extensions with the same extender
/// and target.
///
/// Used when multiple mandatory extensions exist so that both of them are
/// marked as resolved. The merged node forms a binary history tree over the
/// [`unmerge_extensions`] leaves; [`Extension::is_merged`] reports whether a
/// handle is such a node.
///
/// Returns an extension that combines `left` and `right`.
///
/// Returns an error if `left` and `right` have incompatible media contexts,
/// or don't have the same extender and target.
pub fn merge_extensions<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    left: &Extension<'parse>,
    right: &Extension<'parse>,
    io: &dyn Io,
    unicode: bool,
) -> SassResult<Extension<'parse>> {
    if left.extender().selector != right.extender().selector || left.target() != right.target() {
        return Err(Box::new(SassError::Script {
            message: "The two extensions aren't the same extension.".into(),
            argument_name: Some("left, right".into()),
        }));
    }

    if left.media_context().is_some()
        && right.media_context().is_some()
        && !media_queries_equal(
            left.media_context().unwrap(),
            right.media_context().unwrap(),
        )
    {
        let left_msg = span_message(left.span()?, io, unicode);
        return Err(Box::new(SassError::Sass {
            message: format!(
                "From {}\nYou may not @extend the same selector from within different media queries.",
                left_msg
            ),
            span: SourceSpanWithContext::from_file_span(
                &right.span()?,
            )?,
            cause: None,
            loaded_urls: vec![],
        }));
    }

    // If one extension is optional and doesn't add a special media context,
    // it doesn't need to be merged.
    if right.is_optional() && right.media_context().is_none() {
        return Ok(*left);
    }
    if left.is_optional() && left.media_context().is_none() {
        return Ok(*right);
    }

    let media_context = left
        .media_context()
        .or_else(|| right.media_context())
        .cloned();

    let base_ext = BaseExtension::new(
        arena,
        left.extender_selector().clone(),
        left.target().clone(),
        left.span()?,
        media_context,
        true, // merged extensions are always optional
    );

    let base = match base_ext.inner {
        ExtensionKind::Base(b) => b.clone(),
        ExtensionKind::Merged { .. } => unreachable!(),
    };

    Ok(Extension::new_merged(arena, base, *left, *right))
}

/// Returns all leaf-node extensions in the merged extension tree.
/// Extensions are cheaply copied (`&'parse`).
pub fn unmerge_extensions<'parse>(ext: &Extension<'parse>) -> Vec<Extension<'parse>> {
    match ext.inner {
        ExtensionKind::Base(_) => vec![*ext],
        ExtensionKind::Merged { left, right, .. } => {
            let mut result = unmerge_extensions(left);
            result.extend(unmerge_extensions(right));
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::media_query::CssMediaQuery;
    use crate::common::file_span::FileSpan;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::common::source_span_file_source::FileSource;
    use crate::io::VirtualIo;
    use crate::selector::class::ClassSelector;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::SimpleSelector;
    use bumpalo::Bump;

    fn class(name: &str) -> SimpleSelector<'static> {
        SimpleSelector::Class(ClassSelector::new(name.into(), BOGUS_SPAN))
    }

    fn make_complex(name: &str) -> ComplexSelector<'static> {
        let s = class(name);
        let compound = CompoundSelector::new(vec![s], BOGUS_SPAN).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), vec![], BOGUS_SPAN);
        ComplexSelector::new(vec![], vec![comp], BOGUS_SPAN, false).unwrap()
    }

    #[test]
    fn test_merge_same_extender_and_target() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            false,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            false,
        );
        let merged = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true).unwrap();
        assert!(merged.is_merged());
    }

    #[test]
    fn test_merge_different_extender() {
        let arena = Bump::new();
        let extender1 = make_complex("a");
        let extender2 = make_complex("c");
        let target = class("b");
        let left = BaseExtension::new(&arena, extender1, target.clone(), BOGUS_SPAN, None, false);
        let right = BaseExtension::new(&arena, extender2, target.clone(), BOGUS_SPAN, None, false);
        let result = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_different_target() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            class("b"),
            BOGUS_SPAN,
            None,
            false,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            class("c"),
            BOGUS_SPAN,
            None,
            false,
        );
        let result = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_compatible_media_contexts() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let mc = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            Some(mc.clone()),
            false,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            Some(mc),
            false,
        );
        let merged = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true).unwrap();
        assert!(merged.is_merged());
        assert!(merged.media_context().is_some());
    }

    #[test]
    fn test_merge_incompatible_media_contexts() {
        let arena = Bump::new();
        let src = FileSource::new_in(&arena, ".a", None);
        let real_span = FileSpan::new(Some(src), 0, 2);

        let extender = make_complex("a");
        let target = class("b");
        let mc1 = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        let mc2 = vec![CssMediaQuery::new_type(Some("print".into()), None, vec![])];
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            real_span,
            Some(mc1),
            false,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            real_span,
            Some(mc2),
            false,
        );
        let result = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_right_optional_no_media() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            false,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            true,
        );
        let merged = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true).unwrap();
        assert!(!merged.is_merged()); // left is returned directly
    }

    #[test]
    fn test_merge_left_optional_no_media() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            true,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            false,
        );
        let merged = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true).unwrap();
        assert!(!merged.is_merged()); // right is returned directly
    }

    #[test]
    fn test_unmerge_base() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let ext = BaseExtension::new(&arena, extender, target.clone(), BOGUS_SPAN, None, false);
        let unmerged = unmerge_extensions(&ext);
        assert_eq!(unmerged.len(), 1);
        assert_eq!(unmerged[0].target(), &target);
    }

    #[test]
    fn test_unmerge_merged() {
        let arena = Bump::new();
        let extender = make_complex("a");
        let target = class("b");
        let left = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            false,
        );
        let right = BaseExtension::new(
            &arena,
            extender.clone(),
            target.clone(),
            BOGUS_SPAN,
            None,
            false,
        );
        let merged = merge_extensions(&arena, &left, &right, &VirtualIo::new(), true).unwrap();
        let unmerged = unmerge_extensions(&merged);
        assert_eq!(unmerged.len(), 2);
        for ext in &unmerged {
            assert_eq!(ext.target(), &target);
        }
    }
}
