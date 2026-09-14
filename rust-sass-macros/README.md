# rust-sass-macros

Proc-macro crate powering rust-sass's **dual sync/async compilation**: one
source tree, two builds selected by cargo features. Inlined + patched copy of
[maybe-async](https://github.com/fMeow/maybe-async-rs) 0.2.11 (MIT) — provenance
and patch notes are in `src/lib.rs`.

**Full reference: [`../docs/ref/macros.md`](../docs/ref/macros.md)** — macro
surface, mode selection, quick usage, pitfalls, and the sync-build failure
triage table.

```sh
cargo test -p rust-sass-macros                  # self-tests, sync mode
cargo test -p rust-sass-macros --features async # self-tests, async mode
```
