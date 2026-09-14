//! Minimal reproduction of the meta.rs callback shape: closures with
//! `-> LocalBoxFuture` return types, `box_rec!` edges to marked async impl
//! fns, all inline inside a `#[maybe_async]` builder (mirrors
//! `create_meta_functions` + `meta_fn_async` pairs + `call_impl`).

#[cfg(feature = "async")]
use futures::executor::block_on;
use futures::future::LocalBoxFuture;
use rust_sass_macros::{async_impl, maybe_async, maybe_block_on, sync_impl};
use std::rc::Rc;

#[cfg(feature = "async")]
macro_rules! box_rec {
    ($f:expr) => {
        Box::pin($f)
    };
}
#[cfg(not(feature = "async"))]
macro_rules! box_rec {
    ($f:expr) => {
        $f
    };
}

pub type AsyncCb = Rc<dyn for<'a> Fn(&'a u32) -> LocalBoxFuture<'a, u32>>;
pub type SyncCb = Rc<dyn Fn(&u32) -> u32>;

#[async_impl]
async fn take(cb: AsyncCb, v: u32) -> u32 {
    cb(&v).await
}

#[sync_impl]
fn take(cb: SyncCb, v: u32) -> u32 {
    cb(&v)
}

// pair-taking constructor (mirrors meta_fn_async)
#[async_impl]
fn mk(name: &str, cb: AsyncCb) -> (String, AsyncCb) {
    (name.into(), cb)
}

#[sync_impl]
fn mk(name: &str, cb: SyncCb) -> (String, SyncCb) {
    (name.into(), cb)
}

// marked impl fn (mirrors call_impl / load_css_impl / apply_impl)
#[maybe_async]
async fn add_impl(v: &u32, y: u32) -> u32 {
    v + y
}

#[maybe_async]
async fn create(y: u32) -> Vec<(String, u32)> {
    let cctx = y;
    let (name, cb) = mk("add", {
        Rc::new(move |v| -> LocalBoxFuture<'_, u32> { box_rec!(add_impl(v, cctx)) })
    });
    let out = take(cb, 1).await;
    vec![(name, out)]
}

#[test]
fn repro() {
    #[cfg(feature = "async")]
    assert_eq!(block_on(create(10)), vec![("add".into(), 11)]);
    #[cfg(not(feature = "async"))]
    assert_eq!(create(10), vec![("add".into(), 11)]);
}

// Plain (non-async) context driving a maybe_async fn — mirrors the spec
// runner: async mode the call yields a future (macro drives it); sync mode
// the callee converted to a plain fn (macro is identity).
#[maybe_async]
async fn answer() -> u32 {
    41
}

#[test]
fn maybe_block_on_drives_maybe_async_fn() {
    fn drive() -> u32 {
        maybe_block_on!(answer()) + 1
    }
    assert_eq!(drive(), 42);
}
