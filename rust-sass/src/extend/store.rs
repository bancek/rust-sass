// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/extend/extension_store.dart + lib/src/extend/empty_extension_store.dart
// go-source: go/extend/extension_store.go + go/extend/default_extension_store.go
//   + go/extend/empty_extension_store.go + go/box/box.go

use crate::common::ast_css_value::CssValue;
use crate::common::file_span::FileSpan;
use crate::common::pretty_uri::pretty_uri;
use crate::common::source_span_span_with_context::SourceSpanWithContext;
use crate::extend::merge_extensions;
use crate::extend::unmerge_extensions;
use crate::selector::pseudo::PseudoSelector;
use crate::selector::weave::unify_complex;
use crate::selector::weave::weave;
use std::cell::Ref;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::ast::css::media_query::CssMediaQuery;
use crate::ast::sass::statement::extend_rule::ExtendRule;
use crate::common::exception::{SassError, SassResult};
use crate::io::Io;
use crate::selector::complex::ComplexSelector;
use crate::selector::complex_component::ComplexSelectorComponent;
use crate::selector::compound::CompoundSelector;
use crate::selector::list::SelectorList;
use crate::selector::list::SelectorListIdentity;
use crate::selector::Selector;
use crate::selector::SimpleSelector;

use crate::extend::extension::{span_message, BaseExtension, Extender, Extension};
use crate::extend::mode::ExtendMode;

// ---------------------------------------------------------------------------
// MutableBox / StoreBox — identity-based mutable reference wrapper
//   Identity via Rc::ptr_eq on inner Rc<RefCell<T>> (matching Dart's
//   ModifiableBox reference equality with no explicit id).
//
// Matches Dart `lib/src/util/box.dart`: `MutableBox` is the mutable reference
// to a (presumably immutable) value; `seal()` returns the unmodifiable
// `StoreBox` view, which still observes later mutations through the shared
// box. Both compare by box identity even when `T` compares by value.
// ---------------------------------------------------------------------------

/// A mutable reference to a (presumably immutable) value.
///
/// Always uses reference identity, even when the underlying type uses value
/// equality.
pub(crate) struct MutableBox<T> {
    pub(crate) inner: Rc<RefCell<T>>,
}

/// An unmodifiable reference to a value that may be mutated elsewhere.
///
/// Uses reference equality based on the underlying [`MutableBox`], even when
/// the underlying type uses value equality.
pub struct StoreBox<T> {
    pub(crate) inner: Rc<RefCell<T>>,
}

impl<T> MutableBox<T> {
    pub fn new(value: T) -> Self {
        MutableBox {
            inner: Rc::new(RefCell::new(value)),
        }
    }

    /// Returns an unmodifiable reference to this box.
    ///
    /// The underlying modifiable box may still be modified.
    pub fn seal(&self) -> StoreBox<T> {
        StoreBox {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T> Clone for MutableBox<T> {
    fn clone(&self) -> Self {
        MutableBox {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T> Clone for StoreBox<T> {
    fn clone(&self) -> Self {
        StoreBox {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T> StoreBox<T> {
    pub fn value(&self) -> Ref<'_, T> {
        self.inner.borrow()
    }
}

impl<T> PartialEq for MutableBox<T> {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &o.inner)
    }
}
impl<T> Eq for MutableBox<T> {}
impl<T> Hash for MutableBox<T> {
    fn hash<H: Hasher>(&self, s: &mut H) {
        Rc::as_ptr(&self.inner).hash(s);
    }
}
impl<T> PartialEq for StoreBox<T> {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &o.inner)
    }
}
impl<T> Eq for StoreBox<T> {}
impl<T> Hash for StoreBox<T> {
    fn hash<H: Hasher>(&self, s: &mut H) {
        Rc::as_ptr(&self.inner).hash(s);
    }
}

impl<T> Debug for MutableBox<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutableBox")
            .field("ptr", &Rc::as_ptr(&self.inner))
            .finish()
    }
}
impl<T> Debug for StoreBox<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreBox")
            .field("ptr", &Rc::as_ptr(&self.inner))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ExtensionStore enum
//
// The const-empty variant. Matches Dart `empty_extension_store.dart`
// (`EmptyExtensionStore`): contains no extensions, reports empty, and rejects
// every mutating call.
// ---------------------------------------------------------------------------

/// Tracks selectors and extensions, and applies the latter to the former.
///
/// `Default` is the mutable store; `Empty` is the const-empty store that
/// contains no extensions and can have none added.
#[derive(Clone, Debug)]
pub enum ExtensionStore<'parse> {
    Default(Rc<RefCell<DefaultExtensionStore<'parse>>>),
    Empty,
}

impl<'parse> ExtensionStore<'parse> {
    pub fn new(io: Rc<dyn Io>) -> Self {
        ExtensionStore::Default(Rc::new(RefCell::new(DefaultExtensionStore::new(
            ExtendMode::Normal,
            io,
            true,
        ))))
    }

    pub fn new_with_mode(mode: ExtendMode, io: Rc<dyn Io>, unicode: bool) -> Self {
        ExtensionStore::Default(Rc::new(RefCell::new(DefaultExtensionStore::new(
            mode, io, unicode,
        ))))
    }

    /// Whether this store has no extensions.
    pub fn is_empty(&self) -> bool {
        match self {
            ExtensionStore::Default(s) => s.borrow().is_empty(),
            ExtensionStore::Empty => true,
        }
    }

    pub fn simple_selectors(&self) -> Vec<SimpleSelector<'parse>> {
        match self {
            ExtensionStore::Default(s) => s.borrow().simple_selectors(),
            ExtensionStore::Empty => vec![],
        }
    }

    /// Returns all mandatory extensions in this store for whose targets
    /// `callback` returns `true`.
    ///
    /// This un-merges any merged extension so only base extensions are
    /// returned.
    pub fn extensions_where_target(
        &self,
        callback: &dyn Fn(&SimpleSelector) -> bool,
    ) -> Vec<Extension<'parse>> {
        match self {
            ExtensionStore::Default(s) => s.borrow().extensions_where_target(callback),
            ExtensionStore::Empty => vec![],
        }
    }

    /// Adds `sel` to this store.
    ///
    /// Extends `sel` using any registered extensions, then returns a
    /// [`StoreBox`] containing the resulting selector. If any more relevant
    /// extensions are added, the returned selector is automatically updated.
    ///
    /// `media_ctx` is the media query context in which the selector was
    /// defined, or [`None`] if it was defined at the top level.
    pub fn add_selector<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        sel: &SelectorList<'parse>,
        media_ctx: Option<Vec<CssMediaQuery>>,
    ) -> SassResult<StoreBox<SelectorList<'parse>>> {
        match self {
            ExtensionStore::Default(s) => s.borrow_mut().add_selector(arena, sel, media_ctx),
            ExtensionStore::Empty => Err(Box::new(SassError::Script {
                message: "addSelector can't be called for const ExtensionStore.".into(),
                argument_name: None,
            })),
        }
    }

    /// Adds an extension to this store.
    ///
    /// `extender` is the selector for the style rule in which the extension
    /// is defined, and `target` is the selector passed to `@extend`. `extend`
    /// provides the extend span and indicates whether the extension is
    /// optional.
    ///
    /// `media_ctx` defines the media query context in which the extension is
    /// defined. It can only extend selectors within the same context; [`None`]
    /// indicates no media queries.
    ///
    /// If there's already an extend from `extender` to `target`, the existing
    /// entry isn't re-run but may be marked mandatory via merging.
    pub fn add_extension<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        extender: &SelectorList<'parse>,
        target: &SimpleSelector<'parse>,
        extend: &ExtendRule<'parse>,
        media_ctx: Option<Vec<CssMediaQuery>>,
    ) -> SassResult<()> {
        match self {
            ExtensionStore::Default(s) => s
                .borrow_mut()
                .add_extension(arena, extender, target, extend, media_ctx),
            ExtensionStore::Empty => Err(Box::new(SassError::Script {
                message: "addExtension can't be called for const ExtensionStore.".into(),
                argument_name: None,
            })),
        }
    }

    /// Extends `this` with all the extensions in `extenders`.
    ///
    /// These extensions extend all selectors already in `this`, but they
    /// will *not* extend other extensions from `extenders`. Private
    /// placeholders can't be extended across module boundaries.
    pub fn add_extensions<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        extenders: &[ExtensionStore<'parse>],
    ) -> SassResult<()> {
        match self {
            ExtensionStore::Default(s) => s.borrow_mut().add_extensions(arena, extenders),
            ExtensionStore::Empty => Err(Box::new(SassError::Script {
                message: "addExtensions can't be called for const ExtensionStore.".into(),
                argument_name: None,
            })),
        }
    }

    /// Returns a copy of `this` that extends new selectors, as well as a map
    /// (with reference equality) from the selectors extended by `this` to the
    /// selectors extended by the new store.
    ///
    /// If a single box is referenced by multiple simple selectors, only a
    /// single new box is created for it in the cloned structure.
    pub fn clone_store(
        &self,
    ) -> SassResult<(
        ExtensionStore<'parse>,
        HashMap<SelectorListIdentity<'parse>, StoreBox<SelectorList<'parse>>>,
    )> {
        match self {
            ExtensionStore::Default(s) => {
                let (store, map) = s.borrow().clone_store()?;
                Ok((store, map))
            }
            ExtensionStore::Empty => Ok((ExtensionStore::Empty, HashMap::new())),
        }
    }
}

// ---------------------------------------------------------------------------
// DefaultExtensionStore
// ---------------------------------------------------------------------------

/// The mutable `@extend` store: selector/extension indexes plus the trimming
/// bookkeeping (source specificity, originals) that keeps required selectors.
#[derive(Clone, Debug)]
pub struct DefaultExtensionStore<'parse> {
    /// A map from all simple selectors in the stylesheet to the selector
    /// lists that contain them.
    ///
    /// Used to find which selectors an `@extend` applies to and adjust them.
    pub(crate) selectors:
        IndexMap<SimpleSelector<'parse>, HashSet<MutableBox<SelectorList<'parse>>>>,
    /// A map from all extended simple selectors to the sources of those
    /// extensions.
    pub(crate) extensions:
        IndexMap<SimpleSelector<'parse>, IndexMap<ComplexSelector<'parse>, Extension<'parse>>>,
    /// A map from all simple selectors in extenders to the extensions that
    /// those extenders define.
    pub(crate) extensions_by_extender: IndexMap<SimpleSelector<'parse>, Vec<Extension<'parse>>>,
    /// A map from CSS selectors to the media query contexts they're defined
    /// in. A rule defined at the top level has no entry.
    // Key's `RefCell` is excluded from `Eq`/`Hash` (identity only); the lint
    // is a false positive by design. (Sibling `IndexMap` fields with the same
    // key types don't even trigger it.)
    #[allow(clippy::mutable_key_type)]
    pub(crate) media_contexts: HashMap<MutableBox<SelectorList<'parse>>, Vec<CssMediaQuery>>,
    /// A map from simple selectors to the specificity of their source
    /// selectors: the maximum specificity of the complex selector that
    /// originally contained each simple selector.
    ///
    /// Only source specificity for the original selector is relevant;
    /// selectors generated by `@extend` don't get new specificity. Used to
    /// keep selectors required by the second law of extend.
    pub(crate) source_specificity: IndexMap<SimpleSelector<'parse>, usize>,
    /// The complex selectors originally part of their component selector
    /// lists, as opposed to added by `@extend`. Used to keep selectors
    /// required by the first law of extend.
    // Key's span memo `Cell`s are excluded from `Eq`/`Hash`; false positive
    // by design (see `media_contexts` above).
    #[allow(clippy::mutable_key_type)]
    pub(crate) originals: HashSet<ComplexSelector<'parse>>,
    /// The mode that controls this store's behavior.
    pub(crate) mode: ExtendMode,
    pub(crate) io: Rc<dyn Io>,
    pub(crate) unicode: bool,
}

impl<'parse> DefaultExtensionStore<'parse> {
    pub fn new(mode: ExtendMode, io: Rc<dyn Io>, unicode: bool) -> Self {
        DefaultExtensionStore {
            selectors: IndexMap::new(),
            extensions: IndexMap::new(),
            extensions_by_extender: IndexMap::new(),
            media_contexts: HashMap::new(),
            source_specificity: IndexMap::new(),
            originals: HashSet::new(),
            mode,
            io,
            unicode,
        }
    }

    // Private ctor threading the struct fields; a params struct would just
    // duplicate the field list.
    #[allow(clippy::too_many_arguments)]
    // `media_contexts`/`originals` params carry the same by-design interior-
    // mutability-excluded-from-Eq/Hash keys as the fields.
    #[allow(clippy::mutable_key_type)]
    fn new_internal(
        selectors: IndexMap<SimpleSelector<'parse>, HashSet<MutableBox<SelectorList<'parse>>>>,
        extensions: IndexMap<
            SimpleSelector<'parse>,
            IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
        >,
        extensions_by_extender: IndexMap<SimpleSelector<'parse>, Vec<Extension<'parse>>>,
        media_contexts: HashMap<MutableBox<SelectorList<'parse>>, Vec<CssMediaQuery>>,
        source_specificity: IndexMap<SimpleSelector<'parse>, usize>,
        originals: HashSet<ComplexSelector<'parse>>,
        io: Rc<dyn Io>,
        unicode: bool,
    ) -> Self {
        DefaultExtensionStore {
            selectors,
            extensions,
            extensions_by_extender,
            media_contexts,
            source_specificity,
            originals,
            mode: ExtendMode::Normal,
            io,
            unicode,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    /// The set of all simple selectors in selectors handled by this store
    /// (see [`ExtensionStore::simple_selectors`]).
    pub fn simple_selectors(&self) -> Vec<SimpleSelector<'parse>> {
        self.selectors.keys().cloned().collect()
    }

    /// Returns all mandatory extensions matching `callback` (see
    /// [`ExtensionStore::extensions_where_target`]).
    pub fn extensions_where_target(
        &self,
        callback: &dyn Fn(&SimpleSelector) -> bool,
    ) -> Vec<Extension<'parse>> {
        let mut result = Vec::new();
        for (simple, sources) in &self.extensions {
            if !callback(simple) {
                continue;
            }
            for ext in sources.values() {
                if ext.is_merged() {
                    for u in unmerge_extensions(ext) {
                        if !u.is_optional() {
                            result.push(u);
                        }
                    }
                } else if !ext.is_optional() {
                    result.push(*ext);
                }
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // add_selector (public — dispatches to free functions)
    //
    // Extends `sel` with the registered extensions (wrapping errors with the
    // original selector's location), boxes the result, and records its media
    // context and simple-selector index entries.
    // -----------------------------------------------------------------------

    pub fn add_selector<'compile: 'parse>(
        &mut self,
        arena: &'compile Bump,
        sel: &SelectorList<'parse>,
        media_ctx: Option<Vec<CssMediaQuery>>,
    ) -> SassResult<StoreBox<SelectorList<'parse>>> {
        let mut sel = *sel;
        if !sel.is_invisible() {
            for c in &sel.0.components {
                self.originals.insert(c.clone());
            }
        }
        if !self.extensions.is_empty() {
            sel = extend_list(
                arena,
                &sel,
                &self.extensions,
                media_ctx.as_ref(),
                &mut self.originals,
                &self.source_specificity,
                self.mode,
            )
            .map_err(|e| wrap_sass_error(e, self.io.as_ref()))?;
        }
        let mb = MutableBox::new(sel);
        if let Some(ref mc) = media_ctx {
            self.media_contexts.insert(mb.clone(), mc.clone());
        }
        register_selector(&mut self.selectors, &sel, &mb);
        Ok(mb.seal())
    }

    // -----------------------------------------------------------------------
    // add_extension (public — dispatches to free functions)
    //
    // Registers each extender component (skipping useless ones), then
    // retroactively extends previously-registered selectors containing the
    // target and chains through extenders of the target, without
    // double-extending already-extended selectors.
    // -----------------------------------------------------------------------

    pub fn add_extension<'compile: 'parse>(
        &mut self,
        arena: &'compile Bump,
        extender: &SelectorList<'parse>,
        target: &SimpleSelector<'parse>,
        extend: &ExtendRule<'parse>,
        media_ctx: Option<Vec<CssMediaQuery>>,
    ) -> SassResult<()> {
        let selectors_ok = self.selectors.contains_key(target);
        let existing_ext_ok = self.extensions_by_extender.contains_key(target);

        let mut new_exts: Vec<(ComplexSelector<'parse>, Extension<'parse>)> = Vec::new();

        for complex in &extender.0.components {
            if complex.is_useless() {
                continue;
            }
            let simple_selectors = simple_selectors_in_complex(complex);
            let specificity = complex.specificity();
            let extension = BaseExtension::new(
                arena,
                complex.clone(),
                target.clone(),
                extend.span,
                media_ctx.clone(),
                extend.is_optional,
            );

            let sources = self.extensions.entry(target.clone()).or_default();
            if let Some(existing) = sources.get(complex) {
                let merged =
                    merge_extensions(arena, existing, &extension, self.io.as_ref(), self.unicode)?;
                sources.insert(complex.clone(), merged);
            } else {
                sources.insert(complex.clone(), extension);
                for simple in &simple_selectors {
                    self.extensions_by_extender
                        .entry(simple.clone())
                        .or_default()
                        .push(extension);
                    self.source_specificity
                        .entry(simple.clone())
                        .or_insert(specificity);
                }
                if selectors_ok || existing_ext_ok {
                    new_exts.push((complex.clone(), extension));
                }
            }
        }

        if new_exts.is_empty() {
            return Ok(());
        }

        let mut new_by_target: IndexMap<
            SimpleSelector<'parse>,
            IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
        > = IndexMap::new();
        let mut new_map: IndexMap<ComplexSelector<'parse>, Extension<'parse>> = IndexMap::new();
        for (c, e) in &new_exts {
            new_map.insert(c.clone(), *e);
        }
        new_by_target.insert(target.clone(), new_map);

        if existing_ext_ok {
            if let Some(existing) = self.extensions_by_extender.get(target).cloned() {
                let additional = extend_existing_extensions(
                    arena,
                    &existing,
                    &new_by_target,
                    &mut self.extensions,
                    &mut self.extensions_by_extender,
                    &mut self.originals,
                    &self.source_specificity,
                    self.mode,
                    self.io.as_ref(),
                    self.unicode,
                )?;
                for (t, inner) in additional {
                    if let Some(tm) = new_by_target.get_mut(&t) {
                        for (k, v) in inner {
                            tm.insert(k, v);
                        }
                    } else {
                        new_by_target.insert(t, inner);
                    }
                }
            }
        }
        if selectors_ok {
            if let Some(sels) = self.selectors.get(target).cloned() {
                extend_existing_selectors(
                    arena,
                    &sels,
                    &new_by_target,
                    &mut self.selectors,
                    &self.media_contexts,
                    &mut self.originals,
                    &self.source_specificity,
                    self.mode,
                    self.io.as_ref(),
                    self.unicode,
                )?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // extend_static / replace_static (public)
    //
    // Matches Dart `ExtensionStore.extend` / `replace`: work as though
    // `source {@extend target}` were written, except targets may be compound
    // selectors extended as a unit.
    // -----------------------------------------------------------------------

    /// Extends `sel` with the `source` extender and `targets` extendees.
    #[allow(dead_code)]
    pub fn extend_static<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        sel: &SelectorList<'parse>,
        source: &SelectorList<'parse>,
        targets: &SelectorList<'parse>,
        span: FileSpan<'parse>,
    ) -> SassResult<SelectorList<'parse>> {
        self.extend_or_replace(arena, sel, source, targets, ExtendMode::AllTargets, span)
    }

    /// Returns a copy of `sel` with `targets` replaced by `source`.
    #[allow(dead_code)]
    pub fn replace_static<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        sel: &SelectorList<'parse>,
        source: &SelectorList<'parse>,
        targets: &SelectorList<'parse>,
        span: FileSpan<'parse>,
    ) -> SassResult<SelectorList<'parse>> {
        self.extend_or_replace(arena, sel, source, targets, ExtendMode::Replace, span)
    }

    fn extend_or_replace<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        sel: &SelectorList<'parse>,
        source: &SelectorList<'parse>,
        targets: &SelectorList<'parse>,
        mode: ExtendMode,
        span: FileSpan<'parse>,
    ) -> SassResult<SelectorList<'parse>> {
        let mut extender = DefaultExtensionStore::new(mode, self.io.clone(), self.unicode);
        if !sel.is_invisible() {
            for c in &sel.0.components {
                extender.originals.insert(c.clone());
            }
        }
        let mut result = *sel;
        for complex in &targets.0.components {
            let compound = complex.single_compound().ok_or_else(|| SassError::Script {
                message: format!(
                    "Can't extend complex selector {}.",
                    complex.to_css_string(false).unwrap_or_else(|_| "?".into())
                ),
                argument_name: None,
            })?;
            let mut exts: IndexMap<
                SimpleSelector<'parse>,
                IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
            > = IndexMap::new();
            for simple in &compound.components {
                let mut em: IndexMap<ComplexSelector<'parse>, Extension<'parse>> = IndexMap::new();
                for sc in &source.0.components {
                    em.insert(
                        sc.clone(),
                        BaseExtension::new(arena, sc.clone(), simple.clone(), span, None, true),
                    );
                }
                exts.insert(simple.clone(), em);
            }
            result = extend_list(
                arena,
                &result,
                &exts,
                None,
                &mut extender.originals,
                &extender.source_specificity,
                mode,
            )?;
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // add_extensions (public — dispatches to free functions)
    //
    // Merges each donor store's extensions (same extender+target pairs fold
    // via merging), then extends this store's affected extenders and
    // selectors. The chain step's extra loop-handling return is ignored: loops
    // can't span module boundaries.
    // -----------------------------------------------------------------------

    // Local `HashSet<MutableBox>` key rationale as in `media_contexts` above.
    #[allow(clippy::mutable_key_type)]
    pub fn add_extensions<'compile: 'parse>(
        &mut self,
        arena: &'compile Bump,
        stores: &[ExtensionStore<'parse>],
    ) -> SassResult<()> {
        let mut exts_to_extend: Vec<Extension<'parse>> = Vec::new();
        let mut sels_to_extend: HashSet<MutableBox<SelectorList<'parse>>> = HashSet::new();
        let mut new_exts: IndexMap<
            SimpleSelector<'parse>,
            IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
        > = IndexMap::new();

        for es in stores {
            let guard = match es {
                ExtensionStore::Default(s) => s.borrow(),
                ExtensionStore::Empty => continue,
            };
            for (k, v) in &guard.source_specificity {
                self.source_specificity.insert(k.clone(), *v);
            }
            for (target, new_srcs) in &guard.extensions {
                if let SimpleSelector::Placeholder(ref ps) = target {
                    if ps.is_private() {
                        continue;
                    }
                }
                if let Some(ets) = self.extensions_by_extender.get(target) {
                    exts_to_extend.extend(ets.iter().cloned());
                }
                if let Some(sts) = self.selectors.get(target) {
                    sels_to_extend.extend(sts.iter().cloned());
                }
                if let Some(existing) = self.extensions.get_mut(target) {
                    for (ek, ext) in new_srcs {
                        if let Some(ex) = existing.get(ek) {
                            let merged =
                                merge_extensions(arena, ex, ext, self.io.as_ref(), self.unicode)?;
                            existing.insert(ek.clone(), merged);
                            if !exts_to_extend.is_empty() || !sels_to_extend.is_empty() {
                                new_exts
                                    .entry(target.clone())
                                    .or_default()
                                    .insert(ek.clone(), merged);
                            }
                        } else {
                            existing.insert(ek.clone(), *ext);
                            if !exts_to_extend.is_empty() || !sels_to_extend.is_empty() {
                                new_exts
                                    .entry(target.clone())
                                    .or_default()
                                    .insert(ek.clone(), *ext);
                            }
                        }
                    }
                } else {
                    let new_map: IndexMap<ComplexSelector<'parse>, Extension<'parse>> =
                        new_srcs.iter().map(|(k, v)| (k.clone(), *v)).collect();
                    self.extensions.insert(target.clone(), new_map);
                    if !exts_to_extend.is_empty() || !sels_to_extend.is_empty() {
                        let inner: IndexMap<ComplexSelector<'parse>, Extension<'parse>> =
                            new_srcs.iter().map(|(k, v)| (k.clone(), *v)).collect();
                        new_exts.insert(target.clone(), inner);
                    }
                }
            }
        }

        if !new_exts.is_empty() {
            if !exts_to_extend.is_empty() {
                extend_existing_extensions(
                    arena,
                    &exts_to_extend,
                    &new_exts,
                    &mut self.extensions,
                    &mut self.extensions_by_extender,
                    &mut self.originals,
                    &self.source_specificity,
                    self.mode,
                    self.io.as_ref(),
                    self.unicode,
                )?;
            }
            if !sels_to_extend.is_empty() {
                extend_existing_selectors(
                    arena,
                    &sels_to_extend,
                    &new_exts,
                    &mut self.selectors,
                    &self.media_contexts,
                    &mut self.originals,
                    &self.source_specificity,
                    self.mode,
                    self.io.as_ref(),
                    self.unicode,
                )?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // clone_store (public)
    //
    // Selector boxes (and their media contexts) are re-boxed so the clone
    // extends new selectors independently; extension maps are shallow-copied.
    // -----------------------------------------------------------------------

    // Local identity/box key rationale as in `media_contexts` above.
    #[allow(clippy::mutable_key_type)]
    pub fn clone_store(
        &self,
    ) -> SassResult<(
        ExtensionStore<'parse>,
        HashMap<SelectorListIdentity<'parse>, StoreBox<SelectorList<'parse>>>,
    )> {
        let mut new_selectors: IndexMap<
            SimpleSelector<'parse>,
            HashSet<MutableBox<SelectorList<'parse>>>,
        > = IndexMap::new();
        let mut new_mcs: HashMap<MutableBox<SelectorList<'parse>>, Vec<CssMediaQuery>> =
            HashMap::new();
        let mut old_to_new: HashMap<SelectorListIdentity<'parse>, StoreBox<SelectorList<'parse>>> =
            HashMap::new();
        let mut new_boxes: HashMap<SelectorListIdentity<'parse>, MutableBox<SelectorList<'parse>>> =
            HashMap::new();

        for (simple, ss) in &self.selectors {
            let mut new_set = HashSet::new();
            for s in ss {
                let sel_identity = SelectorListIdentity::from(*s.inner.borrow());
                let new_mb = match new_boxes.get(&sel_identity) {
                    Some(mb) => mb.clone(),
                    None => {
                        let mb = MutableBox::new(*s.inner.borrow());
                        new_boxes.insert(sel_identity, mb.clone());
                        mb
                    }
                };
                new_set.insert(new_mb.clone());
                old_to_new.insert(sel_identity, new_mb.seal());
                if let Some(mc) = self.media_contexts.get(s) {
                    new_mcs.insert(new_mb.clone(), mc.clone());
                }
            }
            new_selectors.insert(simple.clone(), new_set);
        }

        let mut new_exts = IndexMap::new();
        for (t, inner) in &self.extensions {
            let mut im = IndexMap::new();
            for (k, v) in inner {
                im.insert(k.clone(), *v);
            }
            new_exts.insert(t.clone(), im);
        }

        let mut new_ebe = IndexMap::new();
        for (k, v) in &self.extensions_by_extender {
            new_ebe.insert(k.clone(), v.clone());
        }

        let mut new_ss = IndexMap::new();
        for (k, v) in &self.source_specificity {
            new_ss.insert(k.clone(), *v);
        }

        let new_orig = self.originals.iter().cloned().collect();

        let store = DefaultExtensionStore::new_internal(
            new_selectors,
            new_exts,
            new_ebe,
            new_mcs,
            new_ss,
            new_orig,
            self.io.clone(),
            self.unicode,
        );

        Ok((
            ExtensionStore::Default(Rc::new(RefCell::new(store))),
            old_to_new,
        ))
    }
}

// ===========================================================================
// FREE FUNCTIONS — all private logic, no &self
// ===========================================================================

fn wrap_sass_error(e: Box<SassError>, io: &dyn Io) -> Box<SassError> {
    if let SassError::Sass {
        ref message,
        ref span,
        ..
    } = *e
    {
        let span_msg = span
            .source_url
            .as_ref()
            .map(|url| {
                format!(
                    "line {}, column {} of {}:",
                    span.line(),
                    span.column(),
                    pretty_uri(url, io)
                )
            })
            .unwrap_or_default();
        let msg = if span_msg.is_empty() {
            message.clone()
        } else {
            format!("From {}\n{}", span_msg, message)
        };
        Box::new(SassError::Sass {
            message: msg,
            span: span.clone(),
            cause: None,
            loaded_urls: vec![],
        })
    } else {
        e
    }
}

// Registers the simple selectors in `list` (recursing into selector
// pseudos) to point at `mb` in the selectors index.
fn register_selector<'parse>(
    selectors: &mut IndexMap<SimpleSelector<'parse>, HashSet<MutableBox<SelectorList<'parse>>>>,
    list: &SelectorList<'parse>,
    mb: &MutableBox<SelectorList<'parse>>,
) {
    for complex in &list.0.components {
        for component in &complex.components {
            for simple in &component.selector.components {
                selectors
                    .entry(simple.clone())
                    .or_default()
                    .insert(mb.clone());
                if let SimpleSelector::Pseudo(ref ps) = simple {
                    if let Some(ref inner) = ps.selector {
                        if let Selector::List(ref il) = inner.as_ref() {
                            register_selector(selectors, il, mb);
                        }
                    }
                }
            }
        }
    }
}

/// Returns an iterable of all simple selectors in `complex` (recursing into
/// selector pseudos).
fn simple_selectors_in_complex<'parse>(
    complex: &ComplexSelector<'parse>,
) -> Vec<SimpleSelector<'parse>> {
    let mut result = Vec::new();
    for c in &complex.components {
        for s in &c.selector.components {
            result.push(s.clone());
            if let SimpleSelector::Pseudo(ref ps) = s {
                if let Some(ref inner) = ps.selector {
                    if let Selector::List(ref il) = inner.as_ref() {
                        for ic in &il.0.components {
                            result.extend(simple_selectors_in_complex(ic));
                        }
                    }
                }
            }
        }
    }
    result
}

// --- extend_list ---
//
// Extends `list` using `exts`. Stays allocation-free (returns the input
// unchanged) when no extends apply, and trims the result so redundant
// subselectors are removed.

// `originals` key rationale as in `media_contexts` above.
#[allow(clippy::mutable_key_type)]
fn extend_list<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    list: &SelectorList<'parse>,
    exts: &IndexMap<SimpleSelector<'parse>, IndexMap<ComplexSelector<'parse>, Extension<'parse>>>,
    media_qc: Option<&Vec<CssMediaQuery>>,
    originals: &mut HashSet<ComplexSelector<'parse>>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
) -> SassResult<SelectorList<'parse>> {
    let mut extended: Option<Vec<ComplexSelector<'parse>>> = None;
    for (i, complex) in list.0.components.iter().enumerate() {
        let result = extend_complex(
            arena,
            complex.clone(),
            exts,
            media_qc.cloned(),
            originals,
            source_specificity,
            mode,
        )?;
        if let Some(ref results) = result {
            extended
                .get_or_insert_with(|| {
                    if i == 0 {
                        Vec::new()
                    } else {
                        list.0.components[..i].to_vec()
                    }
                })
                .extend(results.iter().cloned());
        } else if let Some(ref mut ext) = extended {
            ext.push(complex.clone());
        }
    }
    let extended_vec = match extended {
        Some(v) => v,
        None => return Ok(*list),
    };
    let trimmed = trim(
        &extended_vec,
        &|c| originals.contains(c),
        source_specificity,
    )?;
    SelectorList::new(arena, trimmed, list.0.span)
}

// --- extend_complex ---

// `originals` key rationale as in `media_contexts` above.
#[allow(clippy::mutable_key_type)]
fn extend_complex<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    complex: ComplexSelector<'parse>,
    extensions: &IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    >,
    media_query_context: Option<Vec<CssMediaQuery>>,
    originals: &mut HashSet<ComplexSelector<'parse>>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
    if complex.leading_combinators.len() > 1 {
        return Ok(None);
    }
    let complex_span = complex.span()?;

    let mut extended_not_expanded: Option<Vec<Vec<ComplexSelector<'parse>>>> = None;
    let is_original = originals.contains(&complex);

    for (i, component) in complex.components.iter().enumerate() {
        let extended = extend_compound(
            arena,
            component,
            extensions,
            &media_query_context,
            is_original,
            source_specificity,
            mode,
        )?;
        if let Some(ext) = extended {
            if let Some(ref mut ene) = extended_not_expanded {
                ene.push(ext);
            } else if i != 0 {
                let first_comps: Vec<ComplexSelectorComponent<'parse>> =
                    complex.components[..i].to_vec();
                let cs = ComplexSelector::new(
                    complex.leading_combinators.clone(),
                    first_comps,
                    complex_span,
                    complex.line_break,
                )?;
                extended_not_expanded = Some(vec![vec![cs], ext]);
            } else if complex.leading_combinators.is_empty() {
                extended_not_expanded = Some(vec![ext]);
            } else {
                let mut comb_compat = Vec::new();
                for nc in &ext {
                    if nc.leading_combinators.is_empty()
                        || css_values_equal(&complex.leading_combinators, &nc.leading_combinators)
                    {
                        comb_compat.push(ComplexSelector::new(
                            complex.leading_combinators.clone(),
                            nc.components.clone(),
                            complex_span,
                            complex.line_break || nc.line_break,
                        )?);
                    }
                }
                extended_not_expanded = Some(vec![comb_compat]);
            }
        } else if let Some(ref mut ene) = extended_not_expanded {
            let cs = ComplexSelector::new(
                vec![],
                vec![component.clone()],
                complex_span,
                complex.line_break,
            )?;
            ene.push(vec![cs]);
        }
    }

    let ene = match extended_not_expanded {
        Some(v) => v,
        None => return Ok(None),
    };
    let mut result = Vec::new();
    let is_orig = originals.contains(&complex);
    let mut first = true;
    for path in paths_f(&ene) {
        if let Some(woven) = weave(&path, complex_span, Some(complex.line_break))? {
            if first && is_orig {
                for wc in &woven {
                    originals.insert(wc.clone());
                }
            }
            first = false;
            result.extend(woven);
        }
    }
    if result.is_empty() {
        Ok(None)
    } else {
        Ok(Some(result))
    }
}

// --- extend_compound ---

fn extend_compound<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    component: &ComplexSelectorComponent<'parse>,
    extensions: &IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    >,
    media_ctx: &Option<Vec<CssMediaQuery>>,
    in_original: bool,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
    let mut targets_used: Option<HashSet<SimpleSelector<'parse>>> =
        if mode != ExtendMode::Normal && extensions.len() >= 2 {
            Some(HashSet::new())
        } else {
            None
        };
    let simples = &component.selector.components;
    let mut options: Option<Vec<Vec<Extender<'parse>>>> = None;

    for (i, simple) in simples.iter().enumerate() {
        let extended = extend_simple(
            arena,
            simple,
            extensions,
            media_ctx,
            &mut targets_used,
            source_specificity,
            mode,
        )?;
        if let Some(ext) = extended {
            if options.is_none() {
                let mut opts = Vec::new();
                if i != 0 {
                    opts.push(vec![extender_for_compound(
                        &simples[..i],
                        component.span,
                        source_specificity,
                    )?]);
                }
                opts.extend(ext);
                options = Some(opts);
            } else if let Some(ref mut opts) = options {
                opts.extend(ext);
            }
        } else if let Some(ref mut opts) = options {
            opts.push(vec![extender_for_simple(simple, source_specificity)?]);
        }
    }

    let options = match options {
        Some(o) => o,
        None => return Ok(None),
    };
    if let Some(ref tu) = targets_used {
        if tu.len() != extensions.len() {
            return Ok(None);
        }
    }

    if options.len() == 1 {
        let mut result = Vec::new();
        for extender in &options[0] {
            extender.assert_compatible_media_context(media_ctx)?;
            let complex = extender
                .selector
                .with_additional_combinators(&component.combinators, false);
            if !complex.is_useless() {
                result.push(complex);
            }
        }
        return if result.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result))
        };
    }

    let extender_paths = paths_f(&options);
    let mut result = Vec::new();

    if mode != ExtendMode::Replace {
        let first_comps: Vec<SimpleSelector<'parse>> = extender_paths[0]
            .iter()
            .flat_map(|e| {
                e.selector
                    .components
                    .last()
                    .map(|c| c.selector.components.clone())
                    .unwrap_or_default()
            })
            .collect();
        let fc = CompoundSelector::new(first_comps, component.selector.span()?)?;
        let cs = ComplexSelector::new(
            vec![],
            vec![ComplexSelectorComponent::new(
                Box::new(fc),
                component.combinators.clone(),
                component.span,
            )],
            component.span,
            false,
        )?;
        result.push(cs);
    }

    let start = if mode == ExtendMode::Replace { 0 } else { 1 };
    for path in &extender_paths[start..] {
        if let Some(extended) = unify_extenders(path, media_ctx, component.span)? {
            for complex in extended {
                let wc = complex.with_additional_combinators(&component.combinators, false);
                if !wc.is_useless() {
                    result.push(wc);
                }
            }
        }
    }

    let first_original = if in_original && mode != ExtendMode::Replace {
        result.first().cloned()
    } else {
        None
    };
    let is_original_fn = move |c: &ComplexSelector<'parse>| -> bool {
        first_original.as_ref().is_some_and(|f| *f == *c)
    };
    let trimmed = trim(&result, &is_original_fn, source_specificity)?;
    Ok(Some(trimmed))
}

fn extender_for_compound<'parse>(
    simples: &[SimpleSelector<'parse>],
    span: FileSpan<'parse>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
) -> SassResult<Extender<'parse>> {
    let compound = CompoundSelector::new(simples.to_vec(), span)?;
    let spec = source_specificity_for(&compound, source_specificity);
    let cs = ComplexSelector::new(
        vec![],
        vec![ComplexSelectorComponent::new(
            Box::new(compound),
            vec![],
            span,
        )],
        span,
        false,
    )?;
    Ok(Extender::new(cs, Some(spec), true))
}

fn extender_for_simple<'parse>(
    simple: &SimpleSelector<'parse>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
) -> SassResult<Extender<'parse>> {
    let spec = source_specificity.get(simple).copied().unwrap_or(0);
    let ss = simple_span_s(simple)?;
    let compound = CompoundSelector::new(vec![simple.clone()], ss)?;
    let cs = ComplexSelector::new(
        vec![],
        vec![ComplexSelectorComponent::new(
            Box::new(compound),
            vec![],
            ss,
        )],
        ss,
        false,
    )?;
    Ok(Extender::new(cs, Some(spec), true))
}

// --- extend_simple ---

fn extend_simple<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    simple: &SimpleSelector<'parse>,
    extensions: &IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    >,
    media_ctx: &Option<Vec<CssMediaQuery>>,
    targets_used: &mut Option<HashSet<SimpleSelector<'parse>>>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
) -> SassResult<Option<Vec<Vec<Extender<'parse>>>>> {
    let without_pseudo = |s: &SimpleSelector<'parse>,
                          targets_used: &mut Option<HashSet<SimpleSelector<'parse>>>,
                          mode: ExtendMode,
                          source_specificity: &IndexMap<SimpleSelector<'parse>, usize>|
     -> SassResult<Option<Vec<Extender<'parse>>>> {
        let exts = match extensions.get(s) {
            Some(e) => e,
            None => return Ok(None),
        };
        if let Some(ref mut tu) = targets_used {
            tu.insert(s.clone());
        }
        let mut r = Vec::new();
        if mode != ExtendMode::Replace {
            r.push(extender_for_simple(s, source_specificity)?);
        }
        for ext in exts.values() {
            r.push(ext.extender().clone());
        }
        Ok(Some(r))
    };

    if let SimpleSelector::Pseudo(ref ps) = simple {
        if ps.selector.is_some() {
            if let Some(extended) =
                extend_pseudo(arena, ps, extensions, media_ctx, source_specificity, mode)?
            {
                let mut res = Vec::new();
                for p in &extended {
                    if let Some(w) = without_pseudo(p, targets_used, mode, source_specificity)? {
                        res.push(w);
                    } else {
                        res.push(vec![extender_for_simple(p, source_specificity)?]);
                    }
                }
                return Ok(Some(res));
            }
        }
    }

    if let Some(w) = without_pseudo(simple, targets_used, mode, source_specificity)? {
        Ok(Some(vec![w]))
    } else {
        Ok(None)
    }
}

// --- extend_pseudo ---

// `originals` key rationale as in `media_contexts` above.
#[allow(clippy::mutable_key_type)]
fn extend_pseudo<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    pseudo: &PseudoSelector<'parse>,
    exts: &IndexMap<SimpleSelector<'parse>, IndexMap<ComplexSelector<'parse>, Extension<'parse>>>,
    media_ctx: &Option<Vec<CssMediaQuery>>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
) -> SassResult<Option<Vec<SimpleSelector<'parse>>>> {
    let sel = match pseudo.selector.as_ref() {
        Some(s) => s,
        None => {
            return Err(Box::new(SassError::Script {
                message: "Pseudo must have selector argument.".into(),
                argument_name: None,
            }))
        }
    };
    let sel_list = match sel.as_ref() {
        Selector::List(ref l) => l,
        _ => return Ok(None),
    };
    let sel_span = sel_list.span()?;

    let mut originals = HashSet::new();
    let extended = extend_list(
        arena,
        sel_list,
        exts,
        media_ctx.as_ref(),
        &mut originals,
        source_specificity,
        mode,
    )?;
    if extended == *sel_list {
        return Ok(None);
    }

    let mut complexes = extended.0.components.clone();
    if pseudo.normalized_name == "not" {
        let orig_has_complex = sel_list.0.components.iter().any(|c| c.components.len() > 1);
        let ext_has_simple = extended
            .0
            .components
            .iter()
            .any(|c| c.components.len() == 1);
        if !orig_has_complex && ext_has_simple {
            complexes.retain(|c| c.components.len() <= 1);
        }
    }

    let mut result_comps = Vec::new();
    for complex in &complexes {
        if let Some(ip) = find_single_simple_pseudo(complex) {
            let inner_sel = match ip.selector.as_ref() {
                Some(s) => s,
                None => {
                    result_comps.push(complex.clone());
                    continue;
                }
            };
            let inner_list = match inner_sel.as_ref() {
                Selector::List(ref l) => l,
                _ => {
                    result_comps.push(complex.clone());
                    continue;
                }
            };
            match pseudo.normalized_name.as_str() {
                "not" => {
                    if ip.normalized_name == "is"
                        || ip.normalized_name == "matches"
                        || ip.normalized_name == "where"
                    {
                        result_comps.extend(inner_list.0.components.clone());
                    }
                }
                "is" | "matches" | "where" | "any" | "current" | "nth-child" | "nth-last-child" => {
                    if ip.name == pseudo.name && ip.argument == pseudo.argument {
                        result_comps.extend(inner_list.0.components.clone());
                    }
                }
                "has" | "host" | "host-context" | "slotted" => {
                    result_comps.push(complex.clone());
                }
                _ => {}
            }
        } else {
            result_comps.push(complex.clone());
        }
    }

    if pseudo.normalized_name == "not" && sel_list.0.components.len() == 1 {
        let mut r: Vec<SimpleSelector<'parse>> = Vec::new();
        for c in &result_comps {
            let lst = SelectorList::new(arena, vec![c.clone()], sel_span)?;
            r.push(SimpleSelector::Pseudo(pseudo.with_selector(&lst)?));
        }
        if r.is_empty() {
            Ok(None)
        } else {
            Ok(Some(r))
        }
    } else {
        let lst = SelectorList::new(arena, result_comps, sel_span)?;
        Ok(Some(vec![SimpleSelector::Pseudo(
            pseudo.with_selector(&lst)?,
        )]))
    }
}

fn unify_extenders<'parse>(
    extenders: &[Extender<'parse>],
    media_ctx: &Option<Vec<CssMediaQuery>>,
    span: FileSpan<'parse>,
) -> SassResult<Option<Vec<ComplexSelector<'parse>>>> {
    let mut to_unify: Vec<ComplexSelector<'parse>> = Vec::new();
    let mut originals = Vec::new();
    let mut olb = false;
    for extender in extenders {
        if extender.is_original {
            if let Some(fc) = extender.selector.components.last() {
                originals.extend(fc.selector.components.clone());
                olb = olb || extender.selector.line_break;
            }
        } else if extender.selector.is_useless() {
            return Ok(None);
        } else {
            to_unify.push(extender.selector.clone());
        }
    }
    if !originals.is_empty() {
        let c = CompoundSelector::new(originals, span)?;
        to_unify.insert(
            0,
            ComplexSelector::new(
                vec![],
                vec![ComplexSelectorComponent::new(Box::new(c), vec![], span)],
                span,
                olb,
            )?,
        );
    }
    let complexes = unify_complex(&to_unify, span)?;
    for e in extenders {
        e.assert_compatible_media_context(media_ctx)?;
    }
    Ok(complexes)
}

fn trim<'parse>(
    selectors: &[ComplexSelector<'parse>],
    is_original: &dyn Fn(&ComplexSelector<'parse>) -> bool,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
) -> SassResult<Vec<ComplexSelector<'parse>>> {
    if selectors.len() > 100 {
        return Ok(selectors.to_vec());
    }
    let mut result = Vec::new();
    let mut num_orig = 0;
    'outer: for i in (0..selectors.len()).rev() {
        let c1 = &selectors[i];
        if is_original(c1) {
            for j in 0..num_orig {
                if result[j] == *c1 {
                    rotate_slice_f(&mut result, 0, j + 1);
                    continue 'outer;
                }
            }
            num_orig += 1;
            result.insert(0, c1.clone());
            continue 'outer;
        }
        let mut ms = 0;
        for comp in &c1.components {
            ms = ms.max(source_specificity_for(&comp.selector, source_specificity));
        }
        let mut found = false;
        for c2 in &result {
            if c2.specificity() >= ms && c2.is_superselector(c1)? {
                found = true;
                break;
            }
        }
        if found {
            continue;
        }
        for c2 in selectors.iter().take(i) {
            if c2.specificity() >= ms && c2.is_superselector(c1)? {
                found = true;
                break;
            }
        }
        if !found {
            result.insert(0, c1.clone());
        }
    }
    Ok(result)
}

fn source_specificity_for<'parse>(
    compound: &CompoundSelector<'parse>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
) -> usize {
    let mut s = 0;
    for simple in &compound.components {
        if let Some(&sp) = source_specificity.get(simple) {
            s = s.max(sp);
        }
    }
    s
}

// --- extend_existing_extensions ---

// Free-fn decomposition threading store state; a params struct would just
// rename the parameter list.
#[allow(clippy::too_many_arguments)]
// `originals` key rationale as in `media_contexts` above.
#[allow(clippy::mutable_key_type)]
fn extend_existing_extensions<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    extensions_list: &[Extension<'parse>],
    new_extensions: &IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    >,
    extensions: &mut IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    >,
    extensions_by_extender: &mut IndexMap<SimpleSelector<'parse>, Vec<Extension<'parse>>>,
    originals: &mut HashSet<ComplexSelector<'parse>>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
    io: &dyn Io,
    unicode: bool,
) -> SassResult<
    IndexMap<SimpleSelector<'parse>, IndexMap<ComplexSelector<'parse>, Extension<'parse>>>,
> {
    let mut additional: IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    > = IndexMap::new();
    for extension in extensions_list {
        let target_clone = extension.target().clone();
        let target_medias = extension.media_context().cloned();
        let old_selector = extension.extender_selector().clone();

        let selectors = match extend_complex(
            arena,
            old_selector.clone(),
            new_extensions,
            target_medias,
            originals,
            source_specificity,
            mode,
        ) {
            Ok(s) => s,
            Err(e) => {
                let selector_span = extension.extender_selector().span()?;
                let selector_ctx = SourceSpanWithContext::from_file_span(&selector_span)?;
                return Err(Box::new(SassError::MultiSpan {
                    message: e.message().to_string(),
                    span: selector_ctx,
                    primary_label: Some("target selector".into()),
                    secondary: vec![],
                    original_source: None,
                    cause: Some(e),
                    loaded_urls: vec![],
                    trace: Default::default(),
                }));
            }
        };
        let selectors = match selectors {
            Some(s) => s,
            None => continue,
        };

        let effective: Vec<&ComplexSelector<'parse>> = if selectors
            .first()
            .map(|f| *f == old_selector)
            .unwrap_or(false)
        {
            selectors[1..].iter().collect()
        } else {
            selectors.iter().collect()
        };

        if !extensions.contains_key(&target_clone) {
            // `extend_existing_extensions` only runs for targets present in
            // `extensions` (inserted by `add_extension` above); a missing
            // entry means nothing to extend, not a panic.
            extensions.insert(target_clone.clone(), IndexMap::new());
        }
        let sources = extensions
            .get_mut(&target_clone)
            .expect("just-inserted extension entry must exist");

        for complex in effective {
            let with_ext = extension.with_extender(arena, complex.clone());
            if let Some(existing) = sources.get(complex) {
                let merged = merge_extensions(arena, existing, &with_ext, io, unicode)?;
                sources.insert(complex.clone(), merged);
            } else {
                sources.insert(complex.clone(), with_ext);
                for comp in &complex.components {
                    for s in &comp.selector.components {
                        extensions_by_extender
                            .entry(s.clone())
                            .or_default()
                            .push(with_ext);
                    }
                }
                if new_extensions.contains_key(extension.target()) {
                    additional
                        .entry(extension.target().clone())
                        .or_default()
                        .insert(complex.clone(), with_ext);
                }
            }
        }
    }
    Ok(additional)
}

// --- extend_existing_selectors ---

// Same free-fn state-threading rationale as `extend_existing_extensions`.
#[allow(clippy::too_many_arguments)]
// Box/selector key rationale as in `media_contexts` above.
#[allow(clippy::mutable_key_type)]
fn extend_existing_selectors<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    sels: &HashSet<MutableBox<SelectorList<'parse>>>,
    new_exts: &IndexMap<
        SimpleSelector<'parse>,
        IndexMap<ComplexSelector<'parse>, Extension<'parse>>,
    >,
    selectors: &mut IndexMap<SimpleSelector<'parse>, HashSet<MutableBox<SelectorList<'parse>>>>,
    media_contexts: &HashMap<MutableBox<SelectorList<'parse>>, Vec<CssMediaQuery>>,
    originals: &mut HashSet<ComplexSelector<'parse>>,
    source_specificity: &IndexMap<SimpleSelector<'parse>, usize>,
    mode: ExtendMode,
    io: &dyn Io,
    unicode: bool,
) -> SassResult<()> {
    for mb in sels {
        let old_value = *mb.inner.borrow();
        let mc = media_contexts.get(mb).cloned();
        let new_value = match extend_list(
            arena,
            &old_value,
            new_exts,
            mc.as_ref(),
            originals,
            source_specificity,
            mode,
        ) {
            Ok(v) => v,
            Err(e) => {
                if let SassError::Sass {
                    ref message,
                    ref span,
                    ..
                } = *e
                {
                    let old_span = old_value.span()?;
                    let span_msg = span_message(old_span, io, unicode);
                    let new_msg = format!("From {}\n{}", span_msg, message);
                    return Err(Box::new(SassError::Sass {
                        message: new_msg,
                        span: span.clone(),
                        cause: None,
                        loaded_urls: vec![],
                    }));
                }
                return Err(e);
            }
        };
        if !std::ptr::eq(old_value.0, new_value.0) {
            *mb.inner.borrow_mut() = new_value;
            register_selector(selectors, &new_value, mb);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Static functions
// ---------------------------------------------------------------------------

pub fn extend_static<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    sel: &SelectorList<'parse>,
    source: &SelectorList<'parse>,
    targets: &SelectorList<'parse>,
    span: FileSpan<'parse>,
    io: Rc<dyn Io>,
) -> SassResult<SelectorList<'parse>> {
    let store = DefaultExtensionStore::new(ExtendMode::AllTargets, io, true);
    store.extend_or_replace(arena, sel, source, targets, ExtendMode::AllTargets, span)
}

pub fn replace_static<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    sel: &SelectorList<'parse>,
    source: &SelectorList<'parse>,
    targets: &SelectorList<'parse>,
    span: FileSpan<'parse>,
    io: Rc<dyn Io>,
) -> SassResult<SelectorList<'parse>> {
    let store = DefaultExtensionStore::new(ExtendMode::Replace, io, true);
    store.extend_or_replace(arena, sel, source, targets, ExtendMode::Replace, span)
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn paths_f<T: Clone>(choices: &[Vec<T>]) -> Vec<Vec<T>> {
    let mut paths: Vec<Vec<T>> = vec![vec![]];
    for choice in choices {
        let mut np = Vec::new();
        for opt in choice {
            for p in &paths {
                let mut n = p.clone();
                n.push(opt.clone());
                np.push(n);
            }
        }
        paths = np;
    }
    paths
}

fn rotate_slice_f<T>(slice: &mut [T], start: usize, end: usize) {
    if start >= end || end > slice.len() {
        return;
    }
    let n = end - start;
    for i in 0..n - 1 {
        slice.swap(start + i, start + n - 1);
    }
}

fn find_single_simple_pseudo<'r, 'parse>(
    complex: &'r ComplexSelector<'parse>,
) -> Option<&'r PseudoSelector<'parse>> {
    if !complex.leading_combinators.is_empty() {
        return None;
    }
    if complex.components.len() != 1 {
        return None;
    }
    let comp = &complex.components[0];
    if !comp.combinators.is_empty() {
        return None;
    }
    if comp.selector.components.len() != 1 {
        return None;
    }
    if let SimpleSelector::Pseudo(ref ps) = comp.selector.components[0] {
        return Some(ps);
    }
    None
}

fn css_values_equal<T: PartialEq>(a: &[CssValue<'_, T>], b: &[CssValue<'_, T>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.value == y.value)
}

fn simple_span_s<'parse>(s: &SimpleSelector<'parse>) -> SassResult<FileSpan<'parse>> {
    match s {
        SimpleSelector::Attribute(a) => a.span(),
        SimpleSelector::Class(c) => c.span(),
        SimpleSelector::Id(i) => i.span(),
        SimpleSelector::Pseudo(p) => p.span(),
        SimpleSelector::Parent(p) => p.span(),
        SimpleSelector::Placeholder(p) => p.span(),
        SimpleSelector::Type(t) => t.span(),
        SimpleSelector::Universal(u) => u.span(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::css::media_query::CssMediaQuery;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::ast::sass::statement::extend_rule::ExtendRule;
    use crate::common::ast_css_value::CssValue;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::io::VirtualIo;
    use crate::selector::class::ClassSelector;
    use crate::selector::combinator::Combinator;
    use crate::selector::complex::ComplexSelector;
    use crate::selector::complex_component::ComplexSelectorComponent;
    use crate::selector::compound::CompoundSelector;
    use crate::selector::list::SelectorList;
    use crate::selector::placeholder::PlaceholderSelector;
    use crate::selector::SimpleSelector;

    fn class<'compile>(name: &str) -> SimpleSelector<'compile> {
        SimpleSelector::Class(ClassSelector::new(name.into(), BOGUS_SPAN))
    }

    fn make_complex<'compile>(name: &str) -> ComplexSelector<'compile> {
        let s = class(name);
        let compound = CompoundSelector::new(vec![s], BOGUS_SPAN).unwrap();
        let comp = ComplexSelectorComponent::new(Box::new(compound), vec![], BOGUS_SPAN);
        ComplexSelector::new(vec![], vec![comp], BOGUS_SPAN, false).unwrap()
    }

    fn make_complex_with_combinator<'compile>(
        first: &str,
        second: &str,
    ) -> ComplexSelector<'compile> {
        let s1 = class(first);
        let s2 = class(second);
        let c1 = CompoundSelector::new(vec![s1], BOGUS_SPAN).unwrap();
        let c2 = CompoundSelector::new(vec![s2], BOGUS_SPAN).unwrap();
        let comp1 = ComplexSelectorComponent::new(Box::new(c1), vec![], BOGUS_SPAN);
        let comb = CssValue::new(Combinator::Child, BOGUS_SPAN);
        let comp2 = ComplexSelectorComponent::new(Box::new(c2), vec![comb], BOGUS_SPAN);
        ComplexSelector::new(vec![], vec![comp1, comp2], BOGUS_SPAN, false).unwrap()
    }

    fn make_selector_list<'compile>(
        arena: &'compile Bump,
        complexes: Vec<ComplexSelector<'compile>>,
    ) -> SelectorList<'compile> {
        SelectorList::new(arena, complexes, BOGUS_SPAN).unwrap()
    }

    fn make_extend_rule<'compile>(optional: bool) -> ExtendRule<'compile> {
        let interp = Interpolation::plain(".foo".into(), BOGUS_SPAN);
        ExtendRule::new(interp, BOGUS_SPAN, optional)
    }

    fn has_class(sel: &SelectorList, name: &str) -> bool {
        for c in &sel.0.components {
            for comp in &c.components {
                for simple in &comp.selector.components {
                    if let SimpleSelector::Class(ref cs) = simple {
                        if cs.name == name {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    // --- Tests ---

    #[test]
    fn test_is_empty() {
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        assert!(store.is_empty());
    }

    #[test]
    fn test_is_empty_after_add_extension() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let extender = make_complex("a");
        let sel = make_selector_list(&arena, vec![extender]);
        let target = class("b");
        store
            .add_extension(&arena, &sel, &target, &make_extend_rule(false), None)
            .unwrap();
        assert!(!store.is_empty());
    }

    #[test]
    fn test_simple_selectors() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let complex = make_complex("a");
        let sel = make_selector_list(&arena, vec![complex]);
        store.add_selector(&arena, &sel, None).unwrap();

        let selectors = store.simple_selectors();
        let has_a = selectors
            .iter()
            .any(|s| matches!(s, SimpleSelector::Class(ref cs) if cs.name == "a"));
        assert!(has_a, "should have class selector 'a'");
    }

    #[test]
    fn test_add_selector_no_extensions() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let complex = make_complex("a");
        let sel = make_selector_list(&arena, vec![complex.clone()]);
        let result = store.add_selector(&arena, &sel, None).unwrap();
        assert_eq!(*result.value(), sel);
    }

    #[test]
    fn test_add_selector_with_extension() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // .a { @extend .b; }
        let extender = make_complex("a");
        let target = class("b");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![extender]),
                &target,
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // .b { color: red; }
        let complex_b = make_complex("b");
        let sel_b = make_selector_list(&arena, vec![complex_b]);
        let result = store.add_selector(&arena, &sel_b, None).unwrap();

        assert!(has_class(&result.value(), "b"), "expected .b in result");
        assert!(has_class(&result.value(), "a"), "expected .a in result");
    }

    #[test]
    fn test_retroactive() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // Register .b first
        let complex_b = make_complex("b");
        let sel_b = make_selector_list(&arena, vec![complex_b.clone()]);
        let result = store.add_selector(&arena, &sel_b, None).unwrap();
        assert_eq!(
            *result.value(),
            sel_b,
            "should only contain .b before extension"
        );

        // Now add extension .a extends .b
        let extender = make_complex("a");
        let target = class("b");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![extender]),
                &target,
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // The box should have been updated to include .a
        assert!(
            has_class(&result.value(), "a"),
            "retroactive extension should add .a"
        );
    }

    #[test]
    fn test_chained_extension() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // .a extends .b — .a is now an extender for .b
        let ext_a = make_complex("a");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![ext_a]),
                &class("b"),
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // .c extends .a — chain propagates .c to also extend .b
        let ext_c = make_complex("c");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![ext_c]),
                &class("a"),
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // .b should get both .a (direct) and .c (chained)
        let complex_b = make_complex("b");
        let sel_b = make_selector_list(&arena, vec![complex_b]);
        let result = store.add_selector(&arena, &sel_b, None).unwrap();

        assert!(has_class(&result.value(), "b"), "expected .b");
        assert!(
            has_class(&result.value(), "a"),
            "expected .a from direct extension"
        );
        assert!(
            has_class(&result.value(), "c"),
            "expected .c from chain (.c extends .a extends .b)"
        );
    }

    #[test]
    fn test_useless_extender() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // Useless: >1 leading combinator
        let c1 = CompoundSelector::new(vec![class("a")], BOGUS_SPAN).unwrap();
        let cc = ComplexSelectorComponent::new(Box::new(c1), vec![], BOGUS_SPAN);
        let comb1 = CssValue::new(Combinator::Child, BOGUS_SPAN);
        let comb2 = CssValue::new(Combinator::Child, BOGUS_SPAN);
        let useless =
            ComplexSelector::new(vec![comb1, comb2], vec![cc], BOGUS_SPAN, false).unwrap();
        assert!(useless.is_useless());

        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![useless]),
                &class("b"),
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // Target entry registered but no actual extension
        let found = store.extensions_where_target(
            &|s| matches!(s, SimpleSelector::Class(ref cs) if cs.name == "b"),
        );
        assert_eq!(
            found.len(),
            0,
            "useless extender should not create extensions"
        );
    }

    #[test]
    fn test_complex_extension() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // .x > .y { @extend .z; }
        let extender = make_complex_with_combinator("x", "y");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![extender]),
                &class("z"),
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // .z { color: red; }
        let complex_z = make_complex("z");
        let sel_z = make_selector_list(&arena, vec![complex_z]);
        let result = store.add_selector(&arena, &sel_z, None).unwrap();

        assert!(
            result.value().0.components.len() >= 2,
            "expected at least 2 complex selectors, got {}",
            result.value().0.components.len()
        );
    }

    #[test]
    fn test_clone() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let complex_a = make_complex("a");
        let sel = make_selector_list(&arena, vec![complex_a]);
        store.add_selector(&arena, &sel, None).unwrap();

        let (cloned, old_to_new) = store.clone_store().unwrap();
        assert!(!old_to_new.is_empty());

        assert_eq!(
            cloned.is_empty(),
            store.is_empty(),
            "clone and original should have same isEmpty"
        );
    }

    #[test]
    fn test_add_extensions() {
        let arena = Bump::new();
        let store1 = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let store2 = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // store2 has .a { @extend .b; }
        let ext_a = make_complex("a");
        store2
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![ext_a]),
                &class("b"),
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // Merge store2 into store1
        store1.add_extensions(&arena, &[store2]).unwrap();
        assert!(!store1.is_empty());

        // Adding .b should get extended by .a
        let complex_b = make_complex("b");
        let sel_b = make_selector_list(&arena, vec![complex_b]);
        let result = store1.add_selector(&arena, &sel_b, None).unwrap();

        assert!(
            has_class(&result.value(), "a"),
            "expected .a in result after merging stores"
        );
    }

    #[test]
    fn test_add_extensions_private_placeholder() {
        let arena = Bump::new();
        let store1 = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let store2 = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // store2 has .a { @extend %_private; }
        let ext_a = make_complex("a");
        let private =
            SimpleSelector::Placeholder(PlaceholderSelector::new("_private".into(), BOGUS_SPAN));
        store2
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![ext_a]),
                &private,
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        // Merge — private placeholder should be filtered
        store1.add_extensions(&arena, &[store2]).unwrap();
        assert!(
            store1.is_empty(),
            "store1 should remain empty — private placeholder filtered"
        );
    }

    #[test]
    fn test_extensions_where_target() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let ext_a = make_complex("a");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![ext_a]),
                &class("b"),
                &make_extend_rule(false),
                None,
            )
            .unwrap();

        let found = store.extensions_where_target(
            &|s| matches!(s, SimpleSelector::Class(ref cs) if cs.name == "b"),
        );
        assert!(!found.is_empty(), "should find extension for .b");
    }

    #[test]
    fn test_extensions_where_target_optional() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        let ext_a = make_complex("a");
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![ext_a]),
                &class("b"),
                &make_extend_rule(true),
                None,
            )
            .unwrap();

        let found = store.extensions_where_target(&|_| true);
        assert!(
            found.is_empty(),
            "optional extensions should not be returned"
        );
    }

    #[test]
    fn test_extend_static() {
        let arena = Bump::new();
        let source = make_selector_list(&arena, vec![make_complex("a")]);
        let targets = make_selector_list(&arena, vec![make_complex("b")]);
        let sel = make_selector_list(&arena, vec![make_complex("b")]);

        let result = extend_static(
            &arena,
            &sel,
            &source,
            &targets,
            BOGUS_SPAN,
            Rc::new(VirtualIo::new()),
        )
        .unwrap();
        assert!(
            has_class(&result, "a"),
            "expected .a in extendStatic result"
        );
    }

    #[test]
    fn test_replace_static() {
        let arena = Bump::new();
        let source = make_selector_list(&arena, vec![make_complex("a")]);
        let targets = make_selector_list(&arena, vec![make_complex("b")]);
        let sel = make_selector_list(&arena, vec![make_complex("b")]);

        let result = replace_static(
            &arena,
            &sel,
            &source,
            &targets,
            BOGUS_SPAN,
            Rc::new(VirtualIo::new()),
        )
        .unwrap();
        assert!(
            !has_class(&result, "b"),
            "replace mode should not keep original .b"
        );
        assert!(
            has_class(&result, "a"),
            "expected .a in replaceStatic result"
        );
    }

    #[test]
    fn test_media_context() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));

        // .a { @extend .b; } inside @media screen
        let extender = make_complex("a");
        let target = class("b");
        let mc = vec![CssMediaQuery::new_type(Some("screen".into()), None, vec![])];
        store
            .add_extension(
                &arena,
                &make_selector_list(&arena, vec![extender]),
                &target,
                &make_extend_rule(false),
                Some(mc.clone()),
            )
            .unwrap();

        // .b inside @media screen (same context) should get extended
        let complex_b = make_complex("b");
        let sel_b = make_selector_list(&arena, vec![complex_b]);
        let result = store.add_selector(&arena, &sel_b, Some(mc)).unwrap();

        assert!(
            has_class(&result.value(), "a"),
            "expected .a from extension inside same media context"
        );
    }

    #[test]
    fn test_seal_box() {
        let arena = Bump::new();
        let store = ExtensionStore::new(Rc::new(VirtualIo::new()));
        let complex = make_complex("a");
        let sel = make_selector_list(&arena, vec![complex]);
        let sealed = store.add_selector(&arena, &sel, None).unwrap();
        assert!(
            !sealed.value().0.components.is_empty(),
            "sealed box should contain selectors"
        );
    }
}
