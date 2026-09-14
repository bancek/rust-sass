// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value.dart
// go-source: go/value/value.go

pub mod argument_list;
pub mod boolean;
pub mod calculation;
pub mod color;
pub mod color_channel;
pub mod color_channel_types;
pub mod color_conversions;
pub mod color_conversions_base;
pub mod color_gamut;
pub mod color_gamut_clip;
pub mod color_gamut_local_minde;
pub mod color_interpolation;
pub mod color_names;
pub mod color_space_a98_rgb;
pub mod color_space_display_p3;
pub mod color_space_display_p3_linear;
pub mod color_space_hsl;
pub mod color_space_hwb;
pub mod color_space_lab;
pub mod color_space_lch;
pub mod color_space_lms;
pub mod color_space_oklab;
pub mod color_space_oklch;
pub mod color_space_prophoto_rgb;
pub mod color_space_rec2020;
pub mod color_space_rgb;
pub mod color_space_srgb;
pub mod color_space_srgb_linear;
pub mod color_space_xyz_d50;
pub mod color_space_xyz_d65;
pub mod color_utils;
pub mod function;
pub mod hash;
pub mod interpolation_method;
pub mod list;
pub mod map;
pub mod mixin;
pub mod null;
pub mod number;
pub mod number_math;
pub mod number_util;
pub mod string;

use std::cell::Cell;
use std::fmt;
use std::fmt::Write;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;

use crate::common::exception::{SassError, SassResult};
use crate::deprecation::FUNCTION_UNITS;
use crate::logger::WarnLogger;

pub use argument_list::SassArgumentList;
pub use boolean::{SassBoolean, SASS_FALSE, SASS_TRUE};
use bumpalo::Bump;
pub use calculation::{CalcArgument, CalculationOperation, CalculationOperator, SassCalculation};
pub use color::SassColor;
pub use function::SassFunction;
pub use list::{ListSeparator, SassList};
pub use map::SassMap;
pub use mixin::SassMixin;
pub use null::SassNull;
pub use number::SassNumber;
pub use string::SassString;

#[derive(Clone, Debug)]
/// A SassScript value.
///
/// All SassScript values are unmodifiable. New values are built by allocating
/// a [`ValueKind`] into the compilation arena with
/// [`Value::new_with_arena`]. Untyped values can be narrowed to a particular
/// type with the `assert_*` free functions such as [`assert_string`], which
/// return a user-friendly error when the value has the wrong type.
pub enum ValueKind<'parse> {
    Boolean(SassBoolean),
    Null,
    String(SassString<'parse>),
    Number(SassNumber),
    Color(SassColor),
    List(SassList<'parse>),
    ArgumentList(SassArgumentList<'parse>),
    Map(SassMap<'parse>),
    Calculation(Box<SassCalculation>),
    Function(SassFunction<'parse>),
    Mixin(SassMixin<'parse>),
}

/// A SassScript value handle: a `Copy` reference to arena-allocated data.
///
/// `Value` is the handle used throughout eval/functions/serialize; [`ValueKind`]
/// is the variant data it points to. Cloning a `Value` is a pointer copy —
/// Sass values are immutable, so sharing is always correct. `Value` lives in
/// the compilation arena (see `ref/value.md`), which replaces Dart's GC-shared
/// references with borrowed ownership: nothing is freed until the caller drops
/// the arena.
///
/// The `hash` cache lives in the shared [`ValueInner`]; it is populated on
/// first hash so repeated hashing (for example map-key lookups) does not
/// re-walk a deep list or map.
#[derive(Clone, Copy, Debug)]
pub struct Value<'parse>(&'parse ValueInner<'parse>);

// Rust-only shared storage for a value handle plus its lazily cached hash.
// `Value` derefs to `ValueKind`, so this never appears in Sass-facing logic;
// it exists so the arena can hold the hash cache next to the variant data
// (replacing Dart's GC-shared references with borrowed ownership).
#[derive(Clone, Debug)]
pub struct ValueInner<'parse> {
    kind: ValueKind<'parse>,
    hash: Cell<Option<i32>>,
}

impl<'parse> Value<'parse> {
    /// Creates a new value handle in the compilation arena.
    ///
    /// Values live in the arena for the whole compile, so the returned
    /// handle can be copied freely without further allocation.
    pub fn new_with_arena<'compile: 'parse>(
        arena: &'compile Bump,
        kind: ValueKind<'parse>,
    ) -> Value<'parse> {
        Value(arena.alloc(ValueInner {
            kind,
            hash: Cell::new(None),
        }))
    }

    /// Returns the underlying variant data this handle points to.
    pub fn kind(&self) -> &ValueKind<'parse> {
        &self.0.kind
    }
}

impl<'parse> Deref for Value<'parse> {
    type Target = ValueKind<'parse>;

    fn deref(&self) -> &ValueKind<'parse> {
        &self.0.kind
    }
}

/// Value equality is deep value equality (NOT pointer equality) — Sass `==`
/// compares by value. Only `SassFunction`/`SassMixin` are identity-compared
/// (via their inner `Callable`), matching Dart.
impl PartialEq for Value<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.kind().equals(other.kind())
    }
}

impl Eq for Value<'_> {}

impl Hash for Value<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let inner = self.0;
        let hash = match inner.hash.get() {
            Some(h) => h,
            None => {
                let h = inner.kind.hash_code();
                inner.hash.set(Some(h));
                h
            }
        };
        state.write_i32(hash);
    }
}

/// A list-like view over either a [`SassList`] or a [`SassArgumentList`].
///
/// Matches Dart's `SassArgumentList extends SassList` class hierarchy with a
/// single Rust enum so visitors need only one `visit_list` method and can
/// distinguish via pattern matching when keyword access matters.
///
/// `'v` is the borrow of the list; `'parse` is the value content lifetime. They
/// are decoupled because `Value<'parse>` is invariant in `'parse` (SassMixin/
/// SassFunction carry callables whose callbacks mention `'parse`).
#[derive(Clone, Copy, Debug)]
pub enum ListValue<'v, 'parse> {
    List(&'v SassList<'parse>),
    ArgumentList(&'v SassArgumentList<'parse>),
}

impl<'v, 'parse> ListValue<'v, 'parse> {
    /// The separator for this list view.
    pub fn separator(&self) -> ListSeparator {
        match self {
            ListValue::List(l) => l.separator,
            ListValue::ArgumentList(a) => a.list.separator,
        }
    }

    /// Whether this list view has brackets.
    pub fn has_brackets(&self) -> bool {
        match self {
            ListValue::List(l) => l.has_brackets,
            ListValue::ArgumentList(a) => a.list.has_brackets,
        }
    }

    /// The length of the contents of this list view, without allocating.
    pub fn length_as_list(&self) -> usize {
        match self {
            ListValue::List(l) => l.contents.len(),
            ListValue::ArgumentList(a) => a.list.length_as_list(),
        }
    }
}

/// Dispatches to the matching visit method for each value variant.
///
/// Matches Dart: `ValueVisitor<T>` (visitor/interface/value.dart).
/// Matches Go: `ValueVisitor[T any]`.
pub trait ValueVisitor<'parse> {
    type Output;
    fn visit_boolean(&mut self, value: &SassBoolean) -> SassResult<Self::Output>;
    fn visit_null(&mut self) -> SassResult<Self::Output>;
    fn visit_string(&mut self, value: &SassString<'parse>) -> SassResult<Self::Output>;
    fn visit_number(&mut self, value: &SassNumber) -> SassResult<Self::Output>;
    fn visit_color(&mut self, value: &SassColor) -> SassResult<Self::Output>;
    fn visit_list(&mut self, value: &ListValue<'_, 'parse>) -> SassResult<Self::Output>;
    fn visit_map(&mut self, value: &SassMap<'parse>) -> SassResult<Self::Output>;
    fn visit_calculation(&mut self, value: &SassCalculation) -> SassResult<Self::Output>;
    fn visit_function(&mut self, value: &SassFunction<'parse>) -> SassResult<Self::Output>;
    fn visit_mixin(&mut self, value: &SassMixin<'parse>) -> SassResult<Self::Output>;
}

impl<'parse> ValueKind<'parse> {
    /// Creates a unitless number value in the compilation arena.
    pub fn unitless_number<'compile: 'parse>(arena: &'compile Bump, v: f64) -> Value<'parse> {
        Value::new_with_arena(arena, ValueKind::Number(SassNumber::new(v, None)))
    }

    /// Calls the matching visit method on `visitor` for `self`.
    //
    // Matches Dart: `Value.accept` (value.dart) — intentionally undocumented
    // in Dart (`@nodoc`/`@internal`).
    pub fn accept<V: ValueVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            ValueKind::Boolean(v) => visitor.visit_boolean(v),
            ValueKind::Null => visitor.visit_null(),
            ValueKind::String(v) => visitor.visit_string(v),
            ValueKind::Number(v) => visitor.visit_number(v),
            ValueKind::Color(v) => visitor.visit_color(v),
            ValueKind::List(v) => visitor.visit_list(&ListValue::List(v)),
            ValueKind::ArgumentList(v) => visitor.visit_list(&ListValue::ArgumentList(v)),
            ValueKind::Map(v) => visitor.visit_map(v),
            ValueKind::Calculation(v) => visitor.visit_calculation(v),
            ValueKind::Function(v) => visitor.visit_function(v),
            ValueKind::Mixin(v) => visitor.visit_mixin(v),
        }
    }

    /// Whether the value counts as `true` in an `@if` statement and other
    /// contexts.
    pub fn is_truthy(&self) -> bool {
        match self {
            ValueKind::Boolean(b) => b.is_truthy(),
            ValueKind::Null => false,
            ValueKind::String(s) => s.is_truthy(),
            ValueKind::Number(_) => true,
            ValueKind::Color(_) => true,
            ValueKind::List(_) => true,
            ValueKind::ArgumentList(_) => true,
            ValueKind::Map(_) => true,
            ValueKind::Calculation(c) => c.is_truthy(),
            ValueKind::Function(f) => f.is_truthy(),
            ValueKind::Mixin(m) => m.is_truthy(),
        }
    }

    /// The separator for this value as a list.
    ///
    /// All SassScript values can be used as lists. Maps count as lists of
    /// pairs, and all other values count as single-value lists.
    pub fn separator(&self) -> ListSeparator {
        match self {
            ValueKind::List(l) => l.separator,
            ValueKind::ArgumentList(a) => a.list.separator,
            ValueKind::Map(m) => m.separator(),
            _ => ListSeparator::Undecided,
        }
    }

    /// Whether this value as a list has brackets.
    ///
    /// All SassScript values can be used as lists. Maps count as lists of
    /// pairs, and all other values count as single-value lists.
    pub fn has_brackets(&self) -> bool {
        match self {
            ValueKind::List(l) => l.has_brackets,
            ValueKind::ArgumentList(a) => a.list.has_brackets,
            _ => false,
        }
    }

    /// The length of [`as_list`](Self::as_list).
    ///
    /// Computed without allocating a new list.
    //
    // Matches Dart: `Value.lengthAsList` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@protected`).
    pub fn length_as_list(&self) -> usize {
        match self {
            ValueKind::List(l) => l.contents.len(),
            ValueKind::Map(m) => m.len(),
            ValueKind::ArgumentList(a) => a.list.contents.len(),
            _ => 1,
        }
    }

    /// Whether the value will be represented in CSS as the empty string.
    //
    // Matches Dart: `Value.isBlank` (value.dart) — intentionally undocumented
    // in Dart (`@nodoc`/`@internal`).
    pub fn is_blank(&self) -> bool {
        match self {
            ValueKind::Null => true,
            ValueKind::String(s) => s.is_blank(),
            ValueKind::List(l) => l.is_blank(),
            ValueKind::ArgumentList(a) => a.list.is_blank(),
            _ => false,
        }
    }

    /// Returns the hash code for this value.
    ///
    /// Value equality is deep value equality, and the hash is consistent
    /// with it: only [`SassFunction`](super::function::SassFunction) and
    /// [`SassMixin`](super::mixin::SassMixin) compare by callable identity.
    pub fn hash_code(&self) -> i32 {
        match self {
            ValueKind::Boolean(b) => b.hash_code(),
            ValueKind::Null => 0,
            ValueKind::String(s) => s.hash_code(),
            ValueKind::Number(n) => n.hash_code(),
            ValueKind::Color(c) => c.hash_code(),
            ValueKind::List(l) => l.hash_code(),
            ValueKind::ArgumentList(a) => a.list.hash_code(),
            ValueKind::Map(m) => m.hash_code(),
            ValueKind::Calculation(c) => c.hash_code() as i32,
            ValueKind::Function(f) => f.hash_code(),
            ValueKind::Mixin(m) => m.hash_code(),
        }
    }

    /// Compares this value to `other` by value.
    ///
    /// Empty maps count as equal to empty lists; [`SassFunction`](super::function::SassFunction)
    /// and [`SassMixin`](super::mixin::SassMixin) compare by callable
    /// identity. String comparison ignores quotes.
    pub fn equals(&self, other: &ValueKind<'parse>) -> bool {
        match (self, other) {
            (ValueKind::Boolean(a), ValueKind::Boolean(b)) => a.equals(b),
            (ValueKind::Null, ValueKind::Null) => true,
            (ValueKind::String(a), ValueKind::String(b)) => a.equals(b),
            (ValueKind::Number(a), ValueKind::Number(b)) => a.equals(b),
            (ValueKind::Color(a), ValueKind::Color(b)) => a.equals(b),
            (ValueKind::List(a), ValueKind::List(b)) => a.equals(b),
            (ValueKind::ArgumentList(a), ValueKind::ArgumentList(b)) => a.list.equals(&b.list),
            (ValueKind::List(a), ValueKind::ArgumentList(b)) => a.equals(&b.list),
            (ValueKind::ArgumentList(a), ValueKind::List(b)) => a.list.equals(b),
            (ValueKind::Map(a), ValueKind::Map(b)) => a.equals(b),
            (ValueKind::Function(a), ValueKind::Function(b)) => a.equals(b),
            (ValueKind::Mixin(a), ValueKind::Mixin(b)) => a.equals(b),
            (ValueKind::Calculation(a), ValueKind::Calculation(b)) => a.equals(b),
            (ValueKind::Map(a), ValueKind::List(b)) if a.is_empty() && b.contents.is_empty() => {
                true
            }
            (ValueKind::List(a), ValueKind::Map(b)) if a.contents.is_empty() && b.is_empty() => {
                true
            }
            _ => false,
        }
    }

    /// Whether this is a value that CSS may treat as a number, such as
    /// `calc()` or `var()`.
    ///
    /// Functions that shadow plain CSS functions need to gracefully handle
    /// when these arguments are passed in.
    //
    // Matches Dart: `Value.isSpecialNumber` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`).
    pub fn is_special_number(&self) -> bool {
        match self {
            ValueKind::String(s) => s.is_special_number(),
            ValueKind::Calculation(_) => true,
            _ => false,
        }
    }

    /// Whether this is a call to `var()`, which may be substituted in CSS
    /// for a custom property value.
    ///
    /// Functions that shadow plain CSS functions need to gracefully handle
    /// when these arguments are passed in.
    //
    // Matches Dart: `Value.isSpecialVariable` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`).
    pub fn is_special_variable(&self) -> bool {
        match self {
            ValueKind::String(s) => s.is_special_variable(),
            _ => false,
        }
    }

    /// Returns `None` if this is null, and `Some(self)` otherwise.
    pub fn real_null(&self) -> Option<&ValueKind<'parse>> {
        match self {
            ValueKind::Null => None,
            _ => Some(self),
        }
    }

    /// Returns `self` as a [`SassMap`] if it is one (including empty lists,
    /// which count as empty maps), or `None` if it is not.
    pub fn try_map(&self) -> Option<SassMap<'parse>> {
        match self {
            ValueKind::Map(m) => Some(m.clone()),
            ValueKind::List(l) if l.contents.is_empty() => Some(SassMap::empty()),
            ValueKind::ArgumentList(a) if a.list.contents.is_empty() => Some(SassMap::empty()),
            _ => None,
        }
    }

    /// This value as a list.
    ///
    /// All SassScript values can be used as lists. Maps count as lists of
    /// pairs, and all other values count as single-value lists.
    pub fn as_list<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
    ) -> SassResult<Vec<Value<'parse>>> {
        match self {
            ValueKind::List(l) => Ok(l.contents.clone()),
            ValueKind::ArgumentList(a) => Ok(a.list.contents.clone()),
            ValueKind::Map(m) => Ok(m.as_list(arena)),
            _ => Ok(vec![Value::new_with_arena(arena, self.clone())]),
        }
    }

    /// Returns a valid CSS representation of `self`.
    ///
    /// Returns an error if `self` can't be represented in plain CSS. Use
    /// [`to_display_string`](Self::to_display_string) instead to get a string
    /// representation even if this isn't valid CSS.
    //
    // Internal-only: if `quote` is `false`, quoted strings are emitted
    // without quotes.
    pub fn to_css_string(&self, quote: bool) -> SassResult<String> {
        match self {
            ValueKind::Boolean(b) => b.to_css_string(quote),
            ValueKind::Null => SassNull.to_css_string(quote),
            ValueKind::String(s) => s.to_css_string(quote),
            ValueKind::Number(n) => n.to_css_string(quote),
            ValueKind::Color(c) => c.to_css_string(quote),
            ValueKind::List(l) => l.to_css_string(quote),
            ValueKind::ArgumentList(a) => a.to_css_string(quote),
            ValueKind::Map(m) => m.to_css_string(quote),
            ValueKind::Calculation(c) => c.to_css_string(quote),
            ValueKind::Function(f) => f.to_css_string(quote),
            ValueKind::Mixin(m) => m.to_css_string(quote),
        }
    }

    /// Returns a string representation of `self`.
    ///
    /// Note that this is equivalent to calling `inspect()` on the value, and
    /// thus won't reflect the user's output settings.
    /// [`to_css_string`](Self::to_css_string) should be used instead to
    /// convert `self` to CSS.
    pub fn to_display_string(&self) -> SassResult<String> {
        match self {
            ValueKind::Boolean(b) => b.to_display_string(),
            ValueKind::Null => SassNull.to_display_string(),
            ValueKind::String(s) => s.to_display_string(),
            ValueKind::Number(n) => n.to_display_string(),
            ValueKind::Color(c) => c.to_display_string(),
            ValueKind::List(l) => l.to_display_string(),
            ValueKind::ArgumentList(a) => a.to_display_string(),
            ValueKind::Map(m) => m.to_display_string(),
            ValueKind::Calculation(c) => c.to_display_string(),
            ValueKind::Function(f) => f.to_display_string(),
            ValueKind::Mixin(m) => m.to_display_string(),
        }
    }

    // The SassScript `+` operation.
    //
    // Matches Dart: `Value.plus` (value.dart) — intentionally undocumented in
    // Dart (`@nodoc`/`@internal`). Numbers add numerically; a calculation on
    // either side is an error; anything else concatenates via the CSS form.
    pub fn plus<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(a.plus_num(b)?),
            )),
            (ValueKind::String(a), ValueKind::String(_b)) => a.plus(arena, other),
            (ValueKind::Number(a), ValueKind::Color(_)) => {
                let self_val = Value::new_with_arena(arena, ValueKind::Number(a.clone()));
                let self_str = self_val.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} + {other_str}\"."),
                    argument_name: None,
                }))
            }
            (ValueKind::Calculation(c), ValueKind::String(s)) => {
                let calc_str = c.to_css_string(true)?;
                Ok(Value::new_with_arena(
                    arena,
                    ValueKind::String(SassString::new(
                        arena.alloc_str(&(calc_str + s.text)),
                        s.has_quotes,
                    )),
                ))
            }
            (ValueKind::Calculation(_), _) => {
                let self_str = self.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} + {other_str}\"."),
                    argument_name: None,
                }))
            }
            (ValueKind::Number(_), _) => default_plus(arena, self, other),
            (ValueKind::Color(_), ValueKind::Number(_))
            | (ValueKind::Color(_), ValueKind::Color(_)) => {
                let self_str = self.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} + {other_str}\"."),
                    argument_name: None,
                }))
            }
            (ValueKind::String(a), _) => a.plus(arena, other),
            _ => default_plus(arena, self, other),
        }
    }

    // The SassScript `-` operation.
    //
    // Matches Dart: `Value.minus` (value.dart) — intentionally undocumented in
    // Dart (`@nodoc`/`@internal`). Numbers subtract numerically; a calculation
    // on the right is an error; anything else concatenates with a `-`.
    pub fn minus<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(a.minus_num(b)?),
            )),
            (ValueKind::Number(a), ValueKind::Color(_)) => {
                let self_val = Value::new_with_arena(arena, ValueKind::Number(a.clone()));
                let self_str = self_val.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} - {other_str}\"."),
                    argument_name: None,
                }))
            }
            (ValueKind::Calculation(_), _) => {
                let self_str = self.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} - {other_str}\"."),
                    argument_name: None,
                }))
            }
            (ValueKind::Number(_), _) => default_minus(arena, self, other),
            (ValueKind::Color(_), ValueKind::Number(_))
            | (ValueKind::Color(_), ValueKind::Color(_)) => {
                let self_str = self.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} - {other_str}\"."),
                    argument_name: None,
                }))
            }
            _ => default_minus(arena, self, other),
        }
    }

    // The SassScript `*` operation.
    //
    // Matches Dart: `Value.times` (value.dart) — intentionally undocumented in
    // Dart (`@nodoc`/`@internal`). Only number operands multiply; everything
    // else is an `Undefined operation` error.
    pub fn times<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(a.times_num(b)?),
            )),
            _ => default_times(arena, self, other),
        }
    }

    // The SassScript `/` operation.
    //
    // Matches Dart: `Value.dividedBy` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Non-number operands produce
    // a slash-separated string; number/color mismatches are errors.
    pub fn divided_by<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(a.divided_by_num(b)?),
            )),
            (ValueKind::Number(_), _) => default_divided_by(arena, self, other),
            (ValueKind::Color(_), ValueKind::Number(_))
            | (ValueKind::Color(_), ValueKind::Color(_)) => {
                let self_str = self.to_display_string()?;
                let other_str = other.to_display_string()?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"{self_str} / {other_str}\"."),
                    argument_name: None,
                }))
            }
            _ => default_divided_by(arena, self, other),
        }
    }

    // The SassScript `%` operation.
    //
    // Matches Dart: `Value.modulo` (value.dart) — intentionally undocumented
    // in Dart (`@nodoc`/`@internal`). Only number operands take a remainder;
    // everything else is an `Undefined operation` error.
    pub fn modulo<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(a.modulo_num(b)?),
            )),
            _ => default_modulo(arena, self, other),
        }
    }

    // The SassScript `=` operation.
    //
    // Matches Dart: `Value.singleEquals` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Concatenates both sides in
    // CSS form with `=`.
    pub fn single_equals<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        default_single_equals(arena, self, other)
    }

    // The SassScript `>` operation.
    //
    // Matches Dart: `Value.greaterThan` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Only number operands
    // compare; everything else is an `Undefined operation` error.
    pub fn greater_than<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(SassBoolean::new(a.greater_than_num(b)?)),
            )),
            _ => default_greater_than(arena, self, other),
        }
    }

    // The SassScript `>=` operation.
    //
    // Matches Dart: `Value.greaterThanOrEquals` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Only number operands
    // compare; everything else is an `Undefined operation` error.
    pub fn greater_than_or_equals<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(SassBoolean::new(a.greater_than_or_equals_num(b)?)),
            )),
            _ => default_greater_than_or_equals(arena, self, other),
        }
    }

    // The SassScript `<` operation.
    //
    // Matches Dart: `Value.lessThan` (value.dart) — intentionally undocumented
    // in Dart (`@nodoc`/`@internal`). Only number operands compare; everything
    // else is an `Undefined operation` error.
    pub fn less_than<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(SassBoolean::new(a.less_than_num(b)?)),
            )),
            _ => default_less_than(arena, self, other),
        }
    }

    // The SassScript `<=` operation.
    //
    // Matches Dart: `Value.lessThanOrEquals` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Only number operands
    // compare; everything else is an `Undefined operation` error.
    pub fn less_than_or_equals<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
        other: &ValueKind<'parse>,
    ) -> SassResult<Value<'parse>> {
        match (self, other) {
            (ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(SassBoolean::new(a.less_than_or_equals_num(b)?)),
            )),
            _ => default_less_than_or_equals(arena, self, other),
        }
    }

    // The SassScript unary `+` operation.
    //
    // Matches Dart: `Value.unaryPlus` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Numbers pass through;
    // other values stringify with a `+` prefix.
    pub fn unary_plus<'compile: 'parse>(&self, arena: &'compile Bump) -> SassResult<Value<'parse>> {
        match self {
            ValueKind::Number(_) => Ok(Value::new_with_arena(arena, self.clone())),
            ValueKind::Calculation(c) => {
                let c_str = c.to_css_string(true)?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"+{}\".", c_str),
                    argument_name: None,
                }))
            }
            _ => default_unary_plus(arena, self),
        }
    }

    // The SassScript unary `-` operation.
    //
    // Matches Dart: `Value.unaryMinus` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Numbers negate; other
    // values stringify with a `-` prefix.
    pub fn unary_minus<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
    ) -> SassResult<Value<'parse>> {
        match self {
            ValueKind::Number(a) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Number(a.unary_minus_num()),
            )),
            ValueKind::Calculation(c) => {
                let c_str = c.to_css_string(true)?;
                Err(Box::new(SassError::Script {
                    message: format!("Undefined operation \"-{}\".", c_str),
                    argument_name: None,
                }))
            }
            _ => default_unary_minus(arena, self),
        }
    }

    // The SassScript unary `/` operation.
    //
    // Matches Dart: `Value.unaryDivide` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Stringifies `self` in CSS
    // form with a `/` prefix.
    pub fn unary_divide<'compile: 'parse>(
        &self,
        arena: &'compile Bump,
    ) -> SassResult<Value<'parse>> {
        default_unary_divide(arena, self)
    }

    // The SassScript unary `not` operation.
    //
    // Matches Dart: `Value.unaryNot` (value.dart) — intentionally
    // undocumented in Dart (`@nodoc`/`@internal`). Booleans negate, null
    // counts as falsy, and every other value is truthy.
    pub fn unary_not<'compile: 'parse>(&self, arena: &'compile Bump) -> SassResult<Value<'parse>> {
        match self {
            ValueKind::Boolean(b) => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(b.unary_not()),
            )),
            ValueKind::Null => Ok(Value::new_with_arena(
                arena,
                ValueKind::Boolean(SassNull.unary_not()),
            )),
            _ => Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE))),
        }
    }
}

/// Returns a reference to the boolean in `v`.
///
/// Returns an error if `v` isn't a boolean. Prefer
/// [`ValueKind::is_truthy`] over requiring a literal boolean when any value
/// will do.
///
/// If the value came from a function argument, `name` is the argument name
/// (without the `$`), used for error reporting.
pub fn assert_boolean<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassBoolean> {
    match v {
        ValueKind::Boolean(b) => Ok(b),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a boolean.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the number in `v`.
///
/// Returns an error if `v` isn't a number. If the value came from a function
/// argument, `name` is the argument name (without the `$`), used for error
/// reporting.
pub fn assert_number<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassNumber> {
    match v {
        ValueKind::Number(n) => Ok(n),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a number.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the string in `v`.
///
/// Returns an error if `v` isn't a string. If the value came from a function
/// argument, `name` is the argument name (without the `$`), used for error
/// reporting.
pub fn assert_string<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassString<'parse>> {
    match v {
        ValueKind::String(s) => Ok(s),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a string.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the color in `v`.
///
/// Returns an error if `v` isn't a color. If the value came from a function
/// argument, `name` is the argument name (without the `$`), used for error
/// reporting.
pub fn assert_color<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassColor> {
    match v {
        ValueKind::Color(c) => Ok(c),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a color.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the list in `v`, viewing an argument list as its
/// underlying list.
///
/// Returns an error if `v` isn't a list. If the value came from a function
/// argument, `name` is the argument name (without the `$`), used for error
/// reporting.
pub fn assert_list<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassList<'parse>> {
    match v {
        ValueKind::List(l) => Ok(l),
        ValueKind::ArgumentList(a) => Ok(&a.list),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a list.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a copy of the map in `v`, treating empty lists as empty maps.
///
/// Returns an owned [`SassMap`] (rather than a borrow) because an empty list
/// has no inner map to borrow. Returns an error if `v` is neither a map nor
/// an empty list. If the value came from a function argument, `name` is the
/// argument name (without the `$`), used for error reporting.
pub fn assert_map<'parse>(
    v: &ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<SassMap<'parse>> {
    match v {
        ValueKind::Map(m) => Ok(m.clone()),
        ValueKind::List(l) if l.contents.is_empty() => Ok(SassMap::empty()),
        // Dart `SassList.assertMap` is inherited by `SassArgumentList`: an
        // empty argument list counts as an empty map (per SassList.assertMap inheritance;
        // `try_map` already covers both — this arm was accidentally omitted).
        ValueKind::ArgumentList(a) if a.list.contents.is_empty() => Ok(SassMap::empty()),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a map.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the function value in `v`.
///
/// Returns an error if `v` isn't a function reference. If the value came
/// from a function argument, `name` is the argument name (without the `$`),
/// used for error reporting.
pub fn assert_function<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassFunction<'parse>> {
    match v {
        ValueKind::Function(f) => Ok(f),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a function reference.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the mixin value in `v`.
///
/// Returns an error if `v` isn't a mixin reference. If the value came from
/// a function argument, `name` is the argument name (without the `$`), used
/// for error reporting.
pub fn assert_mixin<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassMixin<'parse>> {
    match v {
        ValueKind::Mixin(m) => Ok(m),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a mixin reference.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

/// Returns a reference to the calculation in `v`.
///
/// Matches Dart: `Value.assertCalculation`.
/// Matches Go: `AssertCalculation`.
///
/// Returns an error if `v` isn't a calculation. If the value came from a
/// function argument, `name` is the argument name (without the `$`), used for
/// error reporting.
pub fn assert_calculation<'v, 'parse>(
    v: &'v ValueKind<'parse>,
    name: Option<&str>,
) -> SassResult<&'v SassCalculation> {
    match v {
        ValueKind::Calculation(c) => Ok(c),
        _ => Err(Box::new(SassError::Script {
            message: format!("{} is not a calculation.", v.to_display_string()?),
            argument_name: name.map(str::to_string),
        })),
    }
}

// Fallback for the SassScript `=` operation: concatenates both sides in CSS
// form with `=`.
//
// Matches Dart: `Value.singleEquals` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_single_equals<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let left = self_.to_css_string(true)?;
    let right = other.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&format!("{left}={right}")),
            false,
        )),
    ))
}

// Fallback for the SassScript `+` operation: stringifies a string operand
// preserving its quotes, errors on calculations, and otherwise concatenates
// both sides in CSS form.
//
// Matches Dart: `Value.plus` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_plus<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    if let ValueKind::String(s) = other {
        let left = self_.to_css_string(true)?;
        let has_quotes = s.has_quotes;
        return Ok(Value::new_with_arena(
            arena,
            ValueKind::String(SassString::new(
                arena.alloc_str(&format!("{left}{}", s.text)),
                has_quotes,
            )),
        ));
    }
    if matches!(other, ValueKind::Calculation(_)) {
        let self_str = self_.to_display_string()?;
        let other_str = other.to_display_string()?;
        return Err(Box::new(SassError::Script {
            message: format!("Undefined operation \"{self_str} + {other_str}\"."),
            argument_name: None,
        }));
    }
    let left = self_.to_css_string(true)?;
    let right = other.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&format!("{left}{right}")),
            false,
        )),
    ))
}

// Fallback for the SassScript `-` operation: errors on calculations and
// otherwise concatenates both sides in CSS form with a `-`.
//
// Matches Dart: `Value.minus` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_minus<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    if matches!(other, ValueKind::Calculation(_)) {
        let self_str = self_.to_display_string()?;
        let other_str = other.to_display_string()?;
        return Err(Box::new(SassError::Script {
            message: format!("Undefined operation \"{self_str} - {other_str}\"."),
            argument_name: None,
        }));
    }
    let left = self_.to_css_string(true)?;
    let right = other.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&format!("{left}-{right}")),
            false,
        )),
    ))
}

// Fallback for the SassScript `*` operation: always an `Undefined operation`
// error.
//
// Matches Dart: `Value.times` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_times<'compile: 'parse, 'parse>(
    _arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let self_str = self_.to_display_string()?;
    let other_str = other.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{self_str} * {other_str}\"."),
        argument_name: None,
    }))
}

// Fallback for the SassScript `/` operation: concatenates both sides in CSS
// form with a `/`.
//
// Matches Dart: `Value.dividedBy` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_divided_by<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let left = self_.to_css_string(true)?;
    let right = other.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(
            arena.alloc_str(&format!("{left}/{right}")),
            false,
        )),
    ))
}

// Fallback for the SassScript `%` operation: always an `Undefined operation`
// error.
//
// Matches Dart: `Value.modulo` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_modulo<'compile: 'parse, 'parse>(
    _arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let self_str = self_.to_display_string()?;
    let other_str = other.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{self_str} % {other_str}\"."),
        argument_name: None,
    }))
}

// Fallback for the SassScript `>` operation: always an `Undefined operation`
// error.
//
// Matches Dart: `Value.greaterThan` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_greater_than<'compile: 'parse, 'parse>(
    _arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let self_str = self_.to_display_string()?;
    let other_str = other.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{self_str} > {other_str}\"."),
        argument_name: None,
    }))
}

// Fallback for the SassScript `>=` operation: always an `Undefined
// operation` error.
//
// Matches Dart: `Value.greaterThanOrEquals` base implementation (value.dart)
// — intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_greater_than_or_equals<'compile: 'parse, 'parse>(
    _arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let self_str = self_.to_display_string()?;
    let other_str = other.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{self_str} >= {other_str}\"."),
        argument_name: None,
    }))
}

// Fallback for the SassScript `<` operation: always an `Undefined operation`
// error.
//
// Matches Dart: `Value.lessThan` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_less_than<'compile: 'parse, 'parse>(
    _arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let self_str = self_.to_display_string()?;
    let other_str = other.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{self_str} < {other_str}\"."),
        argument_name: None,
    }))
}

// Fallback for the SassScript `<=` operation: always an `Undefined
// operation` error.
//
// Matches Dart: `Value.lessThanOrEquals` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_less_than_or_equals<'compile: 'parse, 'parse>(
    _arena: &'compile Bump,
    self_: &ValueKind<'parse>,
    other: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let self_str = self_.to_display_string()?;
    let other_str = other.to_display_string()?;
    Err(Box::new(SassError::Script {
        message: format!("Undefined operation \"{self_str} <= {other_str}\"."),
        argument_name: None,
    }))
}

// Fallback for the SassScript unary `+` operation: stringifies `self` in
// CSS form with a `+` prefix.
//
// Matches Dart: `Value.unaryPlus` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_unary_plus<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let s = self_.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&format!("+{s}")), false)),
    ))
}

// Fallback for the SassScript unary `-` operation: stringifies `self` in
// CSS form with a `-` prefix.
//
// Matches Dart: `Value.unaryMinus` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_unary_minus<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let s = self_.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&format!("-{s}")), false)),
    ))
}

// Fallback for the SassScript unary `/` operation: stringifies `self` in
// CSS form with a `/` prefix.
//
// Matches Dart: `Value.unaryDivide` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_unary_divide<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    self_: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    let s = self_.to_css_string(true)?;
    Ok(Value::new_with_arena(
        arena,
        ValueKind::String(SassString::new(arena.alloc_str(&format!("/{s}")), false)),
    ))
}

// Fallback for the SassScript unary `not` operation: every non-boolean,
// non-null value is truthy, so this returns false.
//
// Matches Dart: `Value.unaryNot` base implementation (value.dart) —
// intentionally undocumented in Dart (`@nodoc`/`@internal`).
pub fn default_unary_not<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    _self: &ValueKind<'parse>,
) -> SassResult<Value<'parse>> {
    Ok(Value::new_with_arena(arena, ValueKind::Boolean(SASS_FALSE)))
}

/// Returns a new list containing `contents` that defaults to this value's
/// separator and brackets.
pub fn with_list_contents<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    v: &Value<'parse>,
    contents: Vec<Value<'parse>>,
    separator: Option<ListSeparator>,
    brackets: Option<bool>,
) -> Value<'parse> {
    let sep = separator.unwrap_or_else(|| v.separator());
    let has_brackets = brackets.unwrap_or_else(|| v.has_brackets());
    Value::new_with_arena(
        arena,
        ValueKind::List(SassList::new(contents, sep, has_brackets)),
    )
}

/// Converts `index` into an index into the list returned by
/// [`ValueKind::as_list`].
///
/// Sass indexes are one-based, while Rust indexes are zero-based. Sass
/// indexes may also be negative in order to index from the end of the list.
///
/// Returns an error if `index` isn't a number, if that number isn't an
/// integer, or if that integer isn't a valid index for
/// [`ValueKind::as_list`]. If `index` came from a function argument, `name`
/// is the argument name (without the `$`), used for error reporting.
pub fn sass_index_to_list_index<'parse>(
    v: &ValueKind<'parse>,
    index: &ValueKind<'parse>,
    name: &str,
    warn_logger: &dyn WarnLogger<'parse>,
) -> SassResult<usize> {
    let index_number = assert_number(index, Some(name))?;
    if index_number.has_units() {
        let unit = index_number.unit_string();
        let suggestion = index_number.unit_suggestion(name, None);
        warn_logger.warn_deprecation(
            &format!(
                "${name}: Passing a number with unit {unit} is deprecated.\n\n\
                 To preserve current behavior: {suggestion}\n\n\
                 More info: https://sass-lang.com/d/function-units"
            ),
            &FUNCTION_UNITS,
            None,
        );
    }
    let int_index = index_number.assert_int(Some(name))?;
    if int_index == 0 {
        return Err(Box::new(SassError::Script {
            message: "List index may not be 0.".to_string(),
            argument_name: Some(name.to_string()),
        }));
    }
    let length = v.length_as_list();
    let abs_index = int_index.unsigned_abs() as usize;
    if abs_index > length {
        let index_str = index.to_display_string()?;
        return Err(Box::new(SassError::Script {
            message: format!("Invalid index {index_str} for a list with {length} elements."),
            argument_name: Some(name.to_string()),
        }));
    }
    if int_index < 0 {
        Ok(length - (int_index.unsigned_abs() as usize))
    } else {
        Ok((int_index - 1) as usize)
    }
}

// Returns an error if `v` isn't a list of the sort commonly used in plain
// CSS expression syntax: space-separated and unbracketed.
//
// If `allow_slash` is `true`, slash-separated lists are allowed as well.
// If the value came from a function argument, `name` is the argument name
// (without the `$`), used for error reporting.
//
// Matches Dart: `Value.assertCommonListStyle` (value.dart) — intentionally
// undocumented in Dart (`@nodoc`/`@internal`).
pub fn assert_common_list_style<'compile: 'parse, 'parse>(
    arena: &'compile Bump,
    v: &Value<'parse>,
    name: &str,
    allow_slash: bool,
) -> SassResult<Vec<Value<'parse>>> {
    let invalid_separator = v.separator() == ListSeparator::Comma
        || (!allow_slash && v.separator() == ListSeparator::Slash);
    if !invalid_separator && !v.has_brackets() {
        return v.as_list(arena);
    }

    let mut msg = String::new();
    msg.push_str("Expected");
    if v.has_brackets() {
        msg.push_str(" an unbracketed");
    }
    if invalid_separator {
        if v.has_brackets() {
            msg.push(',');
        } else {
            msg.push_str(" a");
        }
        msg.push_str(" space-");
        if allow_slash {
            msg.push_str(" or slash-");
        }
        msg.push_str("separated");
    }
    let _ = write!(msg, " list, was {}", v.to_display_string()?);
    if name.is_empty() {
        Err(Box::new(SassError::Script {
            message: msg,
            argument_name: None,
        }))
    } else {
        Err(Box::new(SassError::Script {
            message: msg,
            argument_name: Some(name.to_string()),
        }))
    }
}

/// Builds a `$name: <value> is not <description>.` error message.
pub fn error_message(name: &str, value: &ValueKind<'_>, description: &str) -> SassResult<String> {
    let v_str = value.to_display_string()?;
    Ok(format!("${name}: {v_str} is not {description}."))
}

impl Hash for ValueKind<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(self.hash_code())
    }
}

impl PartialEq for ValueKind<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl Eq for ValueKind<'_> {}

impl fmt::Display for ValueKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Basic display for debugging; full inspect serialization via SerializeVisitor
        match self {
            ValueKind::Boolean(b) => write!(f, "{}", b.value),
            ValueKind::Null => write!(f, "null"),
            ValueKind::String(s) => write!(f, "{}", s.text),
            ValueKind::Number(n) => write!(f, "{:?}", n),
            ValueKind::Color(_) => write!(f, "color"),
            ValueKind::List(l) => write!(f, "list[{}]", l.contents.len()),
            ValueKind::ArgumentList(a) => write!(f, "arglist[{}]", a.list.contents.len()),
            ValueKind::Map(_) => write!(f, "map"),
            ValueKind::Function(_) => write!(f, "get-function(...)"),
            ValueKind::Mixin(_) => write!(f, "get-mixin(...)"),
            ValueKind::Calculation(c) => write!(f, "{}(...)", c.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::test_utils::test_callable;
    use crate::logger::BufferedWarnLogger;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    #[test]
    fn test_value_is_truthy() {
        let arena = Bump::new();
        assert!(ValueKind::Boolean(SASS_TRUE).is_truthy());
        assert!(!ValueKind::Boolean(SASS_FALSE).is_truthy());
        assert!(!ValueKind::Null.is_truthy());
        assert!(ValueKind::unitless_number(&arena, 1.0).is_truthy());
    }

    #[test]
    fn test_value_separator() {
        let arena = Bump::new();
        assert_eq!(ValueKind::Null.separator(), ListSeparator::Undecided);
        let l = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(vec![], ListSeparator::Comma, false)),
        );
        assert_eq!(l.separator(), ListSeparator::Comma);
    }

    #[test]
    fn test_value_has_brackets() {
        let arena = Bump::new();
        assert!(!ValueKind::Null.has_brackets());
        let l = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(vec![], ListSeparator::Space, true)),
        );
        assert!(l.has_brackets());
    }

    #[test]
    fn test_value_length_as_list() {
        let arena = Bump::new();
        assert_eq!(ValueKind::Null.length_as_list(), 1);
        let l = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![
                    ValueKind::unitless_number(&arena, 1.0),
                    ValueKind::unitless_number(&arena, 2.0),
                ],
                ListSeparator::Space,
                false,
            )),
        );
        assert_eq!(l.length_as_list(), 2);
    }

    #[test]
    fn test_value_is_blank() {
        let arena = Bump::new();
        assert!(ValueKind::Null.is_blank());
        assert!(!ValueKind::unitless_number(&arena, 1.0).is_blank());
        let s = SassString::new(arena.alloc_str(""), false);
        assert!(ValueKind::String(s).is_blank());
    }

    #[test]
    fn test_value_hash_and_eq() {
        let arena = Bump::new();
        let a = ValueKind::unitless_number(&arena, 1.0);
        let b = ValueKind::unitless_number(&arena, 1.0);
        assert_eq!(a, b);
        assert_eq!(a.hash_code(), b.hash_code());

        let mut hasher_a = DefaultHasher::new();
        a.hash(&mut hasher_a);
        let mut hasher_b = DefaultHasher::new();
        b.hash(&mut hasher_b);
        assert_eq!(
            hasher_a.finish(),
            hasher_b.finish(),
            "Hash trait should delegate to hash_code"
        );
    }

    #[test]
    fn test_value_null_equals() {
        let arena = Bump::new();
        assert_eq!(ValueKind::Null, ValueKind::Null);
        assert_ne!(
            Value::new_with_arena(&arena, ValueKind::Null),
            ValueKind::unitless_number(&arena, 0.0)
        );
        assert_ne!(ValueKind::Null, ValueKind::Boolean(SASS_FALSE));
    }

    // Matches Go: TestListTryMap, TestMapTryMap, TestArgListTryMap
    #[test]
    fn test_value_try_map_on_map() {
        let arena = Bump::new();
        let m = SassMap::from_entries(vec![(
            ValueKind::unitless_number(&arena, 1.0),
            ValueKind::unitless_number(&arena, 2.0),
        )]);
        let v = Value::new_with_arena(&arena, ValueKind::Map(m.clone()));
        let got = v.try_map().expect("map try_map should return the map");
        assert!(got.equals(&m));
    }

    #[test]
    fn test_value_try_map_on_empty_map() {
        let arena = Bump::new();
        let v = Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty()));
        let got = v
            .try_map()
            .expect("empty map try_map should return the map");
        assert_eq!(got.len(), 0);
    }

    #[test]
    fn test_value_try_map_on_empty_list() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::empty(ListSeparator::Space, false)),
        );
        let got = v
            .try_map()
            .expect("empty list try_map should return an empty map");
        assert_eq!(got.len(), 0);
    }

    #[test]
    fn test_value_try_map_on_non_empty_list() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![ValueKind::unitless_number(&arena, 1.0)],
                ListSeparator::Space,
                false,
            )),
        );
        assert!(v.try_map().is_none());
    }

    #[test]
    fn test_value_try_map_on_empty_argument_list() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![],
                indexmap::IndexMap::new(),
                ListSeparator::Comma,
            )),
        );
        let got = v
            .try_map()
            .expect("empty argument list try_map should return an empty map");
        assert_eq!(got.len(), 0);
    }

    #[test]
    fn test_value_try_map_on_non_empty_argument_list() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![ValueKind::unitless_number(&arena, 1.0)],
                indexmap::IndexMap::new(),
                ListSeparator::Comma,
            )),
        );
        assert!(v.try_map().is_none());
    }

    #[test]
    fn test_value_try_map_on_scalar() {
        let arena = Bump::new();
        assert!(ValueKind::Null.try_map().is_none());
        assert!(ValueKind::unitless_number(&arena, 1.0).try_map().is_none());
        assert!(ValueKind::Boolean(SASS_TRUE).try_map().is_none());
    }

    // Matches Go: TestListAsList, TestMapAsList, TestBooleanAsList
    #[test]
    fn test_value_as_list_on_list() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![
                    ValueKind::unitless_number(&arena, 1.0),
                    ValueKind::unitless_number(&arena, 2.0),
                ],
                ListSeparator::Space,
                false,
            )),
        );
        let got = v.as_list(&arena).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ValueKind::unitless_number(&arena, 1.0));
        assert_eq!(got[1], ValueKind::unitless_number(&arena, 2.0));
    }

    #[test]
    fn test_value_as_list_on_argument_list() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![
                    ValueKind::unitless_number(&arena, 1.0),
                    ValueKind::unitless_number(&arena, 2.0),
                ],
                indexmap::IndexMap::new(),
                ListSeparator::Comma,
            )),
        );
        let got = v.as_list(&arena).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn test_value_as_list_on_map() {
        let arena = Bump::new();
        let m = SassMap::from_entries(vec![(
            Value::new_with_arena(
                &arena,
                ValueKind::String(SassString::new(arena.alloc_str("a"), true)),
            ),
            ValueKind::unitless_number(&arena, 1.0),
        )]);
        let got = Value::new_with_arena(&arena, ValueKind::Map(m))
            .as_list(&arena)
            .unwrap();
        assert_eq!(got.len(), 1);
        match &*got[0] {
            ValueKind::List(pair) => {
                assert_eq!(pair.length_as_list(), 2);
                assert_eq!(pair.separator, ListSeparator::Space);
            }
            other => panic!("as_list element should be a list, got {other:?}"),
        }
    }

    #[test]
    fn test_value_as_list_on_scalar() {
        let arena = Bump::new();
        let got = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))
            .as_list(&arena)
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0],
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))
        );

        let got = Value::new_with_arena(&arena, ValueKind::Null)
            .as_list(&arena)
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], Value::new_with_arena(&arena, ValueKind::Null));
    }

    #[test]
    fn test_value_list_equals_map_empty() {
        let arena = Bump::new();
        let m = Value::new_with_arena(&arena, ValueKind::Map(SassMap::empty()));
        let l = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::empty(ListSeparator::Space, false)),
        );
        assert_eq!(m, l, "empty map should equal empty list");
        assert_eq!(l, m, "empty list should equal empty map");
    }

    #[test]
    fn test_value_list_equals_arglist() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        let l = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(vec![v], ListSeparator::Space, false)),
        );
        let a = Value::new_with_arena(
            &arena,
            ValueKind::ArgumentList(SassArgumentList::new(
                &arena,
                vec![v],
                indexmap::IndexMap::new(),
                ListSeparator::Space,
            )),
        );
        assert_eq!(l, a, "list should equal arglist with same contents");
    }

    #[test]
    fn test_value_function_equals() {
        let arena = Bump::new();
        // Equality is callable identity (Dart: callable == other.callable).
        let callable = test_callable(&arena, "f");
        let f1 = Value::new_with_arena(&arena, ValueKind::Function(SassFunction::new(callable)));
        let f2 = Value::new_with_arena(&arena, ValueKind::Function(SassFunction::new(callable)));
        assert_eq!(f1, f2);
        let f3 = Value::new_with_arena(
            &arena,
            ValueKind::Function(SassFunction::new(test_callable(&arena, "f"))),
        );
        assert_ne!(f1, f3);
    }

    #[test]
    fn test_value_mixin_equals() {
        let arena = Bump::new();
        let callable = test_callable(&arena, "m");
        let m1 = Value::new_with_arena(&arena, ValueKind::Mixin(SassMixin::new(callable)));
        let m2 = Value::new_with_arena(&arena, ValueKind::Mixin(SassMixin::new(callable)));
        assert_eq!(m1, m2);
        let m3 = Value::new_with_arena(
            &arena,
            ValueKind::Mixin(SassMixin::new(test_callable(&arena, "m"))),
        );
        assert_ne!(m1, m3);
    }

    #[test]
    fn test_value_string_equals_ignores_quotes() {
        let arena = Bump::new();
        let a = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("hello"), true)),
        );
        let b = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("hello"), false)),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn test_value_unary_not() {
        let arena = Bump::new();
        assert_eq!(
            ValueKind::Boolean(SASS_TRUE).unary_not(&arena).unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE))
        );
        assert_eq!(
            ValueKind::Boolean(SASS_FALSE).unary_not(&arena).unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))
        );
        assert_eq!(
            ValueKind::Null.unary_not(&arena).unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))
        );
        assert_eq!(
            ValueKind::unitless_number(&arena, 42.0)
                .unary_not(&arena)
                .unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE))
        );
    }

    #[test]
    fn test_value_real_null() {
        let arena = Bump::new();
        assert!(ValueKind::Null.real_null().is_none());
        assert!(ValueKind::unitless_number(&arena, 1.0)
            .real_null()
            .is_some());
        assert!(ValueKind::Boolean(SASS_TRUE).real_null().is_some());
    }

    #[test]
    fn test_value_plus_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, Some("px"))));
        let b = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(3.0, Some("px"))));
        let r = a.plus(&arena, &b).unwrap();
        match &*r {
            ValueKind::Number(n) => {
                assert!((n.value - 8.0).abs() < 1e-9);
                assert!(n.has_unit("px"));
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_value_plus_string_string() {
        let arena = Bump::new();
        let a = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("hello"), true)),
        );
        let b = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str(" world"), true)),
        );
        let r = a.plus(&arena, &b).unwrap();
        match &*r {
            ValueKind::String(s) => {
                assert_eq!(s.text, "hello world");
                assert!(s.has_quotes);
            }
            _ => panic!("expected String"),
        }
    }

    #[test]
    fn test_value_minus_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, Some("px"))));
        let b = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(3.0, Some("px"))));
        let r = a.minus(&arena, &b).unwrap();
        match &*r {
            ValueKind::Number(n) => assert!((n.value - 2.0).abs() < 1e-9),
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_value_times_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, Some("px"))));
        let b = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(3.0, None)));
        let r = a.times(&arena, &b).unwrap();
        match &*r {
            ValueKind::Number(n) => {
                assert!((n.value - 15.0).abs() < 1e-9);
                assert!(n.has_unit("px"));
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_value_divided_by_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(10.0, Some("px"))));
        let b = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(2.0, None)));
        let r = a.divided_by(&arena, &b).unwrap();
        match &*r {
            ValueKind::Number(n) => {
                assert!((n.value - 5.0).abs() < 1e-9);
                assert!(n.has_unit("px"));
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_value_modulo_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(7.0, Some("px"))));
        let b = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(3.0, Some("px"))));
        let r = a.modulo(&arena, &b).unwrap();
        match &*r {
            ValueKind::Number(n) => assert!((n.value - 1.0).abs() < 1e-9),
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_value_comparison() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, Some("px"))));
        let b = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(3.0, Some("px"))));
        assert_eq!(
            a.greater_than(&arena, &b).unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))
        );
        assert_eq!(
            a.less_than(&arena, &b).unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE))
        );
    }

    #[test]
    fn test_value_unary_plus_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, Some("px"))));
        let r = a.unary_plus(&arena).unwrap();
        match &*r {
            ValueKind::Number(n) => {
                assert!((n.value - 5.0).abs() < 1e-9);
                assert!(n.has_unit("px"));
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_value_unary_minus_number() {
        let arena = Bump::new();
        let a = Value::new_with_arena(&arena, ValueKind::Number(SassNumber::new(5.0, Some("px"))));
        let r = a.unary_minus(&arena).unwrap();
        match &*r {
            ValueKind::Number(n) => {
                assert!((n.value + 5.0).abs() < 1e-9);
                assert!(n.has_unit("px"));
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn test_with_list_contents() {
        let arena = Bump::new();
        let l = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![ValueKind::unitless_number(&arena, 1.0)],
                ListSeparator::Space,
                true,
            )),
        );
        let result = with_list_contents(
            &arena,
            &l,
            vec![
                ValueKind::unitless_number(&arena, 2.0),
                ValueKind::unitless_number(&arena, 3.0),
            ],
            None,
            None,
        );
        assert!(result.has_brackets());
        assert_eq!(result.separator(), ListSeparator::Space);
        assert_eq!(result.length_as_list(), 2);

        let result2 = with_list_contents(
            &arena,
            &l,
            vec![ValueKind::unitless_number(&arena, 4.0)],
            Some(ListSeparator::Comma),
            Some(false),
        );
        assert_eq!(result2.separator(), ListSeparator::Comma);
        assert!(!result2.has_brackets());
    }

    #[test]
    fn test_default_unary_not() {
        let arena = Bump::new();
        assert_eq!(
            default_unary_not(&arena, &ValueKind::unitless_number(&arena, 1.0)).unwrap(),
            Value::new_with_arena(&arena, ValueKind::Boolean(SASS_FALSE))
        );
    }

    #[test]
    fn test_assert_boolean() {
        let arena = Bump::new();
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE));
        assert!(assert_boolean(&v, None).is_ok());
        let v = ValueKind::unitless_number(&arena, 1.0);
        assert!(assert_boolean(&v, None).is_err());
    }

    #[test]
    fn test_assert_number() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        assert!(assert_number(&v, None).is_ok());
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE));
        assert!(assert_number(&v, None).is_err());
    }

    #[test]
    fn test_assert_string() {
        let arena = Bump::new();
        let v = Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("hello"), false)),
        );
        assert!(assert_string(&v, None).is_ok());
        let v = ValueKind::unitless_number(&arena, 1.0);
        assert!(assert_string(&v, None).is_err());
    }

    // --- Script error field split (locked in Go: value_test.go TestAssert*Fields) ---
    //
    // Assert* functions put the bare message in `message` and the argument
    // name in `argument_name`; `full_message()` composes "$name: message".

    fn assert_fields(err: Box<SassError>, want_msg: &str, want_arg: Option<&str>) {
        match *err {
            SassError::Script {
                message,
                argument_name,
            } => {
                assert_eq!(message, want_msg);
                assert_eq!(argument_name.as_deref(), want_arg);
            }
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    #[test]
    fn test_assert_number_fields() {
        let arena = Bump::new();
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE));
        let err = assert_number(&v, Some("x")).unwrap_err();
        assert_eq!(err.full_message(), "$x: true is not a number.");
        assert_fields(err, "true is not a number.", Some("x"));

        let err = assert_number(&v, None).unwrap_err();
        assert_fields(err, "true is not a number.", None);
    }

    #[test]
    fn test_assert_boolean_fields() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        let err = assert_boolean(&v, Some("cond")).unwrap_err();
        assert_fields(err, "1 is not a boolean.", Some("cond"));
    }

    #[test]
    fn test_assert_string_fields() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        let err = assert_string(&v, Some("string")).unwrap_err();
        assert_fields(err, "1 is not a string.", Some("string"));
    }

    #[test]
    fn test_assert_color_fields() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 42.0);
        let err = assert_color(&v, Some("color")).unwrap_err();
        assert_fields(err, "42 is not a color.", Some("color"));
    }

    #[test]
    fn test_assert_list_fields() {
        let arena = Bump::new();
        let v = Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE));
        let err = assert_list(&v, Some("list")).unwrap_err();
        assert_fields(err, "true is not a list.", Some("list"));
    }

    #[test]
    fn test_assert_map_fields() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        let err = assert_map(&v, Some("map")).unwrap_err();
        assert_fields(err, "1 is not a map.", Some("map"));
    }

    // Dart `SassList.assertMap` is inherited by `SassArgumentList`: an empty
    // argument list counts as an empty map (per SassList.assertMap inheritance).
    #[test]
    fn test_assert_map_empty_argument_list() {
        let arena = Bump::new();
        let empty_args = ValueKind::ArgumentList(SassArgumentList::new(
            &arena,
            vec![],
            Default::default(),
            ListSeparator::Comma,
        ));
        let m = assert_map(&empty_args, None).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn test_assert_function_fields() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        let err = assert_function(&v, Some("function")).unwrap_err();
        assert_fields(err, "1 is not a function reference.", Some("function"));
    }

    #[test]
    fn test_assert_mixin_fields() {
        let arena = Bump::new();
        let v = ValueKind::unitless_number(&arena, 1.0);
        let err = assert_mixin(&v, Some("mixin")).unwrap_err();
        assert_fields(err, "1 is not a mixin reference.", Some("mixin"));
    }

    #[test]
    fn test_default_operators() {
        let arena = Bump::new();
        let a = ValueKind::unitless_number(&arena, 1.0);
        let b = ValueKind::unitless_number(&arena, 2.0);
        assert!(a.plus(&arena, &b).is_ok());
        assert!(a.minus(&arena, &b).is_ok());
        assert!(a.times(&arena, &b).is_ok());
        assert!(a.divided_by(&arena, &b).is_ok());
    }

    #[test]
    fn test_sass_index_to_list_index() {
        let arena = Bump::new();
        let list = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![
                    ValueKind::unitless_number(&arena, 1.0),
                    ValueKind::unitless_number(&arena, 2.0),
                ],
                ListSeparator::Space,
                false,
            )),
        );
        let warn_buf = BufferedWarnLogger::new(&arena);
        assert_eq!(
            sass_index_to_list_index(
                &list,
                &ValueKind::unitless_number(&arena, 1.0),
                "idx",
                &warn_buf
            )
            .unwrap(),
            0
        );
        let warn_buf = BufferedWarnLogger::new(&arena);
        assert_eq!(
            sass_index_to_list_index(
                &list,
                &ValueKind::unitless_number(&arena, 2.0),
                "idx",
                &warn_buf
            )
            .unwrap(),
            1
        );
        let warn_buf = BufferedWarnLogger::new(&arena);
        assert!(sass_index_to_list_index(
            &list,
            &ValueKind::unitless_number(&arena, 0.0),
            "idx",
            &warn_buf
        )
        .is_err());
    }

    #[test]
    fn test_assert_common_list_style() {
        let arena = Bump::new();
        let list = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![ValueKind::unitless_number(&arena, 1.0)],
                ListSeparator::Space,
                false,
            )),
        );
        assert!(assert_common_list_style(&arena, &list, "test", false).is_ok());
        let comma_list = Value::new_with_arena(
            &arena,
            ValueKind::List(SassList::new(
                vec![
                    ValueKind::unitless_number(&arena, 1.0),
                    ValueKind::unitless_number(&arena, 2.0),
                ],
                ListSeparator::Comma,
                false,
            )),
        );
        assert!(assert_common_list_style(&arena, &comma_list, "test", false).is_err());
    }

    // --- accept() dispatch tests ---

    struct TestVisitor<'r> {
        visited: &'r mut String,
    }

    // The visitor's borrow lifetime `'r` is independent from the value
    // content lifetime `'parse` (Value is invariant in `'parse`).
    impl<'parse, 'r> ValueVisitor<'parse> for TestVisitor<'r> {
        type Output = ();
        fn visit_boolean(&mut self, _: &SassBoolean) -> SassResult<()> {
            self.visited.push_str("bool");
            Ok(())
        }
        fn visit_null(&mut self) -> SassResult<()> {
            self.visited.push_str("null");
            Ok(())
        }
        fn visit_string(&mut self, _: &SassString<'parse>) -> SassResult<()> {
            self.visited.push_str("string");
            Ok(())
        }
        fn visit_number(&mut self, _: &SassNumber) -> SassResult<()> {
            self.visited.push_str("number");
            Ok(())
        }
        fn visit_color(&mut self, _: &SassColor) -> SassResult<()> {
            self.visited.push_str("color");
            Ok(())
        }
        fn visit_list(&mut self, v: &ListValue<'_, 'parse>) -> SassResult<()> {
            match v {
                ListValue::List(_) => self.visited.push_str("list"),
                ListValue::ArgumentList(_) => self.visited.push_str("arglist"),
            }
            Ok(())
        }
        fn visit_map(&mut self, _: &SassMap<'parse>) -> SassResult<()> {
            self.visited.push_str("map");
            Ok(())
        }
        fn visit_calculation(&mut self, _: &SassCalculation) -> SassResult<()> {
            self.visited.push_str("calc");
            Ok(())
        }
        fn visit_function(&mut self, _: &SassFunction) -> SassResult<()> {
            self.visited.push_str("function");
            Ok(())
        }
        fn visit_mixin(&mut self, _: &SassMixin) -> SassResult<()> {
            self.visited.push_str("mixin");
            Ok(())
        }
    }

    #[test]
    fn test_accept_dispatches_boolean() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        Value::new_with_arena(&arena, ValueKind::Boolean(SASS_TRUE))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "bool");
    }

    #[test]
    fn test_accept_dispatches_null() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        Value::new_with_arena(&arena, ValueKind::Null)
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "null");
    }

    #[test]
    fn test_accept_dispatches_number() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        ValueKind::unitless_number(&arena, 1.0)
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "number");
    }

    #[test]
    fn test_accept_dispatches_string() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        Value::new_with_arena(
            &arena,
            ValueKind::String(SassString::new(arena.alloc_str("hello"), true)),
        )
        .accept(&mut v)
        .unwrap();
        assert_eq!(visited, "string");
    }

    #[test]
    fn test_accept_dispatches_color() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let c = SassColor::rgb(1.0, 0.0, 0.0, 1.0);
        Value::new_with_arena(&arena, ValueKind::Color(c))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "color");
    }

    #[test]
    fn test_accept_dispatches_list() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let l = SassList::new(vec![], ListSeparator::Space, false);
        Value::new_with_arena(&arena, ValueKind::List(l))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "list");
    }

    #[test]
    fn test_accept_dispatches_argument_list() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let a = SassArgumentList::new(
            &arena,
            vec![],
            indexmap::IndexMap::new(),
            ListSeparator::Space,
        );
        Value::new_with_arena(&arena, ValueKind::ArgumentList(a))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "arglist");
    }

    #[test]
    fn test_accept_dispatches_map() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let m: SassMap<'_> = SassMap::empty();
        Value::new_with_arena(&arena, ValueKind::Map(m))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "map");
    }

    #[test]
    fn test_accept_dispatches_calculation() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let c = SassCalculation::new_unsimplified("calc", vec![]);
        Value::new_with_arena(&arena, ValueKind::Calculation(Box::new(c)))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "calc");
    }

    #[test]
    fn test_accept_dispatches_function() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let f = SassFunction::new(test_callable(&arena, "f"));
        Value::new_with_arena(&arena, ValueKind::Function(f))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "function");
    }

    #[test]
    fn test_accept_dispatches_mixin() {
        let arena = Bump::new();
        let mut visited = String::new();
        let mut v = TestVisitor {
            visited: &mut visited,
        };
        let m = SassMixin::new(test_callable(&arena, "m"));
        Value::new_with_arena(&arena, ValueKind::Mixin(m))
            .accept(&mut v)
            .unwrap();
        assert_eq!(visited, "mixin");
    }
}
