//! The sync-mode transformer: strips `async`/`await`, unwraps future boxing,
//! and rewrites future return types to their output types.
//!
//! Copied from maybe-async 0.2.11 `src/visit.rs` with:
//! - Patch 1: async closures lose `asyncness` (`visit_expr_closure_mut`).
//! - Patch 2: `LocalBoxFuture` matched in `extract_future_output`.
//! - Deleted: `ReplaceGenericType` + generic-substitution pass (unreachable
//!   for rust-sass and the riskiest upstream code — see plan §3.2).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    visit_mut::{self, VisitMut},
    Expr, ExprBlock, File, GenericArgument, PathArguments, ReturnType, Signature, Stmt, Type,
    TypeParamBound,
};

pub struct AsyncAwaitRemoval;

impl AsyncAwaitRemoval {
    pub fn remove_async_await(&mut self, item: TokenStream) -> TokenStream {
        let mut syntax_tree: File = syn::parse(item.into()).unwrap();
        self.visit_file_mut(&mut syntax_tree);
        quote!(#syntax_tree)
    }
}

impl VisitMut for AsyncAwaitRemoval {
    fn visit_expr_mut(&mut self, node: &mut Expr) {
        // Unwrap `Box::pin(async {..})` / `Box::new(async {..})` BEFORE recursing,
        // so the inner async block remains visible to the Async match-arm below.
        if let Some(unwrapped) = unwrap_box_call_with_async(node) {
            *node = unwrapped;
        }

        // Delegate to the default impl to visit nested expressions.
        visit_mut::visit_expr_mut(self, node);

        match node {
            Expr::Await(expr) => *node = (*expr.base).clone(),

            Expr::Async(expr) => {
                let inner = &expr.block;
                let sync_expr = if let [Stmt::Expr(expr, None)] = inner.stmts.as_slice() {
                    // remove useless braces when there is only one statement
                    expr.clone()
                } else {
                    Expr::Block(ExprBlock {
                        attrs: expr.attrs.clone(),
                        block: inner.clone(),
                        label: None,
                    })
                };
                *node = sync_expr;
            }
            _ => {}
        }
    }

    // Patch 1: `async |args| {..}` closures become plain closures in sync
    // mode. Capture mode (`move`) is untouched — semantics are identical.
    //
    // Patch 2 (closures): syn 2's `ExprClosure` carries its return type as a
    // direct `output: ReturnType` field — there is no `Signature` — so
    // `visit_signature_mut` never fires for closures. Rewrite the output
    // here (`-> LocalBoxFuture<'_, T>` → `-> T`).
    fn visit_expr_closure_mut(&mut self, node: &mut syn::ExprClosure) {
        node.asyncness = None;
        if let syn::ReturnType::Type(arrow, ty) = &node.output {
            if let Some(inner) = extract_future_output(ty) {
                node.output = syn::ReturnType::Type(*arrow, Box::new(inner));
            }
        }
        visit_mut::visit_expr_closure_mut(self, node);
    }

    fn visit_signature_mut(&mut self, sig: &mut Signature) {
        // rewrite `-> impl Future<Output = T> + ...`,
        // `-> Box<dyn Future<Output = T> + ...>`,
        // `-> Pin<Box<dyn Future<Output = T> + ...>>`,
        // `-> LocalBoxFuture<'a, T>` (Patch 2) to `-> T`
        if let ReturnType::Type(arrow, ty) = &sig.output {
            if let Some(inner) = extract_future_output(ty) {
                sig.output = ReturnType::Type(*arrow, Box::new(inner));
            }
        }
        visit_mut::visit_signature_mut(self, sig);
    }
}

/// Extract `T` from any of:
/// - `impl Future<Output = T> + ...`
/// - `Box<dyn Future<Output = T> + ...>`
/// - `Pin<Box<dyn Future<Output = T> + ...>>`
/// - `LocalBoxFuture<'a, T>` (Patch 2 — the crate's `Pin<Box<dyn Future>>`
///   alias; matched by last segment name like `Pin`/`Box`)
///   Paths are matched by last segment name only, so `std::pin::Pin`,
///   `core::pin::Pin`, etc. all match.
fn extract_future_output(ty: &Type) -> Option<Type> {
    match ty {
        Type::ImplTrait(impl_trait) => extract_future_output_from_bounds(impl_trait.bounds.iter()),
        Type::TraitObject(trait_obj) => extract_future_output_from_bounds(trait_obj.bounds.iter()),
        Type::Path(p) => {
            let seg = p.path.segments.last()?;
            let name = seg.ident.to_string();
            let PathArguments::AngleBracketed(args) = &seg.arguments else {
                return None;
            };
            match name.as_str() {
                "Pin" | "Box" => args.args.iter().find_map(|arg| {
                    if let GenericArgument::Type(inner) = arg {
                        extract_future_output(inner)
                    } else {
                        None
                    }
                }),
                // Patch 2: `LocalBoxFuture<'a, T>` is `Pin<Box<dyn Future<Output
                // = T> + 'a>>` — the output is the LAST type argument (the
                // first is a lifetime), so it cannot recurse like Pin/Box.
                "LocalBoxFuture" => args.args.iter().rev().find_map(|arg| {
                    if let GenericArgument::Type(inner) = arg {
                        Some(inner.clone())
                    } else {
                        None
                    }
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_future_output_from_bounds<'a>(
    bounds: impl Iterator<Item = &'a TypeParamBound>,
) -> Option<Type> {
    for bound in bounds {
        let TypeParamBound::Trait(trait_bound) = bound else {
            continue;
        };
        let Some(seg) = trait_bound.path.segments.last() else {
            continue;
        };
        if seg.ident != "Future" {
            continue;
        }
        let PathArguments::AngleBracketed(args) = &seg.arguments else {
            return None;
        };
        for arg in &args.args {
            if let GenericArgument::AssocType(assoc) = arg {
                if assoc.ident == "Output" {
                    return Some(assoc.ty.clone());
                }
            }
        }
    }
    None
}

/// If `node` is `Box::pin(<async block>)` or `Box::new(<async block>)`,
/// return the async block. Path is matched on the last two segments being
/// `Box` then `pin`/`new`, so qualified paths like `::std::boxed::Box::pin`
/// also match. (`Box::pin_in(f, arena)` has two args and is NOT matched —
/// recursion edges on fn calls use the `box_rec!`/`box_rec_in!` companion
/// macros in rust-sass instead.)
fn unwrap_box_call_with_async(node: &Expr) -> Option<Expr> {
    let Expr::Call(call) = node else { return None };
    let Expr::Path(path_expr) = call.func.as_ref() else {
        return None;
    };
    let segs = &path_expr.path.segments;
    if segs.len() < 2 {
        return None;
    }
    let last = segs[segs.len() - 1].ident.to_string();
    let second_last = segs[segs.len() - 2].ident.to_string();
    if second_last != "Box" || (last != "pin" && last != "new") {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    if !matches!(&call.args[0], Expr::Async(_)) {
        return None;
    }
    Some(call.args[0].clone())
}
