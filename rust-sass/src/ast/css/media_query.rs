// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/css/media_query.dart
// go-source: go/value/css_media_query.go

use crate::ast::sass::interpolation_map::InterpolationMap;
use crate::common::source_span_file_source::FileSource;
use crate::parse::media_query::CssMediaQueryParser;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::common::exception::SassResult;

/// A plain CSS media query, as used in `@media` and `@import`.
#[derive(Clone, Debug)]
pub struct CssMediaQuery {
    /// The modifier, probably either "not" or "only".
    ///
    /// This may be `None` if no modifier is in use.
    pub modifier: Option<String>,

    /// The media type, for example "screen" or "print".
    ///
    /// This may be `None`. If so, `conditions` will not be empty.
    pub media_type: Option<String>,

    /// Whether `conditions` is a conjunction or a disjunction.
    ///
    /// In other words, if this is `true` this query matches when _all_
    /// `conditions` are met, and if it's `false` this query matches when _any_
    /// condition in `conditions` is met.
    ///
    /// If this is `false`, `modifier` and `media_type` will both be `None`.
    pub conjunction: bool,

    /// Media conditions, including parentheses.
    ///
    /// This is anything that can appear in the `[<media-in-parens>]`
    /// production.
    ///
    /// `[<media-in-parens>]`: https://drafts.csswg.org/mediaqueries-4/#typedef-media-in-parens
    pub conditions: Vec<String>,
}

impl CssMediaQuery {
    /// Creates a media query that specifies a type and, optionally, conditions.
    ///
    /// This always sets `conjunction` to `true`.
    pub fn new_type(
        media_type: Option<String>,
        modifier: Option<String>,
        conditions: Vec<String>,
    ) -> Self {
        CssMediaQuery {
            modifier,
            media_type,
            conjunction: true,
            conditions,
        }
    }

    /// Creates a media query that matches `conditions` according to
    /// `conjunction`.
    ///
    /// The `conjunction` argument may not be `None` if `conditions` is longer
    /// than a single element.
    pub fn new_condition(
        conditions: Vec<String>,
        conjunction: Option<bool>,
    ) -> Result<Self, String> {
        if conjunction.is_none() && conditions.len() > 1 {
            return Err(
                "If conditions is longer than one element, conjunction may not be null."
                    .to_string(),
            );
        }
        Ok(CssMediaQuery {
            modifier: None,
            media_type: None,
            conjunction: conjunction.unwrap_or(true),
            conditions,
        })
    }

    /// Returns whether this media query matches all media types.
    pub fn matches_all_types(&self) -> bool {
        self.media_type.is_none()
            || self
                .media_type
                .as_ref()
                .is_some_and(|t| t.eq_ignore_ascii_case("all"))
    }

    /// Merges this with `other` to return a query that matches the intersection
    /// of both inputs.
    pub fn merge(&self, other: &CssMediaQuery) -> MediaQueryMergeResult {
        if !self.conjunction || !other.conjunction {
            return MediaQueryMergeResult::Unrepresentable;
        }

        let our_modifier = self.modifier.as_ref().map(|m| m.to_lowercase());
        let our_type = self.media_type.as_ref().map(|t| t.to_lowercase());
        let their_modifier = other.modifier.as_ref().map(|m| m.to_lowercase());
        let their_type = other.media_type.as_ref().map(|t| t.to_lowercase());

        if our_type.is_none() && their_type.is_none() {
            let mut conditions = Vec::with_capacity(self.conditions.len() + other.conditions.len());
            conditions.extend_from_slice(&self.conditions);
            conditions.extend_from_slice(&other.conditions);
            return MediaQueryMergeResult::Successful(Box::new(MediaQuerySuccessfulMergeResult {
                query: CssMediaQuery {
                    modifier: None,
                    media_type: None,
                    conjunction: true,
                    conditions,
                },
            }));
        }

        let our_not = our_modifier.as_deref() == Some("not");
        let their_not = their_modifier.as_deref() == Some("not");

        let (modifier, media_type, conditions) = if our_not != their_not {
            if our_type.as_deref() == their_type.as_deref() {
                let (negative_conditions, positive_conditions) = if our_not {
                    (&self.conditions, &other.conditions)
                } else {
                    (&other.conditions, &self.conditions)
                };

                if all_contained(negative_conditions, positive_conditions) {
                    return MediaQueryMergeResult::Empty;
                }
                return MediaQueryMergeResult::Unrepresentable;
            } else if self.matches_all_types() || other.matches_all_types() {
                return MediaQueryMergeResult::Unrepresentable;
            }

            if our_not {
                (
                    other.modifier.clone(),
                    other.media_type.clone(),
                    other.conditions.clone(),
                )
            } else {
                (
                    self.modifier.clone(),
                    self.media_type.clone(),
                    self.conditions.clone(),
                )
            }
        } else if our_not {
            // Both are "not"
            if our_type.as_deref() != their_type.as_deref() {
                return MediaQueryMergeResult::Unrepresentable;
            }

            let (more_conditions, fewer_conditions) =
                if self.conditions.len() > other.conditions.len() {
                    (&self.conditions, &other.conditions)
                } else {
                    (&other.conditions, &self.conditions)
                };

            if !all_contained(fewer_conditions, more_conditions) {
                return MediaQueryMergeResult::Unrepresentable;
            }

            (
                self.modifier.clone(),
                self.media_type.clone(),
                more_conditions.clone(),
            )
        } else if self.matches_all_types() {
            let media_type = if other.matches_all_types() && our_type.is_none() {
                None
            } else {
                other.media_type.clone()
            };
            let mut conditions = Vec::with_capacity(self.conditions.len() + other.conditions.len());
            conditions.extend_from_slice(&self.conditions);
            conditions.extend_from_slice(&other.conditions);
            (other.modifier.clone(), media_type, conditions)
        } else if other.matches_all_types() {
            let mut conditions = Vec::with_capacity(self.conditions.len() + other.conditions.len());
            conditions.extend_from_slice(&self.conditions);
            conditions.extend_from_slice(&other.conditions);
            (self.modifier.clone(), self.media_type.clone(), conditions)
        } else {
            if our_type.as_deref() != their_type.as_deref() {
                return MediaQueryMergeResult::Empty;
            }

            let modifier = if our_modifier.is_some() {
                self.modifier.clone()
            } else {
                other.modifier.clone()
            };
            let mut conditions = Vec::with_capacity(self.conditions.len() + other.conditions.len());
            conditions.extend_from_slice(&self.conditions);
            conditions.extend_from_slice(&other.conditions);
            (modifier, self.media_type.clone(), conditions)
        };

        MediaQueryMergeResult::Successful(Box::new(MediaQuerySuccessfulMergeResult {
            query: CssMediaQuery {
                modifier,
                media_type,
                conjunction: true,
                conditions,
            },
        }))
    }
}

fn all_contained(needles: &[String], haystack: &[String]) -> bool {
    needles.iter().all(|n| haystack.contains(n))
}

/// Equality for `CssMediaQuery` ignores the `conjunction` field, matching
/// Dart's `operator ==` (modifier, type, conditions only).
impl PartialEq for CssMediaQuery {
    fn eq(&self, other: &Self) -> bool {
        self.modifier == other.modifier
            && self.media_type == other.media_type
            && self.conditions == other.conditions
    }
}

impl Eq for CssMediaQuery {}

/// Hash for `CssMediaQuery` excludes the `conjunction` field, matching Dart's
/// `hashCode` (`modifier.hashCode ^ type.hashCode ^ listHash(conditions)`)
/// — and therefore the `PartialEq` contract (Dart `==` also ignores it).
impl Hash for CssMediaQuery {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.modifier.hash(state);
        self.media_type.hash(state);
        self.conditions.hash(state);
    }
}

impl fmt::Display for CssMediaQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref m) = self.modifier {
            write!(f, "{} ", m)?;
        }
        if let Some(ref t) = self.media_type {
            write!(f, "{}", t)?;
            if !self.conditions.is_empty() {
                write!(f, " and ")?;
            }
        }
        let sep = if self.conjunction { " and " } else { " or " };
        for (i, c) in self.conditions.iter().enumerate() {
            if i > 0 {
                write!(f, "{}", sep)?;
            }
            write!(f, "{}", c)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Merge result types
// ---------------------------------------------------------------------------

/// The result of merging two media queries.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaQueryMergeResult {
    /// A singleton value indicating that there are no contexts that match both
    /// input queries.
    Empty,

    /// A singleton value indicating that the contexts that match both input
    /// queries can't be represented by a Level 3 media query.
    Unrepresentable,

    /// A successful result of merging two media queries.
    Successful(Box<MediaQuerySuccessfulMergeResult>),
}

/// A successful merge result.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaQuerySuccessfulMergeResult {
    /// The merged media query.
    pub query: CssMediaQuery,
}

impl fmt::Display for MediaQuerySuccessfulMergeResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.query)
    }
}

// ---------------------------------------------------------------------------
// Parser (stub — depends on unported parser infrastructure)
// ---------------------------------------------------------------------------

/// Parses a media query from the given contents.
///
/// If passed, `url` is the name of the file from which `contents` comes.
///
/// Matches Dart: `CssMediaQuery.parseList`
pub fn parse_list(contents: &str) -> SassResult<Vec<CssMediaQuery>> {
    let arena = bumpalo::Bump::new();
    let source = FileSource::new_in(&arena, contents, None);
    let mut parser = CssMediaQueryParser::new(source);
    parser.parse().map_err(Into::into)
}

/// Matches Dart: `CssMediaQuery.parseList` with `interpolationMap` — used
/// for `@media` queries built by interpolation, so errors map back to the
/// interpolation site. The returned map borrow ties to the caller's arena.
pub fn parse_list_with_map<'map>(
    arena: &'map bumpalo::Bump,
    contents: &str,
    interpolation_map: Option<&'map InterpolationMap<'map>>,
) -> SassResult<Vec<CssMediaQuery>> {
    let source = FileSource::new_in(arena, contents, None);
    let mut parser = CssMediaQueryParser::new_with_map(source, interpolation_map);
    parser.parse().map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn screen() -> Option<String> {
        Some("screen".to_string())
    }
    fn print() -> Option<String> {
        Some("print".to_string())
    }
    fn all() -> Option<String> {
        Some("all".to_string())
    }
    fn not() -> Option<String> {
        Some("not".to_string())
    }
    fn cond(s: &str) -> Vec<String> {
        vec![s.to_string()]
    }
    fn conds(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    // ---- Constructor tests ----

    #[test]
    fn test_new_type_basic() {
        let q = CssMediaQuery::new_type(screen(), None, vec![]);
        assert!(q.modifier.is_none());
        assert_eq!(q.media_type, screen());
        assert!(q.conjunction);
        assert!(q.conditions.is_empty());
    }

    #[test]
    fn test_new_type_with_modifier() {
        let q = CssMediaQuery::new_type(screen(), Some("only".to_string()), vec![]);
        assert_eq!(q.modifier, Some("only".to_string()));
        assert_eq!(q.media_type, screen());
    }

    #[test]
    fn test_new_type_with_conditions() {
        let q = CssMediaQuery::new_type(screen(), None, conds(&["(min-width: 700px)", "(color)"]));
        assert_eq!(q.conditions.len(), 2);
        assert_eq!(q.conditions[0], "(min-width: 700px)");
        assert_eq!(q.conditions[1], "(color)");
    }

    #[test]
    fn test_new_condition_single() {
        let q = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        assert!(q.modifier.is_none());
        assert!(q.media_type.is_none());
        assert!(q.conjunction);
        assert_eq!(q.conditions, cond("(color)"));
    }

    #[test]
    fn test_new_condition_multi_with_conjunction() {
        let q = CssMediaQuery::new_condition(conds(&["(color)", "(min-width: 700px)"]), Some(true))
            .unwrap();
        assert!(q.conjunction);
        assert_eq!(q.conditions.len(), 2);
    }

    #[test]
    fn test_new_condition_multi_disjunction() {
        let q =
            CssMediaQuery::new_condition(conds(&["(color)", "(monochrome)"]), Some(false)).unwrap();
        assert!(!q.conjunction);
    }

    #[test]
    fn test_new_condition_multi_nil_conjunction_error() {
        let result = CssMediaQuery::new_condition(conds(&["(a)", "(b)"]), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("conjunction may not be null"));
    }

    // ---- MatchesAllTypes tests ----

    #[test]
    fn test_matches_all_types_nil_type() {
        let q = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        assert!(q.matches_all_types());
    }

    #[test]
    fn test_matches_all_types_all_lower() {
        let q = CssMediaQuery::new_type(all(), None, vec![]);
        assert!(q.matches_all_types());
    }

    #[test]
    fn test_matches_all_types_all_upper() {
        let q = CssMediaQuery::new_type(Some("ALL".to_string()), None, vec![]);
        assert!(q.matches_all_types());
    }

    #[test]
    fn test_matches_all_types_all_mixed() {
        let q = CssMediaQuery::new_type(Some("aLl".to_string()), None, vec![]);
        assert!(q.matches_all_types());
    }

    #[test]
    fn test_matches_all_types_screen() {
        let q = CssMediaQuery::new_type(screen(), None, vec![]);
        assert!(!q.matches_all_types());
    }

    // ---- Display tests ----

    #[test]
    fn test_display_type_only() {
        let q = CssMediaQuery::new_type(screen(), None, vec![]);
        assert_eq!(q.to_string(), "screen");
    }

    #[test]
    fn test_display_modifier_type() {
        let q = CssMediaQuery::new_type(screen(), Some("only".to_string()), vec![]);
        assert_eq!(q.to_string(), "only screen");
    }

    #[test]
    fn test_display_type_and_conditions() {
        let q = CssMediaQuery::new_type(screen(), None, conds(&["(min-width: 700px)", "(color)"]));
        assert_eq!(q.to_string(), "screen and (min-width: 700px) and (color)");
    }

    #[test]
    fn test_display_modifier_type_and_conditions() {
        let q = CssMediaQuery::new_type(screen(), not(), cond("(color)"));
        assert_eq!(q.to_string(), "not screen and (color)");
    }

    #[test]
    fn test_display_conditions_only_and() {
        let q = CssMediaQuery::new_condition(conds(&["(min-width: 700px)", "(color)"]), Some(true))
            .unwrap();
        assert_eq!(q.to_string(), "(min-width: 700px) and (color)");
    }

    #[test]
    fn test_display_conditions_only_or() {
        let q =
            CssMediaQuery::new_condition(conds(&["(color)", "(monochrome)"]), Some(false)).unwrap();
        assert_eq!(q.to_string(), "(color) or (monochrome)");
    }

    #[test]
    fn test_display_single_condition() {
        let q = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        assert_eq!(q.to_string(), "(color)");
    }

    #[test]
    fn test_display_empty() {
        let q = CssMediaQuery {
            modifier: None,
            media_type: None,
            conjunction: true,
            conditions: vec![],
        };
        assert_eq!(q.to_string(), "");
    }

    // ---- Equality tests ----

    #[test]
    fn test_eq_identical() {
        let a = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_different_type() {
        let a = CssMediaQuery::new_type(screen(), None, vec![]);
        let b = CssMediaQuery::new_type(print(), None, vec![]);
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_one_nil_type() {
        let a = CssMediaQuery::new_type(screen(), None, vec![]);
        let b = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_both_nil_type() {
        let a = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        let b = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_different_conditions() {
        let a = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        let b = CssMediaQuery::new_condition(cond("(min-width: 700px)"), None).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_different_condition_order() {
        let a = CssMediaQuery::new_type(screen(), None, conds(&["(color)", "(grid)"]));
        let b = CssMediaQuery::new_type(screen(), None, conds(&["(grid)", "(color)"]));
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_ignores_conjunction() {
        let a = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        let b = CssMediaQuery {
            modifier: None,
            media_type: screen(),
            conjunction: false,
            conditions: cond("(color)"),
        };
        assert_eq!(a, b);
    }

    // ---- HashCode tests ----

    fn hash_of(q: &CssMediaQuery) -> u64 {
        let mut h = DefaultHasher::new();
        q.hash(&mut h);
        h.finish()
    }

    #[test]
    fn test_hash_equal_queries() {
        let a = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn test_hash_stable() {
        let q = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        assert_eq!(hash_of(&q), hash_of(&q));
    }

    #[test]
    fn test_hash_ignores_conjunction() {
        // Dart `CssMediaQuery.hashCode` excludes `conjunction`
        // (`modifier.hashCode ^ type.hashCode ^ listHash(conditions)`), so
        // equal queries (== ignores conjunction) must hash equally — the
        // Hash/Eq contract. Previously `hash_code` folded conjunction in.
        let hash_of = |q: &CssMediaQuery| {
            let mut h = DefaultHasher::new();
            q.hash(&mut h);
            h.finish()
        };
        let a = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        let b = CssMediaQuery {
            modifier: None,
            media_type: screen(),
            conjunction: false,
            conditions: cond("(color)"),
        };
        assert_eq!(a, b, "equality ignores conjunction (precondition)");
        assert_eq!(
            hash_of(&a),
            hash_of(&b),
            "Hash must ignore conjunction like PartialEq does"
        );
    }

    // ---- Merge tests ----

    #[test]
    fn test_merge_both_nil_type() {
        let a = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        let b = CssMediaQuery::new_condition(cond("(min-width: 700px)"), None).unwrap();

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert!(merged.query.media_type.is_none());
        assert_eq!(merged.query.conditions.len(), 2);
    }

    #[test]
    fn test_merge_one_not_one_normal_same_type_subset() {
        let a = CssMediaQuery::new_type(screen(), not(), cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, conds(&["(color)", "(grid)"]));

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Empty);
    }

    #[test]
    fn test_merge_one_not_one_normal_same_type_no_subset() {
        let a = CssMediaQuery::new_type(screen(), not(), cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, cond("(grid)"));

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Unrepresentable);
    }

    #[test]
    fn test_merge_one_not_one_normal_both_match_all() {
        let a = CssMediaQuery::new_type(all(), not(), vec![]);
        let b = CssMediaQuery::new_type(screen(), None, vec![]);

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Unrepresentable);
    }

    #[test]
    fn test_merge_one_not_one_normal_different_types() {
        let a = CssMediaQuery::new_type(screen(), not(), vec![]);
        let b = CssMediaQuery::new_type(print(), None, vec![]);

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert!(merged.query.modifier.is_none());
        assert_eq!(merged.query.media_type, print());
    }

    #[test]
    fn test_merge_both_not_same_type_superset() {
        let a = CssMediaQuery::new_type(screen(), not(), conds(&["(color)", "(grid)"]));
        let b = CssMediaQuery::new_type(screen(), not(), cond("(color)"));

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.conditions.len(), 2);
    }

    #[test]
    fn test_merge_both_not_same_type_no_superset() {
        let a = CssMediaQuery::new_type(screen(), not(), cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), not(), cond("(grid)"));

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Unrepresentable);
    }

    #[test]
    fn test_merge_both_not_different_types() {
        let a = CssMediaQuery::new_type(screen(), not(), vec![]);
        let b = CssMediaQuery::new_type(print(), not(), vec![]);

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Unrepresentable);
    }

    #[test]
    fn test_merge_first_matches_all_types() {
        let a = CssMediaQuery::new_type(all(), None, cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, cond("(grid)"));

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.media_type, screen());
        assert_eq!(merged.query.conditions.len(), 2);
    }

    #[test]
    fn test_merge_second_matches_all_types() {
        let a = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        let b = CssMediaQuery::new_type(all(), None, cond("(grid)"));

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.media_type, screen());
    }

    #[test]
    fn test_merge_both_all() {
        let a = CssMediaQuery::new_type(all(), None, cond("(color)"));
        let b = CssMediaQuery::new_type(all(), None, cond("(grid)"));

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.media_type, all());
    }

    #[test]
    fn test_merge_both_all_one_nil_type() {
        let a = CssMediaQuery::new_condition(cond("(color)"), None).unwrap(); // nil type
        let b = CssMediaQuery::new_type(all(), None, cond("(grid)")); // "all" type

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert!(merged.query.media_type.is_none());
    }

    #[test]
    fn test_merge_same_types() {
        let a = CssMediaQuery::new_type(screen(), None, cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, cond("(grid)"));

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.media_type, screen());
        assert_eq!(merged.query.conditions.len(), 2);
    }

    #[test]
    fn test_merge_different_types() {
        let a = CssMediaQuery::new_type(screen(), None, vec![]);
        let b = CssMediaQuery::new_type(print(), None, vec![]);

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Empty);
    }

    #[test]
    fn test_merge_non_conjunction() {
        let a = CssMediaQuery::new_condition(cond("(color)"), None).unwrap();
        let b = CssMediaQuery {
            modifier: None,
            media_type: None,
            conjunction: false,
            conditions: cond("(grid)"),
        };

        let result = a.merge(&b);
        assert_eq!(result, MediaQueryMergeResult::Unrepresentable);
    }

    #[test]
    fn test_merge_case_insensitive_types() {
        let a = CssMediaQuery::new_type(Some("SCREEN".to_string()), None, vec![]);
        let b = CssMediaQuery::new_type(screen(), None, vec![]);

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.media_type, Some("SCREEN".to_string()));
    }

    #[test]
    fn test_merge_case_insensitive_modifier() {
        let a = CssMediaQuery::new_type(screen(), Some("NOT".to_string()), cond("(color)"));
        let b = CssMediaQuery::new_type(screen(), None, vec![]);

        let result = a.merge(&b);
        // NOT is treated as "not" after lowercasing; "(color)" is not subset of empty
        assert_eq!(result, MediaQueryMergeResult::Unrepresentable);
    }

    #[test]
    fn test_merge_both_not_case_insensitive_types() {
        let a = CssMediaQuery::new_type(Some("SCREEN".to_string()), not(), vec![]);
        let b = CssMediaQuery::new_type(screen(), not(), vec![]);

        let result = a.merge(&b);
        let merged = match result {
            MediaQueryMergeResult::Successful(m) => m,
            _ => panic!("expected successful merge, got {:?}", result),
        };
        assert_eq!(merged.query.media_type, Some("SCREEN".to_string()));
    }

    #[test]
    fn test_merge_not_vs_normal_case_insensitive_modifier() {
        let a = CssMediaQuery::new_type(screen(), Some("NOT".to_string()), vec![]);
        let b = CssMediaQuery::new_type(screen(), None, vec![]);

        let result = a.merge(&b);
        // "NOT" lowercased = "not"; both nil conditions → allContained(nil, nil) = true → Empty
        assert_eq!(result, MediaQueryMergeResult::Empty);
    }

    // ---- ParseList tests ----

    #[test]
    fn test_parse_list_valid() {
        let result = parse_list("screen and (min-width: 700px)").unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_parse_list_multiple_queries() {
        let result = parse_list("screen and (color), print").unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_parse_list_invalid() {
        let result = parse_list("not a valid media query!!!");
        assert!(result.is_err());
    }
}
