// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/functions.dart
// go-source: go/value/selector_extend.go

use crate::common::file_span::BOGUS_SPAN;
use crate::selector::placeholder::PlaceholderSelector;
use crate::selector::universal::UniversalSelector;
use crate::selector::Selector;
use std::collections::{HashSet, VecDeque};

use crate::common::ast_css_value::CssValue;
use crate::common::exception::{SassError, SassResult};
use crate::common::file_span::FileSpan;
use crate::selector::combinator::Combinator;
use crate::selector::complex::ComplexSelector;
use crate::selector::complex_component::ComplexSelectorComponent;
use crate::selector::compound::CompoundSelector;
use crate::selector::list::SelectorList;
use crate::selector::pseudo::PseudoSelector;
use crate::selector::SimpleSelector;

/// Pseudo-classes that can only meaningfully appear in the first component
/// of a complex selector. Matches Dart: `_rootishPseudoClasses`
/// (extend/functions.dart).
static ROOTISH_PSEUDO_CLASSES: &[&str] = &["root", "scope", "host", "host-context"];

// --- UnifyComplex ---

/// Returns the contents of a selector list matching only elements matched by
/// every complex selector in `complexes`.
///
/// `span` is used for the unified selectors. Returns `None` when no such
/// list exists. Matches Dart: `unifyComplex` (extend/functions.dart), except
/// Dart returns an empty list where this port returns `None`.
pub fn unify_complex<'parse>(
    complexes: &[ComplexSelector<'parse>],
    span: FileSpan<'parse>,
) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
    if complexes.len() == 1 {
        return Ok(Some(complexes.to_vec()));
    }

    let mut unified_base: Option<CompoundSelector<'parse>> = None;
    let mut leading_combinator: Option<CssValue<'parse, Combinator>> = None;
    let mut trailing_combinator: Option<CssValue<'parse, Combinator>> = None;

    for complex in complexes {
        if complex.is_useless() {
            return Ok(None);
        }
        if complex.leading_combinators.len() == 1 && complex.components.len() == 1 {
            let lc = &complex.leading_combinators[0];
            if leading_combinator.is_none() {
                leading_combinator = Some(lc.clone());
            } else if leading_combinator.as_ref() != Some(lc) {
                return Ok(None);
            }
        }
        let base = &complex.components[complex.components.len() - 1];
        if base.combinators.len() == 1 {
            let tc = &base.combinators[0];
            if trailing_combinator.is_some() && trailing_combinator.as_ref() != Some(tc) {
                return Ok(None);
            }
            trailing_combinator = Some(tc.clone());
        }
        if unified_base.is_none() {
            unified_base = Some(base.selector.as_ref().clone());
        } else {
            let result = unify_compound(unified_base.as_ref().unwrap(), base.selector.as_ref())?;
            if result.is_none() {
                return Ok(None);
            }
            unified_base = result;
        }
    }

    let mut without_bases: Vec<ComplexSelector<'parse>> = Vec::new();
    for complex in complexes {
        if complex.components.len() > 1 {
            let complex_span = complex.span()?;
            let cs = ComplexSelector::new(
                complex.leading_combinators.clone(),
                complex.components[..complex.components.len() - 1].to_vec(),
                complex_span,
                complex.line_break,
            )?;
            without_bases.push(cs);
        }
    }

    let base_lcs = if let Some(ref lc) = leading_combinator {
        vec![lc.clone()]
    } else {
        vec![]
    };
    let base_tcs = if let Some(ref tc) = trailing_combinator {
        vec![tc.clone()]
    } else {
        vec![]
    };
    let base_component =
        ComplexSelectorComponent::new(Box::new(unified_base.unwrap()), base_tcs, span);
    let mut base = ComplexSelector::new(base_lcs, vec![base_component], span, false)?;
    for c in complexes {
        if c.line_break {
            base.line_break = true;
            break;
        }
    }

    let result = if without_bases.is_empty() {
        weave(&[base], span, None)?
    } else {
        let last = without_bases[without_bases.len() - 1].concatenate(&base, span, false);
        let mut combined = without_bases[..without_bases.len() - 1].to_vec();
        combined.push(last);
        weave(&combined, span, None)?
    };

    Ok(result)
}

// --- unifyCompound ---

/// Returns a compound matching only elements matched by both `compound1` and
/// `compound2`, or `None` when unification is impossible.
///
/// `compound1`'s span is used for the result. Pseudo-classes after a
/// pseudo-element unify separately so their relative order with the
/// pseudo-element is preserved; otherwise pseudo-classes sort before
/// pseudo-elements without disturbing an originally interleaved order.
/// Matches Dart: `unifyCompound` (extend/functions.dart).
fn unify_compound<'parse>(
    compound1: &CompoundSelector<'parse>,
    compound2: &CompoundSelector<'parse>,
) -> SassResult<Option<CompoundSelector<'parse>>> {
    let mut result = compound1.components.clone();
    let mut pseudo_result: Vec<SimpleSelector<'parse>> = Vec::new();
    let mut pseudo_element_found = false;

    for simple in &compound2.components {
        if pseudo_element_found {
            if let SimpleSelector::Pseudo(ref ps) = simple {
                let unified = ps.unify(&pseudo_result)?;
                if unified.is_none() {
                    return Ok(None);
                }
                pseudo_result = unified.unwrap();
                continue;
            }
        }
        if let SimpleSelector::Pseudo(ref ps) = simple {
            if ps.is_element() {
                pseudo_element_found = true;
            }
        }
        let unified = simple.unify(&result)?;
        if unified.is_none() {
            return Ok(None);
        }
        result = unified.unwrap();
    }

    let mut all = Vec::with_capacity(result.len() + pseudo_result.len());
    all.extend(result);
    all.extend(pseudo_result);
    let span = compound1.span()?;
    Ok(Some(CompoundSelector::new(all, span)?))
}

// --- Weave ---

/// Expands "parenthesized selectors" in `complexes`.
///
/// Conceptually `.D (.A .B)` (represented as the list `[.D, .A .B]`)
/// becomes `.D .A .B, .A .D .B`. Fully merged selectors are deliberately
/// omitted: they would cause exponential output for little gain. `span` is
/// used for new selectors; `force_line_break` marks every returned complex
/// as having a line break. Returns `None` when the intersection is empty
/// (Dart returns an empty list instead). Matches Dart: `weave`
/// (extend/functions.dart).
pub fn weave<'parse>(
    complexes: &[ComplexSelector<'parse>],
    span: FileSpan<'parse>,
    force_line_break: Option<bool>,
) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
    let fb = force_line_break.unwrap_or(false);
    if complexes.len() == 1 {
        let c = &complexes[0];
        if !fb || c.line_break {
            return Ok(Some(complexes.to_vec()));
        }
        let cs = ComplexSelector::new(
            c.leading_combinators.clone(),
            c.components.clone(),
            c.span()?,
            true,
        )?;
        return Ok(Some(vec![cs]));
    }

    let mut prefixes: Vec<ComplexSelector<'parse>> = vec![complexes[0].clone()];
    for c in &complexes[1..] {
        if c.components.len() == 1 {
            for p in &mut prefixes {
                *p = p.concatenate(c, span, fb);
            }
            continue;
        }
        let mut np: Vec<ComplexSelector<'parse>> = Vec::new();
        for p in &prefixes {
            if let Some(pp) = weave_parents(p, c, span)? {
                for ppc in pp {
                    np.push(ppc.with_additional_component(
                        &c.components[c.components.len() - 1],
                        span,
                        fb,
                    ));
                }
            }
        }
        prefixes = np;
        if prefixes.is_empty() {
            return Ok(None);
        }
    }
    Ok(Some(prefixes))
}

// --- weaveParents ---

/// Interweaves `prefix`'s components with `base`'s components other than the
/// last.
///
/// Returns all orderings (including unifications) that preserve each input's
/// relative order — e.g. `.foo .bar` with `.baz .bang div` yields `.foo .bar
/// .baz .bang div`, `.foo .bar.baz .bang div`, and so on. Semantically the
/// union of the results matches the intersection of `C` with the descendants
/// of `P`, with some results elided to bound output size. Returns `None`
/// when the intersection is empty. Matches Dart: `_weaveParents`
/// (extend/functions.dart).
fn weave_parents<'parse>(
    prefix: &ComplexSelector<'parse>,
    base: &ComplexSelector<'parse>,
    span: FileSpan<'parse>,
) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
    let leading = merge_leading_combinators(&prefix.leading_combinators, &base.leading_combinators);
    if leading.is_none() {
        return Ok(None);
    }
    let leading = leading.unwrap();

    let mut q1: VecDeque<ComplexSelectorComponent<'parse>> =
        VecDeque::from(prefix.components.clone());
    let mut q2: VecDeque<ComplexSelectorComponent<'parse>> =
        VecDeque::from(base.components[..base.components.len() - 1].to_vec());

    let trailing_result = merge_trailing_combinators(&mut q1, &mut q2, span)?;
    if trailing_result.is_none() {
        return Ok(None);
    }
    let trailing_result = trailing_result.unwrap();

    match (first_if_rootish(&mut q1), first_if_rootish(&mut q2)) {
        (Some(r1), Some(r2)) => {
            let rootish = unify_compound(r1.selector.as_ref(), r2.selector.as_ref())?;
            if rootish.is_none() {
                return Ok(None);
            }
            let rootish = rootish.unwrap();
            q1.push_front(ComplexSelectorComponent::new(
                Box::new(rootish.clone()),
                r1.combinators.clone(),
                r1.span,
            ));
            q2.push_front(ComplexSelectorComponent::new(
                Box::new(rootish),
                r2.combinators.clone(),
                r1.span,
            ));
        }
        (Some(r1), None) => {
            q1.push_front(r1.clone());
            q2.push_front(r1);
        }
        (None, Some(r2)) => {
            q1.push_front(r2.clone());
            q2.push_front(r2);
        }
        (None, None) => {}
    }

    let mut g1 = group_selectors(&q1);
    let mut g2 = group_selectors(&q2);
    let lcs = lcs_fn(
        g2.make_contiguous(),
        g1.make_contiguous(),
        &|a: &Vec<ComplexSelectorComponent<'parse>>,
          b: &Vec<ComplexSelectorComponent<'parse>>|
         -> SassResult<Option<Vec<ComplexSelectorComponent<'parse>>>> {
            if a == b {
                return Ok(Some(a.clone()));
            }
            if complex_is_parent_superselector(a, b)? {
                return Ok(Some(b.clone()));
            }
            if complex_is_parent_superselector(b, a)? {
                return Ok(Some(a.clone()));
            }
            if !must_unify(a, b) {
                return Ok(None);
            }
            let unified = unify_complex(
                &[
                    ComplexSelector::new(vec![], a.clone(), span, false)?,
                    ComplexSelector::new(vec![], b.clone(), span, false)?,
                ],
                span,
            )?;
            if let Some(unified) = unified {
                if unified.len() == 1 && !unified[0].components.is_empty() {
                    return Ok(Some(unified[0].components.clone()));
                }
            }
            Ok(None)
        },
    )?;

    let mut choices: Vec<Vec<Vec<ComplexSelectorComponent<'parse>>>> = Vec::new();
    for group in lcs.iter().map(|v| v.to_vec()) {
        let chunk = chunks(&mut g1, &mut g2, &|q: &VecDeque<
            Vec<ComplexSelectorComponent<'parse>>,
        >| {
            if q.is_empty() {
                return Ok(true);
            }
            complex_is_parent_superselector(q.front().unwrap(), &group)
        })?;
        let mut position_choices: Vec<Vec<ComplexSelectorComponent<'parse>>> = Vec::new();
        for ordering in &chunk {
            let mut flat = Vec::new();
            for g in ordering {
                flat.extend_from_slice(g);
            }
            position_choices.push(flat);
        }
        if !position_choices.is_empty() {
            choices.push(position_choices);
        }
        let g: Vec<Vec<ComplexSelectorComponent<'parse>>> = vec![group.to_vec(); 1];
        choices.push(g);
        if !g1.is_empty() {
            g1.pop_front();
        }
        if !g2.is_empty() {
            g2.pop_front();
        }
    }

    let last_chunk = chunks(&mut g1, &mut g2, &|q: &VecDeque<
        Vec<ComplexSelectorComponent<'parse>>,
    >| { Ok(q.is_empty()) })?;
    let mut last_position_choices: Vec<Vec<ComplexSelectorComponent<'parse>>> = Vec::new();
    for ordering in &last_chunk {
        let mut flat = Vec::new();
        for g in ordering {
            flat.extend_from_slice(g);
        }
        last_position_choices.push(flat);
    }
    if !last_position_choices.is_empty() {
        choices.push(last_position_choices);
    }

    choices.extend(trailing_result);

    let mut result: Vec<ComplexSelector<'parse>> = Vec::new();
    let non_empty: Vec<&Vec<Vec<ComplexSelectorComponent<'parse>>>> =
        choices.iter().filter(|c| !c.is_empty()).collect();
    for path in paths(&non_empty) {
        if path.is_empty() {
            continue;
        }
        let mut comps: Vec<ComplexSelectorComponent<'parse>> = Vec::new();
        for g in path {
            comps.push(g.clone());
        }
        let cs = ComplexSelector::new(
            leading.clone(),
            comps,
            span,
            prefix.line_break || base.line_break,
        )?;
        result.push(cs);
    }
    Ok(if result.is_empty() {
        None
    } else {
        Some(result)
    })
}

// --- firstIfRootish ---

/// Removes and returns the first element of `q` when its selector is rootish
/// (`:root`, `:scope`, `:host`, `:host-context`) and so may only appear in a
/// complex selector's first component. Matches Dart: `_firstIfRootish`
/// (extend/functions.dart).
fn first_if_rootish<'parse>(
    q: &mut VecDeque<ComplexSelectorComponent<'parse>>,
) -> Option<ComplexSelectorComponent<'parse>> {
    if let Some(first) = q.front() {
        for s in &first.selector.components {
            if let SimpleSelector::Pseudo(ref ps) = s {
                if ps.is_class && ROOTISH_PSEUDO_CLASSES.contains(&ps.normalized_name.as_str()) {
                    return q.pop_front();
                }
            }
        }
    }
    None
}

// --- mergeLeadingCombinators ---

/// Returns a leading-combinator list compatible with both `c1` and `c2`, or
/// `None` when they cannot be unified. Matches Dart:
/// `_mergeLeadingCombinators` (extend/functions.dart).
fn merge_leading_combinators<'parse>(
    c1: &[CssValue<'parse, Combinator>],
    c2: &[CssValue<'parse, Combinator>],
) -> Option<Vec<CssValue<'parse, Combinator>>> {
    if c1.len() > 1 || c2.len() > 1 {
        return None;
    }
    if c1.is_empty() {
        if c2.is_empty() {
            return Some(vec![]);
        }
        return Some(c2.to_vec());
    }
    if c2.is_empty() {
        return Some(c1.to_vec());
    }
    if c1[0] == c2[0] {
        return Some(c1.to_vec());
    }
    None
}

// --- mergeTrailingCombinators ---

/// Merges trailing combinators off the ends of `c1` and `c2` into a choice
/// list.
///
/// Each returned element is the set of choices for one complex-selector
/// position; every path through the choices matches the required elements.
/// Returns an empty vec when there is nothing to merge, `None` when the
/// sequences are incompatible. The sibling/child special cases (cases 1–6 in
/// the body) encode how each combinator pair constrains the merge. Matches
/// Dart: `_mergeTrailingCombinators` (extend/functions.dart).
fn merge_trailing_combinators<'parse>(
    c1: &mut VecDeque<ComplexSelectorComponent<'parse>>,
    c2: &mut VecDeque<ComplexSelectorComponent<'parse>>,
    span: FileSpan<'parse>,
) -> SassResult<Option<Vec<Vec<Vec<ComplexSelectorComponent<'parse>>>>>> {
    let mut stack: Vec<Vec<Vec<ComplexSelectorComponent<'parse>>>> = Vec::new();

    loop {
        let comb1 = if let Some(last) = c1.back() {
            last.combinators.clone()
        } else {
            vec![]
        };
        let comb2 = if let Some(last) = c2.back() {
            last.combinators.clone()
        } else {
            vec![]
        };
        if comb1.is_empty() && comb2.is_empty() {
            break;
        }
        if comb1.len() > 1 || comb2.len() > 1 {
            return Ok(None);
        }

        let has1 = !comb1.is_empty();
        let has2 = !comb2.is_empty();
        let v1 = comb1.first().map(|cv| cv.value);
        let v2 = comb2.first().map(|cv| cv.value);

        match (has1, v1, has2, v2) {
            // Case 1: both FollowingSibling
            (
                true,
                Some(Combinator::FollowingSibling),
                true,
                Some(Combinator::FollowingSibling),
            ) => {
                let comp1 = c1.pop_back().unwrap();
                let comp2 = c2.pop_back().unwrap();
                let ok = comp1.selector.is_superselector(comp2.selector.as_ref())?;
                if ok {
                    stack.push(vec![vec![comp2]]);
                } else {
                    let ok = comp2.selector.is_superselector(comp1.selector.as_ref())?;
                    if ok {
                        stack.push(vec![vec![comp1]]);
                    } else {
                        let mut choices = vec![
                            vec![comp1.clone(), comp2.clone()],
                            vec![comp2.clone(), comp1.clone()],
                        ];
                        let unified =
                            unify_compound(comp1.selector.as_ref(), comp2.selector.as_ref())?;
                        if let Some(u) = unified {
                            choices.push(vec![ComplexSelectorComponent::new(
                                Box::new(u),
                                comb1.clone(),
                                span,
                            )]);
                        }
                        stack.push(choices);
                    }
                }
            }

            // Case 2: FollowingSibling + NextSibling (either order)
            (true, Some(Combinator::FollowingSibling), true, Some(Combinator::NextSibling))
            | (true, Some(Combinator::NextSibling), true, Some(Combinator::FollowingSibling)) => {
                let (nc, fc) = if v1 == Some(Combinator::NextSibling) {
                    (&mut *c1, &mut *c2)
                } else {
                    (&mut *c2, &mut *c1)
                };
                let next = nc.pop_back().unwrap();
                let following = fc.pop_back().unwrap();
                let ok = following
                    .selector
                    .is_superselector(next.selector.as_ref())?;
                if ok {
                    stack.push(vec![vec![next]]);
                } else {
                    let mut choices = vec![vec![following.clone(), next.clone()]];
                    let unified =
                        unify_compound(following.selector.as_ref(), next.selector.as_ref())?;
                    if let Some(u) = unified {
                        choices.push(vec![ComplexSelectorComponent::new(
                            Box::new(u),
                            next.combinators.clone(),
                            span,
                        )]);
                    }
                    stack.push(choices);
                }
            }

            // Case 3: Child + NextSibling/FollowingSibling (either order) — keep only sibling
            (true, Some(Combinator::Child), true, Some(c))
                if c == Combinator::NextSibling || c == Combinator::FollowingSibling =>
            {
                stack.push(vec![vec![c2.pop_back().unwrap()]]);
            }
            (true, Some(c), true, Some(Combinator::Child))
                if c == Combinator::NextSibling || c == Combinator::FollowingSibling =>
            {
                stack.push(vec![vec![c1.pop_back().unwrap()]]);
            }

            // Case 4: same combinator
            (true, Some(a), true, Some(b)) if a == b => {
                let unified = unify_compound(
                    c1.pop_back().unwrap().selector.as_ref(),
                    c2.pop_back().unwrap().selector.as_ref(),
                )?;
                if unified.is_none() {
                    return Ok(None);
                }
                stack.push(vec![vec![ComplexSelectorComponent::new(
                    Box::new(unified.unwrap()),
                    comb1,
                    span,
                )]]);
            }

            // Case 5: combinator on left side only
            (true, Some(_), false, None) => {
                let combinator = v1.unwrap();
                if combinator == Combinator::Child && !c2.is_empty() {
                    let ok = c2
                        .back()
                        .unwrap()
                        .selector
                        .is_superselector(c1.back().unwrap().selector.as_ref())?;
                    if ok {
                        c2.pop_back();
                    }
                }
                stack.push(vec![vec![c1.pop_back().unwrap()]]);
            }

            // Case 6: combinator on right side only
            (false, None, true, Some(_)) => {
                let combinator = v2.unwrap();
                if combinator == Combinator::Child && !c1.is_empty() {
                    let ok = c1
                        .back()
                        .unwrap()
                        .selector
                        .is_superselector(c2.back().unwrap().selector.as_ref())?;
                    if ok {
                        c1.pop_back();
                    }
                }
                stack.push(vec![vec![c2.pop_back().unwrap()]]);
            }

            _ => return Ok(None),
        }
    }

    stack.reverse();
    Ok(Some(stack))
}

// --- groupSelectors ---

/// Groups `comps` into the longest sub-lists whose only combinator-free
/// component is the last — e.g. `(A B > C D + E ~ G)` becomes
/// `[(A) (B > C) (D + E ~ G)]`. Matches Dart: `_groupSelectors`
/// (extend/functions.dart).
fn group_selectors<'parse>(
    comps: &VecDeque<ComplexSelectorComponent<'parse>>,
) -> VecDeque<Vec<ComplexSelectorComponent<'parse>>> {
    let mut groups: VecDeque<Vec<ComplexSelectorComponent<'parse>>> = VecDeque::new();
    let mut g: Vec<ComplexSelectorComponent<'parse>> = Vec::new();
    for c in comps.iter() {
        g.push(c.clone());
        if c.combinators.is_empty() {
            groups.push_back(g);
            g = Vec::new();
        }
    }
    if !g.is_empty() {
        groups.push_back(g);
    }
    groups
}

// --- chunks ---

/// Returns all orderings of initial subsequences of `q1` and `q2`, draining
/// them up to the `done` boundary.
///
/// For example `(A B C | D E)` with `(1 2 | 3 4 5)` yields
/// `[(A B C 1 2), (1 2 A B C)]`, leaving `(D E)` and `(3 4 5)` in the queues.
/// Matches Dart: `_chunks` (extend/functions.dart).
fn chunks<T: Clone>(
    q1: &mut VecDeque<T>,
    q2: &mut VecDeque<T>,
    done: &dyn Fn(&VecDeque<T>) -> SassResult<bool>,
) -> SassResult<Vec<Vec<T>>> {
    let mut c1: Vec<T> = Vec::new();
    loop {
        if done(q1)? {
            break;
        }
        c1.push(q1.pop_front().unwrap());
    }
    let mut c2: Vec<T> = Vec::new();
    loop {
        if done(q2)? {
            break;
        }
        c2.push(q2.pop_front().unwrap());
    }
    match (c1.is_empty(), c2.is_empty()) {
        (true, true) => Ok(vec![]),
        (true, _) => Ok(vec![c2]),
        (_, true) => Ok(vec![c1]),
        _ => {
            let mut a = Vec::with_capacity(c1.len() + c2.len());
            a.extend(c1.clone());
            a.extend(c2.clone());
            let mut b = Vec::with_capacity(c1.len() + c2.len());
            b.extend(c2);
            b.extend(c1);
            Ok(vec![a, b])
        }
    }
}

// --- Paths ---

/// Returns every path through `choices`: e.g. `[[1, 2], [3, 4], [5]]`
/// becomes `[[1, 3, 5], [2, 3, 5], [1, 4, 5], [2, 4, 5]]`.
/// Matches Dart: `paths` (extend/functions.dart).
fn paths<T: Clone>(choices: &[&Vec<Vec<T>>]) -> Vec<Vec<T>> {
    let mut paths: Vec<Vec<T>> = vec![vec![]];
    for choice in choices {
        let mut np: Vec<Vec<T>> = Vec::new();
        for opt in *choice {
            for p in &paths {
                let mut n = p.clone();
                n.extend_from_slice(opt);
                np.push(n);
            }
        }
        paths = np;
    }
    paths
}

// --- complexIsParentSuperselector ---

/// Like [`complex_is_superselector`], but compares `a` and `b` as though
/// they shared an implicit base selector: `B` is not a superselector of
/// `B A`, but it *is* a parent superselector since `B X` is a superselector
/// of `B A X`. Matches Dart: `_complexIsParentSuperselector`
/// (extend/functions.dart).
fn complex_is_parent_superselector<'parse>(
    a: &[ComplexSelectorComponent<'parse>],
    b: &[ComplexSelectorComponent<'parse>],
) -> SassResult<bool> {
    if a.len() > b.len() {
        return Ok(false);
    }
    let temp_compound = CompoundSelector::new(
        vec![SimpleSelector::Placeholder(PlaceholderSelector::new(
            "<temp>".into(),
            BOGUS_SPAN,
        ))],
        BOGUS_SPAN,
    )?;
    let base = ComplexSelectorComponent::new(Box::new(temp_compound), vec![], BOGUS_SPAN);
    let mut x: Vec<ComplexSelectorComponent<'parse>> = a.to_vec();
    x.push(base.clone());
    let mut y: Vec<ComplexSelectorComponent<'parse>> = b.to_vec();
    y.push(base);
    complex_is_superselector(&x, &y)
}

// --- mustUnify ---

/// Whether `a` and `b` must be unified to combine validly: both contain the
/// same unique simple selector (an ID or pseudo-element). Matches Dart:
/// `_mustUnify` (extend/functions.dart).
// `SimpleSelector` keys carry span memo `Cell`s excluded from `Eq`/`Hash`;
// the lint is a false positive by design.
#[allow(clippy::mutable_key_type)]
fn must_unify<'parse>(
    a: &'parse [ComplexSelectorComponent<'parse>],
    b: &'parse [ComplexSelectorComponent<'parse>],
) -> bool {
    let mut uniq: HashSet<&'parse SimpleSelector<'parse>> = HashSet::new();
    for c in a {
        for s in &c.selector.components {
            if is_unique(s) {
                uniq.insert(s);
            }
        }
    }
    if uniq.is_empty() {
        return false;
    }
    for c in b {
        for s in &c.selector.components {
            if is_unique(s) && uniq.contains(s) {
                return true;
            }
        }
    }
    false
}

/// Whether a compound may contain only one simple selector of the same kind
/// as `s`: IDs and pseudo-elements. Matches Dart: `_isUnique`
/// (extend/functions.dart).
fn is_unique(s: &SimpleSelector<'_>) -> bool {
    matches!(s, SimpleSelector::Id(_))
        || matches!(s, SimpleSelector::Pseudo(ref p) if p.is_element())
}

// --- lcsFn ---

// Longest-common-subsequence over `list1`/`list2` using `select` to score a
// pair (a `Some` value extends the subsequence). Matches Dart's
// `longestCommonSubsequence` from `package:collection` as used by
// `_weaveParents` — including the argument order `(groups2, groups1)`.
fn lcs_fn<T: Clone>(
    list1: &[T],
    list2: &[T],
    select: &dyn Fn(&T, &T) -> SassResult<Option<T>>,
) -> SassResult<Vec<T>> {
    let m = list1.len();
    let n = list2.len();

    let mut lengths: Vec<Vec<i32>> = vec![vec![0; n + 1]; m + 1];
    let mut selections: Vec<Vec<Option<T>>> = {
        let inner = vec![None; n];
        vec![inner; m]
    };

    for i in 0..m {
        for j in 0..n {
            selections[i][j] = select(&list1[i], &list2[j])?;
            if selections[i][j].is_some() {
                lengths[i + 1][j + 1] = lengths[i][j] + 1;
            } else {
                lengths[i + 1][j + 1] = std::cmp::max(lengths[i + 1][j], lengths[i][j + 1]);
            }
        }
    }

    Ok(backtrack(0, m, n, &lengths, &selections))
}

/// Reconstructs the subsequence from the `lcs_fn` length/selection tables.
/// Plain recursion helper; no Dart counterpart (internal to the port of
/// `longestCommonSubsequence`).
fn backtrack<T: Clone>(
    len_offset: usize,
    i: usize,
    j: usize,
    lengths: &[Vec<i32>],
    selections: &[Vec<Option<T>>],
) -> Vec<T> {
    if i == 0 || j == 0 {
        return vec![];
    }
    let idx_i = i - 1;
    let idx_j = j - 1;
    if let Some(ref val) = selections[idx_i][idx_j] {
        let mut result = backtrack(len_offset, idx_i, idx_j, lengths, selections);
        result.push(val.clone());
        return result;
    }
    let len_i = i + len_offset;
    let len_j = j + len_offset;
    if lengths[len_i][len_j - 1] > lengths[len_i - 1][len_j] {
        backtrack(len_offset, i, j - 1, lengths, selections)
    } else {
        backtrack(len_offset, i - 1, j, lengths, selections)
    }
}

// --- ListIsSuperselector ---

/// Whether `a` matches every element `b` matches, plus possibly more.
/// Matches Dart: `listIsSuperselector` (extend/functions.dart).
pub fn list_is_superselector<'parse>(
    a: &[ComplexSelector<'parse>],
    b: &[ComplexSelector<'parse>],
) -> SassResult<bool> {
    for cb in b {
        let mut found = false;
        for ca in a {
            if ca.is_superselector(cb)? {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

// --- ComplexIsSuperselector ---

/// Whether `a` matches every element `b` matches, plus possibly more.
///
/// Selectors with trailing combinators are neither superselectors nor
/// subselectors; a longer `a` is never a superselector of a shorter `b`.
/// Matches Dart: `complexIsSuperselector` (extend/functions.dart).
pub fn complex_is_superselector<'parse>(
    a: &[ComplexSelectorComponent<'parse>],
    b: &[ComplexSelectorComponent<'parse>],
) -> SassResult<bool> {
    if a.is_empty() || b.is_empty() {
        return Ok(false);
    }
    if !a.last().unwrap().combinators.is_empty() {
        return Ok(false);
    }
    if !b.last().unwrap().combinators.is_empty() {
        return Ok(false);
    }

    let mut i1: usize = 0;
    let mut i2: usize = 0;
    let mut prev: Option<CssValue<'parse, Combinator>> = None;

    loop {
        let r1 = a.len() - i1;
        let r2 = b.len() - i2;
        if r1 == 0 || r2 == 0 {
            return Ok(false);
        }
        if r1 > r2 {
            return Ok(false);
        }
        let ca = &a[i1];
        if ca.combinators.len() > 1 {
            return Ok(false);
        }
        if r1 == 1 {
            if any_multi_combinator(&b[i2..]) {
                return Ok(false);
            }
            let parents = if ca.selector.has_complicated_superselector_semantics() {
                Some(&b[i2..b.len() - 1])
            } else {
                None
            };
            return compound_is_superselector(
                ca.selector.as_ref(),
                b.last().unwrap().selector.as_ref(),
                parents,
            );
        }

        let mut eos = i2;
        loop {
            let cb = &b[eos];
            if cb.combinators.len() > 1 {
                return Ok(false);
            }
            let parents = if ca.selector.has_complicated_superselector_semantics() {
                Some(&b[i2..eos])
            } else {
                None
            };
            let ok =
                compound_is_superselector(ca.selector.as_ref(), cb.selector.as_ref(), parents)?;
            if ok {
                break;
            }
            eos += 1;
            if eos == b.len() - 1 {
                return Ok(false);
            }
        }

        if !compat_prev_combinator(prev.as_ref(), &b[i2..eos]) {
            return Ok(false);
        }

        let cb = &b[eos];
        let comb_a = ca.combinators.first();
        let comb_b = cb.combinators.first();
        if !is_supercombinator(comb_a, comb_b) {
            return Ok(false);
        }

        i1 += 1;
        i2 = eos + 1;
        prev = comb_a.cloned();

        if a.len() - i1 == 1 {
            if let Some(ca_comb) = comb_a {
                if ca_comb.value == Combinator::FollowingSibling {
                    for c in &b[i2..b.len() - 1] {
                        let c2 = c.combinators.first();
                        if !is_supercombinator(comb_a, c2) {
                            return Ok(false);
                        }
                    }
                } else if b.len() - i2 > 1 {
                    return Ok(false);
                }
            }
        }
    }
}

/// Whether any component in `comps` carries more than one combinator.
// Such selectors are never superselectors. Inline helper in
// [`complex_is_superselector`].
fn any_multi_combinator<'parse>(comps: &[ComplexSelectorComponent<'parse>]) -> bool {
    comps.iter().any(|c| c.combinators.len() > 1)
}

/// Whether `parents` are valid interstitial components after a complex
/// superselector joined by `prev`: with no parents or no previous combinator
/// this is trivially true; otherwise only `~` allows intermediates, and only
/// sibling (`~`/`+`) ones. Matches Dart: `_compatibleWithPreviousCombinator`
/// (extend/functions.dart).
fn compat_prev_combinator<'parse>(
    prev: Option<&CssValue<'parse, Combinator>>,
    parents: &[ComplexSelectorComponent<'parse>],
) -> bool {
    if parents.is_empty() || prev.is_none() {
        return true;
    }
    let prev = prev.unwrap();
    if prev.value != Combinator::FollowingSibling {
        return false;
    }
    parents.iter().all(|p| {
        if p.combinators.is_empty() {
            return false;
        }
        let v = p.combinators[0].value;
        matches!(v, Combinator::FollowingSibling | Combinator::NextSibling)
    })
}

/// Whether `X a Y` is a superselector of `X b Y`: equal combinators, an
/// absent `a` over child `b`, or following-sibling `a` over next-sibling `b`.
/// Matches Dart: `_isSupercombinator` (extend/functions.dart).
fn is_supercombinator<'parse>(
    a: Option<&CssValue<'parse, Combinator>>,
    b: Option<&CssValue<'parse, Combinator>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) if a == b => true,
        (None, Some(b)) if b.value == Combinator::Child => true,
        (Some(a), Some(b))
            if a.value == Combinator::FollowingSibling && b.value == Combinator::NextSibling =>
        {
            true
        }
        _ => false,
    }
}

// --- CompoundIsSuperselector ---

/// Whether `a` matches every element `b` matches, plus possibly more.
///
/// `parents`, when passed, are the parents of `b` — needed for pseudos with
/// selector arguments. Pseudo-elements change the compound's target rather
/// than narrowing it, so when either side has one both must share the same
/// pseudo-element with matching sides. Matches Dart:
/// `compoundIsSuperselector` (extend/functions.dart).
pub fn compound_is_superselector<'parse>(
    a: &CompoundSelector<'parse>,
    b: &CompoundSelector<'parse>,
    parents: Option<&[ComplexSelectorComponent<'parse>]>,
) -> SassResult<bool> {
    if !a.has_complicated_superselector_semantics() && !b.has_complicated_superselector_semantics()
    {
        if a.components.len() > b.components.len() {
            return Ok(false);
        }
        for sa in &a.components {
            let mut found = false;
            for sb in &b.components {
                if sa.is_superselector(sb)? {
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    let pa = find_pseudo_element(a);
    let pb = find_pseudo_element(b);
    if let (Some((ref pseudo_a, idx_a)), Some((ref pseudo_b, idx_b))) = (&pa, &pb) {
        if !comp_comps_is_superselector(
            a.components[..*idx_a].to_vec(),
            &b.components[..*idx_b],
            parents,
        )? {
            return Ok(false);
        }
        if !SimpleSelector::Pseudo((*pseudo_a).clone())
            .is_superselector(&SimpleSelector::Pseudo((*pseudo_b).clone()))?
        {
            return Ok(false);
        }
        return comp_comps_is_superselector(
            a.components[*idx_a + 1..].to_vec(),
            &b.components[*idx_b + 1..],
            parents,
        );
    }
    if pa.is_some() || pb.is_some() {
        return Ok(false);
    }

    for sa in &a.components {
        if let SimpleSelector::Pseudo(ref ps) = sa {
            if ps.selector.is_some() {
                if !selector_pseudo_is_superselector(ps, b, parents)? {
                    return Ok(false);
                }
                continue;
            }
        }
        if !comp_contains(&b.components, sa)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// If `c` contains a pseudo-element, returns it and its index in
/// `c.components`. Matches Dart: `_findPseudoElementIndexed`
/// (extend/functions.dart).
fn find_pseudo_element<'parse>(
    c: &CompoundSelector<'parse>,
) -> Option<(PseudoSelector<'parse>, usize)> {
    for (i, s) in c.components.iter().enumerate() {
        if let SimpleSelector::Pseudo(ref ps) = s {
            if ps.is_element() {
                return Some((ps.clone(), i));
            }
        }
    }
    None
}

/// Like [`compound_is_superselector`] on raw simple-selector slices; an
/// empty `a` is trivially true and an empty `b` stands in for `*`.
/// Matches Dart: `_compoundComponentsIsSuperselector`
/// (extend/functions.dart).
fn comp_comps_is_superselector<'parse>(
    a: Vec<SimpleSelector<'parse>>,
    b: &[SimpleSelector<'parse>],
    parents: Option<&[ComplexSelectorComponent<'parse>]>,
) -> SassResult<bool> {
    if a.is_empty() {
        return Ok(true);
    }
    let b_sel = if b.is_empty() {
        vec![SimpleSelector::Universal(UniversalSelector::new(
            BOGUS_SPAN,
            Some("*".into()),
        ))]
    } else {
        b.to_vec()
    };
    let ca = CompoundSelector::new(a, BOGUS_SPAN)?;
    let cb = CompoundSelector::new(b_sel, BOGUS_SPAN)?;
    compound_is_superselector(&ca, &cb, parents)
}

/// Whether any selector in `comps` is a superselector of `target`.
/// Inline helper for the non-pseudo branch of [`compound_is_superselector`].
fn comp_contains<'parse>(
    comps: &[SimpleSelector<'parse>],
    target: &SimpleSelector<'parse>,
) -> SassResult<bool> {
    for s in comps {
        if target.is_superselector(s)? {
            return Ok(true);
        }
    }
    Ok(false)
}

// --- selectorPseudoIsSuperselector ---

/// Whether pseudo `p` (which must have a selector argument) matches every
/// element `c2` matches, plus possibly more.
///
/// `parents`, when passed, are the parents of `c2`. Each pseudo name gets
/// its own strategy (`:is`/`:where` union checks, `:not` exclusion checks,
/// `:nth-child` argument-equality checks, and so on). Matches Dart:
/// `_selectorPseudoIsSuperselector` (extend/functions.dart).
pub fn selector_pseudo_is_superselector<'parse>(
    p: &PseudoSelector<'parse>,
    c2: &CompoundSelector<'parse>,
    parents: Option<&[ComplexSelectorComponent<'parse>]>,
) -> SassResult<bool> {
    let sel = p.selector.as_ref().ok_or_else(|| SassError::Script {
        message: format!("Selector {p:?} must have a selector argument"),
        argument_name: None,
    })?;
    let l1 = match sel.as_ref() {
        Selector::List(ref list) => list,
        _ => return Ok(false),
    };

    match p.normalized_name.as_str() {
        "is" | "matches" | "any" | "where" => {
            for arg in selector_pseudo_args(c2, &p.name, true) {
                if l1.is_superselector(arg)? {
                    return Ok(true);
                }
            }
            for c in &l1.0.components {
                if c.leading_combinators.is_empty() {
                    let mut comps: Vec<ComplexSelectorComponent<'parse>> = vec![];
                    if let Some(parents) = parents {
                        comps.extend_from_slice(parents);
                    }
                    comps.push(ComplexSelectorComponent::new(
                        Box::new(c2.clone()),
                        vec![],
                        c2.span,
                    ));
                    if complex_is_superselector(&c.components, &comps)? {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }

        "has" | "host" | "host-context" => {
            for arg in selector_pseudo_args(c2, &p.name, true) {
                if l1.is_superselector(arg)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        "slotted" => {
            for arg in selector_pseudo_args(c2, &p.name, false) {
                if l1.is_superselector(arg)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        "not" => {
            for c in &l1.0.components {
                if c.is_bogus() {
                    return Ok(false);
                }
                let mut found = false;
                for sb in &c2.components {
                    match sb {
                        SimpleSelector::Type(ref ta) => {
                            for sa in &c.components[c.components.len() - 1].selector.components {
                                if let SimpleSelector::Type(ref sa) = sa {
                                    if sa.name.name != ta.name.name
                                        || sa.name.namespace != ta.name.namespace
                                    {
                                        found = true;
                                    }
                                }
                            }
                        }
                        SimpleSelector::Id(ref id_b) => {
                            for sa in &c.components[c.components.len() - 1].selector.components {
                                if let SimpleSelector::Id(ref id_a) = sa {
                                    if id_a.name != id_b.name {
                                        found = true;
                                    }
                                }
                            }
                        }
                        SimpleSelector::Pseudo(ref ps_b) => {
                            if let Some(ref sel_b) = ps_b.selector {
                                if ps_b.name == p.name {
                                    if let Selector::List(ref l2) = sel_b.as_ref() {
                                        if list_is_superselector(
                                            &l2.0.components,
                                            std::slice::from_ref(c),
                                        )? {
                                            found = true;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !found {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        "current" => {
            for arg in selector_pseudo_args(c2, &p.name, true) {
                if arg == l1 {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        "nth-child" | "nth-last-child" => {
            for sb in &c2.components {
                if let SimpleSelector::Pseudo(ref ps) = sb {
                    if ps.name == p.name && ps.argument == p.argument {
                        if let Some(ref sel2) = ps.selector {
                            if let super::Selector::List(ref l2) = sel2.as_ref() {
                                return l1.is_superselector(l2);
                            }
                        }
                    }
                }
            }
            Ok(false)
        }

        _ => panic!("unreachable"),
    }
}

/// Returns the selector arguments of pseudos in `c` named `name` (`:slotted`
/// passes `is_class: false`). Matches Dart: `_selectorPseudoArgs`
/// (extend/functions.dart).
fn selector_pseudo_args<'parse>(
    c: &'parse CompoundSelector<'parse>,
    name: &str,
    is_class: bool,
) -> Vec<&'parse SelectorList<'parse>> {
    let mut r = Vec::new();
    for s in &c.components {
        if let SimpleSelector::Pseudo(ref ps) = s {
            if ps.is_class == is_class && ps.name == name {
                if let Some(ref sel) = ps.selector {
                    if let Selector::List(ref list) = sel.as_ref() {
                        r.push(list);
                    }
                }
            }
        }
    }
    r
}
