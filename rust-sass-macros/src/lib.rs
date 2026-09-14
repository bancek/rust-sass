//! Dual sync/async compilation support for rust-sass.
//!
//! Inlined and trimmed from `maybe-async` 0.2.11 (MIT, © Guoli Lyu,
//! https://github.com/fMeow/maybe-async-rs). Deltas from upstream:
//!
//! Mode selection: **sync is the default**; enabling the `async` feature
//! on this crate compiles the async expansion instead. Downstream crates
//! alias it (`async = ["rust-sass-macros/async"]`) so one feature flows
//! through the whole graph.
//!
//! - **Patch 1** (`visit.rs`): async closures (`async |args| {..}`) have
//!   their `asyncness` stripped in sync mode (upstream ignores closures).
//! - **Patch 2** (`visit.rs`): `-> LocalBoxFuture<'a, T>` return types are
//!   rewritten to `-> T` in sync mode by matching the alias's last path
//!   segment in addition to `Pin`/`Box`.
//! - Deleted: `async-trait` integration (`Send`/`?Send`/`AFIT` modes),
//!   `ReplaceGenericType` machinery, arbitrary-condition `test` parsing.
//! - Added: `maybe_test` with hardcoded conditions for this workspace
//!   (sync = plain `#[test]`, async =
//!   `futures_test::test`).
//!
//! Mode selection: the `async` feature **of this crate**, aliased from
//! `rust-sass`'s own feature (`rust-sass/Cargo.toml`:
//! `async = ["rust-sass-macros/async"]`). Sync is the default (absence of
//! `async`); a unified feature graph always expands consistently because
//! every crate keys on the same feature.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, ImplItem, Item, TraitItem};

mod visit;

use visit::AsyncAwaitRemoval;

/// Strip `async`/`await` (and unwrap boxing) unconditionally.
fn convert_sync(input: &mut Item) -> TokenStream2 {
    match input {
        Item::Impl(item) => {
            for inner in &mut item.items {
                if let ImplItem::Fn(ref mut method) = inner {
                    method.sig.asyncness = None;
                }
            }
            AsyncAwaitRemoval.remove_async_await(quote!(#item))
        }
        Item::Trait(item) => {
            for inner in &mut item.items {
                if let TraitItem::Fn(ref mut method) = inner {
                    method.sig.asyncness = None;
                }
            }
            AsyncAwaitRemoval.remove_async_await(quote!(#item))
        }
        Item::Fn(item) => {
            item.sig.asyncness = None;
            AsyncAwaitRemoval.remove_async_await(quote!(#item))
        }
        Item::Static(item) => AsyncAwaitRemoval.remove_async_await(quote!(#item)),
        // `syn::Item` is non-exhaustive; only fn/trait/impl/static are
        // accepted by `parse::Item`.
        _ => quote!(),
    }
}

/// In async mode: identity. In sync mode: strip async/await.
#[proc_macro_attribute]
pub fn maybe_async(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as Item);
    if !cfg!(feature = "async") {
        convert_sync(&mut item).into()
    } else {
        quote!(#item).into()
    }
}

/// Convert to sync code unconditionally.
#[proc_macro_attribute]
pub fn must_be_sync(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as Item);
    convert_sync(&mut item).into()
}

/// Keep the item only when compiling the sync build.
#[proc_macro_attribute]
pub fn sync_impl(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input = TokenStream2::from(input);
    if !cfg!(feature = "async") {
        quote!(#input).into()
    } else {
        quote!().into()
    }
}

/// Keep the item only when compiling the async build.
#[proc_macro_attribute]
pub fn async_impl(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input = TokenStream2::from(input);
    if !cfg!(feature = "async") {
        quote!().into()
    } else {
        quote!(#input).into()
    }
}

/// Mode-adaptive future driver for plain (non-`async`) contexts: test fns,
/// thread bodies, protocol loops.
///
/// - async build: expands to `::futures::executor::block_on(EXPR)`
/// - sync build (default): `EXPR` is already a plain value; identity.
///
/// The mode is read from THIS crate's feature — which unifies with
/// `rust-sass`'s through the alias chain — so the choice always matches how
/// the library itself was compiled, even in workspace-wide builds where
/// consumer-local `cfg(not(feature = "async"))` cannot see it.
#[proc_macro]
pub fn maybe_block_on(input: TokenStream) -> TokenStream {
    if !cfg!(feature = "async") {
        input
    } else {
        let expr = parse_macro_input!(input as syn::Expr);
        quote!(::futures::executor::block_on(#expr)).into()
    }
}

/// Unified test attribute: runs as a plain `#[test]` (converted to sync) when
/// the `async` feature is off, otherwise as a `#[futures_test::test]`.
///
/// Branches directly on the crate feature (like `maybe_async`) rather than
/// emitting `cfg_attr` — attribute macros inside `cfg_attr` expand in an
/// order that leaves `.await`s unstripped (observed: E0728 in sync builds).
#[proc_macro_attribute]
pub fn maybe_test(_args: TokenStream, input: TokenStream) -> TokenStream {
    if !cfg!(feature = "async") {
        let mut item = parse_macro_input!(input as Item);
        let converted = convert_sync(&mut item);
        quote!(
            #[test]
            #converted
        )
        .into()
    } else {
        let item = TokenStream2::from(input);
        quote!(
            #[futures_test::test]
            #item
        )
        .into()
    }
}
