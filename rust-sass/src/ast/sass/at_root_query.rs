// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/at_root_query.dart
// go-source: go/value/sass_at_root_query.go

use crate::ast::css::node::CssParentNode;
use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::common::source_span_file_source::FileSource;
use crate::parse::at_root_query::AtRootQueryParser;
use std::collections::HashSet;

/// A query for the `@at-root` rule.
///
/// NOTE: This type has no `'parse` lifetime — it has no `FileSpan`.
/// Confirmed by both Dart and Go sources.
#[derive(Clone, Debug)]
pub struct AtRootQuery {
    /// Whether the query includes or excludes rules with the specified names.
    pub include: bool,
    /// The names of the rules included or excluded by this query.
    ///
    /// There are two special names. "all" indicates that all rules are
    /// included or excluded, and "rule" indicates style rules are included or
    /// excluded.
    names: HashSet<String>,
    all: bool,
    rule: bool,
}

impl AtRootQuery {
    /// The default at-root query, which excludes only style rules.
    ///
    /// Matches Dart: AtRootQuery.defaultQuery
    pub fn default_query() -> Self {
        AtRootQuery {
            include: false,
            names: HashSet::new(),
            all: false,
            rule: true,
        }
    }

    /// Creates a query with the given rule `names`, including them when
    /// `include` is true and excluding them otherwise.
    pub fn new(names: HashSet<String>, include: bool) -> Self {
        let has_all = names.contains("all");
        let has_rule = names.contains("rule");
        AtRootQuery {
            include,
            names,
            all: has_all,
            rule: has_rule,
        }
    }

    /// Returns whether this excludes style rules.
    ///
    /// Note that this takes `include` into account.
    ///
    /// Matches Dart: AtRootQuery.excludesStyleRules
    pub fn excludes_style_rules(&self) -> bool {
        (self.all || self.rule) != self.include
    }

    /// Returns whether this excludes an at-rule with the given `name`.
    ///
    /// Matches Dart: AtRootQuery.excludesName
    pub fn excludes_name(&self, name: &str) -> bool {
        (self.all || self.names.contains(name)) != self.include
    }

    /// Whether this includes or excludes *all* rules.
    pub fn is_all(&self) -> bool {
        self.all
    }

    /// Returns whether `self` excludes `node`.
    ///
    // `excludes` is only called from the evaluator, which is why Dart marks
    // it `@internal`.
    ///
    /// Matches Dart: AtRootQuery.excludes
    pub fn excludes(&self, node: &CssParentNode<'_>) -> bool {
        if self.all {
            return !self.include;
        }
        match node {
            CssParentNode::StyleRule(_) => self.excludes_style_rules(),
            CssParentNode::MediaRule(_) => self.excludes_name("media"),
            CssParentNode::SupportsRule(_) => self.excludes_name("supports"),
            CssParentNode::AtRule(r) => self.excludes_name(&r.name.value.to_lowercase()),
            _ => false,
        }
    }

    /// Parses an at-root query from `contents`.
    ///
    /// The `interpolation_map` maps the text of `contents` back to the
    /// original location of the selector in the source file.
    ///
    /// Parse failures fall back to the default query rather than throwing
    /// (Dart's factory throws `SassFormatException`, but the caller here is
    /// the parser mid-`@at-root`, which recovers).
    ///
    /// Matches Dart: AtRootQuery.parse (the interpolation map threads through
    /// to the parser for source-mapped error spans; parse failures still fall
    /// back to the default query — see U24).
    pub fn parse<'compile, 'parse>(
        arena: &'compile bumpalo::Bump,
        contents: &str,
        interpolation_map: Option<&'parse InterpolationMap<'parse>>,
    ) -> Self
    where
        'compile: 'parse,
    {
        let source = FileSource::new_in(arena, contents, None);
        let mut parser = AtRootQueryParser::new(source, interpolation_map);
        parser.parse().unwrap_or_else(|_| Self::default_query())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::at_rule::CssAtRule;
    use crate::ast::css::keyframe_block::CssKeyframeBlock;
    use crate::ast::css::media_query::parse_list;
    use crate::ast::css::media_rule::CssMediaRule;
    use crate::ast::css::style_rule::CssStyleRule;
    use crate::ast::css::stylesheet::CssStylesheet;
    use crate::ast::css::supports_rule::CssSupportsRule;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::list::SelectorList;

    #[test]
    fn test_default_query() {
        let q = AtRootQuery::default_query();
        assert!(!q.include);
        assert!(q.excludes_style_rules());
    }

    #[test]
    fn test_include_rule() {
        let mut names = HashSet::new();
        names.insert("rule".into());
        let q = AtRootQuery::new(names, true);
        assert!(q.include);
        assert!(!q.excludes_style_rules());
    }

    #[test]
    fn test_exclude_all() {
        let mut names = HashSet::new();
        names.insert("all".into());
        let q = AtRootQuery::new(names, false);
        assert!(q.excludes_style_rules());
        assert!(q.excludes_name("media"));
    }

    #[test]
    fn test_excludes_name() {
        let mut names = HashSet::new();
        names.insert("media".into());
        names.insert("supports".into());
        let q = AtRootQuery::new(names, false);
        assert!(q.excludes_name("media"));
        assert!(q.excludes_name("supports"));
        assert!(!q.excludes_name("screen"));
    }

    #[test]
    fn test_excludes_style_rules() {
        let mut names = HashSet::new();
        names.insert("rule".into());
        let q = AtRootQuery::new(names, false);
        assert!(q.excludes_style_rules());
    }

    #[test]
    fn test_excludes_style_rule() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, ".foo", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let cs = ComplexSelector::parse(&arena, ".foo", None, true).unwrap();
        let sel_list = SelectorList::new(&arena, vec![cs], span).unwrap();
        let sr = CssStyleRule::new(sel_list, span, sel_list, false);
        let node = CssParentNode::StyleRule(sr);

        let mut names = HashSet::new();
        names.insert("rule".into());
        let q_excl = AtRootQuery::new(names.clone(), false);
        assert!(q_excl.excludes(&node));

        let q_incl = AtRootQuery::new(names, true);
        assert!(!q_incl.excludes(&node));
    }

    #[test]
    fn test_excludes_media_rule() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, "(color)", None);
        let span = FileSpan::new(Some(fs), 0, 7);
        let mq = parse_list("(color)").unwrap();
        let mr = CssMediaRule::new(mq, span).unwrap();
        let node = CssParentNode::MediaRule(mr);

        let mut names = HashSet::new();
        names.insert("media".into());
        let q_excl = AtRootQuery::new(names.clone(), false);
        assert!(q_excl.excludes(&node));

        let q_incl = AtRootQuery::new(names, true);
        assert!(!q_incl.excludes(&node));
    }

    #[test]
    fn test_excludes_supports_rule() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, "display: grid", None);
        let span = FileSpan::new(Some(fs), 0, 13);
        let cond = CssValue::new("display: grid".into(), span);
        let sr = CssSupportsRule::new(cond, span);
        let node = CssParentNode::SupportsRule(sr);

        let mut names = HashSet::new();
        names.insert("supports".into());
        let q_excl = AtRootQuery::new(names.clone(), false);
        assert!(q_excl.excludes(&node));

        let q_incl = AtRootQuery::new(names, true);
        assert!(!q_incl.excludes(&node));
    }

    #[test]
    fn test_excludes_at_rule_named() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, "@print", None);
        let span = FileSpan::new(Some(fs), 0, 6);
        let name = CssValue::new("print".into(), span);
        let ar = CssAtRule::new(name, span, false, None);
        let node = CssParentNode::AtRule(ar);

        let mut names = HashSet::new();
        names.insert("print".into());
        let q_excl = AtRootQuery::new(names.clone(), false);
        assert!(q_excl.excludes(&node));

        let q_incl = AtRootQuery::new(names, true);
        assert!(!q_incl.excludes(&node));

        let mut other = HashSet::new();
        other.insert("screen".into());
        let q_other = AtRootQuery::new(other, false);
        assert!(!q_other.excludes(&node));
    }

    #[test]
    fn test_excludes_all() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, "@foo", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let name = CssValue::new("foo".into(), span);
        let ar = CssAtRule::new(name, span, false, None);
        let node = CssParentNode::AtRule(ar);

        let mut names = HashSet::new();
        names.insert("all".into());
        let q_excl = AtRootQuery::new(names.clone(), false);
        assert!(q_excl.excludes(&node));

        let q_incl = AtRootQuery::new(names, true);
        assert!(!q_incl.excludes(&node));
    }

    #[test]
    fn test_excludes_stylesheet_false() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, "", None);
        let span = FileSpan::new(Some(fs), 0, 0);
        let ss = CssStylesheet::new(vec![], span);
        let node = CssParentNode::Stylesheet(ss);
        let q = AtRootQuery::default_query();
        assert!(!q.excludes(&node));
    }

    #[test]
    fn test_excludes_keyframe_block_false() {
        let arena = bumpalo::Bump::new();
        let fs = FileSource::new_in(&arena, "from", None);
        let span = FileSpan::new(Some(fs), 0, 4);
        let sel = CssValue::new(vec!["from".into()], span);
        let kb = CssKeyframeBlock::new(sel, span);
        let node = CssParentNode::KeyframeBlock(kb);
        let q = AtRootQuery::default_query();
        assert!(!q.excludes(&node));
    }

    #[test]
    fn test_parse() {
        let arena = bumpalo::Bump::new();
        let q = AtRootQuery::parse(&arena, "(with: rule)", None);
        assert!(q.include);
        assert!(!q.excludes_style_rules());
    }

    #[test]
    fn test_parse_default_on_error() {
        let arena = bumpalo::Bump::new();
        let q = AtRootQuery::parse(&arena, "invalid", None);
        assert!(!q.include);
        assert!(q.excludes_style_rules());
    }
}
