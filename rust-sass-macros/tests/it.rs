//! Self-tests for the inlined maybe-async macros. Every test must pass in
//! BOTH modes: `cargo test -p rust-sass-macros` (async) and
//! `cargo test -p rust-sass-macros --features async` (sync).

#[cfg(feature = "async")]
use futures::executor::block_on;
use rust_sass_macros::{async_impl, maybe_async, maybe_test, must_be_sync, sync_impl};
use std::fmt::Debug;

// --- 1. plain async fn ---

#[maybe_async]
async fn add(a: u32, b: u32) -> u32 {
    a + b
}

// --- 2. awaits, ?-chains, nested blocks ---

#[maybe_async]
async fn chain() -> u32 {
    let a = add(1, 2).await;
    let b = add(a, 3).await;
    {
        add(b, 4).await
    }
}

// --- 3. async block + Box::pin(async block) unwrapping ---

#[maybe_async]
async fn boxed_block() -> u32 {
    7
}

#[maybe_async]
async fn uses_boxed_block() -> u32 {
    boxed_block().await + 1
}

// --- 4. trait with LocalBoxFuture-returning methods + impl with
//        Box::pin(async move {..}) bodies (the Io-trait shape) ---

#[maybe_async]
pub trait Store: Debug {
    fn get<'a>(&'a self, key: &'a str) -> futures::future::LocalBoxFuture<'a, Option<u32>>;
    fn name(&self) -> String;
}

#[derive(Debug)]
struct MemStore;

#[maybe_async]
impl Store for MemStore {
    fn get<'a>(&'a self, key: &'a str) -> futures::future::LocalBoxFuture<'a, Option<u32>> {
        Box::pin(async move {
            if key == "k" {
                Some(42)
            } else {
                None
            }
        })
    }
    fn name(&self) -> String {
        "mem".into()
    }
}

#[maybe_async]
async fn trait_caller(s: &MemStore, key: &str) -> Option<u32> {
    s.get(key).await
}

// --- 5. async closures passed to a callback-taking helper (the combinator
//        shape); sync_impl/async_impl pairs ---

#[async_impl]
async fn with_cb<F>(f: F) -> u32
where
    F: for<'a> AsyncFnOnce(&'a u32) -> u32,
{
    let v = 5;
    f(&v).await
}

#[sync_impl]
fn with_cb<F>(f: F) -> u32
where
    F: FnOnce(&u32) -> u32,
{
    let v = 5;
    f(&v)
}

#[maybe_async]
async fn closure_caller() -> u32 {
    with_cb(async |x| x + 1).await
}

// --- 6. recursion-edge shape: Box::pin(<call>) via a cfg'd helper macro ---

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

#[maybe_async]
async fn rec(n: u32) -> u32 {
    if n == 0 {
        0
    } else {
        n + box_rec!(rec(n - 1)).await
    }
}

// --- 7. maybe_test ---

#[maybe_test]
async fn test_both_modes() {
    let v = add(2, 3).await;
    assert_eq!(v, 5);
}

// --- 8. must_be_sync (used inside maybe_test expansion; direct use here) ---

#[must_be_sync]
async fn always_sync() -> u32 {
    9
}

// --- driver ---

#[test]
fn all_shapes() {
    #[cfg(feature = "async")]
    {
        assert_eq!(block_on(add(1, 1)), 2);
        assert_eq!(block_on(chain()), 10);
        assert_eq!(block_on(uses_boxed_block()), 8);
        assert_eq!(block_on(trait_caller(&MemStore, "k")), Some(42));
        assert_eq!(block_on(closure_caller()), 6);
        assert_eq!(block_on(rec(4)), 10);
    }

    #[cfg(not(feature = "async"))]
    {
        assert_eq!(add(1, 1), 2);
        assert_eq!(chain(), 10);
        assert_eq!(uses_boxed_block(), 8);
        assert_eq!(trait_caller(&MemStore, "k"), Some(42));
        assert_eq!(closure_caller(), 6);
        assert_eq!(rec(4), 10);
        test_both_modes();
    }

    assert_eq!(always_sync(), 9);
    assert_eq!(MemStore.name(), "mem");
}
